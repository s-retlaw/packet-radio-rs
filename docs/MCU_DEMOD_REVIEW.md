# MCU-Oriented Demod Review and Recommendations

## Correction Applied

Phase 1 target is **Track 2 >= 970 for a single decoder**, not multi-decoder.

## Feasibility Estimate (Phase 1)

Estimated likelihood of hitting **Track 2 >= 970 (single decoder)**:

- **Current code as-is:** low (~5%)
- **After DM+PLL integration and tuning:** moderate (~25-40%)
- **With light adaptive timing/threshold feedback (still single decoder):** moderate-to-good (~50-65%)

Interpretation:

- Achieving 970 with a single decoder is ambitious and requires a true DM+PLL runtime path.
- The highest-impact near-term work is replacing fixed Bresenham symbol timing in DM mode with PLL-driven symbol extraction.

## Success Targets

| Phase | Track 2 Target | Technique |
|---|---:|---|
| 1 | >= 970 | Delay-multiply + PLL + HDLC (single decoder) |
| 2 | >= 1000 | + Adaptive tracker |
| 3 | >= 1025 | + Soft-decision bit-flip |
| 4 | >= 1040 | + Hilbert + Viterbi |

## Current State (from code review)

1. `DmDemodulator` currently uses fixed Bresenham timing, not PLL timing.
2. `ClockRecoveryPll` exists but is not wired into active demod runtime.
3. Quality path computes tracker state, but hard decisions are still Goertzel-based and tracker feedback is not driving timing/decision thresholds.
4. `MultiDecoder` uses narrow/wide 11025 filters unconditionally in diversity set, which is incorrect for 22050/44100 runs.
5. Desktop runtime is now wired and includes fast/quality/multi/dm paths, and quality mode uses `SoftHdlcDecoder`.

## Key Gaps vs Phase Plan

### Phase 1 gap (single decoder >= 970)

- The intended Phase 1 architecture is DM + PLL + HDLC.
- Runtime DM path is DM + Bresenham + HDLC, so it is not yet the target architecture.

### Phase 2 gap (adaptive tracker)

- Tracker is present, but not yet used to adapt symbol timing (PLL nominal rate) or hard decision thresholding in runtime.

### Phase 3 gap (soft decode)

- Soft decode is integrated in quality mode, but LLR quality can be improved by using tracker-informed frequency confidence rather than mostly energy ratio.

## MCU Viability

The intended Phase 1 decoder (**single DM + PLL + HDLC**) is MCU-viable.

- Works within fixed-size buffers and fixed-point-friendly arithmetic.
- Suitable for ESP32 and RP2040/Pico class devices.
- No dynamic allocation is required in the demod/HDLC path.

Recommended per-target profiles:

- **Pico/RP2040:** single DM+PLL only, no Hilbert, no multi-decoder.
- **ESP32:** single DM+PLL baseline; optional quality/soft path if CPU budget permits.
- **Desktop:** multi-decoder/high-effort comparison modes.

## Prioritized Recommendations

1. **Wire PLL into `DmDemodulator` first (Phase 1 blocker).**  
   Replace DM symbol sampling by `ClockRecoveryPll::update(disc_out)` events.

2. **Use tracker feedback to adjust timing (Phase 2 blocker).**  
   Feed tracker-estimated symbol period into PLL (`adapt_baud_rate`) once lock confidence is reached.

3. **Fix sample-rate filter diversity in `MultiDecoder`.**  
   Add narrow/wide variants for 22050 and 44100 (or runtime coeff generation on std path).

4. **Improve LLR quality in quality path.**  
   Combine energy-based LLR with tracker/inst-freq distance from adaptive threshold.

5. **Keep profiles target-specific for MCU viability.**  
   - Pico profile: single DM+PLL, fixed filters, no Hilbert, no multi.  
   - ESP32 profile: DM+PLL plus optional quality/soft decode.  
   - Desktop profile: multi-decoder and highest-effort modes.

## Suggested Implementation Sequence

1. Implement `DmDemodulator` PLL timing path and benchmark it on Track 2.
2. Integrate adaptive tracker output into PLL nominal timing.
3. Re-run single-decoder Track 2 and tune only DM/PLL path until it reaches >=970.
4. Add tracker-informed thresholds to target >=1000.
5. Optimize soft LLR path and bit-flip recovery to target >=1025.
6. Add Hilbert+Viterbi experiments only after prior phases are stable.

## Validation Expectations

- Keep a strict baseline and a best-effort baseline for every Track 2 run.
- Record decode count, processing speed, and recovered-frame count.
- Require no regressions in `no_std` builds while improving desktop score.
