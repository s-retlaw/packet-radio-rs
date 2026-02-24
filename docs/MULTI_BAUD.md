# Multi-Baud Support — Design Notes

This document analyzes which parts of the codebase are generic (baud-rate
independent) versus 1200-baud-specific, and outlines what each target baud rate
would require.

---

## Current State: What's 1200-Specific vs Generic

### Generic (works for any baud rate)

These components are parameterized or operate at the bit/frame level with no
frequency or baud rate assumptions:

| Component | Location | Notes |
|-----------|----------|-------|
| `DemodConfig` / `ModConfig` structs | `core/src/modem/mod.rs` | Fields: `mark_freq`, `space_freq`, `baud_rate`, `sample_rate` |
| `samples_per_symbol()` | `mod.rs:126,159` | Computed from config |
| Bresenham symbol timing | `demod.rs` | Uses `config.samples_per_symbol()` |
| Goertzel algorithm | `demod.rs` | Runtime fallback computes coefficients for any frequency pair |
| NRZI encoding/decoding | `hdlc.rs` | Bit-level, frequency-agnostic |
| HDLC framing | `hdlc.rs` | Bit-level (flags, bit-stuffing, CRC-16) |
| SoftHdlcDecoder | `soft_hdlc.rs` | LLR + recovery chain, baud-agnostic |
| AX.25 frame parsing | `ax25.rs` | Operates on decoded bytes |
| APRS parsing | `aprs.rs` | Operates on AX.25 information field |
| KISS protocol | `kiss.rs` | Byte-level framing |
| FNV-1a dedup | `multi.rs`, `corr_slicer.rs` | Hash-based, content-agnostic |
| SIN_TABLE_Q15 | `mod.rs` | 256-entry lookup, used by NCO modulator |
| `corr_lpf_for_config()` | `filter.rs:282` | Dynamic LPF cutoff from tone separation |
| `bandpass_coeffs()` | `filter.rs:82` | Runtime BPF generation for any center/bandwidth |

### 1200-Baud Hardcoded (needs work for multi-baud)

| Item | Location | What's hardcoded |
|------|----------|------------------|
| Module constants | `mod.rs:50-59` | `MARK_FREQ=1200`, `SPACE_FREQ=2200`, `BAUD_RATE=1200`, `MID_FREQ=1700` |
| Goertzel coefficient table | `demod.rs` | Precomputed Q14 values only for 1200/2200 Hz at 11025/22050/44100 Hz |
| `afsk_bandpass_11025()` | `filter.rs:179` | `const fn`, center=1700 Hz hardcoded |
| `afsk_bandpass_22050()` | `filter.rs:185` | `const fn`, center=1700 Hz hardcoded |
| `afsk_bandpass_44100()` | `filter.rs:191` | `const fn`, center=1700 Hz hardcoded |
| `afsk_bandpass_narrow_11025()` | `filter.rs:197` | `const fn`, 1200 Hz bandwidth |
| `afsk_bandpass_wide_11025()` | `filter.rs:203` | `const fn`, 2000 Hz bandwidth |
| `post_detect_lpf()` | `filter.rs:229` | Cutoff=1200 Hz hardcoded |
| `corr_lpf()` | `filter.rs:262` | 500 Hz special case for Bell 202 |
| `dm_delay_filtered()` / `dm_delay()` | `demod.rs` | Lookup tables tuned for 1700 Hz center |
| `is_mark_negative()` | `demod.rs` | Precomputed for Bell 202 delays |
| Multi-decoder diversity | `multi.rs` | Frequency offsets (±50/100 Hz), BPF variants, gain levels all tuned for 1200/2200 |
| MiniDecoder configs | `multi.rs` | Attribution-optimal for 1200 baud specifically |
| CorrSlicerDecoder gains/freqs | `corr_slicer.rs` | Gain table and ±50 Hz offsets tuned for Bell 202 |
| Desktop CLI defaults | `desktop/src/main.rs` | Sample rate defaults assume 1200 baud |

---

## Target Baud Rates

### 300 Baud — HF Packet (Bell 103 AFSK, 200 Hz shift)

**Standard:** Mark=1600 Hz, Space=1800 Hz, 200 Hz shift. Used for HF amateur
packet radio (below 30 MHz).

**Same modulation type as 1200 baud** — AFSK with tone detection — so the entire
Goertzel/correlation/DM pipeline applies. The key differences are timing
(~36.75 samples/symbol at 11025 Hz vs 9.2) and much narrower tone separation.

**What changes:**
- New `DemodConfig::default_300()` factory: mark=1600, space=1800, baud=300
- New Goertzel coefficient precomputation for 1600/1800 Hz
- New BPF: center=1700 Hz (same center as 1200 baud!), but bandwidth ~400 Hz
  (vs 1600 Hz). The existing `bandpass_coeffs()` runtime function handles this.
- Post-detect LPF: cutoff ~300 Hz (matching baud rate)
- Correlation LPF: ~100 Hz cutoff (from `corr_lpf_for_config()` — already parameterized)
- DM delay recomputation: optimal delay for 1700 Hz center at higher samples/symbol
- Multi-decoder diversity: frequency offsets should be ±10-25 Hz (narrower shift
  means tighter tolerance), timing phases still effective

**Unique challenges:**
- **Very narrow tone separation (200 Hz):** Needs much tighter LPF and longer
  Goertzel integration windows. At 300 baud, each symbol integrates over ~36
  cycles of the carrier — better SNR per symbol but more susceptible to
  frequency drift.
- **HF channel effects:** Fading, multipath, ionospheric Doppler (up to ±10 Hz).
  Adaptive retune may actually matter here (unlike 1200 baud VHF).
- **Longer frames:** Same AX.25 structure but 4× longer in time. More total
  bits → more chance of bit errors. Soft decode chain handles arbitrary lengths.

**Difficulty: LOW** — mostly config changes + new filter coefficients. The
existing `bandpass_coeffs()` and `corr_lpf_for_config()` already support
runtime parameterization. Main work is adding precomputed const filter tables
for the new frequencies (for no_std performance) and validating with HF test
recordings.

**Minimum sample rate:** 8000 Hz sufficient (Nyquist for 1800 Hz space tone
with margin).

### 2400 Baud — V.26 DPSK or AFSK

Two possible modulation schemes:

**AFSK variant (rare):** Same Bell 202 frequencies (1200/2200 Hz), double baud
rate. The Goertzel window would only integrate ~4.6 samples at 11025 Hz — very
marginal. Would need 22050+ Hz sample rate for reasonable performance.

- New `DemodConfig` with `baud_rate=2400`, otherwise same frequencies
- Tighter symbol timing, fewer samples per symbol → more sensitive to phase
- Multi-decoder timing diversity even more valuable
- **Difficulty: LOW** if AFSK

**DPSK variant (V.26):** Phase-shift keying — completely different demodulation.
The signal encodes data in phase transitions, not frequency.

- New demodulator module needed: `core/src/modem/dpsk.rs`
- Carrier recovery (Costas loop or decision-directed PLL)
- Differential decoding (compare current phase to previous)
- No Goertzel, no tone detection
- **Difficulty: HIGH** — new architecture, carrier recovery is hard

**Priority: LOW** — 2400 baud packet radio is rare in amateur radio.

### 4800 Baud

No standard amateur radio spec at 4800 baud. Would need either:
- AFSK with wider tone separation (non-standard)
- Multi-level FSK (4-FSK)
- DPSK

**Difficulty: MEDIUM** — likely a custom modulation scheme with no established
test corpus. Not recommended unless there's a specific use case.

### 9600 Baud — G3RUH GMSK/GFSK

**COMPLETELY DIFFERENT modulation.** Not tone detection — direct FM with Gaussian
pulse shaping.

**How G3RUH works:**
1. **Scrambler:** LFSR with polynomial x¹⁷ + x¹² + 1 whitens data to ensure
   DC balance and adequate transitions for clock recovery.
2. **Gaussian filter:** Smooths bit transitions to limit occupied bandwidth
   (BT=0.5 typical).
3. **FM modulation:** Filtered bitstream directly modulates FM transmitter.
   No audio tones — the baseband signal IS the data.
4. **Receive:** FM discriminator output is the baseband data. The radio's
   discriminator replaces our BPF+tone-detection stage.

**What carries over from 1200 baud:**
- HDLC framing (same bit-level protocol)
- SoftHdlcDecoder (LLR + recovery chain)
- AX.25 parsing
- APRS parsing
- KISS protocol
- FNV-1a dedup
- Desktop TNC infrastructure (audio I/O, TCP server)

**What's completely new:**
- `core/src/modem/gfsk.rs` — GFSK demodulator
  - Gaussian matched filter for receive
  - Symbol timing recovery (PLL may work here — much higher samples/symbol)
  - Bit slicer (threshold or multi-level)
- `core/src/modem/scrambler.rs` — G3RUH scrambler/descrambler
  - x¹⁷ + x¹² + 1 LFSR
  - Must run after HDLC decode (descramble) and before HDLC encode (scramble)
- New modulator for Gaussian pulse shaping + FM generation
- Different sample rates: 9600 baud needs ≥48000 Hz sample rate (vs 11025 for 1200)

**Feature flag:** `9600-baud` already exists in `core/Cargo.toml` (currently empty).

**Difficulty: HIGH** — entirely new demodulation pipeline, scrambler, different
sample rate requirements. However, it's the most demanded feature for high-speed
packet and Winlink gateways.

---

## Implementation Priority

| Priority | Baud Rate | Effort | Rationale |
|----------|-----------|--------|-----------|
| 1 | **300** | LOW | Enables HF packet. Same AFSK pipeline, mostly config. |
| 2 | **9600** | HIGH | Most demanded. Enables high-speed packet, Winlink. New demod pipeline. |
| 3 | 2400 AFSK | LOW | Niche use. Trivial if AFSK variant. |
| 4 | 4800 | MEDIUM | Very niche. No standard spec. |

---

## Refactoring Needed for Multi-Baud

### Phase 1: Parameterization (prerequisite for any new baud rate)

1. **Config factories:** Add `DemodConfig::default_300()`, `DemodConfig::default_9600()`,
   `ModConfig::default_300()`, etc. The struct fields are already parameterized;
   only the factory functions need adding.

2. **Dynamic BPF generation:** The `const fn` bandpass filters (`afsk_bandpass_11025()`,
   etc.) are 1200-baud-specific. For multi-baud, either:
   - Add parallel const tables for each baud rate (fast, but code bloat)
   - Use `bandpass_coeffs()` at init time (already exists, uses `libm` on no_std)
   - Recommended: const tables for supported configs, runtime fallback for custom

3. **Parameterize Multi-decoder diversity:** The 5-dimension diversity grid
   (freq offsets, BPF variants, gains) is hardcoded for 1200/2200 Hz. Factor
   the diversity generation into a function of `DemodConfig` so that multi-decoder
   automatically generates appropriate diversity for any AFSK configuration.

4. **Factor "AFSK demod" trait:** Extract the common Goertzel+Bresenham pipeline
   into a parameterized AFSK demodulator that works for 300/1200/2400 baud.
   The correlation and DM demodulators are already mostly parameterized via
   `DemodConfig`.

### Phase 2: 300 Baud Support

5. **Precompute 300-baud filters:** New const BPF (1700 Hz center, 400 Hz BW)
   and LPF (300 Hz cutoff) tables for 8000/11025 Hz sample rates.

6. **Goertzel coefficients:** Add 1600/1800 Hz entries to the coefficient lookup.

7. **Test corpus:** Record or synthesize 300-baud test WAVs for validation.

### Phase 3: 9600 Baud (G3RUH)

8. **New module: `gfsk.rs`** — Gaussian matched filter, symbol timing, bit slicer.

9. **New module: `scrambler.rs`** — x¹⁷ + x¹² + 1 LFSR for G3RUH.

10. **Higher sample rate support:** 48000 Hz minimum. The existing cpal audio
    backend already supports 48000 Hz; ESP32 I2S may need configuration changes.

11. **Runtime baud-rate selection:** Desktop CLI `--baud 300|1200|9600` flag.
    ESP32 config struct gets baud_rate field.

---

## Appendix: What Optimization Learnings Transfer to Other Baud Rates

| Technique | 300 Baud | 9600 Baud | Notes |
|-----------|----------|-----------|-------|
| Multi-decoder diversity | Yes | Yes | Parameter uncertainty is universal |
| Timing phase diversity | Yes | Yes | Even more important at higher baud (fewer samples/symbol) |
| Frequency offset diversity | Yes | N/A | Only for AFSK. G3RUH has no tones. |
| Gain/slicer diversity | Partial | Yes | De-emphasis varies by radio at any baud |
| SoftHdlcDecoder | Yes | Yes | Bit-level, baud-agnostic |
| Energy LLR | Yes | Needs new | Goertzel LLR works for AFSK. G3RUH needs discriminator-based LLR. |
| Bresenham > PLL | Maybe | Likely no | At 300 baud (~36 samples/symbol), PLL may finally have enough resolution. At 9600 baud (5 samples/symbol at 48kHz), Bresenham phase diversity is probably still better. |
| FNV-1a dedup | Yes | Yes | Content-based, baud-agnostic |
| Attribution tooling | Yes | Yes | Same greedy set-cover analysis works for any decoder ensemble |
