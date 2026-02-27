//! RP2040 (Raspberry Pi Pico) Test Harness Firmware
//!
//! Receives audio samples over USB-CDC serial, decodes with
//! `MiniDecoder` / `FastDemodulator`, and returns decoded frames + timing.
//!
//! Also supports live ADC decode from a radio connected to GPIO26 (ADC0).
//! MODE_ADC_KISS: ADC → TncEngine → KISS TNC over USB-CDC (standard TNC)
//! MODE_ADC_LIVE: ADC → MiniDecoder → TNC2 text over USB-CDC (debug)
//!
//! Protocol: length-prefixed binary messages (see `protocol.rs`).
//! Flow: request-response — host sends one AUDIO_CHUNK, waits for CHUNK_ACK.
//!
//! Uses USB-CDC (native USB on RP2040) for communication. The same USB port
//! handles both flashing (BOOTSEL mode) and data. No UART bridge needed.

#![no_std]
#![no_main]
#![allow(static_mut_refs)] // USB bus allocator requires 'static — standard embedded pattern

#[allow(dead_code)]
mod protocol;

use panic_halt as _;

use embedded_hal::digital::{OutputPin, StatefulOutputPin};
use rp_pico::entry;
use rp_pico::hal::{self, pac};
use rp_pico::hal::usb::UsbBus;

use usb_device::class_prelude::*;
use usb_device::prelude::*;
use usb_device::device::StringDescriptors;
use usbd_serial::SerialPort;

use packet_radio_core::modem::demod::{DemodSymbol, FastDemodulator, CorrelationDemodulator};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};
use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::ax25::{Frame, Address};
use packet_radio_core::modem::multi::MiniDecoder;
use packet_radio_core::modem::DemodConfig;
use packet_radio_core::tnc::{TncEngine, MiniAdapter, NullModulate, TncConfig, TncPlatform};
use packet_radio_core::kiss::{KissDecoder, Command};

use protocol::*;

/// Maximum audio chunk size (512 samples).
const MAX_CHUNK_SAMPLES: usize = 512;

/// Read buffer for incoming serial data.
const READ_BUF_SIZE: usize = MAX_MSG_SIZE + 16;

/// RP2040 CPU frequency in MHz (default with 12 MHz crystal).
const CPU_FREQ_MHZ: u32 = 125;

/// ADC clock divider for 11025 Hz: 48MHz / (1 + 4352 + 190/256) ≈ 11025 Hz
const ADC_DIV_INT: u16 = 4352;
const ADC_DIV_FRAC: u8 = 190;

/// Static USB bus allocator — required for 'static lifetime of USB classes.
static mut USB_BUS: Option<UsbBusAllocator<UsbBus>> = None;

/// FNV-1a hash for frame dedup (matches core implementation).
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Format an AX.25 frame as TNC2 text: `SRC-S>DST-S,DIGI1*,DIGI2:info\r\n`
/// Returns number of bytes written to `out`.
fn format_tnc2(frame_data: &[u8], out: &mut [u8]) -> usize {
    if let Some(frame) = Frame::parse(frame_data) {
        let mut pos = 0;

        // Source callsign
        pos += copy_addr(&frame.src, &mut out[pos..]);

        // >
        if pos < out.len() { out[pos] = b'>'; pos += 1; }

        // Destination callsign
        pos += copy_addr(&frame.dest, &mut out[pos..]);

        // Digipeaters
        for i in 0..frame.num_digipeaters as usize {
            if pos < out.len() { out[pos] = b','; pos += 1; }
            pos += copy_addr(&frame.digipeaters[i], &mut out[pos..]);
            if frame.digipeaters[i].h_bit {
                if pos < out.len() { out[pos] = b'*'; pos += 1; }
            }
        }

        // :info
        if pos < out.len() { out[pos] = b':'; pos += 1; }
        let info_len = frame.info.len().min(out.len().saturating_sub(pos + 2));
        out[pos..pos + info_len].copy_from_slice(&frame.info[..info_len]);
        pos += info_len;

        // \r\n
        if pos + 1 < out.len() {
            out[pos] = b'\r'; pos += 1;
            out[pos] = b'\n'; pos += 1;
        }

        pos
    } else {
        // Fallback: hex dump
        let prefix = b"HEX:";
        let mut pos = prefix.len().min(out.len());
        out[..pos].copy_from_slice(&prefix[..pos]);
        for &b in frame_data {
            if pos + 2 >= out.len().saturating_sub(2) { break; }
            out[pos] = HEX_CHARS[(b >> 4) as usize]; pos += 1;
            out[pos] = HEX_CHARS[(b & 0x0F) as usize]; pos += 1;
        }
        if pos + 1 < out.len() {
            out[pos] = b'\r'; pos += 1;
            out[pos] = b'\n'; pos += 1;
        }
        pos
    }
}

const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";

/// Copy address as "CALL-SSID" to output buffer, return bytes written.
fn copy_addr(addr: &Address, out: &mut [u8]) -> usize {
    let call = addr.callsign_str();
    let mut pos = 0;
    let n = call.len().min(out.len());
    out[..n].copy_from_slice(&call[..n]);
    pos += n;
    if addr.ssid != 0 && pos + 2 < out.len() {
        out[pos] = b'-'; pos += 1;
        if addr.ssid >= 10 {
            out[pos] = b'1'; pos += 1;
            out[pos] = b'0' + (addr.ssid - 10); pos += 1;
        } else {
            out[pos] = b'0' + addr.ssid; pos += 1;
        }
    }
    pos
}

/// Corr x3 decoder state: 3 timing phases with dedup.
struct Corr3State {
    demods: [CorrelationDemodulator; 3],
    hdlcs: [HdlcDecoder; 3],
    /// Ring buffer of recent frame hashes for dedup.
    recent_hashes: [u32; 32],
    recent_count: usize,
}

/// Dummy TncPlatform for RX-only operation (no real PTT or CSMA needed).
struct BenchPlatform;

impl TncPlatform for BenchPlatform {
    fn set_ptt(&mut self, _on: bool) {}
    fn channel_busy(&self) -> bool { false }
    fn random_byte(&self) -> u8 { 0 }
    fn now_ms(&self) -> u32 { 0 }
}

/// Decoder state — created on CONFIG message.
enum Decoder {
    None,
    Fast(FastDemodulator, SoftHdlcDecoder),
    Quality(FastDemodulator, SoftHdlcDecoder),
    Mini(MiniDecoder),
    Corr3(Corr3State),
    Tnc(TncEngine<MiniAdapter, NullModulate>, KissDecoder),
}

/// Operating mode for main loop.
enum Mode {
    /// USB binary protocol (existing benchmark/test mode).
    UsbProtocol,
    /// ADC → TncEngine → KISS over USB-CDC (standard KISS TNC).
    AdcKiss,
    /// ADC → MiniDecoder → TNC2 text over USB-CDC (debug).
    AdcLive,
}

/// Benchmark statistics accumulator.
struct BenchStats {
    total_frames: u32,
    chunks: u32,
    total_cycles: u64,
    min_cycles: u32,
    max_cycles: u32,
}

impl BenchStats {
    fn new() -> Self {
        Self {
            total_frames: 0,
            chunks: 0,
            total_cycles: 0,
            min_cycles: u32::MAX,
            max_cycles: 0,
        }
    }

    fn record_chunk(&mut self, cycles: u32, frames: u32) {
        self.chunks += 1;
        self.total_frames += frames;
        self.total_cycles += cycles as u64;
        if cycles < self.min_cycles {
            self.min_cycles = cycles;
        }
        if cycles > self.max_cycles {
            self.max_cycles = cycles;
        }
    }

    fn avg_cycles(&self) -> u32 {
        if self.chunks == 0 {
            0
        } else {
            (self.total_cycles / self.chunks as u64) as u32
        }
    }
}

/// Read RP2040 hardware timer (microseconds since boot).
/// Returns lower 32 bits — wraps every ~71 minutes, fine for chunk timing.
#[inline(always)]
fn read_timer_us(timer: &hal::Timer) -> u32 {
    timer.get_counter().ticks() as u32
}

/// USB-CDC serial wrapper that owns both device and serial port.
/// Bundles poll + read + write to avoid borrow issues with separate references.
struct UsbSerial {
    usb_dev: UsbDevice<'static, UsbBus>,
    serial: SerialPort<'static, UsbBus>,
}

impl UsbSerial {
    /// Poll USB device — must be called frequently to process USB events.
    fn poll(&mut self) -> bool {
        self.usb_dev.poll(&mut [&mut self.serial])
    }

    /// Blocking write: sends all bytes, polling USB between chunks.
    fn write_all(&mut self, data: &[u8]) {
        let mut offset = 0;
        while offset < data.len() {
            self.usb_dev.poll(&mut [&mut self.serial]);
            match self.serial.write(&data[offset..]) {
                Ok(n) if n > 0 => offset += n,
                _ => {}
            }
        }
        // Final poll to flush
        self.usb_dev.poll(&mut [&mut self.serial]);
    }

    /// Non-blocking read: returns bytes available or WouldBlock.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, usb_device::UsbError> {
        self.serial.read(buf)
    }

    /// Send a protocol message over USB-CDC serial.
    fn send_msg(&mut self, msg_type: u8, payload: &[u8]) {
        let mut buf = [0u8; MAX_MSG_SIZE];
        let len = build_msg(msg_type, 0, payload, &mut buf);
        self.write_all(&buf[..len]);
    }

    /// Send an error message with a text description.
    fn send_error(&mut self, msg: &[u8]) {
        self.send_msg(MSG_ERROR, msg);
    }
}

#[entry]
fn main() -> ! {
    let mut pac = pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);

    // Configure clocks: 125 MHz system clock from 12 MHz crystal
    let clocks = hal::clocks::init_clocks_and_plls(
        rp_pico::XOSC_CRYSTAL_FREQ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .ok()
    .unwrap();

    // GPIO setup for onboard LED (GPIO25) — toggles on each decoded frame
    let sio = hal::Sio::new(pac.SIO);
    let pins = rp_pico::Pins::new(pac.IO_BANK0, pac.PADS_BANK0, sio.gpio_bank0, &mut pac.RESETS);
    let mut led = pins.led.into_push_pull_output();

    // Hardware timer for cycle counting (1 us resolution)
    let timer = hal::Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);

    // ADC setup — init peripheral and pin at boot, FIFO started on demand
    let mut adc = hal::Adc::new(pac.ADC, &mut pac.RESETS);
    let mut adc_pin = hal::adc::AdcPin::new(pins.gpio26.into_floating_input()).unwrap();

    // USB-CDC setup
    unsafe {
        USB_BUS = Some(UsbBusAllocator::new(UsbBus::new(
            pac.USBCTRL_REGS,
            pac.USBCTRL_DPRAM,
            clocks.usb_clock,
            true,
            &mut pac.RESETS,
        )));
    }
    let usb_bus = unsafe { USB_BUS.as_ref().unwrap() };

    let serial = SerialPort::new(usb_bus);
    let usb_dev = UsbDeviceBuilder::new(usb_bus, UsbVidPid(0x2E8A, 0x000A))
        .strings(&[StringDescriptors::default()
            .manufacturer("packet-radio-rs")
            .product("RP2040 Test Harness")
            .serial_number("TEST0001")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    let mut usb = UsbSerial { usb_dev, serial };

    // Wait for USB enumeration (~500ms-2s after plug-in)
    let enum_start = read_timer_us(&timer);
    loop {
        usb.poll();
        let elapsed = read_timer_us(&timer).wrapping_sub(enum_start);
        if elapsed > 2_000_000 {
            break; // 2 second timeout
        }
        if usb.usb_dev.state() == UsbDeviceState::Configured {
            break;
        }
    }

    let mut mode = Mode::UsbProtocol;

    // Main loop — branches on current mode
    loop {
        match mode {
            Mode::UsbProtocol => {
                run_usb_protocol(&mut usb, &timer, &mut led, &mut adc, &mut adc_pin, &mut mode);
            }
            Mode::AdcKiss => {
                run_adc_kiss(&mut usb, &mut led, &mut adc, &mut adc_pin, &mut mode);
            }
            Mode::AdcLive => {
                run_adc_live(&mut usb, &mut led, &mut adc, &mut adc_pin, &mut mode);
            }
        }
    }
}

type LedPin = hal::gpio::Pin<hal::gpio::bank0::Gpio25, hal::gpio::FunctionSioOutput, hal::gpio::PullDown>;
type AdcPinType = hal::adc::AdcPin<hal::gpio::Pin<hal::gpio::bank0::Gpio26, hal::gpio::FunctionSio<hal::gpio::SioInput>, hal::gpio::PullNone>>;

/// USB binary protocol mode — existing benchmark/test harness logic.
fn run_usb_protocol(
    usb: &mut UsbSerial,
    timer: &hal::Timer,
    led: &mut LedPin,
    _adc: &mut hal::Adc,
    _adc_pin: &mut AdcPinType,
    mode: &mut Mode,
) {
    let mut decoder = Decoder::None;
    let mut stats = BenchStats::new();
    let mut read_buf = [0u8; READ_BUF_SIZE];
    let mut read_pos: usize = 0;

    // Reusable buffers
    let mut sample_buf = [0i16; MAX_CHUNK_SAMPLES];
    let mut symbol_buf = [DemodSymbol { bit: false, llr: 0 }; 1024];

    loop {
        // Poll USB for events
        usb.poll();

        let avail = READ_BUF_SIZE - read_pos;
        if avail == 0 {
            // Buffer full without a valid message — discard and resync
            read_pos = 0;
            continue;
        }

        match usb.read(&mut read_buf[read_pos..]) {
            Ok(n) if n > 0 => read_pos += n,
            _ => continue,
        }

        // Try to parse complete messages
        while read_pos >= HEADER_SIZE {
            let hdr = Header::parse(read_buf[..HEADER_SIZE].try_into().unwrap());
            let total = hdr.total_len();

            if hdr.payload_len as usize > MAX_PAYLOAD {
                // Invalid — discard one byte and try to resync
                read_buf.copy_within(1..read_pos, 0);
                read_pos -= 1;
                continue;
            }

            if read_pos < total {
                break; // need more data
            }

            // Process complete message — copy payload to avoid borrow issues
            // with send_msg needing &mut usb while payload borrows read_buf
            let msg_type = hdr.msg_type;

            match msg_type {
                MSG_PING => {
                    // Shift buffer first to release read_buf borrow
                    if read_pos > total {
                        read_buf.copy_within(total..read_pos, 0);
                    }
                    read_pos -= total;

                    usb.send_msg(MSG_PONG, &[]);
                    continue;
                }

                MSG_CONFIG => {
                    let payload = &read_buf[HEADER_SIZE..total];
                    if let Some(cfg) = ConfigPayload::parse(payload) {
                        // Check for ADC modes — switch mode and return
                        match cfg.decoder_mode {
                            MODE_ADC_KISS => {
                                usb.send_msg(MSG_READY, &[]);
                                *mode = Mode::AdcKiss;
                                return;
                            }
                            MODE_ADC_LIVE => {
                                usb.send_msg(MSG_READY, &[]);
                                *mode = Mode::AdcLive;
                                return;
                            }
                            _ => {}
                        }

                        let demod_cfg = DemodConfig {
                            sample_rate: cfg.sample_rate,
                            ..DemodConfig::default_1200()
                        };

                        decoder = match cfg.decoder_mode {
                            MODE_FAST => {
                                let d = FastDemodulator::new(demod_cfg);
                                Decoder::Fast(d, SoftHdlcDecoder::new())
                            }
                            MODE_QUALITY => {
                                let d = FastDemodulator::new(demod_cfg).with_energy_llr();
                                Decoder::Quality(d, SoftHdlcDecoder::new())
                            }
                            MODE_MINI => {
                                Decoder::Mini(MiniDecoder::new(demod_cfg))
                            }
                            MODE_TNC => {
                                let adapter = MiniAdapter::new(demod_cfg);
                                let tnc = TncEngine::new(adapter, NullModulate, TncConfig::default());
                                Decoder::Tnc(tnc, KissDecoder::new())
                            }
                            MODE_CORR3 => {
                                let offsets = [0, cfg.sample_rate / 3, 2 * cfg.sample_rate / 3];
                                let mut d0 = CorrelationDemodulator::new(demod_cfg).with_adaptive_gain();
                                let mut d1 = CorrelationDemodulator::new(demod_cfg).with_adaptive_gain();
                                let mut d2 = CorrelationDemodulator::new(demod_cfg).with_adaptive_gain();
                                d0.set_bit_phase(offsets[0]);
                                d1.set_bit_phase(offsets[1]);
                                d2.set_bit_phase(offsets[2]);
                                Decoder::Corr3(Corr3State {
                                    demods: [d0, d1, d2],
                                    hdlcs: [HdlcDecoder::new(), HdlcDecoder::new(), HdlcDecoder::new()],
                                    recent_hashes: [0u32; 32],
                                    recent_count: 0,
                                })
                            }
                            _ => {
                                // Shift buffer before sending error
                                if read_pos > total {
                                    read_buf.copy_within(total..read_pos, 0);
                                }
                                read_pos -= total;
                                usb.send_error(b"unknown decoder mode");
                                decoder = Decoder::None;
                                continue;
                            }
                        };

                        stats = BenchStats::new();
                        led.set_high(); // LED on — stream starting

                        // Shift buffer before sending response
                        if read_pos > total {
                            read_buf.copy_within(total..read_pos, 0);
                        }
                        read_pos -= total;

                        usb.send_msg(MSG_READY, &[]);
                        continue;
                    } else {
                        if read_pos > total {
                            read_buf.copy_within(total..read_pos, 0);
                        }
                        read_pos -= total;
                        usb.send_error(b"bad config payload");
                        continue;
                    }
                }

                MSG_AUDIO_CHUNK => {
                    let payload = &read_buf[HEADER_SIZE..total];
                    let seq = AudioChunkPayload::parse_seq(payload).unwrap_or(0);
                    let n_samples = AudioChunkPayload::parse_samples(payload, &mut sample_buf);

                    // Shift buffer now — we've copied samples to sample_buf
                    if read_pos > total {
                        read_buf.copy_within(total..read_pos, 0);
                    }
                    read_pos -= total;

                    // Time the decode with hardware timer
                    let start_us = read_timer_us(timer);
                    let mut chunk_frames: u32 = 0;

                    match &mut decoder {
                        Decoder::None => {
                            usb.send_error(b"no decoder configured");
                        }

                        Decoder::Fast(demod, hdlc) | Decoder::Quality(demod, hdlc) => {
                            let n_sym = demod.process_samples(
                                &sample_buf[..n_samples],
                                &mut symbol_buf,
                            );
                            for i in 0..n_sym {
                                if let Some(result) = hdlc.feed_soft_bit(symbol_buf[i].llr) {
                                    let frame_data = match &result {
                                        FrameResult::Valid(d) => &d[..],
                                        FrameResult::Recovered { data, .. } => &data[..],
                                    };
                                    let mut frame_payload = [0u8; 340];
                                    let fp_len = FramePayload::encode(
                                        seq, frame_data, &mut frame_payload,
                                    );
                                    usb.send_msg(
                                        MSG_FRAME,
                                        &frame_payload[..fp_len],
                                    );
                                    chunk_frames += 1;
                                    let _ = led.toggle();
                                }
                            }
                        }

                        Decoder::Mini(mini) => {
                            let output = mini.process_samples(&sample_buf[..n_samples]);
                            for i in 0..output.len() {
                                let frame_data = output.frame(i);
                                let mut frame_payload = [0u8; 340];
                                let fp_len = FramePayload::encode(
                                    seq, frame_data, &mut frame_payload,
                                );
                                usb.send_msg(
                                    MSG_FRAME,
                                    &frame_payload[..fp_len],
                                );
                                chunk_frames += 1;
                                let _ = led.toggle();
                            }
                        }

                        Decoder::Corr3(state) => {
                            for phase in 0..3 {
                                let n_sym = state.demods[phase].process_samples(
                                    &sample_buf[..n_samples],
                                    &mut symbol_buf,
                                );
                                for i in 0..n_sym {
                                    if let Some(frame_data) = state.hdlcs[phase].feed_bit(symbol_buf[i].bit) {
                                        // Copy frame data to break borrow before is_duplicate
                                        let hash = fnv1a_hash(frame_data);
                                        let flen = frame_data.len().min(330);
                                        let mut frame_copy = [0u8; 330];
                                        frame_copy[..flen].copy_from_slice(&frame_data[..flen]);

                                        // Check dedup using hash directly
                                        let is_dup = {
                                            let mut found = false;
                                            for j in 0..state.recent_count.min(32) {
                                                if state.recent_hashes[j] == hash {
                                                    found = true;
                                                    break;
                                                }
                                            }
                                            if !found {
                                                state.recent_hashes[state.recent_count % 32] = hash;
                                                state.recent_count += 1;
                                            }
                                            found
                                        };

                                        if !is_dup {
                                            let mut frame_payload = [0u8; 340];
                                            let fp_len = FramePayload::encode(
                                                seq, &frame_copy[..flen], &mut frame_payload,
                                            );
                                            usb.send_msg(
                                                MSG_FRAME,
                                                &frame_payload[..fp_len],
                                            );
                                            chunk_frames += 1;
                                            let _ = led.toggle();
                                        }
                                    }
                                }
                            }
                        }

                        Decoder::Tnc(tnc, kiss_dec) => {
                            // Full TNC pipeline: demod -> KISS encode -> drain
                            tnc.poll_rx(&sample_buf[..n_samples], &mut BenchPlatform);

                            // Drain KISS outbox and decode frames back
                            let mut kiss_buf = [0u8; 1024];
                            loop {
                                let n = tnc.read_kiss(&mut kiss_buf);
                                if n == 0 {
                                    break;
                                }
                                for j in 0..n {
                                    if let Some((_port, Command::DataFrame, frame_data)) = kiss_dec.feed_byte(kiss_buf[j]) {
                                        let flen = frame_data.len().min(330);
                                        let mut frame_copy = [0u8; 330];
                                        frame_copy[..flen].copy_from_slice(&frame_data[..flen]);
                                        let mut frame_payload = [0u8; 340];
                                        let fp_len = FramePayload::encode(
                                            seq, &frame_copy[..flen], &mut frame_payload,
                                        );
                                        usb.send_msg(
                                            MSG_FRAME,
                                            &frame_payload[..fp_len],
                                        );
                                        chunk_frames += 1;
                                        let _ = led.toggle();
                                    }
                                }
                            }
                        }
                    }

                    let elapsed_us = read_timer_us(timer).wrapping_sub(start_us);
                    // Convert to synthetic cycles at 125 MHz for protocol compatibility.
                    // Host uses --cpu-freq 125 to interpret these correctly.
                    let synthetic_cycles = elapsed_us.wrapping_mul(CPU_FREQ_MHZ);
                    stats.record_chunk(synthetic_cycles, chunk_frames);

                    let ack = ChunkAckPayload { seq, cycles: synthetic_cycles };
                    let mut ack_payload = [0u8; 6];
                    ack.encode(&mut ack_payload);
                    usb.send_msg(MSG_CHUNK_ACK, &ack_payload);
                    continue;
                }

                MSG_STREAM_END => {
                    led.set_low(); // LED off — stream done
                    // Shift buffer before sending
                    if read_pos > total {
                        read_buf.copy_within(total..read_pos, 0);
                    }
                    read_pos -= total;

                    let stats_payload = StatsPayload {
                        total_frames: stats.total_frames,
                        chunks: stats.chunks,
                        total_cycles: stats.total_cycles,
                        min_cycles: if stats.min_cycles == u32::MAX { 0 } else { stats.min_cycles },
                        max_cycles: stats.max_cycles,
                        avg_cycles: stats.avg_cycles(),
                    };
                    let mut sp = [0u8; 28];
                    stats_payload.encode(&mut sp);
                    usb.send_msg(MSG_STATS, &sp);
                    continue;
                }

                _ => {} // unknown — ignore
            }

            // Shift remaining data forward (for unhandled message types)
            if read_pos > total {
                read_buf.copy_within(total..read_pos, 0);
            }
            read_pos -= total;
        }
    }
}

/// ADC KISS mode — standard KISS TNC over USB-CDC.
/// ADC samples at 11025 Hz → MiniDecoder → TncEngine → KISS frames out.
/// Incoming KISS data from USB fed to TncEngine for TX path (future).
fn run_adc_kiss(
    usb: &mut UsbSerial,
    led: &mut LedPin,
    adc: &mut hal::Adc,
    adc_pin: &mut AdcPinType,
    mode: &mut Mode,
) {
    let demod_cfg = DemodConfig {
        sample_rate: 11025,
        ..DemodConfig::default_1200()
    };
    let adapter = MiniAdapter::new(demod_cfg);
    let mut tnc = TncEngine::new(adapter, NullModulate, TncConfig::default());
    let mut platform = BenchPlatform;

    let mut sample_buf = [0i16; MAX_CHUNK_SAMPLES];
    let mut sample_pos: usize = 0;
    let mut kiss_buf = [0u8; 512];
    let mut read_byte = [0u8; 64];

    // Start ADC FIFO at 11025 Hz
    let mut fifo = adc.build_fifo()
        .clock_divider(ADC_DIV_INT, ADC_DIV_FRAC)
        .set_channel(adc_pin)
        .start();

    led.set_high(); // LED on — streaming

    loop {
        usb.poll();

        // Check for stop command (any protocol message causes exit back to USB mode)
        match usb.read(&mut read_byte) {
            Ok(n) if n > 0 => {
                // Check for MSG_STREAM_END or MSG_CONFIG — return to USB protocol mode
                if n >= HEADER_SIZE {
                    let hdr = Header::parse(read_byte[..HEADER_SIZE].try_into().unwrap());
                    if hdr.msg_type == MSG_STREAM_END || hdr.msg_type == MSG_CONFIG {
                        fifo.stop();
                        led.set_low();
                        *mode = Mode::UsbProtocol;
                        return;
                    }
                }
                // Feed KISS data from host to TNC (for TX path)
                for i in 0..n {
                    tnc.feed_kiss(read_byte[i]);
                }
            }
            _ => {}
        }

        // Read ADC FIFO samples
        while fifo.len() > 0 && sample_pos < MAX_CHUNK_SAMPLES {
            let raw: u16 = fifo.read();
            // 12-bit unsigned (0-4095) → signed i16 centered at 0
            sample_buf[sample_pos] = (raw as i16) - 2048;
            sample_pos += 1;
        }

        // Process a full buffer
        if sample_pos >= MAX_CHUNK_SAMPLES {
            tnc.poll_rx(&sample_buf[..sample_pos], &mut platform);
            sample_pos = 0;

            // Drain KISS output → USB
            loop {
                let n = tnc.read_kiss(&mut kiss_buf);
                if n == 0 { break; }
                usb.write_all(&kiss_buf[..n]);
                let _ = led.toggle(); // blink on KISS output
            }
        }
    }
}

/// ADC Live mode — TNC2 text output over USB-CDC for debugging.
/// ADC samples at 11025 Hz → MiniDecoder → TNC2 text lines.
fn run_adc_live(
    usb: &mut UsbSerial,
    led: &mut LedPin,
    adc: &mut hal::Adc,
    adc_pin: &mut AdcPinType,
    mode: &mut Mode,
) {
    let demod_cfg = DemodConfig {
        sample_rate: 11025,
        ..DemodConfig::default_1200()
    };
    let mut mini = MiniDecoder::new(demod_cfg);

    let mut sample_buf = [0i16; MAX_CHUNK_SAMPLES];
    let mut sample_pos: usize = 0;
    let mut read_byte = [0u8; 64];
    let mut tnc2_buf = [0u8; 512];

    // Start ADC FIFO at 11025 Hz
    let mut fifo = adc.build_fifo()
        .clock_divider(ADC_DIV_INT, ADC_DIV_FRAC)
        .set_channel(adc_pin)
        .start();

    led.set_high(); // LED on — streaming

    loop {
        usb.poll();

        // Check for stop command
        match usb.read(&mut read_byte) {
            Ok(n) if n > 0 => {
                if n >= HEADER_SIZE {
                    let hdr = Header::parse(read_byte[..HEADER_SIZE].try_into().unwrap());
                    if hdr.msg_type == MSG_STREAM_END || hdr.msg_type == MSG_CONFIG {
                        fifo.stop();
                        led.set_low();
                        *mode = Mode::UsbProtocol;
                        return;
                    }
                }
            }
            _ => {}
        }

        // Read ADC FIFO samples
        while fifo.len() > 0 && sample_pos < MAX_CHUNK_SAMPLES {
            let raw: u16 = fifo.read();
            sample_buf[sample_pos] = (raw as i16) - 2048;
            sample_pos += 1;
        }

        // Process a full buffer
        if sample_pos >= MAX_CHUNK_SAMPLES {
            let output = mini.process_samples(&sample_buf[..sample_pos]);
            sample_pos = 0;

            for i in 0..output.len() {
                let frame_data = output.frame(i);
                let n = format_tnc2(frame_data, &mut tnc2_buf);
                if n > 0 {
                    usb.write_all(&tnc2_buf[..n]);
                }
                let _ = led.toggle();
            }
        }
    }
}
