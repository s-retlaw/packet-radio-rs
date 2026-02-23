//! DSP Filters — biquad implementations for the modem.
//!
//! All filters use Direct Form II Transposed for numerical stability.
//! Coefficients are Q15 fixed-point.
//!
//! Reference: Audio EQ Cookbook by Robert Bristow-Johnson
//! https://www.w3.org/2011/audio/audio-eq-cookbook.html

/// Second-order IIR (biquad) filter in Q15 fixed-point.
///
/// Transfer function:
///   H(z) = (b0 + b1·z⁻¹ + b2·z⁻²) / (1 + a1·z⁻¹ + a2·z⁻²)
#[derive(Clone, Copy)]
pub struct BiquadFilter {
    pub b0: i32,
    pub b1: i32,
    pub b2: i32,
    pub a1: i32,
    pub a2: i32,
    s1: i32,
    s2: i32,
}

impl BiquadFilter {
    /// Create a biquad with given Q15 coefficients.
    pub const fn new(b0: i32, b1: i32, b2: i32, a1: i32, a2: i32) -> Self {
        Self { b0, b1, b2, a1, a2, s1: 0, s2: 0 }
    }

    /// Identity (passthrough) filter.
    pub const fn passthrough() -> Self {
        Self::new(32768, 0, 0, 0, 0)
    }

    /// Reset the filter state.
    pub fn reset(&mut self) {
        self.s1 = 0;
        self.s2 = 0;
    }

    /// Process a single sample. Direct Form II Transposed.
    #[inline]
    pub fn process(&mut self, input: i16) -> i16 {
        let x = input as i32;
        let y = (self.b0 * x + self.s1) >> 15;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y.clamp(-32768, 32767) as i16
    }

    /// Process a buffer of samples in place.
    pub fn process_buffer(&mut self, samples: &mut [i16]) {
        for s in samples.iter_mut() {
            *s = self.process(*s);
        }
    }
}

/// Compute Q15 biquad coefficients for a bandpass filter.
///
/// - `sample_rate`: Audio sample rate in Hz
/// - `center_freq`: Center frequency in Hz
/// - `bandwidth`: Bandwidth in Hz (−3 dB points)
///
/// Returns (b0, b1, b2, a1, a2) in Q15 format.
///
/// Requires `std` or `libm` for trig functions. On `no_std` targets,
/// use precomputed coefficients instead.
#[cfg(feature = "std")]
pub fn bandpass_coeffs(sample_rate: u32, center_freq: f64, bandwidth: f64) -> BiquadFilter {
    use core::f64::consts::PI;

    let fs = sample_rate as f64;
    let w0 = 2.0 * PI * center_freq / fs;
    let q = center_freq / bandwidth;
    let alpha = libm::sin(w0) / (2.0 * q);

    let b0 = alpha;
    let b1 = 0.0;
    let b2 = -alpha;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * libm::cos(w0);
    let a2 = 1.0 - alpha;

    // Normalize by a0 and convert to Q15
    let scale = 32768.0 / a0;
    BiquadFilter::new(
        (b0 * scale) as i32,
        (b1 * scale) as i32,
        (b2 * scale) as i32,
        (a1 * scale) as i32,
        (a2 * scale) as i32,
    )
}

/// Compute Q15 biquad coefficients for a lowpass filter.
///
/// - `sample_rate`: Audio sample rate in Hz
/// - `cutoff`: Cutoff frequency in Hz (−3 dB point)
/// - `q`: Quality factor (0.707 = Butterworth)
#[cfg(feature = "std")]
pub fn lowpass_coeffs(sample_rate: u32, cutoff: f64, q: f64) -> BiquadFilter {
    use core::f64::consts::PI;

    let fs = sample_rate as f64;
    let w0 = 2.0 * PI * cutoff / fs;
    let alpha = libm::sin(w0) / (2.0 * q);

    let cos_w0 = libm::cos(w0);
    let b0 = (1.0 - cos_w0) / 2.0;
    let b1 = 1.0 - cos_w0;
    let b2 = (1.0 - cos_w0) / 2.0;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha;

    let scale = 32768.0 / a0;
    BiquadFilter::new(
        (b0 * scale) as i32,
        (b1 * scale) as i32,
        (b2 * scale) as i32,
        (a1 * scale) as i32,
        (a2 * scale) as i32,
    )
}

/// Precomputed bandpass filter for AFSK passband (900-2500 Hz) at 11025 Hz.
/// Passes mark (1200 Hz) and space (2200 Hz), rejects out-of-band noise.
///
/// Computed from Audio EQ Cookbook BPF (constant 0 dB peak gain):
/// center=1700 Hz, BW=1600 Hz (Q=1.0625), Fs=11025 Hz.
pub const fn afsk_bandpass_11025() -> BiquadFilter {
    BiquadFilter::new(9158, 0, -9158, -26739, 14453)
}

/// Precomputed bandpass filter for AFSK passband at 22050 Hz sample rate.
/// center=1700 Hz, BW=1600 Hz.
pub const fn afsk_bandpass_22050() -> BiquadFilter {
    BiquadFilter::new(5890, 0, -5890, -47570, 20987)
}

/// Precomputed bandpass filter for AFSK passband at 44100 Hz sample rate.
/// center=1700 Hz, BW=1600 Hz.
pub const fn afsk_bandpass_44100() -> BiquadFilter {
    BiquadFilter::new(3323, 0, -3323, -57170, 26121)
}

/// Narrow bandpass filter at 11025 Hz — better noise rejection.
/// center=1700 Hz, BW=1200 Hz (Q=1.417).
pub const fn afsk_bandpass_narrow_11025() -> BiquadFilter {
    BiquadFilter::new(7384, 0, -7384, -28747, 17999)
}

/// Wide bandpass filter at 11025 Hz — tolerates frequency drift.
/// center=1700 Hz, BW=2000 Hz (Q=0.85).
pub const fn afsk_bandpass_wide_11025() -> BiquadFilter {
    BiquadFilter::new(10699, 0, -10699, -24992, 11368)
}

/// Precomputed lowpass filter for post-detection at 11025 Hz.
/// Cutoff at 1200 Hz to smooth the delay-multiply discriminator output.
///
/// Computed from Audio EQ Cookbook LPF:
/// cutoff=1200 Hz, Q=0.707 (Butterworth), Fs=11025 Hz.
pub const fn post_detect_lpf_11025() -> BiquadFilter {
    BiquadFilter::new(2547, 5093, 2547, -35110, 12528)
}

/// Precomputed post-detection LPF at 22050 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_22050() -> BiquadFilter {
    BiquadFilter::new(767, 1533, 767, -49907, 20206)
}

/// Precomputed post-detection LPF at 44100 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_44100() -> BiquadFilter {
    BiquadFilter::new(213, 426, 213, -57644, 25729)
}

/// Select the post-detection LPF for a given sample rate.
/// Uses precomputed coefficients for common rates, runtime computation
/// on std targets for others.
pub fn post_detect_lpf(sample_rate: u32) -> BiquadFilter {
    match sample_rate {
        11025 => post_detect_lpf_11025(),
        22050 => post_detect_lpf_22050(),
        44100 => post_detect_lpf_44100(),
        #[cfg(feature = "std")]
        _ => lowpass_coeffs(sample_rate, 1200.0, 0.707),
        #[cfg(not(feature = "std"))]
        _ => post_detect_lpf_11025(), // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_filter() {
        let mut filt = BiquadFilter::passthrough();
        assert_eq!(filt.process(1000), 1000);
        assert_eq!(filt.process(-5000), -5000);
        assert_eq!(filt.process(0), 0);
    }

    #[test]
    fn test_filter_reset() {
        let mut filt = afsk_bandpass_11025();
        // Feed some data
        for _ in 0..100 {
            filt.process(10000);
        }
        filt.reset();
        assert_eq!(filt.s1, 0);
        assert_eq!(filt.s2, 0);
    }

    #[test]
    fn test_filter_no_overflow() {
        let mut filt = afsk_bandpass_11025();
        // Max amplitude input should not overflow
        for _ in 0..1000 {
            let out = filt.process(32767);
            assert!(out >= -32768 && out <= 32767);
        }
        for _ in 0..1000 {
            let out = filt.process(-32768);
            assert!(out >= -32768 && out <= 32767);
        }
    }

    #[test]
    fn test_dc_rejection() {
        // Bandpass filter should reject DC
        let mut filt = afsk_bandpass_11025();
        let mut last_output = 0i16;
        // Feed constant (DC) for many samples
        for _ in 0..2000 {
            last_output = filt.process(10000);
        }
        // Output should be near zero (DC rejected)
        assert!(last_output.abs() < 500,
            "Bandpass should reject DC, got {}", last_output);
    }
}
