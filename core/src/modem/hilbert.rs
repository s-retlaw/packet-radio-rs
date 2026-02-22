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
    // h[n] = (2/(π·k)) × hamming(n) for odd k = n-15, 0 for even k
    // hamming(n) = 0.54 - 0.46·cos(2πn/30)
    // Antisymmetric: h[n] = -h[30-n], non-zero only at even array indices.
    let coeffs: [i16; 31] = [
        -111,  0,  -192,  0,   -440,  0,   -922,  0,  -1753,  0,  -3213,
           0,  -6343,   0, -20651,
           0,
        20651,  0,   6343,  0,   3213,  0,   1753,  0,    922,  0,    440,
           0,    192,  0,    111,
    ];

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
/// Returns angle where −32768..+32767 maps to −π..+π.
/// (Equivalently: 1 radian ≈ 32768/π ≈ 10430 units.)
/// Maximum error approximately 0.3 degrees.
///
/// Uses the identity atan(x) ≈ x·(π/4 + 0.273·(1−x)) for |x| ≤ 1,
/// with octant decomposition for the full atan2.
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

    // ratio in Q15 (0..32768 represents 0.0..1.0)
    let ratio = ((numer as i64) << 15) / denom as i64;
    let r = ratio as i32;

    // atan(x) ≈ x · (π/4 + 0.273 · (1 − x))
    // Scaled directly to output format (32768 = π):
    //   atan(x)/π ≈ x · (0.25 + 0.0869 − 0.0869·x) = x · (0.3369 − 0.0869·x)
    // In Q15 arithmetic: output = r × (11039 − (2848 × r >> 15)) >> 15
    let term = 11039 - ((2848i64 * r as i64) >> 15) as i32;
    let mut angle = ((r as i64 * term as i64) >> 15) as i32;

    // Adjust for octant: if |y| > |x|, atan = π/2 − atan(x/y)
    if abs_x < abs_y {
        angle = 16384 - angle; // π/2 in output format
    }
    // Adjust for quadrant
    if x < 0 {
        angle = 32768 - angle; // π in output format
    }
    if y < 0 {
        angle = -angle;
    }

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

        // Positive y-axis: angle ≈ π/2 ≈ 16384
        let angle = fast_atan2(1000, 0);
        assert!((angle - 16384_i16).abs() < 500,
            "Expected ~16384 (π/2), got {}", angle);

        // Negative x-axis: angle ≈ ±π ≈ ±32768
        let angle = fast_atan2(0, -1000);
        assert!(angle.abs() > 30000,
            "Expected ~±32768 (π), got {}", angle);

        // Negative y-axis: angle ≈ −π/2 ≈ −16384
        let angle = fast_atan2(-1000, 0);
        assert!((angle + 16384_i16).abs() < 500,
            "Expected ~-16384 (-π/2), got {}", angle);
    }

    #[test]
    fn test_fast_atan2_45_degrees() {
        // 45 degrees: atan2(1, 1) ≈ π/4 ≈ 8192
        let angle = fast_atan2(1000, 1000);
        assert!((angle - 8192_i16).abs() < 500,
            "Expected ~8192 (π/4), got {}", angle);
    }

    #[test]
    fn test_hilbert_group_delay() {
        let h = hilbert_31();
        assert_eq!(h.group_delay(), 15);
    }

    /// Recompute 31-tap Hilbert coefficients with f64 and validate the
    /// precomputed Q15 table matches.
    #[test]
    fn test_hilbert_coefficients_match_formula() {
        use core::f64::consts::PI;

        let h = hilbert_31();
        let n_taps = 31;
        let center = 15;

        for n in 0..n_taps {
            let k = n as i32 - center as i32;
            let expected = if k % 2 == 0 {
                0i16
            } else {
                let hamming = 0.54 - 0.46 * (2.0 * PI * n as f64 / 30.0).cos();
                let h_val = (2.0 / (PI * k as f64)) * hamming;
                let q15 = (h_val * 32768.0).round() as i16;
                q15
            };

            assert_eq!(h.coeffs[n], expected,
                "Coefficient mismatch at n={}, k={}: got {}, expected {}",
                n, k, h.coeffs[n], expected);
        }
    }

    /// Verify antisymmetry: h[n] = -h[30-n]
    #[test]
    fn test_hilbert_antisymmetry() {
        let h = hilbert_31();
        for n in 0..15 {
            assert_eq!(h.coeffs[n], -h.coeffs[30 - n],
                "Antisymmetry failed at n={}: h[{}]={}, h[{}]={}",
                n, n, h.coeffs[n], 30 - n, h.coeffs[30 - n]);
        }
        assert_eq!(h.coeffs[15], 0, "Center tap must be zero");
    }

    /// Feed a 1700 Hz tone through the Hilbert transform and verify the
    /// output envelope (real² + imag²) is roughly constant after the
    /// initial transient settles.
    #[test]
    fn test_hilbert_envelope_constant_tone() {
        use core::f64::consts::PI;

        let sample_rate = 11025u32;
        let freq = 1700.0f64;
        let amplitude = 16000i16;

        let mut h = hilbert_31();
        let num_samples = 500;
        let transient = 50; // Skip initial samples for filter to settle

        let mut envelopes = [0i64; 500];
        for i in 0..num_samples {
            let t = i as f64 / sample_rate as f64;
            let sample = (amplitude as f64 * (2.0 * PI * freq * t).sin()) as i16;
            let (real, imag) = h.process(sample);
            envelopes[i] = (real as i64) * (real as i64) + (imag as i64) * (imag as i64);
        }

        // After the transient, the envelope should be roughly constant.
        let steady = &envelopes[transient..num_samples];
        let mean: i64 = steady.iter().sum::<i64>() / steady.len() as i64;
        assert!(mean > 0, "Envelope mean should be positive, got {}", mean);

        // Check that all steady-state envelopes are within 25% of the mean
        for (i, &env) in steady.iter().enumerate() {
            let ratio = (env as f64) / (mean as f64);
            assert!(ratio > 0.75 && ratio < 1.25,
                "Envelope at sample {} deviates too much: ratio={:.3}, env={}, mean={}",
                i + transient, ratio, env, mean);
        }
    }

    /// Feed a 1700 Hz tone through Hilbert + InstFreqDetector and verify
    /// the frequency estimate is within 10% of 1700 Hz.
    #[test]
    fn test_hilbert_inst_freq_1700hz() {
        use core::f64::consts::PI;

        let sample_rate = 11025u32;
        let freq = 1700.0f64;
        let amplitude = 16000i16;

        let mut h = hilbert_31();
        let mut det = InstFreqDetector::new(sample_rate);

        let num_samples = 500;
        let transient = 60; // Filter + detector settling

        let mut freq_estimates = [0i32; 500];
        for i in 0..num_samples {
            let t = i as f64 / sample_rate as f64;
            let sample = (amplitude as f64 * (2.0 * PI * freq * t).sin()) as i16;
            let (real, imag) = h.process(sample);
            freq_estimates[i] = det.process(real, imag);
        }

        // After settling, frequency estimates (in Hz × 256) should be near 1700 × 256
        let expected_fp = (freq * 256.0) as i32;
        let tolerance = (expected_fp as f64 * 0.10) as i32; // 10%

        let steady = &freq_estimates[transient..num_samples];
        let mean: i64 = steady.iter().map(|&f| f as i64).sum::<i64>() / steady.len() as i64;
        let mean_hz = mean as f64 / 256.0;

        assert!((mean as i32 - expected_fp).abs() < tolerance,
            "Mean frequency estimate {:.1} Hz deviates from expected {:.1} Hz by more than 10%",
            mean_hz, freq);
    }
}
