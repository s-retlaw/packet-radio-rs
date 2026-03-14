//! Pico 2 W (RP2350) Embassy Test Harness Firmware
//!
//! Receives audio samples over USB-CDC serial, decodes with
//! `MiniDecoder` / `FastDemodulator`, and returns decoded frames + timing.
//!
//! Protocol: length-prefixed binary messages (see `protocol.rs`).
//! Flow: request-response — host sends one AUDIO_CHUNK, waits for CHUNK_ACK.
//!
//! Uses Embassy async runtime with `embassy-rp` HAL and `embassy-usb` for
//! USB-CDC communication. Logging via `defmt` + RTT.

#![no_std]
#![no_main]

#[allow(dead_code)]
mod protocol;

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::usb;
use embassy_rp::{bind_interrupts, peripherals};
use embassy_time::Instant;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::EndpointError;
use embassy_usb::UsbDevice;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

use packet_radio_core::ax25::frame::HdlcDecoder;
use packet_radio_core::ax25::{Address, Frame};
use packet_radio_core::kiss::{Command, KissDecoder};
use packet_radio_core::modem::demod::{
    CorrelationDemodulator, DemodSymbol, FastDemodulator,
};
use packet_radio_core::modem::multi::MiniDecoder;
use packet_radio_core::modem::soft_hdlc::{FrameResult, SoftHdlcDecoder};
use packet_radio_core::modem::DemodConfig;
use packet_radio_core::tnc::{
    MiniAdapter, NullModulate, TncConfig, TncEngine, TncPlatform,
};

use protocol::*;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => usb::InterruptHandler<peripherals::USB>;
});

/// Maximum audio chunk size (512 samples).
const MAX_CHUNK_SAMPLES: usize = 512;

/// Read buffer for incoming serial data.
const READ_BUF_SIZE: usize = MAX_MSG_SIZE + 16;

/// RP2350 CPU frequency in MHz (default with 12 MHz crystal).
const CPU_FREQ_MHZ: u32 = 150;

/// USB max packet size for CDC ACM.
const MAX_PACKET_SIZE: u16 = 64;

/// FNV-1a hash for frame dedup (matches core implementation).
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
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
        out[pos] = b'-';
        pos += 1;
        if addr.ssid >= 10 {
            out[pos] = b'1';
            pos += 1;
            out[pos] = b'0' + (addr.ssid - 10);
            pos += 1;
        } else {
            out[pos] = b'0' + addr.ssid;
            pos += 1;
        }
    }
    pos
}

/// Format an AX.25 frame as TNC2 text: `SRC-S>DST-S,DIGI1*,DIGI2:info\r\n`
/// Returns number of bytes written to `out`.
#[allow(dead_code)]
fn format_tnc2(frame_data: &[u8], out: &mut [u8]) -> usize {
    if let Some(frame) = Frame::parse(frame_data) {
        let mut pos = 0;

        pos += copy_addr(&frame.src, &mut out[pos..]);
        if pos < out.len() {
            out[pos] = b'>';
            pos += 1;
        }
        pos += copy_addr(&frame.dest, &mut out[pos..]);

        for i in 0..frame.num_digipeaters as usize {
            if pos < out.len() {
                out[pos] = b',';
                pos += 1;
            }
            pos += copy_addr(&frame.digipeaters[i], &mut out[pos..]);
            if frame.digipeaters[i].h_bit {
                if pos < out.len() {
                    out[pos] = b'*';
                    pos += 1;
                }
            }
        }

        if pos < out.len() {
            out[pos] = b':';
            pos += 1;
        }
        let info_len = frame.info.len().min(out.len().saturating_sub(pos + 2));
        out[pos..pos + info_len].copy_from_slice(&frame.info[..info_len]);
        pos += info_len;

        if pos + 1 < out.len() {
            out[pos] = b'\r';
            pos += 1;
            out[pos] = b'\n';
            pos += 1;
        }

        pos
    } else {
        let prefix = b"HEX:";
        let mut pos = prefix.len().min(out.len());
        out[..pos].copy_from_slice(&prefix[..pos]);
        for &b in frame_data {
            if pos + 2 >= out.len().saturating_sub(2) {
                break;
            }
            out[pos] = HEX_CHARS[(b >> 4) as usize];
            pos += 1;
            out[pos] = HEX_CHARS[(b & 0x0F) as usize];
            pos += 1;
        }
        if pos + 1 < out.len() {
            out[pos] = b'\r';
            pos += 1;
            out[pos] = b'\n';
            pos += 1;
        }
        pos
    }
}

/// Corr x3 decoder state: 3 timing phases with dedup.
struct Corr3State {
    demods: [CorrelationDemodulator; 3],
    hdlcs: [HdlcDecoder; 3],
    recent_hashes: [u32; 32],
    recent_count: usize,
}

/// Dummy TncPlatform for RX-only operation.
struct BenchPlatform;

impl TncPlatform for BenchPlatform {
    fn set_ptt(&mut self, _on: bool) {}
    fn channel_busy(&self) -> bool {
        false
    }
    fn random_byte(&self) -> u8 {
        0
    }
    fn now_ms(&self) -> u32 {
        0
    }
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

/// Async wrapper: send all bytes over CDC ACM, handling packet fragmentation.
async fn write_all<'d, D: embassy_usb::driver::Driver<'d>>(
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    data: &[u8],
) -> Result<(), EndpointError> {
    // CDC ACM packets are max 64 bytes; fragment as needed
    for chunk in data.chunks(MAX_PACKET_SIZE as usize) {
        sender.write_packet(chunk).await?;
    }
    // If data was exact multiple of packet size, send ZLP to flush
    if !data.is_empty() && data.len() % MAX_PACKET_SIZE as usize == 0 {
        sender.write_packet(&[]).await?;
    }
    Ok(())
}

/// Build and send a protocol message over CDC ACM.
async fn send_msg<'d, D: embassy_usb::driver::Driver<'d>>(
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    msg_type: u8,
    payload: &[u8],
) -> Result<(), EndpointError> {
    let mut buf = [0u8; MAX_MSG_SIZE];
    let len = build_msg(msg_type, 0, payload, &mut buf);
    write_all(sender, &buf[..len]).await
}

/// Send an error message.
async fn send_error<'d, D: embassy_usb::driver::Driver<'d>>(
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    msg: &[u8],
) -> Result<(), EndpointError> {
    send_msg(sender, MSG_ERROR, msg).await
}

type UsbDriver = usb::Driver<'static, peripherals::USB>;

#[embassy_executor::task]
async fn usb_task(mut usb_dev: UsbDevice<'static, UsbDriver>) -> ! {
    usb_dev.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Pico 2 W test harness starting");

    // USB driver
    let driver = usb::Driver::new(p.USB, Irqs);

    // USB device config
    let mut config = embassy_usb::Config::new(0x2E8A, 0x000A);
    config.manufacturer = Some("packet-radio-rs");
    config.product = Some("Pico2W Test Harness");
    config.serial_number = Some("TEST0002");

    // USB buffers — static for 'static lifetime
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static STATE: StaticCell<State<'static>> = StaticCell::new();

    let config_desc = CONFIG_DESC.init([0; 256]);
    let bos_desc = BOS_DESC.init([0; 256]);
    let msos_desc = MSOS_DESC.init([0; 256]);
    let control_buf = CONTROL_BUF.init([0; 64]);
    let state = STATE.init(State::new());

    // Build USB device
    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        config_desc,
        bos_desc,
        msos_desc,
        control_buf,
    );

    // Create CDC ACM class
    let class = CdcAcmClass::new(&mut builder, state, MAX_PACKET_SIZE);

    // Build and spawn USB driver task
    let usb_dev = builder.build();
    spawner.spawn(usb_task(usb_dev)).unwrap();

    // Split CDC class into sender + receiver
    let (mut sender, mut receiver) = class.split();

    info!("USB initialized, waiting for connection");

    // Main protocol loop
    loop {
        // Wait for USB host connection
        receiver.wait_connection().await;
        info!("USB connected");

        match run_usb_protocol(&mut sender, &mut receiver).await {
            Ok(()) => info!("Protocol session ended"),
            Err(EndpointError::Disabled) => info!("USB disconnected"),
            Err(_e) => warn!("USB error"),
        }
    }
}

/// USB binary protocol mode — async version of the rp2040-test protocol loop.
async fn run_usb_protocol<'d, D: embassy_usb::driver::Driver<'d>>(
    sender: &mut embassy_usb::class::cdc_acm::Sender<'d, D>,
    receiver: &mut embassy_usb::class::cdc_acm::Receiver<'d, D>,
) -> Result<(), EndpointError> {
    let mut decoder = Decoder::None;
    let mut stats = BenchStats::new();
    let mut read_buf = [0u8; READ_BUF_SIZE];
    let mut read_pos: usize = 0;

    // Reusable buffers
    let mut sample_buf = [0i16; MAX_CHUNK_SAMPLES];
    let mut symbol_buf = [DemodSymbol { bit: false, llr: 0, sample_idx: 0 }; 1024];

    // Packet read buffer (max 64 bytes per USB packet)
    let mut pkt_buf = [0u8; 64];

    loop {
        // Read a USB packet (async — yields until data available)
        let n = receiver.read_packet(&mut pkt_buf).await?;
        if n == 0 {
            continue;
        }

        // Append to read buffer
        let avail = READ_BUF_SIZE - read_pos;
        let copy_n = n.min(avail);
        read_buf[read_pos..read_pos + copy_n].copy_from_slice(&pkt_buf[..copy_n]);
        read_pos += copy_n;

        // Try to parse complete messages
        while read_pos >= HEADER_SIZE {
            let hdr = Header::parse(read_buf[..HEADER_SIZE].try_into().unwrap());
            let total = hdr.total_len();

            if hdr.payload_len as usize > MAX_PAYLOAD {
                // Invalid — discard one byte and resync
                read_buf.copy_within(1..read_pos, 0);
                read_pos -= 1;
                continue;
            }

            if read_pos < total {
                break; // need more data
            }

            let msg_type = hdr.msg_type;

            match msg_type {
                MSG_PING => {
                    if read_pos > total {
                        read_buf.copy_within(total..read_pos, 0);
                    }
                    read_pos -= total;
                    send_msg(sender, MSG_PONG, &[]).await?;
                    continue;
                }

                MSG_CONFIG => {
                    let payload = &read_buf[HEADER_SIZE..total];
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
                                let d =
                                    FastDemodulator::new(demod_cfg).with_energy_llr();
                                Decoder::Quality(d, SoftHdlcDecoder::new())
                            }
                            MODE_MINI => Decoder::Mini(MiniDecoder::new(demod_cfg)),
                            MODE_TNC => {
                                let adapter = MiniAdapter::new(demod_cfg);
                                let tnc = TncEngine::new(
                                    adapter,
                                    NullModulate,
                                    TncConfig::default(),
                                );
                                Decoder::Tnc(tnc, KissDecoder::new())
                            }
                            MODE_CORR3 => {
                                let offsets = [
                                    0,
                                    cfg.sample_rate / 3,
                                    2 * cfg.sample_rate / 3,
                                ];
                                let mut d0 = CorrelationDemodulator::new(demod_cfg)
                                    .with_adaptive_gain();
                                let mut d1 = CorrelationDemodulator::new(demod_cfg)
                                    .with_adaptive_gain();
                                let mut d2 = CorrelationDemodulator::new(demod_cfg)
                                    .with_adaptive_gain();
                                d0.set_bit_phase(offsets[0]);
                                d1.set_bit_phase(offsets[1]);
                                d2.set_bit_phase(offsets[2]);
                                Decoder::Corr3(Corr3State {
                                    demods: [d0, d1, d2],
                                    hdlcs: [
                                        HdlcDecoder::new(),
                                        HdlcDecoder::new(),
                                        HdlcDecoder::new(),
                                    ],
                                    recent_hashes: [0u32; 32],
                                    recent_count: 0,
                                })
                            }
                            _ => {
                                if read_pos > total {
                                    read_buf.copy_within(total..read_pos, 0);
                                }
                                read_pos -= total;
                                send_error(sender, b"unknown decoder mode").await?;
                                decoder = Decoder::None;
                                continue;
                            }
                        };

                        stats = BenchStats::new();
                        info!("Configured decoder mode {}", cfg.decoder_mode);

                        if read_pos > total {
                            read_buf.copy_within(total..read_pos, 0);
                        }
                        read_pos -= total;

                        send_msg(sender, MSG_READY, &[]).await?;
                        continue;
                    } else {
                        if read_pos > total {
                            read_buf.copy_within(total..read_pos, 0);
                        }
                        read_pos -= total;
                        send_error(sender, b"bad config payload").await?;
                        continue;
                    }
                }

                MSG_AUDIO_CHUNK => {
                    let payload = &read_buf[HEADER_SIZE..total];
                    let seq = AudioChunkPayload::parse_seq(payload).unwrap_or(0);
                    let n_samples =
                        AudioChunkPayload::parse_samples(payload, &mut sample_buf);

                    if read_pos > total {
                        read_buf.copy_within(total..read_pos, 0);
                    }
                    read_pos -= total;

                    // Time the decode
                    let start = Instant::now();
                    let mut chunk_frames: u32 = 0;

                    match &mut decoder {
                        Decoder::None => {
                            send_error(sender, b"no decoder configured").await?;
                        }

                        Decoder::Fast(demod, hdlc)
                        | Decoder::Quality(demod, hdlc) => {
                            let n_sym = demod.process_samples(
                                &sample_buf[..n_samples],
                                &mut symbol_buf,
                            );
                            for i in 0..n_sym {
                                if let Some(result) =
                                    hdlc.feed_soft_bit(symbol_buf[i].llr)
                                {
                                    let frame_data = match &result {
                                        FrameResult::Valid(d) => &d[..],
                                        FrameResult::Recovered { data, .. } => {
                                            &data[..]
                                        }
                                    };
                                    let mut frame_payload = [0u8; 340];
                                    let fp_len = FramePayload::encode(
                                        seq,
                                        frame_data,
                                        &mut frame_payload,
                                    );
                                    send_msg(
                                        sender,
                                        MSG_FRAME,
                                        &frame_payload[..fp_len],
                                    )
                                    .await?;
                                    chunk_frames += 1;
                                }
                            }
                        }

                        Decoder::Mini(mini) => {
                            let output =
                                mini.process_samples(&sample_buf[..n_samples]);
                            for i in 0..output.len() {
                                let frame_data = output.frame(i);
                                let mut frame_payload = [0u8; 340];
                                let fp_len = FramePayload::encode(
                                    seq,
                                    frame_data,
                                    &mut frame_payload,
                                );
                                send_msg(
                                    sender,
                                    MSG_FRAME,
                                    &frame_payload[..fp_len],
                                )
                                .await?;
                                chunk_frames += 1;
                            }
                        }

                        Decoder::Corr3(state) => {
                            for phase in 0..3 {
                                let n_sym = state.demods[phase].process_samples(
                                    &sample_buf[..n_samples],
                                    &mut symbol_buf,
                                );
                                for i in 0..n_sym {
                                    if let Some(frame_data) = state.hdlcs[phase]
                                        .feed_bit(symbol_buf[i].bit)
                                    {
                                        let hash = fnv1a_hash(frame_data);
                                        let flen = frame_data.len().min(330);
                                        let mut frame_copy = [0u8; 330];
                                        frame_copy[..flen]
                                            .copy_from_slice(&frame_data[..flen]);

                                        let is_dup = {
                                            let mut found = false;
                                            for j in
                                                0..state.recent_count.min(32)
                                            {
                                                if state.recent_hashes[j] == hash
                                                {
                                                    found = true;
                                                    break;
                                                }
                                            }
                                            if !found {
                                                state.recent_hashes
                                                    [state.recent_count % 32] =
                                                    hash;
                                                state.recent_count += 1;
                                            }
                                            found
                                        };

                                        if !is_dup {
                                            let mut frame_payload = [0u8; 340];
                                            let fp_len = FramePayload::encode(
                                                seq,
                                                &frame_copy[..flen],
                                                &mut frame_payload,
                                            );
                                            send_msg(
                                                sender,
                                                MSG_FRAME,
                                                &frame_payload[..fp_len],
                                            )
                                            .await?;
                                            chunk_frames += 1;
                                        }
                                    }
                                }
                            }
                        }

                        Decoder::Tnc(tnc, kiss_dec) => {
                            tnc.poll_rx(
                                &sample_buf[..n_samples],
                                &mut BenchPlatform,
                            );

                            let mut kiss_buf = [0u8; 1024];
                            loop {
                                let n = tnc.read_kiss(&mut kiss_buf);
                                if n == 0 {
                                    break;
                                }
                                for j in 0..n {
                                    if let Some((
                                        _port,
                                        Command::DataFrame,
                                        frame_data,
                                    )) = kiss_dec.feed_byte(kiss_buf[j])
                                    {
                                        let flen = frame_data.len().min(330);
                                        let mut frame_copy = [0u8; 330];
                                        frame_copy[..flen].copy_from_slice(
                                            &frame_data[..flen],
                                        );
                                        let mut frame_payload = [0u8; 340];
                                        let fp_len = FramePayload::encode(
                                            seq,
                                            &frame_copy[..flen],
                                            &mut frame_payload,
                                        );
                                        send_msg(
                                            sender,
                                            MSG_FRAME,
                                            &frame_payload[..fp_len],
                                        )
                                        .await?;
                                        chunk_frames += 1;
                                    }
                                }
                            }
                        }
                    }

                    let elapsed_us = start.elapsed().as_micros() as u32;
                    let synthetic_cycles = elapsed_us.wrapping_mul(CPU_FREQ_MHZ);
                    stats.record_chunk(synthetic_cycles, chunk_frames);

                    let ack = ChunkAckPayload {
                        seq,
                        cycles: synthetic_cycles,
                    };
                    let mut ack_payload = [0u8; 6];
                    ack.encode(&mut ack_payload);
                    send_msg(sender, MSG_CHUNK_ACK, &ack_payload).await?;
                    continue;
                }

                MSG_STREAM_END => {
                    if read_pos > total {
                        read_buf.copy_within(total..read_pos, 0);
                    }
                    read_pos -= total;

                    let stats_payload = StatsPayload {
                        total_frames: stats.total_frames,
                        chunks: stats.chunks,
                        total_cycles: stats.total_cycles,
                        min_cycles: if stats.min_cycles == u32::MAX {
                            0
                        } else {
                            stats.min_cycles
                        },
                        max_cycles: stats.max_cycles,
                        avg_cycles: stats.avg_cycles(),
                    };
                    let mut sp = [0u8; 28];
                    stats_payload.encode(&mut sp);
                    send_msg(sender, MSG_STATS, &sp).await?;

                    info!(
                        "Stream done: {} frames in {} chunks",
                        stats.total_frames, stats.chunks
                    );
                    continue;
                }

                _ => {} // unknown — ignore
            }

            // Shift remaining data for unhandled message types
            if read_pos > total {
                read_buf.copy_within(total..read_pos, 0);
            }
            read_pos -= total;
        }
    }
}
