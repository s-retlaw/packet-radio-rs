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

    /// Replace coefficients without resetting filter state.
    ///
    /// Useful for adaptive filtering where you want to change the frequency
    /// response mid-stream without the transient from zeroing state.
    pub fn set_coefficients(&mut self, other: &BiquadFilter) {
        self.b0 = other.b0;
        self.b1 = other.b1;
        self.b2 = other.b2;
        self.a1 = other.a1;
        self.a2 = other.a2;
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

/// Cascaded (4th-order) biquad filter — two identical biquad stages in series.
///
/// Provides -12 dB/octave rolloff instead of -6 dB/octave from a single biquad.
/// Significantly better out-of-band rejection for AFSK bandpass filtering.
/// Cost: one extra biquad per sample (~5 ops/sample).
#[derive(Clone, Copy)]
pub struct CascadedBpf {
    stage1: BiquadFilter,
    stage2: BiquadFilter,
}

impl CascadedBpf {
    /// Create a cascaded filter from two identical biquad stages.
    pub const fn new(bpf: BiquadFilter) -> Self {
        // Clone the coefficients for stage2 (state is zeroed)
        Self {
            stage1: bpf,
            stage2: BiquadFilter::new(bpf.b0, bpf.b1, bpf.b2, bpf.a1, bpf.a2),
        }
    }

    /// Reset both stages.
    pub fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
    }

    /// Process a single sample through both stages.
    #[inline]
    pub fn process(&mut self, input: i16) -> i16 {
        let mid = self.stage1.process(input);
        self.stage2.process(mid)
    }
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

/// Precomputed bandpass filter for AFSK passband at 12000 Hz sample rate.
/// center=1700 Hz, BW=1600 Hz. 12000/1200 = 10 sps (integer mark alignment).
pub const fn afsk_bandpass_12000() -> BiquadFilter {
    BiquadFilter::new(8774, 0, -8774, -30198, 15218)
}

/// Precomputed bandpass filter for AFSK passband at 13200 Hz sample rate.
/// center=1700 Hz, BW=1600 Hz.
pub const fn afsk_bandpass_13200() -> BiquadFilter {
    BiquadFilter::new(8324, 0, -8324, -33735, 16118)
}

/// Precomputed bandpass filter for AFSK passband at 26400 Hz sample rate.
/// center=1700 Hz, BW=1600 Hz.
pub const fn afsk_bandpass_26400() -> BiquadFilter {
    BiquadFilter::new(5121, 0, -5121, -50828, 22525)
}

/// Narrow bandpass filter at 11025 Hz — better noise rejection.
/// center=1700 Hz, BW=1200 Hz (Q=1.417).
pub const fn afsk_bandpass_narrow_11025() -> BiquadFilter {
    BiquadFilter::new(7384, 0, -7384, -28747, 17999)
}

/// Narrow bandpass filter at 12000 Hz. center=1700 Hz, BW=1200 Hz.
pub const fn afsk_bandpass_narrow_12000() -> BiquadFilter {
    BiquadFilter::new(7053, 0, -7053, -32365, 18661)
}

/// Narrow bandpass filter at 13200 Hz. center=1700 Hz, BW=1200 Hz.
pub const fn afsk_bandpass_narrow_13200() -> BiquadFilter {
    BiquadFilter::new(6667, 0, -6667, -36023, 19433)
}

/// Narrow bandpass filter at 26400 Hz. center=1700 Hz, BW=1200 Hz.
pub const fn afsk_bandpass_narrow_26400() -> BiquadFilter {
    BiquadFilter::new(3997, 0, -3997, -52895, 24773)
}

/// Wide bandpass filter at 11025 Hz — tolerates frequency drift.
/// center=1700 Hz, BW=2000 Hz (Q=0.85).
pub const fn afsk_bandpass_wide_11025() -> BiquadFilter {
    BiquadFilter::new(10699, 0, -10699, -24992, 11368)
}

/// Wide bandpass filter at 12000 Hz. center=1700 Hz, BW=2000 Hz.
pub const fn afsk_bandpass_wide_12000() -> BiquadFilter {
    BiquadFilter::new(10280, 0, -10280, -28304, 12207)
}

/// Wide bandpass filter at 13200 Hz. center=1700 Hz, BW=2000 Hz.
pub const fn afsk_bandpass_wide_13200() -> BiquadFilter {
    BiquadFilter::new(9784, 0, -9784, -31720, 13198)
}

/// Wide bandpass filter at 26400 Hz. center=1700 Hz, BW=2000 Hz.
pub const fn afsk_bandpass_wide_26400() -> BiquadFilter {
    BiquadFilter::new(6161, 0, -6161, -48917, 20445)
}

/// Precomputed bandpass filter for AFSK passband at 48000 Hz sample rate.
/// center=1700 Hz, BW=1600 Hz.
pub const fn afsk_bandpass_48000() -> BiquadFilter {
    BiquadFilter::new(3083, 0, -3083, -57906, 26601)
}

/// Narrow bandpass filter at 48000 Hz. center=1700 Hz, BW=1200 Hz.
pub const fn afsk_bandpass_narrow_48000() -> BiquadFilter {
    BiquadFilter::new(2367, 0, -2367, -59300, 28032)
}

/// Wide bandpass filter at 48000 Hz. center=1700 Hz, BW=2000 Hz.
pub const fn afsk_bandpass_wide_48000() -> BiquadFilter {
    BiquadFilter::new(3765, 0, -3765, -56575, 25237)
}

// ─── 300 baud BPF (center=1700 Hz, mark=1600/space=1800 Hz) ────────────

/// Precomputed bandpass filter for 300 baud AFSK at 8000 Hz.
/// center=1700 Hz, BW=400 Hz (Q=4.25).
pub const fn afsk_300_bandpass_8000() -> BiquadFilter {
    BiquadFilter::new(3364, 0, -3364, -13729, 26041)
}

/// Narrow bandpass for 300 baud at 8000 Hz.
/// center=1700 Hz, BW=300 Hz (Q=5.67).
pub const fn afsk_300_bandpass_narrow_8000() -> BiquadFilter {
    BiquadFilter::new(2589, 0, -2589, -14090, 27589)
}

/// Wide bandpass for 300 baud at 8000 Hz.
/// center=1700 Hz, BW=500 Hz (Q=3.4).
pub const fn afsk_300_bandpass_wide_8000() -> BiquadFilter {
    BiquadFilter::new(4099, 0, -4099, -13385, 24569)
}

/// Precomputed bandpass filter for 300 baud AFSK at 11025 Hz.
/// center=1700 Hz, BW=400 Hz (Q=4.25).
pub const fn afsk_300_bandpass_11025() -> BiquadFilter {
    BiquadFilter::new(2897, 0, -2897, -33830, 26975)
}

/// Narrow bandpass for 300 baud at 11025 Hz.
/// center=1700 Hz, BW=300 Hz (Q=5.67).
pub const fn afsk_300_bandpass_narrow_11025() -> BiquadFilter {
    BiquadFilter::new(2222, 0, -2222, -34594, 28325)
}

/// Wide bandpass for 300 baud at 11025 Hz.
/// center=1700 Hz, BW=500 Hz (Q=3.4).
pub const fn afsk_300_bandpass_wide_11025() -> BiquadFilter {
    BiquadFilter::new(3542, 0, -3542, -33099, 25683)
}

/// Precomputed bandpass filter for 300 baud AFSK at 22050 Hz.
/// center=1700 Hz, BW=400 Hz (Q=4.25).
pub const fn afsk_300_bandpass_22050() -> BiquadFilter {
    BiquadFilter::new(1702, 0, -1702, -54983, 29364)
}

/// Narrow bandpass for 300 baud at 22050 Hz.
/// center=1700 Hz, BW=300 Hz (Q=5.67).
pub const fn afsk_300_bandpass_narrow_22050() -> BiquadFilter {
    BiquadFilter::new(1293, 0, -1293, -55707, 30181)
}

/// Wide bandpass for 300 baud at 22050 Hz.
/// center=1700 Hz, BW=500 Hz (Q=3.4).
pub const fn afsk_300_bandpass_wide_22050() -> BiquadFilter {
    BiquadFilter::new(2100, 0, -2100, -54279, 28567)
}

/// Precomputed bandpass filter for 300 baud AFSK at 44100 Hz.
/// center=1700 Hz, BW=400 Hz (Q=4.25).
pub const fn afsk_300_bandpass_44100() -> BiquadFilter {
    BiquadFilter::new(899, 0, -899, -61877, 30969)
}

/// Narrow bandpass for 300 baud at 44100 Hz.
/// center=1700 Hz, BW=300 Hz (Q=5.67).
pub const fn afsk_300_bandpass_narrow_44100() -> BiquadFilter {
    BiquadFilter::new(679, 0, -679, -62304, 31410)
}

/// Wide bandpass for 300 baud at 44100 Hz.
/// center=1700 Hz, BW=500 Hz (Q=3.4).
pub const fn afsk_300_bandpass_wide_44100() -> BiquadFilter {
    BiquadFilter::new(1116, 0, -1116, -61455, 30535)
}

/// Precomputed bandpass filter for 300 baud AFSK at 48000 Hz.
/// center=1700 Hz, BW=400 Hz (Q=4.25).
pub const fn afsk_300_bandpass_48000() -> BiquadFilter {
    BiquadFilter::new(829, 0, -829, -62302, 31109)
}

/// Narrow bandpass for 300 baud at 48000 Hz.
/// center=1700 Hz, BW=300 Hz (Q=5.67).
pub const fn afsk_300_bandpass_narrow_48000() -> BiquadFilter {
    BiquadFilter::new(626, 0, -626, -62699, 31516)
}

/// Wide bandpass for 300 baud at 48000 Hz.
/// center=1700 Hz, BW=500 Hz (Q=3.4).
pub const fn afsk_300_bandpass_wide_48000() -> BiquadFilter {
    BiquadFilter::new(1030, 0, -1030, -61911, 30708)
}

// ─── 300 baud post-detection LPF (cutoff=300 Hz) ──────────────────────

/// Post-detection LPF for 300 baud at 8000 Hz.
/// Butterworth LPF, cutoff=300 Hz, Q=0.707.
pub const fn post_detect_lpf_300_8000() -> BiquadFilter {
    BiquadFilter::new(389, 777, 389, -54696, 23483)
}

/// Post-detection LPF for 300 baud at 11025 Hz.
/// Butterworth LPF, cutoff=300 Hz, Q=0.707.
pub const fn post_detect_lpf_300_11025() -> BiquadFilter {
    BiquadFilter::new(213, 426, 213, -57645, 25730)
}

/// Post-detection LPF for 300 baud at 22050 Hz.
/// Butterworth LPF, cutoff=300 Hz, Q=0.707.
pub const fn post_detect_lpf_300_22050() -> BiquadFilter {
    BiquadFilter::new(56, 113, 56, -61579, 29037)
}

/// Post-detection LPF for 300 baud at 44100 Hz.
/// Butterworth LPF, cutoff=300 Hz, Q=0.707.
pub const fn post_detect_lpf_300_44100() -> BiquadFilter {
    BiquadFilter::new(15, 29, 15, -63556, 30846)
}

/// Post-detection LPF for 300 baud at 48000 Hz.
/// Butterworth LPF, cutoff=300 Hz, Q=0.707.
pub const fn post_detect_lpf_300_48000() -> BiquadFilter {
    BiquadFilter::new(12, 25, 12, -63717, 30998)
}

/// Select the 300 baud post-detection LPF for a given sample rate.
pub fn post_detect_lpf_300(sample_rate: u32) -> BiquadFilter {
    match sample_rate {
        8000 => post_detect_lpf_300_8000(),
        11025 => post_detect_lpf_300_11025(),
        22050 => post_detect_lpf_300_22050(),
        44100 => post_detect_lpf_300_44100(),
        48000 => post_detect_lpf_300_48000(),
        #[cfg(feature = "std")]
        _ => lowpass_coeffs(sample_rate, 300.0, 0.707),
        #[cfg(not(feature = "std"))]
        _ => post_detect_lpf_300_11025(), // fallback
    }
}

/// Precomputed lowpass filter for post-detection at 11025 Hz.
/// Cutoff at 1200 Hz to smooth the delay-multiply discriminator output.
///
/// Computed from Audio EQ Cookbook LPF:
/// cutoff=1200 Hz, Q=0.707 (Butterworth), Fs=11025 Hz.
pub const fn post_detect_lpf_11025() -> BiquadFilter {
    BiquadFilter::new(2547, 5093, 2547, -35110, 12528)
}

/// Precomputed post-detection LPF at 12000 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_12000() -> BiquadFilter {
    BiquadFilter::new(2210, 4420, 2210, -37451, 13524)
}

/// Precomputed post-detection LPF at 13200 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_13200() -> BiquadFilter {
    BiquadFilter::new(1881, 3763, 1881, -39883, 14641)
}

/// Precomputed post-detection LPF at 22050 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_22050() -> BiquadFilter {
    BiquadFilter::new(767, 1533, 767, -49907, 20206)
}

/// Precomputed post-detection LPF at 26400 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_26400() -> BiquadFilter {
    BiquadFilter::new(553, 1106, 553, -52434, 21879)
}

/// Precomputed post-detection LPF at 44100 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_44100() -> BiquadFilter {
    BiquadFilter::new(213, 426, 213, -57644, 25729)
}

/// Precomputed post-detection LPF at 48000 Hz. Cutoff 1200 Hz, Q=0.707.
pub const fn post_detect_lpf_48000() -> BiquadFilter {
    BiquadFilter::new(181, 363, 181, -58281, 26239)
}

/// Select the post-detection LPF for a given sample rate.
/// Uses precomputed coefficients for common rates, runtime computation
/// on std targets for others.
pub fn post_detect_lpf(sample_rate: u32) -> BiquadFilter {
    match sample_rate {
        11025 => post_detect_lpf_11025(),
        12000 => post_detect_lpf_12000(),
        13200 => post_detect_lpf_13200(),
        22050 => post_detect_lpf_22050(),
        26400 => post_detect_lpf_26400(),
        44100 => post_detect_lpf_44100(),
        48000 => post_detect_lpf_48000(),
        #[cfg(feature = "std")]
        _ => lowpass_coeffs(sample_rate, 1200.0, 0.707),
        #[cfg(not(feature = "std"))]
        _ => post_detect_lpf_11025(), // fallback
    }
}

/// Precomputed lowpass filter for correlation demodulator at 11025 Hz.
/// Cutoff at 500 Hz — empirically optimal across WA8LMF tracks.
/// Tighter than 600 Hz: better cross-tone rejection with minimal
/// transition detail loss. +44 packets on Track 2 vs 600 Hz.
///
/// Computed from Audio EQ Cookbook LPF: cutoff=500 Hz, Q=0.707, Fs=11025 Hz.
pub const fn corr_lpf_11025() -> BiquadFilter {
    BiquadFilter::new(551, 1102, 551, -52463, 21899)
}

/// Precomputed correlation LPF at 12000 Hz. Cutoff 500 Hz, Q=0.707.
pub const fn corr_lpf_12000() -> BiquadFilter {
    BiquadFilter::new(471, 943, 471, -53508, 22628)
}

/// Precomputed correlation LPF at 13200 Hz. Cutoff 500 Hz, Q=0.707.
pub const fn corr_lpf_13200() -> BiquadFilter {
    BiquadFilter::new(395, 791, 395, -54587, 23402)
}

/// Precomputed correlation LPF at 22050 Hz. Cutoff 500 Hz, Q=0.707.
pub const fn corr_lpf_22050() -> BiquadFilter {
    BiquadFilter::new(150, 301, 150, -58951, 26787)
}

/// Precomputed correlation LPF at 26400 Hz. Cutoff 500 Hz, Q=0.707.
pub const fn corr_lpf_26400() -> BiquadFilter {
    BiquadFilter::new(106, 213, 106, -60032, 27691)
}

/// Precomputed correlation LPF at 44100 Hz. Cutoff 500 Hz, Q=0.707.
pub const fn corr_lpf_44100() -> BiquadFilter {
    BiquadFilter::new(39, 79, 39, -62236, 29627)
}

/// Precomputed correlation LPF at 48000 Hz. Cutoff 500 Hz, Q=0.707.
pub const fn corr_lpf_48000() -> BiquadFilter {
    BiquadFilter::new(33, 67, 33, -62504, 29870)
}

/// Select the correlation demodulator LPF for a given sample rate.
pub fn corr_lpf(sample_rate: u32) -> BiquadFilter {
    match sample_rate {
        11025 => corr_lpf_11025(),
        12000 => corr_lpf_12000(),
        13200 => corr_lpf_13200(),
        22050 => corr_lpf_22050(),
        26400 => corr_lpf_26400(),
        44100 => corr_lpf_44100(),
        48000 => corr_lpf_48000(),
        #[cfg(feature = "std")]
        _ => lowpass_coeffs(sample_rate, 500.0, 0.707),
        #[cfg(not(feature = "std"))]
        _ => corr_lpf_11025(), // fallback
    }
}

/// Compute correlation LPF from signal parameters.
///
/// LPF must reject the cross-tone beat (`tone_sep`) AND pass the symbol
/// envelope (`baud_rate / 2`).  `tone_sep / 2` handles the beat;
/// `baud_rate * 2 / 5` handles the envelope.
///
/// For Bell 202 (mark=1200, space=2200, baud=1200): max(500, 480) = 500 Hz.
/// For V.23 (mark=1300, space=2100, baud=1200): max(400, 480) = 480 Hz.
pub fn corr_lpf_for_config(mark_freq: u32, space_freq: u32, baud_rate: u32, sample_rate: u32) -> BiquadFilter {
    let tone_sep = space_freq.abs_diff(mark_freq);
    let cutoff = core::cmp::max(tone_sep / 2, baud_rate * 2 / 5);

    // Fast path: standard Bell 202 → precomputed 500 Hz
    if cutoff == 500 {
        return corr_lpf(sample_rate);
    }

    corr_lpf_by_cutoff(sample_rate, cutoff)
}

// ─── Precomputed correlation LPF table (5 cutoffs × 3 sample rates) ────

/// Correlation LPF: 400 Hz cutoff at 11025 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_11025_400() -> BiquadFilter {
    BiquadFilter::new(365, 730, 365, -55043, 23737)
}

/// Correlation LPF: 450 Hz cutoff at 11025 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_11025_450() -> BiquadFilter {
    BiquadFilter::new(454, 908, 454, -53750, 22799)
}

/// Correlation LPF: 550 Hz cutoff at 11025 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_11025_550() -> BiquadFilter {
    BiquadFilter::new(655, 1310, 655, -51182, 21035)
}

/// Correlation LPF: 600 Hz cutoff at 11025 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_11025_600() -> BiquadFilter {
    BiquadFilter::new(766, 1533, 766, -49906, 20205)
}

/// Correlation LPF: 400 Hz cutoff at 13200 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_13200_400() -> BiquadFilter {
    BiquadFilter::new(261, 522, 261, -56755, 25031)
}

/// Correlation LPF: 450 Hz cutoff at 13200 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_13200_450() -> BiquadFilter {
    BiquadFilter::new(325, 650, 325, -55669, 24203)
}

/// Correlation LPF: 550 Hz cutoff at 13200 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_13200_550() -> BiquadFilter {
    BiquadFilter::new(471, 943, 471, -53508, 22628)
}

/// Correlation LPF: 600 Hz cutoff at 13200 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_13200_600() -> BiquadFilter {
    BiquadFilter::new(553, 1106, 553, -52434, 21879)
}

/// Correlation LPF: 400 Hz cutoff at 26400 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_26400_400() -> BiquadFilter {
    BiquadFilter::new(69, 139, 69, -61129, 28639)
}

/// Correlation LPF: 450 Hz cutoff at 26400 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_26400_450() -> BiquadFilter {
    BiquadFilter::new(87, 174, 87, -60580, 28161)
}

/// Correlation LPF: 550 Hz cutoff at 26400 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_26400_550() -> BiquadFilter {
    BiquadFilter::new(128, 256, 128, -59484, 27229)
}

/// Correlation LPF: 600 Hz cutoff at 26400 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_26400_600() -> BiquadFilter {
    BiquadFilter::new(151, 303, 151, -58937, 26775)
}

/// Correlation LPF: 400 Hz cutoff at 22050 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_22050_400() -> BiquadFilter {
    BiquadFilter::new(98, 196, 98, -60263, 27889)
}

/// Correlation LPF: 450 Hz cutoff at 22050 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_22050_450() -> BiquadFilter {
    BiquadFilter::new(123, 246, 123, -59607, 27332)
}

/// Correlation LPF: 550 Hz cutoff at 22050 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_22050_550() -> BiquadFilter {
    BiquadFilter::new(180, 361, 180, -58297, 26253)
}

/// Correlation LPF: 600 Hz cutoff at 22050 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_22050_600() -> BiquadFilter {
    BiquadFilter::new(213, 426, 213, -57644, 25729)
}

/// Correlation LPF: 400 Hz cutoff at 44100 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_44100_400() -> BiquadFilter {
    BiquadFilter::new(25, 51, 25, -62895, 30230)
}

/// Correlation LPF: 450 Hz cutoff at 44100 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_44100_450() -> BiquadFilter {
    BiquadFilter::new(32, 64, 32, -62566, 29927)
}

/// Correlation LPF: 550 Hz cutoff at 44100 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_44100_550() -> BiquadFilter {
    BiquadFilter::new(47, 95, 47, -61907, 29330)
}

/// Correlation LPF: 600 Hz cutoff at 44100 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_44100_600() -> BiquadFilter {
    BiquadFilter::new(56, 112, 56, -61578, 29036)
}

// ─── Correlation LPF for 300 baud (100 Hz and 120 Hz cutoffs) ──────────
// For 300 baud: tone_sep=200 Hz → cutoff = max(200/2, 300*2/5) = max(100, 120) = 120 Hz.

/// Correlation LPF: 100 Hz cutoff at 8000 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_8000_100() -> BiquadFilter {
    BiquadFilter::new(48, 96, 48, -61900, 29323)
}

/// Correlation LPF: 100 Hz cutoff at 11025 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_11025_100() -> BiquadFilter {
    BiquadFilter::new(26, 51, 26, -62896, 30231)
}

/// Correlation LPF: 100 Hz cutoff at 22050 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_22050_100() -> BiquadFilter {
    BiquadFilter::new(7, 13, 7, -64216, 31474)
}

/// Correlation LPF: 100 Hz cutoff at 44100 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_44100_100() -> BiquadFilter {
    BiquadFilter::new(2, 3, 2, -64876, 32114)
}

/// Correlation LPF: 100 Hz cutoff at 48000 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_48000_100() -> BiquadFilter {
    BiquadFilter::new(1, 3, 1, -64929, 32167)
}

/// Correlation LPF: 120 Hz cutoff at 8000 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_8000_120() -> BiquadFilter {
    BiquadFilter::new(68, 136, 68, -61174, 28679)
}

/// Correlation LPF: 120 Hz cutoff at 11025 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_11025_120() -> BiquadFilter {
    BiquadFilter::new(37, 73, 37, -62369, 29747)
}

/// Correlation LPF: 120 Hz cutoff at 12000 Hz. Q=0.707 Butterworth.
/// Used after 4:1 decimation from 48000 Hz for 300 baud.
pub const fn corr_lpf_12000_120() -> BiquadFilter {
    BiquadFilter::new(31, 62, 31, -62626, 29982)
}

/// Correlation LPF: 120 Hz cutoff at 22050 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_22050_120() -> BiquadFilter {
    BiquadFilter::new(9, 19, 9, -63952, 31221)
}

/// Correlation LPF: 120 Hz cutoff at 44100 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_44100_120() -> BiquadFilter {
    BiquadFilter::new(2, 5, 2, -64744, 31985)
}

/// Correlation LPF: 120 Hz cutoff at 48000 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_48000_120() -> BiquadFilter {
    BiquadFilter::new(2, 4, 2, -64808, 32048)
}

// ─── Cascaded 240 Hz LPF for 300 baud at high sample rates ──────────────
// Two cascaded 240 Hz biquads ≈ 170 Hz effective cutoff with ~4x better
// Q15 coefficient resolution than a single 120 Hz biquad.

/// Correlation LPF: 240 Hz cutoff at 8000 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_8000_240() -> BiquadFilter {
    BiquadFilter::new(256, 512, 256, -56842, 25099)
}

/// Correlation LPF: 240 Hz cutoff at 11025 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_11025_240() -> BiquadFilter {
    BiquadFilter::new(139, 279, 139, -59213, 27004)
}

/// Correlation LPF: 240 Hz cutoff at 22050 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_22050_240() -> BiquadFilter {
    BiquadFilter::new(36, 73, 36, -62368, 29746)
}

/// Correlation LPF: 240 Hz cutoff at 44100 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_44100_240() -> BiquadFilter {
    BiquadFilter::new(9, 18, 9, -63951, 31220)
}

/// Correlation LPF: 240 Hz cutoff at 48000 Hz. Q=0.707 Butterworth.
pub const fn corr_lpf_48000_240() -> BiquadFilter {
    BiquadFilter::new(7, 15, 7, -64080, 31343)
}

/// Select the 240 Hz cascaded LPF for a given sample rate (300 baud at high rates).
pub fn corr_lpf_240(sample_rate: u32) -> BiquadFilter {
    match sample_rate {
        8000 => corr_lpf_8000_240(),
        11025 => corr_lpf_11025_240(),
        22050 => corr_lpf_22050_240(),
        44100 => corr_lpf_44100_240(),
        48000 => corr_lpf_48000_240(),
        #[cfg(feature = "std")]
        _ => lowpass_coeffs(sample_rate, 240.0, 0.707),
        #[cfg(not(feature = "std"))]
        _ => corr_lpf_11025_240(), // fallback
    }
}

/// Check if 300 baud correlation LPF needs cascading at a given sample rate.
///
/// At high sample rates (>=22050 Hz), a single 120 Hz biquad has coefficients
/// too small for Q15 precision (b0=2 at 44100 Hz). Two cascaded 240 Hz biquads
/// provide equivalent rolloff with ~4x better coefficient resolution.
pub fn corr_300_needs_cascade(sample_rate: u32) -> bool {
    sample_rate >= 22050
}

/// Select a precomputed correlation LPF by cutoff frequency and sample rate.
///
/// Supports 100/120/400/450/500/550/600 Hz cutoffs at common sample rates.
/// Falls back to runtime computation on `std`, or 500 Hz on `no_std`.
pub fn corr_lpf_by_cutoff(sample_rate: u32, cutoff_hz: u32) -> BiquadFilter {
    // Snap to nearest supported cutoff
    let snapped = if cutoff_hz <= 110 { 100 }
        else if cutoff_hz <= 260 { 120 }
        else if cutoff_hz <= 425 { 400 }
        else if cutoff_hz <= 475 { 450 }
        else if cutoff_hz <= 525 { 500 }
        else if cutoff_hz <= 575 { 550 }
        else { 600 };

    match (sample_rate, snapped) {
        // 300 baud cutoffs
        (8000, 100) => corr_lpf_8000_100(),
        (11025, 100) => corr_lpf_11025_100(),
        (22050, 100) => corr_lpf_22050_100(),
        (44100, 100) => corr_lpf_44100_100(),
        (48000, 100) => corr_lpf_48000_100(),
        (8000, 120) => corr_lpf_8000_120(),
        (11025, 120) => corr_lpf_11025_120(),
        (22050, 120) => corr_lpf_22050_120(),
        (44100, 120) => corr_lpf_44100_120(),
        (48000, 120) => corr_lpf_48000_120(),
        // 1200 baud cutoffs
        (11025, 400) => corr_lpf_11025_400(),
        (11025, 450) => corr_lpf_11025_450(),
        (11025, 500) => corr_lpf_11025(),
        (11025, 550) => corr_lpf_11025_550(),
        (11025, 600) => corr_lpf_11025_600(),
        (13200, 400) => corr_lpf_13200_400(),
        (13200, 450) => corr_lpf_13200_450(),
        (13200, 500) => corr_lpf_13200(),
        (13200, 550) => corr_lpf_13200_550(),
        (13200, 600) => corr_lpf_13200_600(),
        (22050, 400) => corr_lpf_22050_400(),
        (22050, 450) => corr_lpf_22050_450(),
        (22050, 500) => corr_lpf_22050(),
        (22050, 550) => corr_lpf_22050_550(),
        (22050, 600) => corr_lpf_22050_600(),
        (26400, 400) => corr_lpf_26400_400(),
        (26400, 450) => corr_lpf_26400_450(),
        (26400, 500) => corr_lpf_26400(),
        (26400, 550) => corr_lpf_26400_550(),
        (26400, 600) => corr_lpf_26400_600(),
        (44100, 400) => corr_lpf_44100_400(),
        (44100, 450) => corr_lpf_44100_450(),
        (44100, 500) => corr_lpf_44100(),
        (44100, 550) => corr_lpf_44100_550(),
        (44100, 600) => corr_lpf_44100_600(),
        #[cfg(feature = "std")]
        _ => lowpass_coeffs(sample_rate, cutoff_hz as f64, 0.707),
        #[cfg(not(feature = "std"))]
        _ => corr_lpf(sample_rate), // fallback to 500 Hz
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
