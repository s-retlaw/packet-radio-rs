//! TNC Engine — transport-agnostic KISS TNC state machine.
//!
//! `TncEngine` is a `no_std`, poll-based state machine that owns the modem
//! and KISS logic. Platform code feeds audio in, reads audio out, and pipes
//! KISS bytes to/from any transport (serial, USB, BLE, WiFi, TCP).
//!
//! # Architecture
//!
//! ```text
//!                     ┌─────────────────────────────────┐
//!  Audio In (i16) ──▶ │         TncEngine<D, M>         │ ──▶ Audio Out (i16)
//!                     │                                   │
//!  KISS bytes in ──▶  │  KissDecoder → TxQueue → CSMA    │
//!                     │  Demod → HDLC → KissEncoder ──▶   │ ──▶ KISS bytes out
//!                     │                                   │
//!                     │  Callbacks: PTT, DCD, Random      │
//!                     └─────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use packet_radio_core::tnc::*;
//! use packet_radio_core::modem::{DemodConfig, ModConfig};
//!
//! let demod = FastAdapter::new(DemodConfig::default_1200());
//! let modulator = AfskModulateAdapter::new(ModConfig::default_1200());
//! let mut tnc = TncEngine::new(demod, modulator, TncConfig::default());
//!
//! // Platform code main loop:
//! loop {
//!     tnc.poll_rx(&rx_samples, &mut platform);
//!     let n = tnc.poll_tx(&mut tx_buf, &mut platform);
//!     let k = tnc.read_kiss(&mut kiss_buf);
//!     for &b in &transport_bytes { tnc.feed_kiss(b); }
//! }
//! ```

use crate::MAX_FRAME_LEN;
use crate::kiss::{self, KissDecoder, Command};
use crate::ax25::crc16_ccitt;
use crate::modem::afsk::AfskModulator;
use crate::modem::demod::{DemodSymbol, FastDemodulator};
use crate::modem::soft_hdlc::{SoftHdlcDecoder, FrameResult};
use crate::ax25::frame::HdlcDecoder;
use crate::modem::{DemodConfig, ModConfig};
#[cfg(feature = "9600-baud")]
use crate::modem::scrambler::Scrambler;
#[cfg(feature = "9600-baud")]
use crate::modem::mod_9600::Mod9600Config;

#[cfg(feature = "multi-decoder")]
use crate::modem::multi::{MiniDecoder, MultiDecoder};

// ── Traits ──────────────────────────────────────────────────────────────

/// Demodulator trait — wraps any decoder (Fast, Quality, Mini, Multi).
///
/// Feed audio samples, collect decoded AX.25 frames via callback.
pub trait Demodulate {
    /// Process audio samples. Calls `handler` for each decoded AX.25 frame.
    fn process_audio(&mut self, samples: &[i16], handler: &mut dyn FnMut(&[u8]));
}

/// Modulator trait — generates AFSK audio from individual bits.
///
/// The TNC engine handles HDLC framing (flags, bit stuffing, CRC) and
/// calls `modulate_bit` for each bit in sequence.
pub trait Modulate {
    /// Modulate a single bit using NRZI encoding.
    /// Writes ~`samples_per_symbol()` audio samples to `out`.
    /// Returns number of samples written.
    fn modulate_bit(&mut self, bit: bool, out: &mut [i16]) -> usize;

    /// Number of audio samples per symbol (approximate).
    fn samples_per_symbol(&self) -> usize;
}

/// Platform callbacks for hardware control.
pub trait TncPlatform {
    /// Assert/deassert PTT (push-to-talk).
    fn set_ptt(&mut self, on: bool);

    /// Check if channel is busy (DCD — data carrier detect).
    fn channel_busy(&self) -> bool;

    /// Random byte for p-persist CSMA slot decision.
    fn random_byte(&self) -> u8;

    /// Current time in milliseconds (for CSMA slot timing).
    fn now_ms(&self) -> u32;
}

// ── Configuration ───────────────────────────────────────────────────────

/// TNC configuration (KISS parameters + baud rate).
#[derive(Clone, Debug)]
pub struct TncConfig {
    /// TX preamble delay in 10ms units (default 50 = 500ms)
    pub txdelay: u8,
    /// CSMA persistence 0-255, probability = (p+1)/256 (default 63 ≈ 25%)
    pub persistence: u8,
    /// CSMA slot time in 10ms units (default 10 = 100ms)
    pub slottime: u8,
    /// TX postamble in 10ms units (default 10 = 100ms)
    pub txtail: u8,
    /// Full duplex mode (skip CSMA)
    pub full_duplex: bool,
    /// Baud rate for flag timing calculations
    pub baud_rate: u32,
}

impl Default for TncConfig {
    fn default() -> Self {
        Self {
            txdelay: 50,
            persistence: 63,
            slottime: 10,
            txtail: 10,
            full_duplex: false,
            baud_rate: 1200,
        }
    }
}

// ── KissOutbox ──────────────────────────────────────────────────────────

const KISS_OUTBOX_SIZE: usize = 2048;

/// Fixed-size ring buffer for KISS-encoded output bytes.
///
/// Decoded frames are KISS-encoded into this buffer. Platform code
/// drains it to whichever transport(s) are active.
pub struct KissOutbox {
    buf: [u8; KISS_OUTBOX_SIZE],
    head: usize,
    tail: usize,
}

impl KissOutbox {
    /// Create an empty outbox.
    pub const fn new() -> Self {
        Self {
            buf: [0u8; KISS_OUTBOX_SIZE],
            head: 0,
            tail: 0,
        }
    }

    /// Number of bytes available to read.
    pub fn len(&self) -> usize {
        if self.tail >= self.head {
            self.tail - self.head
        } else {
            KISS_OUTBOX_SIZE - self.head + self.tail
        }
    }

    /// Whether the outbox is empty.
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// Free space available for writing.
    pub fn free_space(&self) -> usize {
        KISS_OUTBOX_SIZE - 1 - self.len()
    }

    /// Write bytes into the ring buffer. Returns false if insufficient space.
    pub fn write(&mut self, data: &[u8]) -> bool {
        if data.len() > self.free_space() {
            return false;
        }
        for &b in data {
            self.buf[self.tail] = b;
            self.tail = (self.tail + 1) % KISS_OUTBOX_SIZE;
        }
        true
    }

    /// Read bytes from the ring buffer. Returns number of bytes read.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let available = self.len();
        let n = buf.len().min(available);
        for i in 0..n {
            buf[i] = self.buf[self.head];
            self.head = (self.head + 1) % KISS_OUTBOX_SIZE;
        }
        n
    }

    /// KISS-encode an AX.25 frame and write it to the outbox.
    pub fn write_kiss_frame(&mut self, frame: &[u8]) -> bool {
        // Max KISS-encoded size: 2 FEND + 1 cmd + data with worst-case escaping
        let mut tmp = [0u8; 700];
        if let Some(n) = kiss::encode_frame(0, frame, &mut tmp) {
            self.write(&tmp[..n])
        } else {
            false
        }
    }
}

// ── TxQueue ─────────────────────────────────────────────────────────────

/// Number of TX frame slots (4 for MCU, configurable for desktop).
const TX_QUEUE_SIZE: usize = 4;

/// Circular buffer of pending TX frames.
pub struct TxQueue {
    frames: [[u8; MAX_FRAME_LEN]; TX_QUEUE_SIZE],
    lengths: [usize; TX_QUEUE_SIZE],
    head: usize,
    count: usize,
}

impl TxQueue {
    /// Create an empty TX queue.
    pub const fn new() -> Self {
        Self {
            frames: [[0u8; MAX_FRAME_LEN]; TX_QUEUE_SIZE],
            lengths: [0usize; TX_QUEUE_SIZE],
            head: 0,
            count: 0,
        }
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether the queue is full.
    pub fn is_full(&self) -> bool {
        self.count >= TX_QUEUE_SIZE
    }

    /// Number of queued frames.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Enqueue a frame. Returns false if queue is full or frame is too large.
    pub fn enqueue(&mut self, data: &[u8]) -> bool {
        if self.is_full() || data.len() > MAX_FRAME_LEN {
            return false;
        }
        let idx = (self.head + self.count) % TX_QUEUE_SIZE;
        self.frames[idx][..data.len()].copy_from_slice(data);
        self.lengths[idx] = data.len();
        self.count += 1;
        true
    }

    /// Dequeue a frame into the provided buffer. Returns frame length, or None if empty.
    pub fn dequeue_into(&mut self, out: &mut [u8; MAX_FRAME_LEN]) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        let idx = self.head;
        let len = self.lengths[idx];
        out[..len].copy_from_slice(&self.frames[idx][..len]);
        self.head = (self.head + 1) % TX_QUEUE_SIZE;
        self.count -= 1;
        Some(len)
    }
}

// ── TX State Machine ────────────────────────────────────────────────────

/// HDLC flag byte 0x7E in LSB-first bit order.
const FLAG_BITS: [bool; 8] = [false, true, true, true, true, true, true, false];

/// Maximum encoded frame bits (data + CRC + bit stuffing, no flags).
const MAX_TX_BITS: usize = 4096;

/// TX state machine implementing p-persist CSMA.
///
/// ```text
/// Idle → [frame queued] → WaitCsma → [channel clear + persist win] → TxDelay
///                              ↑                                         ↓
///                              └── SlotWait ← [persist lose] ←──── Transmitting
///                                                                        ↓
///                                                                     TxTail → Idle
/// ```
#[derive(Clone, Copy)]
enum TxState {
    /// No transmission in progress.
    Idle,
    /// Waiting for clear channel (CSMA check).
    WaitCsma,
    /// Waiting for slot time to expire before re-checking.
    SlotWait { next_check_ms: u32 },
    /// Transmitting preamble flags (txdelay).
    TxDelay {
        flags_sent: usize,
        flags_total: usize,
        bit_in_flag: u8,
    },
    /// Transmitting frame data (bit-stuffed + CRC).
    Transmitting { bit_index: usize },
    /// Transmitting postamble flags (txtail).
    TxTail {
        flags_sent: usize,
        flags_total: usize,
        bit_in_flag: u8,
    },
}

// ── TNC Engine ──────────────────────────────────────────────────────────

/// Transport-agnostic TNC engine.
///
/// Generic over demodulator `D` and modulator `M`. Platform code
/// calls `poll_rx`, `poll_tx`, `feed_kiss`, and `read_kiss` from
/// whatever event loop it has (bare-metal, RTOS, async, etc).
pub struct TncEngine<D: Demodulate, M: Modulate> {
    demod: D,
    modulator: M,
    kiss_decoder: KissDecoder,
    kiss_out: KissOutbox,
    tx_queue: TxQueue,
    tx_state: TxState,
    config: TncConfig,
    /// Pre-encoded frame bits for current TX (data + CRC, bit-stuffed, no flags).
    tx_bits: [u8; MAX_TX_BITS],
    /// Number of valid bits in tx_bits.
    tx_bit_count: usize,
}

impl<D: Demodulate, M: Modulate> TncEngine<D, M> {
    /// Create a new TNC engine.
    pub fn new(demod: D, modulator: M, config: TncConfig) -> Self {
        Self {
            demod,
            modulator,
            kiss_decoder: KissDecoder::new(),
            kiss_out: KissOutbox::new(),
            tx_queue: TxQueue::new(),
            tx_state: TxState::Idle,
            config,
            tx_bits: [0u8; MAX_TX_BITS],
            tx_bit_count: 0,
        }
    }

    /// Feed received audio samples. Decoded frames are KISS-encoded into the outbox.
    pub fn poll_rx(&mut self, samples: &[i16], _platform: &mut impl TncPlatform) {
        // Split borrow: demod and kiss_out are disjoint fields
        let kiss_out = &mut self.kiss_out;
        self.demod.process_audio(samples, &mut |frame: &[u8]| {
            kiss_out.write_kiss_frame(frame);
        });
    }

    /// Generate TX audio into `out`. Returns number of samples written.
    ///
    /// Call this repeatedly to drain the TX state machine. Returns 0
    /// when idle (no frames queued or channel busy).
    pub fn poll_tx(&mut self, out: &mut [i16], platform: &mut impl TncPlatform) -> usize {
        let sps = self.modulator.samples_per_symbol();
        if sps == 0 {
            return 0;
        }
        let mut written = 0;

        loop {
            // Need room for at least one symbol (Bresenham may produce sps or sps+1)
            if written + sps + 1 > out.len() {
                break;
            }

            match self.tx_state {
                TxState::Idle => {
                    if self.tx_queue.is_empty() {
                        break;
                    }
                    self.tx_state = TxState::WaitCsma;
                }

                TxState::WaitCsma => {
                    if self.config.full_duplex || !platform.channel_busy() {
                        if platform.random_byte() <= self.config.persistence {
                            // Won the slot — begin transmission
                            platform.set_ptt(true);
                            let num_flags = self.txdelay_flags();
                            self.tx_state = TxState::TxDelay {
                                flags_sent: 0,
                                flags_total: num_flags,
                                bit_in_flag: 0,
                            };
                        } else {
                            // Lost the slot — wait one slot time
                            let next = platform
                                .now_ms()
                                .wrapping_add(self.config.slottime as u32 * 10);
                            self.tx_state = TxState::SlotWait {
                                next_check_ms: next,
                            };
                            break;
                        }
                    } else {
                        // Channel busy — try again next poll
                        break;
                    }
                }

                TxState::SlotWait { next_check_ms } => {
                    let elapsed = platform.now_ms().wrapping_sub(next_check_ms);
                    if elapsed < 0x8000_0000 {
                        // Slot time expired — re-check channel
                        self.tx_state = TxState::WaitCsma;
                    } else {
                        break;
                    }
                }

                TxState::TxDelay {
                    flags_sent,
                    flags_total,
                    bit_in_flag,
                } => {
                    let n = self.modulator.modulate_bit(
                        FLAG_BITS[bit_in_flag as usize],
                        &mut out[written..],
                    );
                    written += n;
                    let next_bit = bit_in_flag + 1;
                    if next_bit >= 8 {
                        let next_flags = flags_sent + 1;
                        if next_flags >= flags_total {
                            // Preamble done — encode frame and start transmitting
                            self.begin_frame_tx();
                        } else {
                            self.tx_state = TxState::TxDelay {
                                flags_sent: next_flags,
                                flags_total,
                                bit_in_flag: 0,
                            };
                        }
                    } else {
                        self.tx_state = TxState::TxDelay {
                            flags_sent,
                            flags_total,
                            bit_in_flag: next_bit,
                        };
                    }
                }

                TxState::Transmitting { bit_index } => {
                    if bit_index >= self.tx_bit_count {
                        // Frame data done — start postamble
                        let num_flags = self.txtail_flags();
                        self.tx_state = TxState::TxTail {
                            flags_sent: 0,
                            flags_total: num_flags,
                            bit_in_flag: 0,
                        };
                        continue;
                    }
                    let bit = self.tx_bits[bit_index] != 0;
                    let n = self.modulator.modulate_bit(bit, &mut out[written..]);
                    written += n;
                    self.tx_state = TxState::Transmitting {
                        bit_index: bit_index + 1,
                    };
                }

                TxState::TxTail {
                    flags_sent,
                    flags_total,
                    bit_in_flag,
                } => {
                    let n = self.modulator.modulate_bit(
                        FLAG_BITS[bit_in_flag as usize],
                        &mut out[written..],
                    );
                    written += n;
                    let next_bit = bit_in_flag + 1;
                    if next_bit >= 8 {
                        let next_flags = flags_sent + 1;
                        if next_flags >= flags_total {
                            if !self.tx_queue.is_empty() {
                                // Back-to-back TX: send next frame without
                                // dropping PTT or re-entering CSMA. The tail
                                // flags serve as inter-frame delimiter.
                                self.begin_frame_tx();
                            } else {
                                // Transmission complete
                                platform.set_ptt(false);
                                self.tx_state = TxState::Idle;
                            }
                        } else {
                            self.tx_state = TxState::TxTail {
                                flags_sent: next_flags,
                                flags_total,
                                bit_in_flag: 0,
                            };
                        }
                    } else {
                        self.tx_state = TxState::TxTail {
                            flags_sent,
                            flags_total,
                            bit_in_flag: next_bit,
                        };
                    }
                }
            }
        }

        written
    }

    /// Feed a KISS byte from any transport (serial/USB/BLE/WiFi/TCP).
    ///
    /// Data frames are queued for TX. Configuration commands (TxDelay,
    /// Persistence, SlotTime, TxTail, FullDuplex) update the TNC config.
    pub fn feed_kiss(&mut self, byte: u8) {
        // Copy frame data out to avoid borrow conflict with kiss_decoder
        let mut frame_buf = [0u8; MAX_FRAME_LEN];
        let mut frame_len = 0;
        let mut command = None;

        if let Some((_port, cmd, data)) = self.kiss_decoder.feed_byte(byte) {
            command = Some(cmd);
            frame_len = data.len().min(MAX_FRAME_LEN);
            frame_buf[..frame_len].copy_from_slice(&data[..frame_len]);
        }

        if let Some(cmd) = command {
            match cmd {
                Command::DataFrame => {
                    let _ = self.tx_queue.enqueue(&frame_buf[..frame_len]);
                }
                Command::TxDelay => {
                    if frame_len > 0 {
                        self.config.txdelay = frame_buf[0];
                    }
                }
                Command::Persistence => {
                    if frame_len > 0 {
                        self.config.persistence = frame_buf[0];
                    }
                }
                Command::SlotTime => {
                    if frame_len > 0 {
                        self.config.slottime = frame_buf[0];
                    }
                }
                Command::TxTail => {
                    if frame_len > 0 {
                        self.config.txtail = frame_buf[0];
                    }
                }
                Command::FullDuplex => {
                    if frame_len > 0 {
                        self.config.full_duplex = frame_buf[0] != 0;
                    }
                }
                _ => {}
            }
        }
    }

    /// Read KISS-encoded bytes from the outbox into `buf`.
    /// Returns number of bytes read.
    pub fn read_kiss(&mut self, buf: &mut [u8]) -> usize {
        self.kiss_out.read(buf)
    }

    /// Number of KISS bytes available to read.
    pub fn kiss_available(&self) -> usize {
        self.kiss_out.len()
    }

    /// Whether a transmission is in progress.
    pub fn is_transmitting(&self) -> bool {
        !matches!(self.tx_state, TxState::Idle)
    }

    /// Number of frames queued for transmission.
    pub fn tx_queued(&self) -> usize {
        self.tx_queue.len()
    }

    /// Get a reference to the TNC configuration.
    pub fn config(&self) -> &TncConfig {
        &self.config
    }

    /// Get a mutable reference to the TNC configuration.
    pub fn config_mut(&mut self) -> &mut TncConfig {
        &mut self.config
    }

    /// Get a mutable reference to the demodulator.
    pub fn demod_mut(&mut self) -> &mut D {
        &mut self.demod
    }

    /// Get a mutable reference to the modulator.
    pub fn modulator_mut(&mut self) -> &mut M {
        &mut self.modulator
    }

    // ── Internal helpers ────────────────────────────────────────────────

    /// Calculate number of flag bytes for txdelay.
    fn txdelay_flags(&self) -> usize {
        // txdelay is in 10ms units. Each flag = 8 bits / baud_rate seconds.
        // num_flags = txdelay_ms / flag_ms = (txdelay * 10) / (8000 / baud_rate)
        //           = txdelay * baud_rate / 800
        ((self.config.txdelay as u32 * self.config.baud_rate / 800) as usize).max(1)
    }

    /// Calculate number of flag bytes for txtail.
    fn txtail_flags(&self) -> usize {
        ((self.config.txtail as u32 * self.config.baud_rate / 800) as usize).max(1)
    }

    /// Dequeue a frame, HDLC-encode it (CRC + bit stuffing), and transition
    /// to Transmitting state.
    fn begin_frame_tx(&mut self) {
        let mut frame_data = [0u8; MAX_FRAME_LEN];
        if let Some(frame_len) = self.tx_queue.dequeue_into(&mut frame_data) {
            self.tx_bit_count =
                hdlc_encode_data(&frame_data[..frame_len], &mut self.tx_bits);
            self.tx_state = TxState::Transmitting { bit_index: 0 };
        } else {
            // Queue was empty (shouldn't happen) — go to tail
            let num_flags = self.txtail_flags();
            self.tx_state = TxState::TxTail {
                flags_sent: 0,
                flags_total: num_flags,
                bit_in_flag: 0,
            };
        }
    }
}

// ── HDLC Data Encoder ───────────────────────────────────────────────────

/// Encode frame data with CRC-16-CCITT and HDLC bit stuffing (no flag bytes).
///
/// Writes individual bits (0 or 1) to `bits`. Returns number of bits written.
/// Used by the TX state machine — flag bytes are generated separately.
fn hdlc_encode_data(data: &[u8], bits: &mut [u8]) -> usize {
    let crc = crc16_ccitt(data);
    let crc_lo = crc as u8;
    let crc_hi = (crc >> 8) as u8;

    let mut count = 0;
    let mut ones: u8 = 0;

    for &byte in data.iter().chain(&[crc_lo, crc_hi]) {
        for bit_pos in 0..8u8 {
            if count >= bits.len() {
                return count;
            }
            let bit = (byte >> bit_pos) & 1;
            bits[count] = bit;
            count += 1;
            if bit == 1 {
                ones += 1;
                if ones == 5 {
                    // Insert stuffed zero after five consecutive 1-bits
                    if count >= bits.len() {
                        return count;
                    }
                    bits[count] = 0;
                    count += 1;
                    ones = 0;
                }
            } else {
                ones = 0;
            }
        }
    }

    count
}

// ── Adapters ────────────────────────────────────────────────────────────

/// Symbol buffer size for single-decoder adapters.
/// At 11025 Hz / 1200 baud, 1024 audio samples → ~111 symbols.
const ADAPTER_SYMBOL_BUF: usize = 256;

/// Single Goertzel fast decoder adapter.
pub struct FastAdapter {
    demod: FastDemodulator,
    hdlc: HdlcDecoder,
}

impl FastAdapter {
    /// Create a fast (hard-decision) demodulator adapter.
    pub fn new(config: DemodConfig) -> Self {
        Self {
            demod: FastDemodulator::new(config),
            hdlc: HdlcDecoder::new(),
        }
    }
}

impl Demodulate for FastAdapter {
    fn process_audio(&mut self, samples: &[i16], handler: &mut dyn FnMut(&[u8])) {
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; ADAPTER_SYMBOL_BUF];
        // Process in chunks to avoid symbol buffer overflow (~8 samples per symbol)
        let chunk_size = ADAPTER_SYMBOL_BUF * 8;
        for chunk in samples.chunks(chunk_size) {
            let n = self.demod.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if let Some(frame) = self.hdlc.feed_bit(symbols[i].bit) {
                    handler(frame);
                }
            }
        }
    }
}

/// Single Goertzel + SoftHDLC quality decoder adapter.
pub struct QualityAdapter {
    demod: FastDemodulator,
    hdlc: SoftHdlcDecoder,
}

impl QualityAdapter {
    /// Create a quality (soft-decision) demodulator adapter.
    pub fn new(config: DemodConfig) -> Self {
        Self {
            demod: FastDemodulator::new(config).with_energy_llr(),
            hdlc: SoftHdlcDecoder::new(),
        }
    }
}

impl Demodulate for QualityAdapter {
    fn process_audio(&mut self, samples: &[i16], handler: &mut dyn FnMut(&[u8])) {
        let mut symbols = [DemodSymbol { bit: false, llr: 0 }; ADAPTER_SYMBOL_BUF];
        let chunk_size = ADAPTER_SYMBOL_BUF * 8;
        for chunk in samples.chunks(chunk_size) {
            let n = self.demod.process_samples(chunk, &mut symbols);
            for i in 0..n {
                if let Some(result) = self.hdlc.feed_soft_bit(symbols[i].llr) {
                    let data = match &result {
                        FrameResult::Valid(d) => *d,
                        FrameResult::Recovered { data, .. } => *data,
                    };
                    handler(data);
                }
            }
        }
    }
}

/// MiniDecoder (3-decoder ensemble) adapter.
#[cfg(feature = "multi-decoder")]
pub struct MiniAdapter {
    mini: MiniDecoder,
}

#[cfg(feature = "multi-decoder")]
impl MiniAdapter {
    /// Create a mini-decoder (3 attribution-optimal Goertzel decoders).
    pub fn new(config: DemodConfig) -> Self {
        Self {
            mini: MiniDecoder::new(config),
        }
    }
}

#[cfg(feature = "multi-decoder")]
impl Demodulate for MiniAdapter {
    fn process_audio(&mut self, samples: &[i16], handler: &mut dyn FnMut(&[u8])) {
        let output = self.mini.process_samples(samples);
        for i in 0..output.len() {
            handler(output.frame(i));
        }
    }
}

/// MultiDecoder (38-decoder ensemble) adapter.
#[cfg(feature = "multi-decoder")]
pub struct MultiAdapter {
    multi: MultiDecoder,
}

#[cfg(feature = "multi-decoder")]
impl MultiAdapter {
    /// Create a full multi-decoder (32 Goertzel + 6 DM).
    pub fn new(config: DemodConfig) -> Self {
        Self {
            multi: MultiDecoder::new(config),
        }
    }
}

#[cfg(feature = "multi-decoder")]
impl Demodulate for MultiAdapter {
    fn process_audio(&mut self, samples: &[i16], handler: &mut dyn FnMut(&[u8])) {
        let output = self.multi.process_samples(samples);
        for i in 0..output.len() {
            handler(output.frame(i));
        }
    }
}

/// AFSK modulator adapter.
pub struct AfskModulateAdapter {
    modulator: AfskModulator,
}

impl AfskModulateAdapter {
    /// Create an AFSK modulator adapter.
    pub fn new(config: ModConfig) -> Self {
        Self {
            modulator: AfskModulator::new(config),
        }
    }

    /// Reset the modulator state.
    pub fn reset(&mut self) {
        self.modulator.reset();
    }
}

impl Modulate for AfskModulateAdapter {
    fn modulate_bit(&mut self, bit: bool, out: &mut [i16]) -> usize {
        self.modulator.modulate_bit(bit, out)
    }

    fn samples_per_symbol(&self) -> usize {
        self.modulator.samples_per_symbol()
    }
}

/// 9600 baud G3RUH FSK modulator adapter.
///
/// Implements the `Modulate` trait for 9600 baud: NRZI encode → G3RUH scramble
/// → ±amplitude baseband samples. Uses Bresenham timing for fractional sps.
#[cfg(feature = "9600-baud")]
pub struct Fsk9600ModulateAdapter {
    scrambler: Scrambler,
    prev_nrzi: bool,
    config: Mod9600Config,
    bit_phase: u32,
}

#[cfg(feature = "9600-baud")]
impl Fsk9600ModulateAdapter {
    /// Create a 9600 baud modulator adapter.
    pub fn new(config: Mod9600Config) -> Self {
        Self {
            scrambler: Scrambler::new(),
            prev_nrzi: false,
            config,
            bit_phase: 0,
        }
    }
}

#[cfg(feature = "9600-baud")]
impl Modulate for Fsk9600ModulateAdapter {
    fn modulate_bit(&mut self, bit: bool, out: &mut [i16]) -> usize {
        // NRZI: transition on 0, no transition on 1
        if !bit {
            self.prev_nrzi = !self.prev_nrzi;
        }
        let scrambled = self.scrambler.scramble(self.prev_nrzi);
        let level = if scrambled { self.config.amplitude } else { -self.config.amplitude };

        // Bresenham timing for fractional samples-per-symbol
        self.bit_phase += self.config.sample_rate;
        let count = ((self.bit_phase / self.config.baud_rate) as usize).min(out.len());
        self.bit_phase %= self.config.baud_rate;

        for sample in out[..count].iter_mut() {
            *sample = level;
        }
        count
    }

    fn samples_per_symbol(&self) -> usize {
        (self.config.sample_rate / self.config.baud_rate) as usize
    }
}

/// Null demodulator — no-op for TX-only TNCs.
pub struct NullDemod;

impl Demodulate for NullDemod {
    fn process_audio(&mut self, _: &[i16], _: &mut dyn FnMut(&[u8])) {}
}

/// Null modulator that produces no audio (for RX-only TNCs).
pub struct NullModulate;

impl Modulate for NullModulate {
    fn modulate_bit(&mut self, _bit: bool, _out: &mut [i16]) -> usize {
        0
    }

    fn samples_per_symbol(&self) -> usize {
        0
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;

    use super::*;
    use crate::ax25::frame::{build_test_frame, hdlc_encode};

    /// Test platform for unit tests.
    struct TestPlatform {
        ptt: bool,
        busy: bool,
        random: u8,
        time_ms: u32,
    }

    impl TestPlatform {
        fn new() -> Self {
            Self {
                ptt: false,
                busy: false,
                random: 0,
                time_ms: 0,
            }
        }
    }

    impl TncPlatform for TestPlatform {
        fn set_ptt(&mut self, on: bool) {
            self.ptt = on;
        }
        fn channel_busy(&self) -> bool {
            self.busy
        }
        fn random_byte(&self) -> u8 {
            self.random
        }
        fn now_ms(&self) -> u32 {
            self.time_ms
        }
    }

    // ── KissOutbox tests ────────────────────────────────────────────

    #[test]
    fn test_kiss_outbox_empty() {
        let outbox = KissOutbox::new();
        assert!(outbox.is_empty());
        assert_eq!(outbox.len(), 0);
        assert_eq!(outbox.free_space(), KISS_OUTBOX_SIZE - 1);
    }

    #[test]
    fn test_kiss_outbox_write_read() {
        let mut outbox = KissOutbox::new();
        let data = b"Hello, TNC!";
        assert!(outbox.write(data));
        assert_eq!(outbox.len(), data.len());

        let mut buf = [0u8; 32];
        let n = outbox.read(&mut buf);
        assert_eq!(n, data.len());
        assert_eq!(&buf[..n], data.as_slice());
        assert!(outbox.is_empty());
    }

    #[test]
    fn test_kiss_outbox_wrap_around() {
        let mut outbox = KissOutbox::new();

        // Fill most of the buffer
        let big = [0xAA; 2000];
        assert!(outbox.write(&big));
        // Drain it
        let mut drain = [0u8; 2000];
        outbox.read(&mut drain);

        // Now head is at 2000. Write data that wraps around.
        let wrap_data = [0xBB; 100];
        assert!(outbox.write(&wrap_data));
        assert_eq!(outbox.len(), 100);

        let mut buf = [0u8; 100];
        let n = outbox.read(&mut buf);
        assert_eq!(n, 100);
        assert!(buf.iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn test_kiss_outbox_full() {
        let mut outbox = KissOutbox::new();
        let big = [0u8; KISS_OUTBOX_SIZE - 1];
        assert!(outbox.write(&big));
        // One more byte should fail
        assert!(!outbox.write(&[0u8]));
    }

    #[test]
    fn test_kiss_outbox_write_kiss_frame() {
        let mut outbox = KissOutbox::new();
        let frame = b"test frame data";
        assert!(outbox.write_kiss_frame(frame));
        assert!(outbox.len() > 0);

        // Read and verify it's a valid KISS frame
        let mut buf = [0u8; 256];
        let n = outbox.read(&mut buf);
        // Should start and end with FEND
        assert_eq!(buf[0], kiss::FEND);
        assert_eq!(buf[n - 1], kiss::FEND);
    }

    // ── TxQueue tests ───────────────────────────────────────────────

    #[test]
    fn test_tx_queue_empty() {
        let queue = TxQueue::new();
        assert!(queue.is_empty());
        assert!(!queue.is_full());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_tx_queue_enqueue_dequeue() {
        let mut queue = TxQueue::new();
        let data = b"test frame";
        assert!(queue.enqueue(data));
        assert_eq!(queue.len(), 1);

        let mut buf = [0u8; MAX_FRAME_LEN];
        let len = queue.dequeue_into(&mut buf).unwrap();
        assert_eq!(&buf[..len], data.as_slice());
        assert!(queue.is_empty());
    }

    #[test]
    fn test_tx_queue_fifo_order() {
        let mut queue = TxQueue::new();
        queue.enqueue(b"first");
        queue.enqueue(b"second");
        queue.enqueue(b"third");

        let mut buf = [0u8; MAX_FRAME_LEN];
        let len = queue.dequeue_into(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"first");
        let len = queue.dequeue_into(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"second");
        let len = queue.dequeue_into(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"third");
    }

    #[test]
    fn test_tx_queue_full() {
        let mut queue = TxQueue::new();
        for i in 0..TX_QUEUE_SIZE {
            assert!(queue.enqueue(&[i as u8]), "enqueue {i} should succeed");
        }
        assert!(queue.is_full());
        assert!(!queue.enqueue(&[0xFF]), "enqueue beyond capacity should fail");
    }

    #[test]
    fn test_tx_queue_wrap_around() {
        let mut queue = TxQueue::new();
        // Fill and drain a few times to exercise wrap-around
        for _ in 0..3 {
            for i in 0..TX_QUEUE_SIZE {
                assert!(queue.enqueue(&[i as u8]));
            }
            for i in 0..TX_QUEUE_SIZE {
                let mut buf = [0u8; MAX_FRAME_LEN];
                let len = queue.dequeue_into(&mut buf).unwrap();
                assert_eq!(buf[0], i as u8);
                assert_eq!(len, 1);
            }
            assert!(queue.is_empty());
        }
    }

    // ── TncConfig tests ─────────────────────────────────────────────

    #[test]
    fn test_tnc_config_defaults() {
        let config = TncConfig::default();
        assert_eq!(config.txdelay, 50);
        assert_eq!(config.persistence, 63);
        assert_eq!(config.slottime, 10);
        assert_eq!(config.txtail, 10);
        assert!(!config.full_duplex);
        assert_eq!(config.baud_rate, 1200);
    }

    // ── hdlc_encode_data tests ──────────────────────────────────────

    #[test]
    fn test_hdlc_encode_data_produces_bits() {
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Test");
        let mut bits = [0u8; MAX_TX_BITS];
        let count = hdlc_encode_data(&frame_data[..frame_len], &mut bits);
        // 20 data bytes + 2 CRC = 22 bytes × 8 = 176 bits + stuffing
        assert!(count >= 176, "expected at least 176 bits, got {count}");
        // All values should be 0 or 1
        assert!(bits[..count].iter().all(|&b| b <= 1));
    }

    #[test]
    fn test_hdlc_encode_data_roundtrip() {
        // Verify that flag + encoded data + flag decodes correctly
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Hello!");
        let raw = &frame_data[..frame_len];

        // Encode data (no flags)
        let mut data_bits = [0u8; MAX_TX_BITS];
        let data_count = hdlc_encode_data(raw, &mut data_bits);

        // Build full bit stream: flags + data + flag
        let mut all_bits = [0u8; MAX_TX_BITS + 100];
        let mut pos = 0;

        // 4 preamble flags
        for _ in 0..4 {
            for &bit in &[0u8, 1, 1, 1, 1, 1, 1, 0] {
                all_bits[pos] = bit;
                pos += 1;
            }
        }
        // Data bits
        all_bits[pos..pos + data_count].copy_from_slice(&data_bits[..data_count]);
        pos += data_count;
        // 1 postamble flag
        for &bit in &[0u8, 1, 1, 1, 1, 1, 1, 0] {
            all_bits[pos] = bit;
            pos += 1;
        }

        // Decode
        let mut decoder = HdlcDecoder::new();
        let mut decoded: Option<([u8; 330], usize)> = None;
        for i in 0..pos {
            if let Some(frame) = decoder.feed_bit(all_bits[i] != 0) {
                let mut buf = [0u8; 330];
                let len = frame.len();
                buf[..len].copy_from_slice(frame);
                decoded = Some((buf, len));
            }
        }

        let (dec_buf, dec_len) = decoded.expect("should decode a frame");
        assert_eq!(&dec_buf[..dec_len], raw);
    }

    // ── Adapter tests ───────────────────────────────────────────────

    /// Generate AFSK audio for a test frame using the modulator pipeline.
    /// Includes extended preamble (50 flags) for demodulator timing sync.
    fn generate_test_audio(frame: &[u8]) -> ([i16; 65536], usize) {
        let encoded = hdlc_encode(frame);
        let mut modulator = AfskModulator::new(ModConfig::default_1200());
        let mut audio = [0i16; 65536];
        let mut total = 0;

        // Extended preamble for Bresenham timing to sync
        for _ in 0..50 {
            let n = modulator.modulate_flag(&mut audio[total..]);
            total += n;
        }

        // Modulate the encoded frame (4 flags + data + CRC + 1 flag)
        for i in 0..encoded.bit_count {
            let n = modulator.modulate_bit(encoded.bits[i] != 0, &mut audio[total..]);
            total += n;
        }

        // Trailing silence for decoder flush
        for _ in 0..100 {
            if total < audio.len() {
                audio[total] = 0;
                total += 1;
            }
        }

        (audio, total)
    }

    #[test]
    fn test_fast_adapter_decode() {
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Hello!");
        let raw = &frame_data[..frame_len];
        let (audio, audio_len) = generate_test_audio(raw);

        let mut adapter = FastAdapter::new(DemodConfig::default_1200());
        let mut decoded_frames: Vec<Vec<u8>> = Vec::new();
        adapter.process_audio(&audio[..audio_len], &mut |frame: &[u8]| {
            decoded_frames.push(frame.to_vec());
        });

        assert_eq!(decoded_frames.len(), 1, "should decode exactly one frame");
        assert_eq!(decoded_frames[0].as_slice(), raw);
    }

    #[test]
    fn test_quality_adapter_decode() {
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Quality!");
        let raw = &frame_data[..frame_len];
        let (audio, audio_len) = generate_test_audio(raw);

        let mut adapter = QualityAdapter::new(DemodConfig::default_1200());
        let mut decoded_frames: Vec<Vec<u8>> = Vec::new();
        adapter.process_audio(&audio[..audio_len], &mut |frame: &[u8]| {
            decoded_frames.push(frame.to_vec());
        });

        assert_eq!(decoded_frames.len(), 1, "should decode exactly one frame");
        assert_eq!(decoded_frames[0].as_slice(), raw);
    }

    // ── TncEngine tests ─────────────────────────────────────────────

    #[test]
    fn test_engine_poll_rx() {
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"RX test!");
        let raw = &frame_data[..frame_len];
        let (audio, audio_len) = generate_test_audio(raw);

        let demod = FastAdapter::new(DemodConfig::default_1200());
        let modulator = NullModulate;
        let mut tnc = TncEngine::new(demod, modulator, TncConfig::default());
        let mut platform = TestPlatform::new();

        tnc.poll_rx(&audio[..audio_len], &mut platform);

        // Should have KISS data available
        assert!(tnc.kiss_available() > 0);

        // Read and decode KISS output
        let mut kiss_buf = [0u8; 2048];
        let n = tnc.read_kiss(&mut kiss_buf);
        assert!(n > 0);

        let mut decoder = KissDecoder::new();
        let mut received = None;
        for &b in &kiss_buf[..n] {
            if let Some((_port, cmd, data)) = decoder.feed_byte(b) {
                if cmd == Command::DataFrame {
                    let mut frame = [0u8; MAX_FRAME_LEN];
                    let flen = data.len().min(MAX_FRAME_LEN);
                    frame[..flen].copy_from_slice(&data[..flen]);
                    received = Some((frame, flen));
                }
            }
        }

        let (rx_frame, rx_len) = received.expect("should receive a KISS data frame");
        assert_eq!(&rx_frame[..rx_len], raw);
    }

    #[test]
    fn test_engine_feed_kiss_queues_frame() {
        let demod = FastAdapter::new(DemodConfig::default_1200());
        let modulator = AfskModulateAdapter::new(ModConfig::default_1200());
        let mut tnc = TncEngine::new(demod, modulator, TncConfig::default());

        // Send a KISS data frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"TX test!");
        let mut kiss_buf = [0u8; 700];
        let kiss_len = kiss::encode_frame(0, &frame_data[..frame_len], &mut kiss_buf).unwrap();

        for &b in &kiss_buf[..kiss_len] {
            tnc.feed_kiss(b);
        }

        assert_eq!(tnc.tx_queued(), 1);
    }

    #[test]
    fn test_engine_kiss_config_commands() {
        let demod = FastAdapter::new(DemodConfig::default_1200());
        let modulator = NullModulate;
        let mut tnc = TncEngine::new(demod, modulator, TncConfig::default());

        // Send TxDelay command: FEND + (port=0, cmd=0x01) + value + FEND
        let txdelay_cmd = [kiss::FEND, 0x01, 30, kiss::FEND];
        for &b in &txdelay_cmd {
            tnc.feed_kiss(b);
        }
        assert_eq!(tnc.config().txdelay, 30);

        // Send Persistence command
        let persist_cmd = [kiss::FEND, 0x02, 127, kiss::FEND];
        for &b in &persist_cmd {
            tnc.feed_kiss(b);
        }
        assert_eq!(tnc.config().persistence, 127);

        // Send SlotTime command
        let slot_cmd = [kiss::FEND, 0x03, 5, kiss::FEND];
        for &b in &slot_cmd {
            tnc.feed_kiss(b);
        }
        assert_eq!(tnc.config().slottime, 5);

        // Send TxTail command
        let tail_cmd = [kiss::FEND, 0x04, 20, kiss::FEND];
        for &b in &tail_cmd {
            tnc.feed_kiss(b);
        }
        assert_eq!(tnc.config().txtail, 20);

        // Send FullDuplex command
        let fd_cmd = [kiss::FEND, 0x05, 1, kiss::FEND];
        for &b in &fd_cmd {
            tnc.feed_kiss(b);
        }
        assert!(tnc.config().full_duplex);
    }

    #[test]
    fn test_engine_poll_tx_generates_audio() {
        let demod = FastAdapter::new(DemodConfig::default_1200());
        let modulator = AfskModulateAdapter::new(ModConfig::default_1200());
        let mut config = TncConfig::default();
        config.txdelay = 5; // Short preamble for testing
        config.txtail = 2;
        let mut tnc = TncEngine::new(demod, modulator, config);
        let mut platform = TestPlatform::new();

        // Queue a frame via KISS
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"TX!");
        let mut kiss_buf = [0u8; 700];
        let kiss_len = kiss::encode_frame(0, &frame_data[..frame_len], &mut kiss_buf).unwrap();
        for &b in &kiss_buf[..kiss_len] {
            tnc.feed_kiss(b);
        }

        // Generate TX audio
        let mut audio = [0i16; 65536];
        let mut total = 0;
        for _ in 0..10000 {
            let n = tnc.poll_tx(&mut audio[total..], &mut platform);
            if n == 0 {
                break;
            }
            total += n;
        }

        assert!(total > 0, "should generate TX audio");
        assert!(!tnc.is_transmitting(), "should be idle after TX completes");
        assert!(platform.ptt == false, "PTT should be deasserted after TX");
    }

    #[test]
    fn test_engine_ptt_lifecycle() {
        let demod = FastAdapter::new(DemodConfig::default_1200());
        let modulator = AfskModulateAdapter::new(ModConfig::default_1200());
        let mut config = TncConfig::default();
        config.txdelay = 1;
        config.txtail = 1;
        let mut tnc = TncEngine::new(demod, modulator, config);
        let mut platform = TestPlatform::new();

        // Queue frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"PTT");
        let mut kiss_buf = [0u8; 700];
        let kiss_len = kiss::encode_frame(0, &frame_data[..frame_len], &mut kiss_buf).unwrap();
        for &b in &kiss_buf[..kiss_len] {
            tnc.feed_kiss(b);
        }

        assert!(!platform.ptt, "PTT should start off");

        // First poll_tx should assert PTT
        let mut audio = [0i16; 64];
        tnc.poll_tx(&mut audio, &mut platform);
        assert!(platform.ptt, "PTT should be on during TX");

        // Drain the rest
        let mut big_buf = [0i16; 65536];
        loop {
            let n = tnc.poll_tx(&mut big_buf, &mut platform);
            if n == 0 {
                break;
            }
        }
        assert!(!platform.ptt, "PTT should be off after TX completes");
    }

    #[test]
    fn test_engine_loopback() {
        let demod_config = DemodConfig::default_1200();
        let mod_config = ModConfig::default_1200();

        // Build test frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Loopback test!");
        let raw = &frame_data[..frame_len];

        // TX side: generate audio via TncEngine
        let tx_demod = FastAdapter::new(demod_config);
        let tx_mod = AfskModulateAdapter::new(mod_config.clone());
        let mut tx_config = TncConfig::default();
        tx_config.txdelay = 10; // Enough preamble for demod to sync
        tx_config.txtail = 3;
        let mut tx_tnc = TncEngine::new(tx_demod, tx_mod, tx_config);
        let mut tx_platform = TestPlatform::new();

        // Feed KISS frame to TX side
        let mut kiss_buf = [0u8; 700];
        let kiss_len = kiss::encode_frame(0, raw, &mut kiss_buf).unwrap();
        for &b in &kiss_buf[..kiss_len] {
            tx_tnc.feed_kiss(b);
        }

        // Generate TX audio
        let mut tx_audio = [0i16; 65536];
        let mut tx_total = 0;
        loop {
            let n = tx_tnc.poll_tx(&mut tx_audio[tx_total..], &mut tx_platform);
            if n == 0 {
                break;
            }
            tx_total += n;
        }
        assert!(tx_total > 0, "TX should generate audio");

        // RX side: feed audio and extract KISS output
        let rx_demod = FastAdapter::new(demod_config);
        let rx_mod = NullModulate;
        let mut rx_tnc = TncEngine::new(rx_demod, rx_mod, TncConfig::default());
        let mut rx_platform = TestPlatform::new();

        rx_tnc.poll_rx(&tx_audio[..tx_total], &mut rx_platform);

        // Read KISS output
        let mut kiss_out = [0u8; 2048];
        let n = rx_tnc.read_kiss(&mut kiss_out);
        assert!(n > 0, "RX should produce KISS output");

        // Decode KISS output
        let mut decoder = KissDecoder::new();
        let mut received = None;
        for &b in &kiss_out[..n] {
            if let Some((_port, cmd, data)) = decoder.feed_byte(b) {
                if cmd == Command::DataFrame {
                    let mut frame = [0u8; MAX_FRAME_LEN];
                    let flen = data.len().min(MAX_FRAME_LEN);
                    frame[..flen].copy_from_slice(&data[..flen]);
                    received = Some((frame, flen));
                }
            }
        }

        let (rx_frame, rx_len) = received.expect("should receive loopback frame");
        assert_eq!(
            &rx_frame[..rx_len],
            raw,
            "loopback frame should match original"
        );
    }

    #[test]
    fn test_engine_csma_channel_busy() {
        let demod = FastAdapter::new(DemodConfig::default_1200());
        let modulator = AfskModulateAdapter::new(ModConfig::default_1200());
        let mut tnc = TncEngine::new(demod, modulator, TncConfig::default());
        let mut platform = TestPlatform::new();
        platform.busy = true; // Channel busy

        // Queue a frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Busy");
        let mut kiss_buf = [0u8; 700];
        let kiss_len = kiss::encode_frame(0, &frame_data[..frame_len], &mut kiss_buf).unwrap();
        for &b in &kiss_buf[..kiss_len] {
            tnc.feed_kiss(b);
        }

        // poll_tx should not generate audio when channel is busy
        let mut audio = [0i16; 1024];
        let n = tnc.poll_tx(&mut audio, &mut platform);
        assert_eq!(n, 0, "should not transmit when channel busy");
        assert!(!platform.ptt);
    }

    #[test]
    fn test_engine_csma_persist_fail() {
        let demod = FastAdapter::new(DemodConfig::default_1200());
        let modulator = AfskModulateAdapter::new(ModConfig::default_1200());
        let mut config = TncConfig::default();
        config.persistence = 0; // Very low persistence
        let mut tnc = TncEngine::new(demod, modulator, config);
        let mut platform = TestPlatform::new();
        platform.random = 255; // Always lose the slot (255 > 0)

        // Queue a frame
        let (frame_data, frame_len) = build_test_frame("N0CALL", "APRS", b"Fail");
        let mut kiss_buf = [0u8; 700];
        let kiss_len = kiss::encode_frame(0, &frame_data[..frame_len], &mut kiss_buf).unwrap();
        for &b in &kiss_buf[..kiss_len] {
            tnc.feed_kiss(b);
        }

        // First poll_tx should enter SlotWait (channel clear but persist fails)
        let mut audio = [0i16; 1024];
        let n = tnc.poll_tx(&mut audio, &mut platform);
        assert_eq!(n, 0, "should not transmit when persist check fails");
    }

    #[test]
    fn test_null_modulate() {
        let mut null_mod = NullModulate;
        let mut buf = [0i16; 64];
        assert_eq!(null_mod.modulate_bit(true, &mut buf), 0);
        assert_eq!(null_mod.samples_per_symbol(), 0);
    }
}
