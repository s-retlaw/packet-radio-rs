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
            done: false,
        })
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

/// Raw i16 LE PCM audio from stdin (for pipe mode).
pub struct StdinSource {
    reader: std::io::BufReader<std::io::Stdin>,
}

impl StdinSource {
    /// Create a new stdin audio source.
    pub fn new() -> Self {
        Self {
            reader: std::io::BufReader::new(std::io::stdin()),
        }
    }
}

impl SampleSource for StdinSource {
    fn read_samples(&mut self, buf: &mut [i16]) -> usize {
        use std::io::Read;
        // Read raw bytes: 2 bytes per i16 sample, little-endian
        let byte_count = buf.len() * 2;
        let mut raw = vec![0u8; byte_count];
        let mut total_read = 0;
        while total_read < byte_count {
            match self.reader.read(&mut raw[total_read..]) {
                Ok(0) => break, // EOF
                Ok(n) => total_read += n,
                Err(_) => break,
            }
        }
        // Convert pairs of bytes to i16 LE
        let complete_samples = total_read / 2;
        for i in 0..complete_samples {
            buf[i] = i16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]);
        }
        complete_samples
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
