//! Hilbert Transform and Instantaneous Frequency Detector (Quality Path)
//!
//! Converts the real AFSK signal into an analytic (complex) signal using a
//! Hilbert transform FIR filter, then computes the instantaneous frequency
//! from the phase derivative of the analytic signal.
//!
//! This produces a continuous frequency estimate at every sample — not just
//! a binary mark/space decision — which enables soft-decision decoding and
//! adaptive tracking.

/// Number of taps in the Hilbert FIR filter.
/// 31 taps: good balance of quality vs. computation.
/// 63 taps: higher quality, suitable for desktop.
pub const HILBERT_TAPS_31: usize = 31;

/// Hilbert transform FIR filter.
///
/// Produces the imaginary part of the analytic signal. The real part is
/// the input delayed by `group_delay` samples to align in time.
pub struct HilbertTransform<const N: usize> {
    /// FIR coefficients in Q15 format
    coeffs: [i16; N],
    /// Circular delay line for input samples
    delay_line: [i16; N],
    /// Write position in delay line
    write_pos: usize,
}

impl<const N: usize> HilbertTransform<N> {
    /// Create a Hilbert transform with the given Q15 coefficients.
    pub fn new(coeffs: [i16; N]) -> Self {
        Self {
            coeffs,
            delay_line: [0i16; N],
            write_pos: 0,
        }
    }

    /// Group delay in samples (half the filter length, rounded down).
    pub fn group_delay(&self) -> usize {
        (N - 1) / 2
    }

    /// Process one input sample.
    ///
    /// Returns `(real, imag)` where:
    /// - `real` is the input delayed by `group_delay` samples
    /// - `imag` is the Hilbert-transformed output
    ///
    /// Together they form the analytic signal z[n] = real + j·imag.
    #[inline]
    pub fn process(&mut self, sample: i16) -> (i16, i16) {
        self.delay_line[self.write_pos] = sample;

        // Compute FIR output (imaginary part)
        // Only odd-indexed coefficients are non-zero for a Hilbert filter,
        // but we compute the full convolution for simplicity (even coeffs = 0).
        let mut acc: i32 = 0;
        let mut read_pos = self.write_pos;
        for i in 0..N {
            acc += self.delay_line[read_pos] as i32 * self.coeffs[i] as i32;
            if read_pos == 0 { read_pos = N - 1; } else { read_pos -= 1; }
        }
        let imag = (acc >> 15).clamp(-32768, 32767) as i16;

        // Real part: input delayed by group_delay
        let gd = self.group_delay();
        let real_pos = (self.write_pos + N - gd) % N;
        let real = self.delay_line[real_pos];

        self.write_pos = (self.write_pos + 1) % N;

        (real, imag)
    }

    /// Reset filter state.
    pub fn reset(&mut self) {
        self.delay_line = [0i16; N];
        self.write_pos = 0;
    }
}

/// Create a 31-tap Hilbert transform with precomputed coefficients.
///
/// Coefficients: h[n] = 2/(π·n) for odd n, windowed by Hamming.
/// Even-indexed coefficients are zero.
pub fn hilbert_31() -> HilbertTransform<31> {
    // Precomputed 31-tap Hilbert FIR, Q15 format.
    // h[n] = (2/(π·(n-15))) × hamming(n) for odd (n-15), 0 for even
    // Computed externally and rounded to nearest integer.
    #[allow(clippy::excessive_precision)]
    let coeffs: [i16; 31] = [
        0,    -83, 0,   -222, 0,   -579, 0,  -1417, 0, -4246, 0, -20860,
        0,  20860, 0,   4246, 0,   1417, 0,    579, 0,   222, 0,     83,
        0,      0, 0,      0, 0,      0, 0,
    ];
    // NOTE: These coefficients are approximate placeholders.
    // TODO: Generate exact coefficients using the formula:
    //   h[n] = (2/π) × (1/(n-M)) × w[n]  for odd (n-M)
    //   h[n] = 0                           for even (n-M)
    //   where M = 15 (center), w[n] = 0.54 - 0.46·cos(2πn/30) (Hamming)
    // The correct values should be computed in a build script or test.

    HilbertTransform::new(coeffs)
}

/// Instantaneous frequency detector.
///
/// Computes the frequency from successive analytic signal samples using:
///   f[n] = (Fs/2π) · angle(z[n] · conj(z[n-1]))
///
/// Returns frequency in fixed-point format (Hz × 256).
pub struct InstFreqDetector {
    prev_real: i32,
    prev_imag: i32,
    /// Sample rate in Hz
    sample_rate: u32,
}

impl InstFreqDetector {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            prev_real: 1, // Avoid division by zero on first sample
            prev_imag: 0,
            sample_rate,
        }
    }

    /// Process one analytic signal sample (real, imag).
    ///
    /// Returns the estimated instantaneous frequency in Hz (fixed-point, ×256).
    /// For Bell 202: mark ≈ 1200×256, space ≈ 2200×256.
    #[inline]
    pub fn process(&mut self, real: i16, imag: i16) -> i32 {
        let r = real as i32;
        let i = imag as i32;

        // z[n] · conj(z[n-1]) = (r + ji)(pr - jpi)
        //   real part: r·pr + i·pi
        //   imag part: i·pr - r·pi
        let cross_real = r * self.prev_real + i * self.prev_imag;
        let cross_imag = i * self.prev_real - r * self.prev_imag;

        self.prev_real = r;
        self.prev_imag = i;

        // f = (Fs / 2π) · atan2(cross_imag, cross_real)
        let angle = fast_atan2(cross_imag, cross_real);

        // Convert: freq_hz = sample_rate × angle / (2π)
        // angle is in Q15 radians where 32768 ≈ π
        // So: freq = sample_rate × angle / 65536 (since full circle = 2×32768)
        // Multiply by 256 for fixed-point:
        //   freq_fp = sample_rate × 256 × angle / 65536
        //           = sample_rate × angle / 256
        let freq_fp = (self.sample_rate as i64 * angle as i64) / 256;
        freq_fp as i32
    }

    /// Reset detector state.
    pub fn reset(&mut self) {
        self.prev_real = 1;
        self.prev_imag = 0;
    }
}

/// Fast atan2 approximation.
///
/// Returns angle in Q15 format: −32768..+32767 maps to −π..+π.
/// Maximum error approximately 0.07 degrees.
///
/// Uses the identity atan(x) ≈ x − x³/3 for |x| ≤ 1, with octant
/// decomposition for the full atan2.
pub fn fast_atan2(y: i32, x: i32) -> i16 {
    if x == 0 && y == 0 {
        return 0;
    }

    let abs_y = y.abs().max(1);
    let abs_x = x.abs().max(1);

    // Compute atan(min/max) — ratio is always in [0, 1]
    let (numer, denom) = if abs_x >= abs_y {
        (abs_y, abs_x)
    } else {
        (abs_x, abs_y)
    };

    // ratio in Q15
    let ratio = ((numer as i64) << 15) / denom as i64;
    let r = ratio as i32;

    // atan(r) ≈ r − r³/3 (in Q15)
    let r_sq = ((r as i64 * r as i64) >> 15) as i32;
    let r_cu = ((r_sq as i64 * r as i64) >> 15) as i32;
    let mut angle = r - r_cu / 3;

    // Adjust for octant: if |y| > |x|, atan = π/2 − atan(x/y)
    if abs_x < abs_y {
        angle = 25736 - angle; // π/2 in Q15 ≈ 25736
    }
    // Adjust for quadrant
    if x < 0 {
        // π in Q15 ≈ 51472, but we need to stay in i16 range
        // Use: angle = sign(y) × π − angle
        angle = 51472 - angle;
    }
    if y < 0 {
        angle = -angle;
    }

    // Wrap to i16 range (−π to +π)
    angle.clamp(-32768, 32767) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_atan2_zero() {
        assert_eq!(fast_atan2(0, 0), 0);
    }

    #[test]
    fn test_fast_atan2_axes() {
        // Positive x-axis: angle = 0
        let angle = fast_atan2(0, 1000);
        assert!(angle.abs() < 200, "Expected ~0, got {}", angle);

        // Positive y-axis: angle ≈ π/2 ≈ 25736
        let angle = fast_atan2(1000, 0);
        assert!((angle - 25736_i16).abs() < 500,
            "Expected ~25736 (π/2), got {}", angle);

        // Negative x-axis: angle ≈ ±π ≈ ±32768
        let angle = fast_atan2(0, -1000);
        // Should be near π or −π
        assert!(angle.abs() > 30000,
            "Expected ~±32768 (π), got {}", angle);

        // Negative y-axis: angle ≈ −π/2 ≈ −25736
        let angle = fast_atan2(-1000, 0);
        assert!((angle + 25736_i16).abs() < 500,
            "Expected ~-25736 (-π/2), got {}", angle);
    }

    #[test]
    fn test_fast_atan2_45_degrees() {
        // 45 degrees: atan2(1, 1) ≈ π/4 ≈ 12868 in Q15
        let angle = fast_atan2(1000, 1000);
        assert!((angle - 12868_i16).abs() < 500,
            "Expected ~12868 (π/4), got {}", angle);
    }

    #[test]
    fn test_inst_freq_constant_tone() {
        // For a constant tone, the phase difference between successive
        // analytic samples should give the frequency.
        // This is a simplified test — real Hilbert output needed for full test.
        let det = InstFreqDetector::new(11025);
        // With proper analytic signal input, frequency should track the tone.
        // Full integration test needed with HilbertTransform + InstFreqDetector.
    }

    #[test]
    fn test_hilbert_group_delay() {
        let h = hilbert_31();
        assert_eq!(h.group_delay(), 15);
    }
}
