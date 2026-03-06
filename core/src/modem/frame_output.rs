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

    /// Append a frame to the buffer. Returns `true` if added, `false` if full.
    pub fn push(&mut self, data: &[u8]) -> bool {
        if self.count >= N {
            return false;
        }
        let len = data.len().min(MAX_FRAME_SIZE);
        self.frames[self.count].data[..len].copy_from_slice(&data[..len]);
        self.frames[self.count].len = len;
        self.count += 1;
        true
    }
}
