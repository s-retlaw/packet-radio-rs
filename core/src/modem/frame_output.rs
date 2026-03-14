//! Shared frame output buffer for multi-decoder modules.
//!
//! Provides a fixed-size buffer of decoded AX.25 frames, used by
//! `MultiDecoder`, `MiniDecoder`, `CorrSlicerDecoder`, `Multi9600Decoder`,
//! and their variants to collect unique frames from parallel decoders.
//!
//! `no_std` compatible — no heap allocation.

/// Maximum AX.25 frame size (bytes). Matches the AX.25 spec limit.
const MAX_FRAME_SIZE: usize = 330;

/// A decoded frame with its content stored in a fixed-size buffer.
pub struct DecodedFrame {
    pub data: [u8; MAX_FRAME_SIZE],
    pub len: usize,
    /// Quality metric: 0 = hard decode, 1 = syndrome, higher = soft recovery cost.
    pub cost: u16,
}

/// Fixed-size output buffer holding up to `N` decoded frames.
///
/// Frames are appended with [`push`](Self::push) and accessed by index
/// with [`frame`](Self::frame). When full, additional frames are silently
/// dropped (the caller should check [`is_full`](Self::is_full) if needed).
pub struct FrameOutputBuffer<const N: usize> {
    frames: [DecodedFrame; N],
    pub count: usize,
}

impl<const N: usize> Default for FrameOutputBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FrameOutputBuffer<N> {
    /// Create a new empty output buffer.
    pub fn new() -> Self {
        Self {
            frames: core::array::from_fn(|_| DecodedFrame {
                data: [0u8; MAX_FRAME_SIZE],
                len: 0,
                cost: 0,
            }),
            count: 0,
        }
    }

    /// Number of frames in the buffer.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the buffer contains no frames.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether the buffer is at capacity.
    pub fn is_full(&self) -> bool {
        self.count >= N
    }

    /// Get a decoded frame's data by index.
    pub fn frame(&self, index: usize) -> &[u8] {
        &self.frames[index].data[..self.frames[index].len]
    }

    /// Append a frame to the buffer. Returns the slot index, or `None` if full.
    pub fn push(&mut self, data: &[u8]) -> Option<u8> {
        self.push_with_cost(data, 0)
    }

    /// Append a frame with its decode cost. Returns the slot index, or `None` if full.
    pub fn push_with_cost(&mut self, data: &[u8], cost: u16) -> Option<u8> {
        if self.count >= N {
            return None;
        }
        let slot = self.count as u8;
        let len = data.len().min(MAX_FRAME_SIZE);
        self.frames[self.count].data[..len].copy_from_slice(&data[..len]);
        self.frames[self.count].len = len;
        self.frames[self.count].cost = cost;
        self.count += 1;
        Some(slot)
    }

    /// Replace a frame at the given slot with better data. Returns `true` on success.
    pub fn replace(&mut self, slot: u8, data: &[u8], cost: u16) -> bool {
        let idx = slot as usize;
        if idx >= self.count {
            return false;
        }
        let len = data.len().min(MAX_FRAME_SIZE);
        self.frames[idx].data[..len].copy_from_slice(&data[..len]);
        self.frames[idx].len = len;
        self.frames[idx].cost = cost;
        true
    }
}
