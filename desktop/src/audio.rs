//! Audio input sources — sound card (cpal) and WAV file (hound).

use packet_radio_core::SampleSource;
use crossbeam_channel::{Receiver, bounded};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Live sound card input via cpal.
pub struct CpalSource {
    rx: Receiver<Vec<i16>>,
    buf: Vec<i16>,
    pos: usize,
    _stream: cpal::Stream,
}

impl CpalSource {
    /// Open the default (or named) input device at the given sample rate.
    pub fn open(device_name: &str, sample_rate: u32) -> Result<Self, String> {
        let host = cpal::default_host();

        let device = if device_name == "default" {
            host.default_input_device()
                .ok_or_else(|| "no default input device".to_string())?
        } else {
            host.input_devices()
                .map_err(|e| format!("enumerating devices: {e}"))?
                .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
                .ok_or_else(|| format!("device not found: {device_name}"))?
        };

        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let (tx, rx) = bounded::<Vec<i16>>(64);

        let stream = device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let _ = tx.try_send(data.to_vec());
            },
            move |err| {
                tracing::error!("audio input error: {err}");
            },
            None,
        ).map_err(|e| format!("building input stream: {e}"))?;

        stream.play().map_err(|e| format!("starting stream: {e}"))?;

        Ok(Self {
            rx,
            buf: Vec::new(),
            pos: 0,
            _stream: stream,
        })
    }
}

impl SampleSource for CpalSource {
    fn read_samples(&mut self, buf: &mut [i16]) -> usize {
        let mut written = 0;

        // Drain leftover from previous chunk
        while written < buf.len() && self.pos < self.buf.len() {
            buf[written] = self.buf[self.pos];
            self.pos += 1;
            written += 1;
        }

        // Get more chunks from the channel
        while written < buf.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.buf = chunk;
                    self.pos = 0;
                    while written < buf.len() && self.pos < self.buf.len() {
                        buf[written] = self.buf[self.pos];
                        self.pos += 1;
                        written += 1;
                    }
                }
                Err(_) => break, // Channel closed
            }
        }

        written
    }
}

/// WAV file audio source.
pub struct WavSource {
    reader: hound::WavReader<std::io::BufReader<std::fs::File>>,
    channels: u16,
    sample_rate: u32,
    done: bool,
}

impl WavSource {
    /// Open a WAV file for reading.
    pub fn open(path: &std::path::Path, expected_rate: u32) -> Result<Self, String> {
        let reader = hound::WavReader::open(path)
            .map_err(|e| format!("opening WAV: {e}"))?;
        let spec = reader.spec();

        if spec.sample_rate != expected_rate {
            tracing::warn!(
                "WAV sample rate {} != expected {}, proceeding anyway",
                spec.sample_rate, expected_rate
            );
        }

        if spec.bits_per_sample != 16 {
            tracing::warn!("WAV is {} bits, converting to i16", spec.bits_per_sample);
        }

        Ok(Self {
            reader,
            channels: spec.channels,
            sample_rate: spec.sample_rate,
            done: false,
        })
    }

    /// Return the WAV file's actual sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl SampleSource for WavSource {
    fn read_samples(&mut self, buf: &mut [i16]) -> usize {
        if self.done {
            return 0;
        }

        let mut written = 0;
        let mut samples = self.reader.samples::<i16>();
        let channels = self.channels as usize;

        while written < buf.len() {
            match samples.next() {
                Some(Ok(s)) => {
                    // Take only the first channel if stereo
                    if channels <= 1 {
                        buf[written] = s;
                        written += 1;
                    } else {
                        buf[written] = s;
                        written += 1;
                        // Skip remaining channels
                        for _ in 1..channels {
                            let _ = samples.next();
                        }
                    }
                }
                Some(Err(_)) => {
                    self.done = true;
                    break;
                }
                None => {
                    self.done = true;
                    break;
                }
            }
        }

        written
    }
}

/// Audio from stdin with auto-detection of WAV vs raw PCM.
///
/// On construction, peeks at the first 4 bytes of stdin. If they match the
/// RIFF magic, the stream is parsed as WAV (via hound) and the sample rate,
/// channels, and bit depth are extracted automatically. Otherwise the stream
/// is treated as raw i16 little-endian PCM.
pub struct StdinSource {
    inner: StdinInner,
}

enum StdinInner {
    Wav {
        reader: hound::WavReader<
            std::io::Chain<std::io::Cursor<Vec<u8>>, std::io::BufReader<std::io::Stdin>>,
        >,
        channels: u16,
        detected_rate: u32,
        done: bool,
    },
    Raw {
        reader: std::io::BufReader<std::io::Stdin>,
        leftover: Vec<u8>,
        leftover_pos: usize,
    },
}

impl StdinSource {
    /// Create a new stdin audio source, auto-detecting WAV vs raw PCM.
    pub fn new() -> Result<Self, String> {
        use std::io::Read;
        let mut reader = std::io::BufReader::new(std::io::stdin());
        let mut magic = [0u8; 4];
        let mut total = 0;
        while total < 4 {
            match reader.read(&mut magic[total..]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) => return Err(format!("reading stdin: {e}")),
            }
        }

        if total >= 4 && &magic == b"RIFF" {
            let peek = magic[..total].to_vec();
            let chained = std::io::Cursor::new(peek).chain(reader);
            match hound::WavReader::new(chained) {
                Ok(wav_reader) => {
                    let spec = wav_reader.spec();
                    Ok(Self {
                        inner: StdinInner::Wav {
                            channels: spec.channels,
                            detected_rate: spec.sample_rate,
                            reader: wav_reader,
                            done: false,
                        },
                    })
                }
                Err(e) => Err(format!(
                    "RIFF header detected on stdin but WAV parse failed: {e}"
                )),
            }
        } else {
            Ok(Self {
                inner: StdinInner::Raw {
                    reader,
                    leftover: magic[..total].to_vec(),
                    leftover_pos: 0,
                },
            })
        }
    }

    /// Returns the WAV sample rate if WAV was detected on stdin.
    pub fn detected_sample_rate(&self) -> Option<u32> {
        match &self.inner {
            StdinInner::Wav { detected_rate, .. } => Some(*detected_rate),
            StdinInner::Raw { .. } => None,
        }
    }
}

impl SampleSource for StdinSource {
    fn read_samples(&mut self, buf: &mut [i16]) -> usize {
        match &mut self.inner {
            StdinInner::Wav {
                reader,
                channels,
                done,
                ..
            } => {
                if *done {
                    return 0;
                }
                let mut written = 0;
                let ch = *channels as usize;
                let mut samples = reader.samples::<i16>();
                while written < buf.len() {
                    match samples.next() {
                        Some(Ok(s)) => {
                            buf[written] = s;
                            written += 1;
                            if ch > 1 {
                                for _ in 1..ch {
                                    let _ = samples.next();
                                }
                            }
                        }
                        _ => {
                            *done = true;
                            break;
                        }
                    }
                }
                written
            }
            StdinInner::Raw {
                reader,
                leftover,
                leftover_pos,
            } => {
                use std::io::Read;
                let byte_count = buf.len() * 2;
                let mut raw = vec![0u8; byte_count];
                let mut total_read = 0;

                // Drain leftover bytes from the magic peek
                while total_read < byte_count && *leftover_pos < leftover.len() {
                    raw[total_read] = leftover[*leftover_pos];
                    *leftover_pos += 1;
                    total_read += 1;
                }

                // Read remaining from stdin
                while total_read < byte_count {
                    match reader.read(&mut raw[total_read..]) {
                        Ok(0) => break,
                        Ok(n) => total_read += n,
                        Err(_) => break,
                    }
                }

                let complete_samples = total_read / 2;
                for i in 0..complete_samples {
                    buf[i] = i16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]);
                }
                complete_samples
            }
        }
    }
}

/// List available audio input devices.
pub fn list_devices() {
    let host = cpal::default_host();

    println!("Audio input devices:");
    println!("--------------------");

    if let Some(device) = host.default_input_device() {
        let name = device.name().unwrap_or_else(|_| "(unknown)".to_string());
        println!("  * {name}  (default)");
    }

    match host.input_devices() {
        Ok(devices) => {
            for device in devices {
                let name = device.name().unwrap_or_else(|_| "(unknown)".to_string());
                println!("    {name}");
            }
        }
        Err(e) => {
            eprintln!("Error enumerating devices: {e}");
        }
    }
}
