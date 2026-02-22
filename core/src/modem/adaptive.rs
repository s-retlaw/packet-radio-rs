//! Adaptive Preamble Tracker
//!
//! Estimates the actual mark/space frequencies, baud rate, and signal level
//! of each transmitter by analyzing the preamble (flag byte sequence) that
//! precedes every AX.25 packet.
//!
//! This allows a single decoder to adapt to each transmitter's characteristics,
//! replacing the need for multiple parallel decoders with fixed parameters.

use super::{MARK_FREQ, SPACE_FREQ, MID_FREQ};

/// Tracking state machine
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackState {
    /// No signal detected, waiting for carrier
    Idle,
    /// Receiving preamble, accumulating statistics
    Training,
    /// Estimates locked for duration of this packet
    Locked,
}

/// Adaptive parameter tracker.
///
/// During preamble reception, measures the actual characteristics of the
/// current transmitter and tunes the demodulator accordingly.
pub struct AdaptiveTracker {
    // --- Public estimates (read by demodulator after lock) ---

    /// Estimated mark frequency (Hz × 256, fixed-point)
    pub mark_freq_est: i32,
    /// Estimated space frequency (Hz × 256, fixed-point)
    pub space_freq_est: i32,
    /// Decision threshold: midpoint between mark and space (Hz × 256)
    pub threshold: i32,
    /// Estimated samples per symbol (fixed-point × 256)
    pub samples_per_symbol_est: i32,
    /// Estimated signal envelope level
    pub signal_level: i16,

    // --- Internal state ---

    /// Current tracking state
    state: TrackState,
    /// Accumulator for mark frequency measurements
    mark_accum: i64,
    /// Number of mark frequency samples
    mark_count: u32,
    /// Accumulator for space frequency measurements
    space_accum: i64,
    /// Number of space frequency samples
    space_count: u32,
    /// Ring buffer of time intervals between mark/space transitions
    transition_periods: [u16; 16],
    /// Index into transition ring buffer
    transition_idx: u8,
    /// Number of transitions recorded
    transition_count: u8,
    /// Sample index of last detected transition
    last_transition_sample: u32,
    /// Previous frequency sample (for transition detection)
    prev_freq: i32,
    /// Nominal midpoint frequency (Hz × 256) for initial classification
    nominal_mid: i32,
    /// Minimum samples before locking (configurable)
    min_training_samples: u32,
    /// Carrier detect threshold
    carrier_threshold: i32,
}

impl AdaptiveTracker {
    /// Create a new tracker with default parameters.
    pub fn new(sample_rate: u32) -> Self {
        let nominal_mid = (MID_FREQ as i32) * 256;
        let samples_per_symbol = (sample_rate as i32 * 256) / 1200;

        Self {
            mark_freq_est: (MARK_FREQ as i32) * 256,
            space_freq_est: (SPACE_FREQ as i32) * 256,
            threshold: nominal_mid,
            samples_per_symbol_est: samples_per_symbol,
            signal_level: 0,

            state: TrackState::Idle,
            mark_accum: 0,
            mark_count: 0,
            space_accum: 0,
            space_count: 0,
            transition_periods: [0u16; 16],
            transition_idx: 0,
            transition_count: 0,
            last_transition_sample: 0,
            prev_freq: 0,
            nominal_mid,
            min_training_samples: 40,
            carrier_threshold: 200 * 256, // 200 Hz minimum deviation from mid
        }
    }

    /// Get current tracking state.
    pub fn state(&self) -> TrackState {
        self.state
    }

    /// Is the tracker locked onto this packet's characteristics?
    pub fn is_locked(&self) -> bool {
        self.state == TrackState::Locked
    }

    /// Feed an instantaneous frequency sample (Hz × 256, from InstFreqDetector).
    ///
    /// Also provide the absolute sample index for baud rate timing.
    pub fn feed(&mut self, freq_fp: i32, sample_idx: u32) {
        match self.state {
            TrackState::Idle => {
                // Look for signal: frequency should be near mark or space,
                // not just sitting at the midpoint
                let deviation = (freq_fp - self.nominal_mid).abs();
                if deviation > self.carrier_threshold {
                    self.state = TrackState::Training;
                    self.reset_accumulators();
                    self.last_transition_sample = sample_idx;
                    self.prev_freq = freq_fp;
                    // Classify this first sample
                    self.classify_sample(freq_fp);
                }
            }
            TrackState::Training => {
                // Classify sample and accumulate statistics
                self.classify_sample(freq_fp);

                // Detect mark/space transitions for baud rate estimation
                let crossed = (freq_fp > self.nominal_mid) != (self.prev_freq > self.nominal_mid);
                if crossed {
                    let period = sample_idx.wrapping_sub(self.last_transition_sample);
                    if period > 0 && period < 1000 {
                        // Valid transition period (sanity check)
                        self.transition_periods[self.transition_idx as usize] = period as u16;
                        self.transition_idx = (self.transition_idx + 1) % 16;
                        self.transition_count = self.transition_count.saturating_add(1);
                    }
                    self.last_transition_sample = sample_idx;
                }
                self.prev_freq = freq_fp;

                // Lock when we have enough data
                if self.mark_count >= self.min_training_samples
                    && self.space_count >= self.min_training_samples
                {
                    self.finalize();
                }
            }
            TrackState::Locked => {
                // Estimates are fixed for the rest of this packet.
                // Optionally: slow adaptation could continue here.
            }
        }
    }

    /// Convert an instantaneous frequency to a soft bit (LLR) using
    /// the tracker's current estimates.
    ///
    /// Returns: +127 = definitely mark (1), −127 = definitely space (0).
    pub fn freq_to_llr(&self, freq_fp: i32) -> i8 {
        let half_sep = (self.space_freq_est - self.mark_freq_est) / 2;
        if half_sep <= 0 {
            return 0;
        }

        // Distance from threshold, normalized to separation
        let distance = ((freq_fp - self.threshold) as i64 * 127) / half_sep as i64;
        // Invert: mark (lower freq) → positive LLR
        (-distance).clamp(-127, 127) as i8
    }

    /// Reset the tracker for a new packet.
    pub fn reset(&mut self) {
        self.state = TrackState::Idle;
        self.reset_accumulators();
        // Restore nominal values
        self.mark_freq_est = (MARK_FREQ as i32) * 256;
        self.space_freq_est = (SPACE_FREQ as i32) * 256;
        self.threshold = self.nominal_mid;
    }

    // --- Private methods ---

    fn classify_sample(&mut self, freq_fp: i32) {
        if freq_fp > self.nominal_mid {
            // Closer to space (2200 Hz)
            self.space_accum += freq_fp as i64;
            self.space_count += 1;
        } else {
            // Closer to mark (1200 Hz)
            self.mark_accum += freq_fp as i64;
            self.mark_count += 1;
        }
    }

    fn finalize(&mut self) {
        // Average frequencies
        if self.mark_count > 0 {
            self.mark_freq_est = (self.mark_accum / self.mark_count as i64) as i32;
        }
        if self.space_count > 0 {
            self.space_freq_est = (self.space_accum / self.space_count as i64) as i32;
        }
        self.threshold = (self.mark_freq_est + self.space_freq_est) / 2;

        // Baud rate from transition periods
        if self.transition_count >= 4 {
            self.estimate_baud_rate();
        }

        self.state = TrackState::Locked;
    }

    fn estimate_baud_rate(&mut self) {
        // Use median of transition periods for robustness
        let count = (self.transition_count as usize).min(16);
        let mut periods = [0u16; 16];
        periods[..count].copy_from_slice(&self.transition_periods[..count]);
        periods[..count].sort_unstable();
        let median = periods[count / 2] as i32;

        // Each transition period is approximately half a symbol
        // (mark-to-space or space-to-mark), but during preamble flags
        // the pattern is more complex. Use as rough baud rate estimate.
        if median > 0 {
            // samples_per_symbol ≈ 2 × median transition period
            // (for alternating mark/space, each half is one transition)
            self.samples_per_symbol_est = median * 256; // Already in sample units, ×256 for fp
        }
    }

    fn reset_accumulators(&mut self) {
        self.mark_accum = 0;
        self.mark_count = 0;
        self.space_accum = 0;
        self.space_count = 0;
        self.transition_idx = 0;
        self.transition_count = 0;
        self.transition_periods = [0u16; 16];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let tracker = AdaptiveTracker::new(11025);
        assert_eq!(tracker.state(), TrackState::Idle);
        assert!(!tracker.is_locked());
    }

    #[test]
    fn test_carrier_detect() {
        let mut tracker = AdaptiveTracker::new(11025);

        // Feed frequency near mark — should trigger training
        let mark_fp = 1200 * 256;
        tracker.feed(mark_fp, 0);
        assert_eq!(tracker.state(), TrackState::Training);
    }

    #[test]
    fn test_no_false_carrier_detect() {
        let mut tracker = AdaptiveTracker::new(11025);

        // Feed frequency at exact midpoint — should NOT trigger
        let mid_fp = 1700 * 256;
        tracker.feed(mid_fp, 0);
        assert_eq!(tracker.state(), TrackState::Idle);
    }

    #[test]
    fn test_training_to_locked() {
        let mut tracker = AdaptiveTracker::new(11025);
        let mark_fp = 1210 * 256; // Slightly off-nominal mark
        let space_fp = 2190 * 256; // Slightly off-nominal space

        // Feed alternating mark/space to simulate preamble
        for i in 0..200 {
            let freq = if i % 18 < 9 { mark_fp } else { space_fp };
            // 9 samples ≈ 1 symbol at 11025/1200 ≈ 9.2 samples/symbol
            tracker.feed(freq, i);
        }

        assert!(tracker.is_locked(), "Should be locked after 200 samples");

        // Estimates should be near the input values
        let mark_err = (tracker.mark_freq_est - mark_fp).abs();
        let space_err = (tracker.space_freq_est - space_fp).abs();
        assert!(mark_err < 10 * 256,
            "Mark estimate off by {} Hz", mark_err / 256);
        assert!(space_err < 10 * 256,
            "Space estimate off by {} Hz", space_err / 256);
    }

    #[test]
    fn test_llr_output_range() {
        let tracker = AdaptiveTracker::new(11025);

        // At mark frequency: should be strongly positive
        let llr_mark = tracker.freq_to_llr(1200 * 256);
        assert!(llr_mark > 100, "Mark LLR should be strongly positive, got {}", llr_mark);

        // At space frequency: should be strongly negative
        let llr_space = tracker.freq_to_llr(2200 * 256);
        assert!(llr_space < -100, "Space LLR should be strongly negative, got {}", llr_space);

        // At midpoint: should be near zero
        let llr_mid = tracker.freq_to_llr(1700 * 256);
        assert!(llr_mid.abs() < 10, "Midpoint LLR should be ~0, got {}", llr_mid);
    }

    #[test]
    fn test_reset() {
        let mut tracker = AdaptiveTracker::new(11025);
        // Trigger training
        tracker.feed(1200 * 256, 0);
        assert_eq!(tracker.state(), TrackState::Training);

        tracker.reset();
        assert_eq!(tracker.state(), TrackState::Idle);
        assert_eq!(tracker.mark_count, 0);
        assert_eq!(tracker.space_count, 0);
    }
}
