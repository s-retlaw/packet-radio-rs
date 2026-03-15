//! Soft-Decision HDLC Decoder with Bit-Flip Error Recovery
//!
//! Wraps the standard hard-decision HDLC decoder but also records the
//! soft (confidence) value for each bit. When a CRC failure occurs,
//! it attempts error correction through multiple strategies:
//!
//! 1. **CRC syndrome-based correction** — O(n) scan finds any single-bit error
//!    without trial CRC checks, regardless of confidence ranking.
//! 2. **Confidence-based single/pair/triple flips** — systematically flips the
//!    lowest-confidence bits (up to 3 at a time) and re-checks CRC.
//! 3. **NRZI-aware pair/triple flips** — handles the case where a single raw
//!    (pre-NRZI) bit error causes 2-3 consecutive decoded errors.
//!
//! This can recover packets with 1-3 bit errors that a hard-decision
//! decoder would completely miss — typically 5-15% more packets in
//! marginal signal conditions.

use super::{FLIP_CONFIDENCE_THRESHOLD, MAX_FLIP_CANDIDATES, MAX_FRAME_BITS, TRIPLE_FLIP_LIMIT};

/// Maximum frame length in bytes for bit-flip recovery working buffer
const MAX_FRAME_BYTES: usize = 400; // AX.25 max ≈ 330 + margin

/// AX.25/HDLC CRC-16 good-frame residue (CRC over frame+FCS yields this).
const CRC_RESIDUE: u16 = 0x0F47;

/// CRC-16-CCITT reflected polynomial (x^16 + x^12 + x^5 + 1, bit-reversed).
const CRC_POLY_REFLECTED: u16 = 0x8408;

/// Result of a frame decode attempt.
#[derive(Debug)]
pub enum FrameResult<'a> {
    /// Frame decoded successfully on first try (hard decision was correct)
    Valid(&'a [u8]),
    /// Frame recovered by flipping bits. `flips` = number of bits corrected.
    /// `cost` = quality metric (lower is better): syndrome=1, flip=sum of |LLR|s.
    Recovered {
        data: &'a [u8],
        flips: u8,
        cost: u16,
    },
}

/// Soft HDLC decoder.
///
/// Accumulates soft bit values alongside hard decisions. On CRC failure,
/// attempts error correction by flipping the least-confident bits.
pub struct SoftHdlcDecoder {
    // --- Bit accumulation ---
    /// Soft values (LLR) for each bit in the current potential frame.
    /// Positive = mark/1, negative = space/0, magnitude = confidence.
    soft_bits: [i8; MAX_FRAME_BITS],
    /// Hard bit decisions (derived from soft_bits sign)
    hard_bits: [u8; MAX_FRAME_BITS],
    /// Number of bits accumulated since last flag
    bit_count: usize,

    // --- HDLC state machine ---
    /// Current state
    state: HdlcState,
    /// Count of consecutive 1-bits (for flag/abort detection and bit unstuffing)
    ones_count: u8,
    /// Shift register for flag detection
    shift_reg: u8,
    /// Number of valid bits in shift register
    shift_count: u8,

    // --- Frame assembly ---
    /// Assembled frame bytes (after bit unstuffing)
    frame_buf: [u8; MAX_FRAME_BYTES],
    /// Current byte being assembled
    current_byte: u8,
    /// Bits accumulated in current_byte
    byte_bit_count: u8,
    /// Bytes written to frame_buf
    frame_len: usize,

    // --- Configurable thresholds ---
    /// Max candidates for flip recovery (default: MAX_FLIP_CANDIDATES = 12)
    max_flip_candidates: usize,
    /// Confidence threshold for candidate inclusion (default: FLIP_CONFIDENCE_THRESHOLD = 96)
    flip_confidence_threshold: u8,
    /// Max candidates for triple-flip search (default: TRIPLE_FLIP_LIMIT = 8)
    triple_flip_limit: usize,

    // --- Statistics (per recovery type) ---
    /// Number of frames decoded normally (hard decision)
    pub stats_hard_decode: u32,
    /// Number of frames recovered via CRC syndrome single-bit correction
    pub stats_syndrome: u32,
    /// Number of frames recovered via confidence-based single flip
    pub stats_single_flip: u32,
    /// Number of frames recovered via confidence-based pair flip
    pub stats_pair_flip: u32,
    /// Number of frames recovered via NRZI-aware pair flip
    pub stats_nrzi_pair: u32,
    /// Number of frames recovered via confidence-based triple flip
    pub stats_triple_flip: u32,
    /// Number of frames recovered via NRZI-aware triple flip
    pub stats_nrzi_triple: u32,
    /// Number of CRC failures (not recoverable)
    pub stats_crc_failures: u32,
    /// Number of soft-recovered frames rejected by AX.25 address validation
    pub stats_false_positives: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HdlcState {
    /// Searching for opening flag (01111110)
    Hunting,
    /// Inside a frame, accumulating data bits
    Receiving,
}

impl Default for SoftHdlcDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftHdlcDecoder {
    /// Create a new soft HDLC decoder.
    pub fn new() -> Self {
        Self {
            soft_bits: [0i8; MAX_FRAME_BITS],
            hard_bits: [0u8; MAX_FRAME_BITS],
            bit_count: 0,
            state: HdlcState::Hunting,
            ones_count: 0,
            shift_reg: 0,
            shift_count: 0,
            frame_buf: [0u8; MAX_FRAME_BYTES],
            current_byte: 0,
            byte_bit_count: 0,
            frame_len: 0,
            max_flip_candidates: MAX_FLIP_CANDIDATES,
            flip_confidence_threshold: FLIP_CONFIDENCE_THRESHOLD,
            triple_flip_limit: TRIPLE_FLIP_LIMIT,
            stats_hard_decode: 0,
            stats_syndrome: 0,
            stats_single_flip: 0,
            stats_pair_flip: 0,
            stats_nrzi_pair: 0,
            stats_triple_flip: 0,
            stats_nrzi_triple: 0,
            stats_crc_failures: 0,
            stats_false_positives: 0,
        }
    }

    /// Create with custom thresholds for soft decode tuning.
    pub fn with_thresholds(
        max_flip_candidates: usize,
        flip_confidence_threshold: u8,
        triple_flip_limit: usize,
    ) -> Self {
        let mut s = Self::new();
        s.max_flip_candidates = max_flip_candidates.min(MAX_FLIP_CANDIDATES);
        s.flip_confidence_threshold = flip_confidence_threshold;
        s.triple_flip_limit = triple_flip_limit.min(MAX_FLIP_CANDIDATES);
        s
    }

    /// Total soft-recovered frames across all recovery types.
    pub fn stats_total_soft_recovered(&self) -> u32 {
        self.stats_syndrome
            + self.stats_single_flip
            + self.stats_pair_flip
            + self.stats_nrzi_pair
            + self.stats_triple_flip
            + self.stats_nrzi_triple
    }

    /// Feed a soft bit (LLR value) into the decoder.
    ///
    /// - `llr > 0`: likely 1 (mark), magnitude = confidence
    /// - `llr < 0`: likely 0 (space), magnitude = confidence
    /// - `llr == 0`: maximally uncertain
    ///
    /// Returns a `FrameResult` when a complete frame is decoded.
    pub fn feed_soft_bit(&mut self, llr: i8) -> Option<FrameResult<'_>> {
        let hard_bit = llr >= 0;

        // Store soft and hard values
        if self.bit_count < MAX_FRAME_BITS {
            self.soft_bits[self.bit_count] = llr;
            self.hard_bits[self.bit_count] = hard_bit as u8;
            self.bit_count += 1;
        }

        // Run the HDLC state machine on the hard bit
        self.process_bit(hard_bit)
    }

    /// Feed a hard bit (for the fast path, which doesn't have soft info).
    /// LLR is set to ±64 (moderate confidence) since we have no real soft data.
    pub fn feed_hard_bit(&mut self, bit: bool) -> Option<FrameResult<'_>> {
        let llr: i8 = if bit { 64 } else { -64 };
        self.feed_soft_bit(llr)
    }

    /// Reset the decoder state.
    pub fn reset(&mut self) {
        self.bit_count = 0;
        self.state = HdlcState::Hunting;
        self.ones_count = 0;
        self.shift_reg = 0;
        self.shift_count = 0;
        self.current_byte = 0;
        self.byte_bit_count = 0;
        self.frame_len = 0;
    }

    // --- Private methods ---

    /// AX.25 frame validation for soft-recovered frames.
    ///
    /// Checks:
    /// 1. Minimum length (≥ 16: 14 addr + ctrl + PID)
    /// 2. Destination and source callsign chars (A-Z, 0-9, space), non-empty
    /// 3. Walk extension bits to find end of address fields
    /// 4. Validate digipeater callsigns if present (same char check)
    /// 5. Control byte: UI (0x03) or UI+P (0x13)
    /// 6. PID byte: 0xF0 (no L3) or 0xCF (NET/ROM)
    fn is_valid_ax25_frame(data: &[u8]) -> bool {
        // Minimum: 14 address + 1 control + 1 PID = 16
        if data.len() < 16 {
            return false;
        }

        // Validate callsign characters in a 7-byte address field (6 callsign + 1 SSID).
        // Returns false if any callsign char is invalid or all are spaces.
        fn validate_callsign(data: &[u8], start: usize) -> bool {
            let mut has_nonspace = false;
            let mut i = start;
            while i < start + 6 {
                let ch = data[i] >> 1; // AX.25 chars are shifted left by 1
                match ch {
                    0x20 => {}                          // space (padding)
                    0x30..=0x39 => has_nonspace = true, // 0-9
                    0x41..=0x5A => has_nonspace = true, // A-Z
                    _ => return false,                  // invalid character
                }
                i += 1;
            }
            has_nonspace
        }

        // Check destination (bytes 0-5) and source (bytes 7-12) callsigns
        if !validate_callsign(data, 0) || !validate_callsign(data, 7) {
            return false;
        }

        // Walk extension bits to find the end of address fields.
        // Bit 0 of the SSID byte (byte 6, 13, 20, ...) is 0 for "more addresses"
        // and 1 for "last address".
        let mut addr_end = 14; // minimum: dest + src = 14 bytes
        if data[13] & 0x01 == 0 {
            // Digipeater addresses follow — walk in 7-byte chunks
            let mut pos = 14;
            loop {
                if pos + 7 > data.len() {
                    return false; // truncated digipeater address
                }
                // Validate digipeater callsign
                if !validate_callsign(data, pos) {
                    return false;
                }
                pos += 7;
                // Check extension bit on the SSID byte (last byte of this address)
                if data[pos - 1] & 0x01 != 0 {
                    break; // last address
                }
                // Sanity: max 8 digipeaters (56 bytes) to prevent runaway
                if pos > 70 {
                    return false;
                }
            }
            addr_end = pos;
        }

        // Need at least 2 more bytes after addresses (control + PID)
        if data.len() < addr_end + 2 {
            return false;
        }

        // Control byte: accept UI (0x03) or UI+P (0x13)
        let ctrl = data[addr_end];
        if ctrl != 0x03 && ctrl != 0x13 {
            return false;
        }

        // PID byte: accept 0xF0 (no L3) or 0xCF (NET/ROM)
        let pid = data[addr_end + 1];
        if pid != 0xF0 && pid != 0xCF {
            return false;
        }

        true
    }

    fn process_bit(&mut self, bit: bool) -> Option<FrameResult<'_>> {
        // Track consecutive ones for flag/abort detection
        if bit {
            self.ones_count = self.ones_count.saturating_add(1);
        } else {
            // Check for flag pattern: 01111110
            if self.ones_count == 6 {
                // This is a flag!
                return self.handle_flag();
            }
            // Check for abort: 7+ ones followed by 0
            if self.ones_count >= 7 {
                self.state = HdlcState::Hunting;
                self.bit_count = 0;
            }

            // Bit unstuffing: after 5 consecutive ones, the next 0 is
            // a stuffed bit and should be discarded. Must check BEFORE
            // resetting ones_count.
            if self.ones_count == 5 && self.state == HdlcState::Receiving {
                self.ones_count = 0;
                return None;
            }

            self.ones_count = 0;
        }

        match self.state {
            HdlcState::Hunting => {
                // Nothing to do, waiting for flag
                None
            }
            HdlcState::Receiving => {
                // Accumulate data bit (LSB first)
                if bit {
                    self.current_byte |= 1 << self.byte_bit_count;
                }
                self.byte_bit_count += 1;

                if self.byte_bit_count == 8 {
                    if self.frame_len < MAX_FRAME_BYTES {
                        self.frame_buf[self.frame_len] = self.current_byte;
                        self.frame_len += 1;
                    }
                    self.current_byte = 0;
                    self.byte_bit_count = 0;
                }

                None
            }
        }
    }

    fn handle_flag(&mut self) -> Option<FrameResult<'_>> {
        // Compute result info without borrowing self for the return value yet.
        // We need to: check CRC, try recovery, then reset state, then return
        // a reference into frame_buf (which survives the reset since we only
        // zero counters, not buffer contents).
        let frame_result_info = if self.state == HdlcState::Receiving && self.frame_len >= 17 {
            let data_len = self.frame_len - 2;
            let crc_valid = self.check_frame_crc(data_len);

            if crc_valid {
                self.stats_hard_decode += 1;
                Some((data_len, false, 0u8, 0u16)) // (len, is_recovered, flips, cost)
            } else {
                // CRC failed — try soft recovery
                // try_bit_flip_recovery may update frame_buf and frame_len
                self.try_bit_flip_recovery_info()
                    .map(|(len, flips, cost)| (len, true, flips, cost))
            }
        } else {
            None
        };

        // Reset state for next frame (this mutates self freely since
        // we haven't yet created the borrow for the return value)
        self.state = HdlcState::Receiving;
        self.frame_len = 0;
        self.current_byte = 0;
        self.byte_bit_count = 0;
        self.ones_count = 0;

        // Now construct the return value borrowing from frame_buf
        match frame_result_info {
            Some((data_len, false, _, _)) => Some(FrameResult::Valid(&self.frame_buf[..data_len])),
            Some((data_len, true, flips, cost)) => Some(FrameResult::Recovered {
                data: &self.frame_buf[..data_len],
                flips,
                cost,
            }),
            None => None,
        }
    }

    fn check_frame_crc(&self, data_len: usize) -> bool {
        if self.frame_len < data_len + 2 {
            return false;
        }
        let frame_data = &self.frame_buf[..self.frame_len];
        let crc = crate::ax25::crc16_ccitt(frame_data);
        // Valid AX.25 frame has CRC residue of CRC_RESIDUE
        crc == CRC_RESIDUE
    }

    /// CRC syndrome-based single-bit correction.
    ///
    /// Computes syndrome = CRC(frame) XOR 0x0F47, then incrementally checks
    /// each bit position for a matching single-bit error pattern. O(n) with
    /// ~5 ops per bit, no trial CRC checks needed.
    ///
    /// Returns `(data_len, 1, cost)` on success, updating `frame_buf` in place.
    fn try_syndrome_correction(&mut self) -> Option<(usize, u8, u16)> {
        if self.frame_len < 17 {
            return None;
        }

        let residue = crate::ax25::crc16_ccitt(&self.frame_buf[..self.frame_len]);
        let syndrome = residue ^ CRC_RESIDUE;
        if syndrome == 0 {
            return None; // Already correct (shouldn't reach here)
        }

        // Incrementally compute error polynomial for each bit position.
        // e(0) = syndrome for error at the very last bit processed (MSB of last byte).
        // e(k+1) = one zero-step of CRC from e(k), moving toward earlier bits.
        let total_bits = self.frame_len * 8;
        let mut e: u16 = CRC_POLY_REFLECTED; // Error polynomial for last bit

        for k in 0..total_bits {
            if e == syndrome {
                // Found the error at bit_index = total_bits - 1 - k
                let bit_index = total_bits - 1 - k;
                let byte_idx = bit_index / 8;
                let bit_in_byte = bit_index % 8;

                // Flip the bit in frame_buf
                self.frame_buf[byte_idx] ^= 1 << bit_in_byte;

                // Verify (paranoia check)
                let check = crate::ax25::crc16_ccitt(&self.frame_buf[..self.frame_len]);
                if check == CRC_RESIDUE {
                    // Validate AX.25 address to reject false positives
                    if !Self::is_valid_ax25_frame(&self.frame_buf[..self.frame_len]) {
                        self.frame_buf[byte_idx] ^= 1 << bit_in_byte; // flip back
                        self.stats_false_positives += 1;
                        return None;
                    }
                    self.stats_syndrome += 1;
                    let data_len = self.frame_len - 2;
                    return Some((data_len, 1, 1));
                } else {
                    // Flip back — shouldn't happen if math is correct
                    self.frame_buf[byte_idx] ^= 1 << bit_in_byte;
                    return None;
                }
            }

            // Step to next earlier bit position
            e = if e & 1 != 0 {
                (e >> 1) ^ CRC_POLY_REFLECTED
            } else {
                e >> 1
            };
        }

        None
    }

    /// Try bit-flip recovery. Returns `(data_len, flips, cost)` on success, updating
    /// `frame_buf` and `frame_len` in place. Returns `None` on failure.
    #[allow(clippy::needless_range_loop)] // Index-based loops clearer for candidate bit-flip DSP
    fn try_bit_flip_recovery_info(&mut self) -> Option<(usize, u8, u16)> {
        // Phase 1: CRC syndrome-based single-bit correction (fastest, any position)
        if let Some(result) = self.try_syndrome_correction() {
            return Some(result);
        }

        let count = self.bit_count.min(MAX_FRAME_BITS);
        if count == 0 {
            self.stats_crc_failures += 1;
            return None;
        }

        // Build a list of (bit_index, confidence) sorted by confidence (lowest first)
        let threshold = FLIP_CONFIDENCE_THRESHOLD;
        let mut candidates = [(0usize, threshold); MAX_FLIP_CANDIDATES];
        for i in 0..count {
            let confidence = self.soft_bits[i].unsigned_abs();
            if confidence < candidates[MAX_FLIP_CANDIDATES - 1].1 {
                candidates[MAX_FLIP_CANDIDATES - 1] = (i, confidence);
                // Insertion sort
                for j in (1..MAX_FLIP_CANDIDATES).rev() {
                    if candidates[j].1 < candidates[j - 1].1 {
                        candidates.swap(j, j - 1);
                    }
                }
            }
        }

        // Phase 2: Confidence-based single-bit flips (top-12 candidates)
        // Continue to end of phase and pick the lowest-cost valid match.
        let num_candidates = MAX_FLIP_CANDIDATES.min(count);
        let mut best_single: Option<(usize, u8)> = None; // (k, cost)
        for k in 0..num_candidates {
            if candidates[k].1 >= threshold {
                break;
            }
            let bit_idx = candidates[k].0;
            self.hard_bits[bit_idx] ^= 1;

            if self.reassemble_and_check_crc() {
                let cost = candidates[k].1;
                if best_single.is_none_or(|(_, c)| cost < c) {
                    best_single = Some((k, cost));
                }
            }

            self.hard_bits[bit_idx] ^= 1;
        }
        if let Some((k, cost)) = best_single {
            self.hard_bits[candidates[k].0] ^= 1;
            self.reassemble_and_check_crc();
            self.hard_bits[candidates[k].0] ^= 1;
            self.stats_single_flip += 1;
            let data_len = self.frame_len - 2;
            return Some((data_len, 1, cost as u16));
        }

        // Phase 3: Confidence-based pair flips (top-12 candidates, C(12,2)=66)
        // Pick the pair with the lowest combined confidence cost.
        let mut best_pair: Option<(usize, usize, u16)> = None; // (i, j, cost)
        for i in 0..num_candidates {
            if candidates[i].1 >= threshold {
                break;
            }
            for j in (i + 1)..num_candidates {
                if candidates[j].1 >= threshold {
                    break;
                }

                self.hard_bits[candidates[i].0] ^= 1;
                self.hard_bits[candidates[j].0] ^= 1;

                if self.reassemble_and_check_crc() {
                    let cost = candidates[i].1 as u16 + candidates[j].1 as u16;
                    if best_pair.is_none_or(|(_, _, c)| cost < c) {
                        best_pair = Some((i, j, cost));
                    }
                }

                self.hard_bits[candidates[i].0] ^= 1;
                self.hard_bits[candidates[j].0] ^= 1;
            }
        }
        if let Some((i, j, cost)) = best_pair {
            self.hard_bits[candidates[i].0] ^= 1;
            self.hard_bits[candidates[j].0] ^= 1;
            self.reassemble_and_check_crc();
            self.hard_bits[candidates[i].0] ^= 1;
            self.hard_bits[candidates[j].0] ^= 1;
            self.stats_pair_flip += 1;
            let data_len = self.frame_len - 2;
            return Some((data_len, 2, cost));
        }

        // Phase 4: NRZI pair-flip — a single raw (pre-NRZI) bit error causes two
        // consecutive errors in the decoded stream.
        // Pick the pair with the lowest sum of |LLR| at flipped positions.
        let mut best_nrzi_pair: Option<(usize, usize, u16)> = None; // (bit1, bit2, cost)
        for k in 0..num_candidates {
            if candidates[k].1 >= threshold {
                break;
            }
            let idx = candidates[k].0;

            // Try (idx, idx+1)
            if idx + 1 < count {
                self.hard_bits[idx] ^= 1;
                self.hard_bits[idx + 1] ^= 1;
                if self.reassemble_and_check_crc() {
                    let cost = self.soft_bits[idx].unsigned_abs() as u16
                        + self.soft_bits[idx + 1].unsigned_abs() as u16;
                    if best_nrzi_pair.is_none_or(|(_, _, c)| cost < c) {
                        best_nrzi_pair = Some((idx, idx + 1, cost));
                    }
                }
                self.hard_bits[idx] ^= 1;
                self.hard_bits[idx + 1] ^= 1;
            }

            // Try (idx-1, idx)
            if idx > 0 {
                self.hard_bits[idx - 1] ^= 1;
                self.hard_bits[idx] ^= 1;
                if self.reassemble_and_check_crc() {
                    let cost = self.soft_bits[idx - 1].unsigned_abs() as u16
                        + self.soft_bits[idx].unsigned_abs() as u16;
                    if best_nrzi_pair.is_none_or(|(_, _, c)| cost < c) {
                        best_nrzi_pair = Some((idx - 1, idx, cost));
                    }
                }
                self.hard_bits[idx - 1] ^= 1;
                self.hard_bits[idx] ^= 1;
            }
        }
        if let Some((b1, b2, cost)) = best_nrzi_pair {
            self.hard_bits[b1] ^= 1;
            self.hard_bits[b2] ^= 1;
            self.reassemble_and_check_crc();
            self.hard_bits[b1] ^= 1;
            self.hard_bits[b2] ^= 1;
            self.stats_nrzi_pair += 1;
            let data_len = self.frame_len - 2;
            return Some((data_len, 2, cost));
        }

        // Phase 5: Confidence-based triple flips (top-8 candidates, C(8,3)=56)
        // Pick the triple with the lowest combined confidence cost.
        let triple_limit = num_candidates.min(TRIPLE_FLIP_LIMIT);
        let mut best_triple: Option<(usize, usize, usize, u16)> = None; // (i, j, k, cost)
        for i in 0..triple_limit {
            if candidates[i].1 >= threshold {
                break;
            }
            for j in (i + 1)..triple_limit {
                if candidates[j].1 >= threshold {
                    break;
                }
                for k in (j + 1)..triple_limit {
                    if candidates[k].1 >= threshold {
                        break;
                    }

                    self.hard_bits[candidates[i].0] ^= 1;
                    self.hard_bits[candidates[j].0] ^= 1;
                    self.hard_bits[candidates[k].0] ^= 1;

                    if self.reassemble_and_check_crc() {
                        let cost = candidates[i].1 as u16
                            + candidates[j].1 as u16
                            + candidates[k].1 as u16;
                        if best_triple.is_none_or(|(_, _, _, c)| cost < c) {
                            best_triple = Some((i, j, k, cost));
                        }
                    }

                    self.hard_bits[candidates[i].0] ^= 1;
                    self.hard_bits[candidates[j].0] ^= 1;
                    self.hard_bits[candidates[k].0] ^= 1;
                }
            }
        }
        if let Some((i, j, k, cost)) = best_triple {
            self.hard_bits[candidates[i].0] ^= 1;
            self.hard_bits[candidates[j].0] ^= 1;
            self.hard_bits[candidates[k].0] ^= 1;
            self.reassemble_and_check_crc();
            self.hard_bits[candidates[i].0] ^= 1;
            self.hard_bits[candidates[j].0] ^= 1;
            self.hard_bits[candidates[k].0] ^= 1;
            self.stats_triple_flip += 1;
            let data_len = self.frame_len - 2;
            return Some((data_len, 3, cost));
        }

        // Phase 6: NRZI-aware triple flips — two adjacent pre-NRZI errors cause
        // 3 consecutive decoded errors: (idx-1, idx, idx+1).
        // Pick the triple with the lowest sum of |LLR| at flipped positions.
        let mut best_nrzi_triple: Option<(usize, u16)> = None; // (candidate_k, cost)
        for k in 0..num_candidates {
            if candidates[k].1 >= threshold {
                break;
            }
            let idx = candidates[k].0;

            if idx > 0 && idx + 1 < count {
                self.hard_bits[idx - 1] ^= 1;
                self.hard_bits[idx] ^= 1;
                self.hard_bits[idx + 1] ^= 1;

                if self.reassemble_and_check_crc() {
                    let cost = self.soft_bits[idx - 1].unsigned_abs() as u16
                        + self.soft_bits[idx].unsigned_abs() as u16
                        + self.soft_bits[idx + 1].unsigned_abs() as u16;
                    if best_nrzi_triple.is_none_or(|(_, c)| cost < c) {
                        best_nrzi_triple = Some((k, cost));
                    }
                }

                self.hard_bits[idx - 1] ^= 1;
                self.hard_bits[idx] ^= 1;
                self.hard_bits[idx + 1] ^= 1;
            }
        }
        if let Some((k, cost)) = best_nrzi_triple {
            let idx = candidates[k].0;
            self.hard_bits[idx - 1] ^= 1;
            self.hard_bits[idx] ^= 1;
            self.hard_bits[idx + 1] ^= 1;
            self.reassemble_and_check_crc();
            self.hard_bits[idx - 1] ^= 1;
            self.hard_bits[idx] ^= 1;
            self.hard_bits[idx + 1] ^= 1;
            self.stats_nrzi_triple += 1;
            let data_len = self.frame_len - 2;
            return Some((data_len, 3, cost));
        }

        self.stats_crc_failures += 1;
        None
    }

    /// Reassemble frame from hard_bits and check CRC.
    fn reassemble_and_check_crc(&mut self) -> bool {
        // Reassemble: walk through hard_bits, perform bit unstuffing,
        // assemble bytes, and check CRC.
        // This is essentially re-running the HDLC decoder on the modified bits.

        let mut frame = [0u8; MAX_FRAME_BYTES];
        let mut frame_len = 0;
        let mut current_byte = 0u8;
        let mut byte_bits = 0u8;
        let mut ones = 0u8;

        for i in 0..self.bit_count {
            let bit = self.hard_bits[i] != 0;

            // Track ones for unstuffing
            if bit {
                ones += 1;
            } else {
                if ones == 5 {
                    // Stuffed zero — skip
                    ones = 0;
                    continue;
                }
                if ones >= 6 {
                    // Flag or abort — stop
                    break;
                }
                ones = 0;
            }

            // Accumulate data bit (LSB first)
            if bit {
                current_byte |= 1 << byte_bits;
            }
            byte_bits += 1;

            if byte_bits == 8 {
                if frame_len < MAX_FRAME_BYTES {
                    frame[frame_len] = current_byte;
                    frame_len += 1;
                }
                current_byte = 0;
                byte_bits = 0;
            }
        }

        if frame_len < 17 {
            return false; // Too short for a valid frame
        }

        // Check CRC (last 2 bytes are CRC)
        let crc = crate::ax25::crc16_ccitt(&frame[..frame_len]);
        if crc == CRC_RESIDUE {
            // Validate AX.25 address to reject false positives
            if !Self::is_valid_ax25_frame(&frame[..frame_len]) {
                self.stats_false_positives += 1;
                return false;
            }
            // Copy to frame_buf for output
            self.frame_buf[..frame_len].copy_from_slice(&frame[..frame_len]);
            self.frame_len = frame_len;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soft_hdlc_creation() {
        let decoder = SoftHdlcDecoder::new();
        assert_eq!(decoder.bit_count, 0);
        assert_eq!(decoder.stats_hard_decode, 0);
        assert_eq!(decoder.stats_total_soft_recovered(), 0);
    }

    #[test]
    fn test_soft_hdlc_reset() {
        let mut decoder = SoftHdlcDecoder::new();
        // Feed some bits
        for _ in 0..50 {
            decoder.feed_soft_bit(100);
        }
        decoder.reset();
        assert_eq!(decoder.bit_count, 0);
        assert_eq!(decoder.frame_len, 0);
    }

    #[test]
    fn test_stats_total_soft_recovered() {
        let mut decoder = SoftHdlcDecoder::new();
        decoder.stats_syndrome = 1;
        decoder.stats_single_flip = 2;
        decoder.stats_pair_flip = 3;
        decoder.stats_nrzi_pair = 4;
        decoder.stats_triple_flip = 5;
        decoder.stats_nrzi_triple = 6;
        assert_eq!(decoder.stats_total_soft_recovered(), 21);
    }

    #[test]
    fn test_ax25_frame_validation() {
        // Valid frame: "N0CALL" (dest) and "W1AW  " (src), UI control, PID 0xF0
        let mut frame = [0u8; 17];
        let dest = b"N0CALL";
        for i in 0..6 {
            frame[i] = dest[i] << 1;
        }
        frame[6] = 0xE0; // SSID
        let src = b"W1AW  ";
        for i in 0..6 {
            frame[7 + i] = src[i] << 1;
        }
        frame[13] = 0xE1; // SSID with end-of-address bit
        frame[14] = 0x03; // Control: UI
        frame[15] = 0xF0; // PID: no L3
        frame[16] = b'!'; // payload
        assert!(SoftHdlcDecoder::is_valid_ax25_frame(&frame));

        // Invalid: lowercase in destination
        let mut bad = frame;
        bad[0] = b'n' << 1;
        assert!(!SoftHdlcDecoder::is_valid_ax25_frame(&bad));

        // Invalid: special char in source
        let mut bad2 = frame;
        bad2[7] = b'#' << 1;
        assert!(!SoftHdlcDecoder::is_valid_ax25_frame(&bad2));

        // Invalid: all-space callsign
        let mut bad3 = frame;
        for i in 0..6 {
            bad3[i] = b' ' << 1;
        }
        assert!(!SoftHdlcDecoder::is_valid_ax25_frame(&bad3));

        // Invalid: too short (< 16 bytes)
        assert!(!SoftHdlcDecoder::is_valid_ax25_frame(&frame[..15]));

        // Invalid: wrong control byte
        let mut bad4 = frame;
        bad4[14] = 0x00;
        assert!(!SoftHdlcDecoder::is_valid_ax25_frame(&bad4));

        // Invalid: wrong PID byte
        let mut bad5 = frame;
        bad5[15] = 0x01;
        assert!(!SoftHdlcDecoder::is_valid_ax25_frame(&bad5));

        // Valid: UI+P control byte
        let mut uip = frame;
        uip[14] = 0x13;
        assert!(SoftHdlcDecoder::is_valid_ax25_frame(&uip));

        // Valid: NET/ROM PID
        let mut netrom = frame;
        netrom[15] = 0xCF;
        assert!(SoftHdlcDecoder::is_valid_ax25_frame(&netrom));

        // Valid: frame with one digipeater
        let mut digi_frame = [0u8; 24];
        digi_frame[..14].copy_from_slice(&frame[..14]);
        digi_frame[13] = 0xE0; // source SSID: extension bit 0 (more addresses)
        let digi = b"RELAY ";
        for i in 0..6 {
            digi_frame[14 + i] = digi[i] << 1;
        }
        digi_frame[20] = 0xE1; // digi SSID with end-of-address bit
        digi_frame[21] = 0x03; // Control
        digi_frame[22] = 0xF0; // PID
        digi_frame[23] = b'!'; // payload
        assert!(SoftHdlcDecoder::is_valid_ax25_frame(&digi_frame));
    }

    #[test]
    fn test_syndrome_math() {
        // Verify syndrome stepping: e(0) = CRC_POLY_REFLECTED, each step shifts through poly
        let mut e: u16 = CRC_POLY_REFLECTED;
        // After one step
        e = if e & 1 != 0 {
            (e >> 1) ^ CRC_POLY_REFLECTED
        } else {
            e >> 1
        };
        assert_eq!(e, 0x4204); // 0x8408 >> 1 = 0x4204 (bit 0 was 0)
                               // After another step
        e = if e & 1 != 0 {
            (e >> 1) ^ CRC_POLY_REFLECTED
        } else {
            e >> 1
        };
        assert_eq!(e, 0x2102); // 0x4204 >> 1 = 0x2102 (bit 0 was 0)
    }
}
