//! ESP32-C3 Test Harness Firmware
//!
//! Receives audio samples over USB-Serial-JTAG, decodes with
//! `MiniDecoder` / `FastDemodulator`, and returns decoded frames + cycle counts.
//!
//! Protocol: length-prefixed binary messages (see `protocol.rs`).
//! Flow: request-response — host sends one AUDIO_CHUNK, waits for CHUNK_ACK.
//!
//! Uses the built-in USB-Serial-JTAG peripheral (shows up as /dev/ttyACMx).
//! The same USB port handles both flashing and data communication.
//!
//! IMPORTANT: UsbSerialJtag::write() blocks until the USB host reads the data.
//! Never write before receiving (the host may not have the port open yet).
//! After espflash, press RST (not BOOT+RST) to enter the application.

#![no_std]
#![no_main]

#[allow(dead_code)]
mod protocol;

use esp_backtrace as _;
use esp_hal::rmt::Rmt;
use esp_hal::time::Rate;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_hal_smartled::{SmartLedsAdapter, smart_led_buffer};
use smart_leds::{RGB8, SmartLedsWrite};

esp_bootloader_esp_idf::esp_app_desc!();

use packet_radio_core::modem::demod::{DemodSymbol, FastDemodulator, CorrelationDemodulator};
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};
use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::modem::multi::MiniDecoder;
use packet_radio_core::modem::DemodConfig;
use packet_radio_core::tnc::{TncEngine, MiniAdapter, NullModulate, TncConfig, TncPlatform};
use packet_radio_core::kiss::{KissDecoder, Command};

use protocol::*;

/// Maximum audio chunk size (512 samples).
const MAX_CHUNK_SAMPLES: usize = 512;

/// Read buffer for incoming serial data.
const READ_BUF_SIZE: usize = MAX_MSG_SIZE + 16;

/// FNV-1a hash for frame dedup (matches core implementation).
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Corr×3 decoder state: 3 timing phases with dedup.
struct Corr3State {
    demods: [CorrelationDemodulator; 3],
    hdlcs: [HdlcDecoder; 3],
    /// Ring buffer of recent frame hashes for dedup.
    recent_hashes: [u32; 32],
    recent_count: usize,
}

/// Dummy TncPlatform for RX-only benchmarking (no real PTT or CSMA needed).
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

/// Initialize ESP32-C3 performance counter for cycle counting.
/// mpcer (0x7E0) = event type, mpcmr (0x7E1) = enable.
fn init_perf_counter() {
    unsafe {
        // Set event type to 1 (cycle count)
        core::arch::asm!("csrw 0x7E0, {}", in(reg) 1u32);
        // Enable the counter
        core::arch::asm!("csrw 0x7E1, {}", in(reg) 1u32);
    }
}

/// Read ESP32-C3 performance counter (mpccr CSR 0x7E2).
/// Returns 32-bit cycle count (wraps every ~27s at 160 MHz).
#[inline(always)]
fn read_cycles() -> u32 {
    let cycles: u32;
    unsafe {
        core::arch::asm!("csrr {}, 0x7E2", out(reg) cycles);
    }
    cycles
}

/// Blocking write all bytes to USB-Serial-JTAG.
/// Uses the HAL write() which blocks until USB host ACKs each 64-byte chunk.
/// Only call after receiving data from host (proves host is connected).
fn serial_write_all(serial: &mut UsbSerialJtag<'static, esp_hal::Blocking>, data: &[u8]) {
    let _ = serial.write(data);
}

/// Send a protocol message over USB-Serial-JTAG.
fn send_msg(serial: &mut UsbSerialJtag<'static, esp_hal::Blocking>, msg_type: u8, payload: &[u8]) {
    let mut buf = [0u8; MAX_MSG_SIZE];
    let len = build_msg(msg_type, 0, payload, &mut buf);
    serial_write_all(serial, &buf[..len]);
}

/// Send an error message with a text description.
fn send_error(serial: &mut UsbSerialJtag<'static, esp_hal::Blocking>, msg: &[u8]) {
    send_msg(serial, MSG_ERROR, msg);
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    init_perf_counter();

    // USB-Serial-JTAG — built-in USB on ESP32-C3, shows up as /dev/ttyACMx
    // NOTE: Do NOT write before receiving data — write() blocks until host reads,
    // and if host hasn't opened the port yet, it blocks forever.
    let mut serial = UsbSerialJtag::new(peripherals.USB_DEVICE);

    // WS2812 addressable RGB LED on GPIO2 (ESP32-C3-DevKit-RUST-1) via RMT
    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).expect("RMT init");
    let mut rmt_buf = smart_led_buffer!(1);
    let mut led = SmartLedsAdapter::new(rmt.channel0, peripherals.GPIO2, &mut rmt_buf);
    let led_off: [RGB8; 1] = [RGB8 { r: 0, g: 0, b: 0 }];
    let led_on: [RGB8; 1] = [RGB8 { r: 0, g: 20, b: 0 }]; // dim green
    let mut led_state = false;
    let _ = led.write(led_off.iter().cloned());

    let mut decoder = Decoder::None;
    let mut stats = BenchStats::new();
    let mut read_buf = [0u8; READ_BUF_SIZE];
    let mut read_pos: usize = 0;

    // Reusable buffers
    let mut sample_buf = [0i16; MAX_CHUNK_SAMPLES];
    let mut symbol_buf = [DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 1024];

    loop {
        // Poll for available bytes (non-blocking read_byte, spin until data)
        let avail = READ_BUF_SIZE - read_pos;
        if avail == 0 {
            read_pos = 0;
            continue;
        }

        // Read one byte at a time using nb API (non-blocking, no USB host dependency)
        match serial.read_byte() {
            Ok(b) => {
                read_buf[read_pos] = b;
                read_pos += 1;
            }
            Err(_) => continue, // WouldBlock — no data yet
        }

        // Try to parse complete messages
        while read_pos >= HEADER_SIZE {
            let hdr = Header::parse(read_buf[..HEADER_SIZE].try_into().unwrap());
            let total = hdr.total_len();

            if hdr.payload_len as usize > MAX_PAYLOAD {
                read_buf.copy_within(1..read_pos, 0);
                read_pos -= 1;
                continue;
            }

            if read_pos < total {
                break;
            }

            let payload = &read_buf[HEADER_SIZE..total];
            match hdr.msg_type {
                MSG_PING => {
                    send_msg(&mut serial, MSG_PONG, &[]);
                }

                MSG_CONFIG => {
                    if let Some(cfg) = ConfigPayload::parse(payload) {
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
                                send_error(&mut serial, b"unknown decoder mode");
                                Decoder::None
                            }
                        };

                        stats = BenchStats::new();
                        let _ = led.write(led_on.iter().cloned()); // LED on — stream starting
                        led_state = true;
                        send_msg(&mut serial, MSG_READY, &[]);
                    } else {
                        send_error(&mut serial, b"bad config payload");
                    }
                }

                MSG_AUDIO_CHUNK => {
                    let seq = AudioChunkPayload::parse_seq(payload).unwrap_or(0);
                    let n_samples = AudioChunkPayload::parse_samples(payload, &mut sample_buf);

                    let start = read_cycles();
                    let mut chunk_frames: u32 = 0;

                    match &mut decoder {
                        Decoder::None => {
                            send_error(&mut serial, b"no decoder configured");
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
                                    send_msg(
                                        &mut serial,
                                        MSG_FRAME,
                                        &frame_payload[..fp_len],
                                    );
                                    chunk_frames += 1;
                                    led_state = !led_state;
                                    let c = if led_state { &led_on } else { &led_off };
                                    let _ = led.write(c.iter().cloned());
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
                                send_msg(
                                    &mut serial,
                                    MSG_FRAME,
                                    &frame_payload[..fp_len],
                                );
                                chunk_frames += 1;
                                led_state = !led_state;
                                let c = if led_state { &led_on } else { &led_off };
                                let _ = led.write(c.iter().cloned());
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
                                        let hash = fnv1a_hash(frame_data);
                                        let flen = frame_data.len().min(330);
                                        let mut frame_copy = [0u8; 330];
                                        frame_copy[..flen].copy_from_slice(&frame_data[..flen]);

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
                                            send_msg(
                                                &mut serial,
                                                MSG_FRAME,
                                                &frame_payload[..fp_len],
                                            );
                                            chunk_frames += 1;
                                            led_state = !led_state;
                                            let c = if led_state { &led_on } else { &led_off };
                                            let _ = led.write(c.iter().cloned());
                                        }
                                    }
                                }
                            }
                        }

                        Decoder::Tnc(tnc, kiss_dec) => {
                            tnc.poll_rx(&sample_buf[..n_samples], &mut BenchPlatform);

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
                                        send_msg(
                                            &mut serial,
                                            MSG_FRAME,
                                            &frame_payload[..fp_len],
                                        );
                                        chunk_frames += 1;
                                        led_state = !led_state;
                                        let c = if led_state { &led_on } else { &led_off };
                                        let _ = led.write(c.iter().cloned());
                                    }
                                }
                            }
                        }
                    }

                    let elapsed = read_cycles().wrapping_sub(start);
                    stats.record_chunk(elapsed, chunk_frames);

                    let ack = ChunkAckPayload { seq, cycles: elapsed };
                    let mut ack_payload = [0u8; 6];
                    ack.encode(&mut ack_payload);
                    send_msg(&mut serial, MSG_CHUNK_ACK, &ack_payload);
                }

                MSG_STREAM_END => {
                    let _ = led.write(led_off.iter().cloned()); // LED off — stream done
                    led_state = false;
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
                    send_msg(&mut serial, MSG_STATS, &sp);
                }

                _ => {} // unknown — ignore
            }

            // Shift remaining data forward
            if read_pos > total {
                read_buf.copy_within(total..read_pos, 0);
            }
            read_pos -= total;
        }
    }
}
