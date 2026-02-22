# Test WAV Files

This directory holds WAV audio files for testing the AFSK demodulator.
**These files are NOT committed to git** (they're in .gitignore).

## How to Obtain Test Files

### 1. WA8LMF TNC Test CD (Primary Benchmark)

The gold standard for APRS decoder benchmarking.

- **URL:** http://wa8lmf.net/TNCtest/
- Download the WAV files and place them here
- Track 1 is the primary benchmark target
- Expected results: Dire Wolf decodes ~1000+ packets from Track 1

### 2. Dire Wolf Test Files

- Clone Dire Wolf: `git clone https://github.com/wb2osz/direwolf.git`
- Copy WAV files from `direwolf/test/` into this directory

### 3. Generate Test WAV Files

Once the modulator is implemented:

```bash
cargo run -p test-gen -- generate --output tests/wav/generated_test.wav
```

### 4. Record Your Own

- Tune an FM receiver to 144.390 MHz (North America) or 144.800 MHz (Europe)
- Record audio to a WAV file: 44100 Hz, 16-bit, mono
- An RTL-SDR ($25) with GQRX works well for this

## File Format Requirements

- Format: WAV (RIFF), PCM
- Sample rate: 11025, 22050, 44100, or 48000 Hz
- Bit depth: 16-bit signed integer
- Channels: Mono (or left channel will be used)
