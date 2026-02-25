/// Binary protocol for RP2040 test harness.
///
/// Simple length-prefixed messages over USB-CDC serial.
/// Header: [msg_type: u8] [flags: u8] [payload_len: u16 LE]

/// Header size in bytes.
pub const HEADER_SIZE: usize = 4;

/// Maximum payload size (512 samples * 2 bytes + seq overhead).
pub const MAX_PAYLOAD: usize = 1030;

/// Maximum message size (header + payload).
pub const MAX_MSG_SIZE: usize = HEADER_SIZE + MAX_PAYLOAD;

// Host -> device message types
pub const MSG_PING: u8 = 0x01;
pub const MSG_CONFIG: u8 = 0x02;
pub const MSG_AUDIO_CHUNK: u8 = 0x10;
pub const MSG_STREAM_END: u8 = 0x11;

// Device -> Host message types
pub const MSG_PONG: u8 = 0x81;
pub const MSG_READY: u8 = 0x82;
pub const MSG_FRAME: u8 = 0x90;
pub const MSG_CHUNK_ACK: u8 = 0x91;
pub const MSG_STATS: u8 = 0x92;
pub const MSG_ERROR: u8 = 0xFF;

// Decoder mode constants
pub const MODE_FAST: u8 = 0x00;
pub const MODE_QUALITY: u8 = 0x01;
pub const MODE_MINI: u8 = 0x02;
pub const MODE_CORR3: u8 = 0x03;
pub const MODE_TNC: u8 = 0x04;

/// Parsed message header.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub msg_type: u8,
    pub flags: u8,
    pub payload_len: u16,
}

impl Header {
    /// Parse header from 4 bytes.
    pub fn parse(buf: &[u8; HEADER_SIZE]) -> Self {
        Self {
            msg_type: buf[0],
            flags: buf[1],
            payload_len: u16::from_le_bytes([buf[2], buf[3]]),
        }
    }

    /// Serialize header to 4 bytes.
    pub fn encode(&self, buf: &mut [u8; HEADER_SIZE]) {
        buf[0] = self.msg_type;
        buf[1] = self.flags;
        let len_bytes = self.payload_len.to_le_bytes();
        buf[2] = len_bytes[0];
        buf[3] = len_bytes[1];
    }

    /// Total message size (header + payload).
    pub fn total_len(&self) -> usize {
        HEADER_SIZE + self.payload_len as usize
    }
}

/// CONFIG payload: decoder_mode:u8, sample_rate:u32 LE
pub struct ConfigPayload {
    pub decoder_mode: u8,
    pub sample_rate: u32,
}

impl ConfigPayload {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 5 {
            return None;
        }
        Some(Self {
            decoder_mode: buf[0],
            sample_rate: u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]),
        })
    }

    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0] = self.decoder_mode;
        let sr = self.sample_rate.to_le_bytes();
        buf[1..5].copy_from_slice(&sr);
        5
    }
}

/// AUDIO_CHUNK payload: seq:u16 LE, then N*i16 LE samples
pub struct AudioChunkPayload;

impl AudioChunkPayload {
    /// Parse sequence number from chunk payload.
    pub fn parse_seq(buf: &[u8]) -> Option<u16> {
        if buf.len() < 2 {
            return None;
        }
        Some(u16::from_le_bytes([buf[0], buf[1]]))
    }

    /// Number of samples in payload (after seq field).
    pub fn num_samples(payload_len: u16) -> usize {
        if payload_len < 2 {
            return 0;
        }
        (payload_len as usize - 2) / 2
    }

    /// Parse i16 samples from payload (after seq field).
    /// Returns number of samples written to `out`.
    pub fn parse_samples(buf: &[u8], out: &mut [i16]) -> usize {
        if buf.len() < 2 {
            return 0;
        }
        let sample_bytes = &buf[2..];
        let n = core::cmp::min(sample_bytes.len() / 2, out.len());
        for i in 0..n {
            out[i] = i16::from_le_bytes([sample_bytes[i * 2], sample_bytes[i * 2 + 1]]);
        }
        n
    }
}

/// CHUNK_ACK payload: seq:u16 LE, cycles:u32 LE
pub struct ChunkAckPayload {
    pub seq: u16,
    pub cycles: u32,
}

impl ChunkAckPayload {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 6 {
            return None;
        }
        Some(Self {
            seq: u16::from_le_bytes([buf[0], buf[1]]),
            cycles: u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]),
        })
    }

    pub fn encode(&self, buf: &mut [u8]) -> usize {
        let s = self.seq.to_le_bytes();
        buf[0] = s[0];
        buf[1] = s[1];
        let c = self.cycles.to_le_bytes();
        buf[2..6].copy_from_slice(&c);
        6
    }
}

/// FRAME payload: seq:u16 LE, len:u16 LE, data:N bytes
pub struct FramePayload;

impl FramePayload {
    pub fn encode(seq: u16, frame_data: &[u8], buf: &mut [u8]) -> usize {
        let s = seq.to_le_bytes();
        buf[0] = s[0];
        buf[1] = s[1];
        let l = (frame_data.len() as u16).to_le_bytes();
        buf[2] = l[0];
        buf[3] = l[1];
        buf[4..4 + frame_data.len()].copy_from_slice(frame_data);
        4 + frame_data.len()
    }

    pub fn parse_seq(buf: &[u8]) -> Option<u16> {
        if buf.len() < 2 {
            return None;
        }
        Some(u16::from_le_bytes([buf[0], buf[1]]))
    }

    pub fn parse_len(buf: &[u8]) -> Option<u16> {
        if buf.len() < 4 {
            return None;
        }
        Some(u16::from_le_bytes([buf[2], buf[3]]))
    }

    pub fn parse_data<'a>(buf: &'a [u8]) -> Option<&'a [u8]> {
        let len = Self::parse_len(buf)? as usize;
        if buf.len() < 4 + len {
            return None;
        }
        Some(&buf[4..4 + len])
    }
}

/// STATS payload: total_frames:u32, chunks:u32, total_cycles:u64,
///                min_cycles:u32, max_cycles:u32, avg_cycles:u32
pub struct StatsPayload {
    pub total_frames: u32,
    pub chunks: u32,
    pub total_cycles: u64,
    pub min_cycles: u32,
    pub max_cycles: u32,
    pub avg_cycles: u32,
}

impl StatsPayload {
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0..4].copy_from_slice(&self.total_frames.to_le_bytes());
        buf[4..8].copy_from_slice(&self.chunks.to_le_bytes());
        buf[8..16].copy_from_slice(&self.total_cycles.to_le_bytes());
        buf[16..20].copy_from_slice(&self.min_cycles.to_le_bytes());
        buf[20..24].copy_from_slice(&self.max_cycles.to_le_bytes());
        buf[24..28].copy_from_slice(&self.avg_cycles.to_le_bytes());
        28
    }

    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 28 {
            return None;
        }
        Some(Self {
            total_frames: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            chunks: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            total_cycles: u64::from_le_bytes([
                buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
            ]),
            min_cycles: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
            max_cycles: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
            avg_cycles: u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]),
        })
    }
}

/// Helper: build a complete message into `buf`. Returns total bytes written.
pub fn build_msg(msg_type: u8, flags: u8, payload: &[u8], buf: &mut [u8]) -> usize {
    let hdr = Header {
        msg_type,
        flags,
        payload_len: payload.len() as u16,
    };
    let mut hdr_buf = [0u8; HEADER_SIZE];
    hdr.encode(&mut hdr_buf);
    buf[..HEADER_SIZE].copy_from_slice(&hdr_buf);
    buf[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
    hdr.total_len()
}
