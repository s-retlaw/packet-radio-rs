# Novel Demodulation Strategies — Analysis & Proposals

## Context

We're at 99.0% of Dire Wolf on Combined mode (2153/2174), with 81 frames still
missed on Track 2 alone. Dozens of optimization experiments have been run. This
document synthesizes WHY things worked or didn't, and proposes genuinely creative
new ideas that emerge from those patterns.

---

## Part 1: The High-Level Concepts

### Why Things Worked — Three Core Principles

**Principle 1: Hedge, Don't Track.**

At 9.2 samples/symbol, you CAN'T adaptively track parameters with enough
resolution. PLL corrections are ~11% of a symbol — too coarse, and any filter in
the feedback path adds group delay that shifts the error signal by 20-30% of a
symbol. Every PLL experiment failed for this exact reason.

What works instead is *hedging*: run multiple fixed instances that cover the
parameter space, and take whatever sticks. This is the same insight behind
portfolio diversification and ensemble methods in ML. The multi-decoder's 66%
improvement over single-decoder (563→935 on T2) is entirely from hedging, not
from any individual decoder being better.

The corollary: **timing phase diversity is the highest-value dimension.** Going
from 1 to 3 Bresenham phases improved correlation from 639→906 (+42%). This
outperformed every other single technique tried.

**Principle 2: Orthogonal Math Finds Orthogonal Frames.**

Goertzel (windowed energy), Correlation (matched filter), and DM (frequency
discriminator) use fundamentally different mathematics. Their error patterns are
decorrelated — they fail on *different* signals. Combined mode's 23 exclusive
frames (14 Multi-only + 9 CorrSlicer-only on T2) prove this.

This is exactly the same principle behind ensemble learning: if classifiers are
diverse, combining them beats any individual. The key is that the diversity must
be *algorithmic* (different math), not just *parametric* (same math, different
settings). Parameter diversity gives you Multi's 935; algorithmic diversity on
top gives you Combined's 946.

**Principle 3: Confidence Must Be Calibrated to Be Useful.**

Soft decode only works when LLR values actually correlate with error probability.
Goertzel energy ratios provide this — a 60/40 energy split genuinely means ~60%
confidence in that bit. DM accumulator magnitude does NOT — accumulate-and-dump
destroys the fine structure, producing binary-like confidence (very high or very
low, no useful middle ground). Result: 28 soft saves with Goertzel, 0 with DM.

The lesson: **the detection method must preserve fine-grained uncertainty.** Any
processing step that forces a hard decision or saturates the confidence metric
destroys information that soft decode needs.

### Why Things Didn't Work — Three Failure Modes

**Failure Mode 1: Group Delay Poisons Feedback Loops.**

Every filter (BPF, LPF, Hilbert) adds delay that shifts the error signal relative
to the optimal sampling point. Gardner PLL on Goertzel failed because the DM delay
(2-8 samples) + LPF (3-4 samples) = 5-12 samples of shift — more than an entire
symbol period. The PLL locks onto *the wrong point*. Beta>0 fails for the same
reason: systematic bias from group delay gets integrated into frequency correction,
pushing to saturation.

**Failure Mode 2: Amplifying Everything Amplifies Noise More Than Signal.**

Pre-emphasis, cascaded BPF, and other "sharpen the signal" approaches fail because
they can't distinguish signal from noise in the frequency domain — they amplify
both. On de-emphasized Track 2 signals, the noise floor is already elevated at
high frequencies; any boost makes it worse. The demodulator's energy comparison is
already somewhat robust to mild imbalance.

**Failure Mode 3: Diminishing Returns in Ensembles.**

Once you have 38 decoders covering the parameter space, tuning any individual
decoder has negligible impact (alpha tuning: <1 frame). The ensemble already
covers each decoder's failure cases. This is the "curse of ensemble success" — the
better the ensemble, the less any single member matters.

---

## Part 2: Where Information Is Discarded (Pipeline Analysis)

Understanding WHERE information is lost points to WHERE novel techniques could
recover it.

### Goertzel: State Reset Destroys Cross-Symbol Correlation

At every symbol boundary, `mark_s1, mark_s2, space_s1, space_s2` are zeroed.
This means:
- **Phase continuity** is lost (s1/s2 encode phase, but it's thrown away)
- **Inter-symbol interference** from BPF ringing is neither measured nor
  compensated
- **Energy trajectory** across the symbol (rising? falling? flat?) is compressed
  to a single number

The Goertzel state at boundary actually contains `(energy, phase)` — we use only
the energy and discard the phase.

### Multi-Decoder: Frame-Level Dedup Wastes Bit-Level Diversity

32 Goertzel decoders with the same timing phase but different freq/gain/BPF
settings produce 32 different LLR streams for the *same bit positions*. Each
feeds an independent SoftHdlcDecoder. There is ZERO cross-decoder information
sharing at the bit level.

This is like having 32 witnesses to the same event, asking each one
independently, and then deduplicating their stories — instead of letting them
compare notes.

### Correlation: I/Q Phase Discarded

The correlation demodulator computes mark_i, mark_q, space_i, space_q — full
complex baseband. But only `I² + Q²` (magnitude squared) is used. The phase
`atan2(Q, I)` is thrown away. Phase carries information about frequency offset
and timing alignment that magnitude doesn't.

### Preamble: Free Training Data Underutilized

The preamble (0x7E flags) is a KNOWN pattern generating a specific NRZI sequence.
It's used for: flag detection, phase scoring (CorrSlicer), adaptive retune
(Goertzel). But it could also provide: optimal timing phase, de-emphasis curve,
SNR estimate, and per-packet decoder ranking.

---

## Part 3: Novel Ideas

### Tier 1 — Most Promising & Novel

#### Idea A: Cross-Decoder Soft Bit Fusion (Maximum Ratio Combining)

**Core concept:** Instead of running 32 independent SoftHdlcDecoders, COMBINE
the LLR values from multiple decoders at the bit level before soft decode.

This is **Maximum Ratio Combining (MRC)** from MIMO wireless systems, but applied
to *parameter diversity* instead of *antenna diversity*. No AFSK implementation
does this.

**How it works:**
1. Group decoders by timing phase (same Bresenham offset → bits align 1:1)
2. Within each timing group, for each bit position:
   `fused_llr[i] = mean(llr_d[i] for d in group)`
3. Feed the fused LLR stream to ONE SoftHdlcDecoder per timing group
4. Noise averages out across parameter-diverse decoders; signal reinforces

**Why it works (theoretically):** Consider a bit where decoder A (narrow BPF)
gives LLR=+30 (weak mark) and decoder B (freq-shifted) gives LLR=+80 (strong
mark). Individually, decoder A marks this as a low-confidence bit (flip
candidate). But fused LLR=+55 — medium confidence, correctly deprioritized for
flipping. The fusion correctly identifies that this is actually a mark despite
one decoder's poor view.

**Why it's novel:** Every multi-decoder implementation (Dire Wolf, multimon-ng,
this project) deduplicates at the frame level. No one fuses soft bits across
parallel decoders. The key insight enabling this is that within-timing-group
decoders see the *same* bits through different "lenses."

**Cost:** 3 additional SoftHdlcDecoders plus LLR accumulator buffers. Net memory
savings possible if individual decoders can be reduced, but currently additive.

**Expected impact:** 5-30 additional frames on T2 where individual decoders all
failed CRC but the fused LLR pinpoints correct flip targets.

#### Idea B: Retrospective Segment Re-Decoding

**Core concept:** When a decoder detects a preamble but fails CRC (even after
soft recovery), buffer the raw audio segment and re-decode it with a fine
parameter grid search.

**How it works:**
1. Preamble detector fires → start buffering raw audio into a ring buffer
2. Frame attempt fails CRC after all recovery stages
3. Save segment: ~3300 samples (300ms at 11025 Hz) = 6.6KB of i16
4. Re-decode with a focused grid:
   - 8 sub-sample timing offsets (via polyphase interpolation)
   - 5 frequency offsets (−60, −30, 0, +30, +60 Hz)
   - 4 gain levels
   = 160 attempts on ONE audio segment
5. If any attempt yields valid CRC → output frame

**ESP32 feasibility:** A frame is ~300ms. Re-decode with 160 parameter
combinations takes ~50ms on ESP32. There's typically 200-500ms between frames.

**Expected impact:** Directly targets the 81 DW-only frames on T2.

#### Idea C: Preamble-Trained Per-Packet Decoder Selection

**Core concept:** Use the known preamble as a free training sequence to SELECT
which decoder configuration to use for this specific packet, rather than running
all decoders blindly.

**How it works:**
1. During preamble flags, run 8-10 candidate decoder configurations
2. Score each by "preamble quality": energy contrast ratio on known bit pattern,
   flag detection timing accuracy, phase stability
3. Rank candidates; commit top-3 for the data portion
4. Drop the other 5-7 candidates (save compute)

**ESP32 impact:** Instead of MiniDecoder's fixed 3 decoders, run 8-10 during the
short preamble (~100ms), pick the best 3, then run only those for data. Turns a
static ensemble into an adaptive one.

---

### Tier 2 — High Potential, Moderate Complexity

#### Idea D: Viterbi Sequence Decoder Over NRZI+Stuffing Trellis

**Core concept:** Instead of hard bit decisions → HDLC → CRC check → post-hoc
bit flipping, use a Viterbi algorithm that finds the maximum-likelihood bit
sequence through the NRZI+bit-stuffing state machine.

**Trellis structure:**
- NRZI: 2 states (last raw bit 0 or 1)
- Bit stuffing counter: 0-5 consecutive 1s
- Total: 12 states
- Branch metric: Goertzel energy LLR for each bit choice

For a 300-byte frame (~2400 bits): 2400 × 12 = 28,800 state evaluations. Very
feasible, even on ESP32.

**Why it's better than SoftHdlcDecoder:** The current soft decoder flips
individual bits independently. Viterbi naturally handles correlated burst errors
from timing slips by connecting adjacent symbols through the trellis.

#### Idea E: Goertzel Phase Continuity as Confidence Metric

**Core concept:** The Goertzel state (s1, s2) at the symbol boundary encodes
both energy AND phase. We use only energy. Extract the phase and track its
continuity across symbols as a third confidence dimension.

**How it works:**
- At boundary: `phase = atan2(s2, s1)` (integer CORDIC approximation)
- Track `delta_phase[n] = phase[n] - phase[n-1]`
- Large deviation from expected → timing slip or frequency offset
- Use phase continuity score as LLR modifier

**Cost:** One integer atan2 per symbol (CORDIC: ~30 ops). Negligible.

#### Idea F: Fractional-Sample Polyphase Timing

**Core concept:** At 9.2 samples/symbol, integer timing gives ~11% resolution.
Polyphase interpolation gives sub-sample timing at minimal cost.

**How it works:**
- Precompute 4 short polyphase FIR banks (4 taps each, fixed Q15 coefficients)
- At each symbol boundary, compute interpolated sample values
- Run Goertzel energy on interpolated values at each candidate offset
- Select offset with maximum mark/space energy contrast

Gives 4× timing resolution (effective 36.8 "phases" per symbol vs. 9.2).

---

### Tier 3 — Creative / Experimental

#### Idea G: Stochastic Resonance Re-Decoding

**Core concept:** For frames that fail CRC, add controlled random noise to the
audio segment and re-decode multiple times. Majority-vote the bit sequences.

**The physics:** Stochastic resonance is a real phenomenon where noise improves
detection in nonlinear threshold systems. The Goertzel energy comparison IS a
nonlinear threshold. Majority vote extracts the statistical edge over many trials.

**Risk:** May not help if errors are from timing slips. Most useful for
borderline energy decisions.

#### Idea H: Inter-Frame Temporal Coherence

**Core concept:** On a busy channel, the same transmitter sends multiple frames
within seconds. Its frequency offset, de-emphasis curve, and timing don't change
between frames. Cache the decoder configuration that worked for frame N and bias
toward it for frame N+1.

Simple LRU cache: last 4 successful (decoder_config, timestamp) pairs. Low
complexity, modest expected gain, trivial to implement.

---

## Part 4: Ranking & Recommended Exploration Order

| Rank | Idea | Novelty | Expected Impact | Complexity | ESP32 Feasible |
|------|------|---------|-----------------|------------|----------------|
| 1 | **A: Soft Bit Fusion (MRC)** | Very high | High (10-30 frames) | Medium | Yes (saves memory) |
| 2 | **B: Retrospective Re-Decode** | High | High (15-40 frames) | Medium | Yes (async) |
| 3 | **C: Preamble-Trained Selection** | High | Medium (5-15 frames) | Low | Yes (designed for it) |
| 4 | **D: Viterbi NRZI Trellis** | Very high | Medium-High (10-25 frames) | Medium-High | Yes (28K ops/frame) |
| 5 | **E: Phase Continuity** | High | Low-Medium (3-10 frames) | Low | Yes (~30 ops/symbol) |
| 6 | **F: Polyphase Timing** | Medium | Medium (5-15 frames) | Low-Medium | Yes (16 muls/boundary) |
| 7 | **G: Stochastic Resonance** | Very high | Unknown (0-20 frames) | Low | Marginal (20× re-decode) |
| 8 | **H: Temporal Coherence** | Medium | Low (2-5 frames) | Very Low | Yes |

### Recommended exploration order for maximum learning:

1. **Start with E (Phase Continuity)** — cheapest experiment, reveals whether
   Goertzel phase carries useful information. Validates the "we're throwing away
   information" thesis.

2. **Then A (Soft Bit Fusion)** — the biggest conceptual leap. If fusing LLR
   across decoders improves soft decode, it's a fundamental advance in
   multi-decoder architecture.

3. **Then B (Retrospective Re-Decode)** — directly targets the 81-frame gap.
   Combined with F (polyphase timing), covers much finer parameter space.

4. **Then D (Viterbi)** — the most theoretically grounded approach. Replaces
   SoftHdlcDecoder's ad-hoc bit-flipping with optimal sequence decoding.

---

## Verification

- **A (MRC):** Compare fused-LLR SoftHdlc results vs independent-SoftHdlc on T2
- **B (Re-decode):** Count preamble-detected-but-CRC-failed events, test re-decode hit rate
- **C (Preamble selection):** Compare preamble-selected top-3 vs fixed Smart3
- **D (Viterbi):** Compare Viterbi vs SoftHdlc on frames with known burst errors
- **E (Phase):** Compute phase continuity metric on T2, correlate with bit errors
- **F (Polyphase):** Compare 4× fractional timing vs integer timing on T2
- **G (Stochastic):** Re-decode failed frames with noise injection, measure hit rate
- **H (Temporal):** Measure frame-to-frame decoder config correlation on T1
