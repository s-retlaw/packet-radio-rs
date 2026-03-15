//! `FixedVec<T, N>` — a Vec-like container that uses `Vec<T>` on alloc builds
//! and a fixed `[MaybeUninit<T>; N]` array on no_alloc builds.
//!
//! This eliminates cfg-gated Vec/array field duplication throughout the codebase.

#[cfg(not(feature = "alloc"))]
use core::mem::MaybeUninit;
use core::ops::{Index, IndexMut};

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// A fixed-capacity vector: `Vec<T>` on alloc, `[MaybeUninit<T>; N]` on no_alloc.
pub struct FixedVec<T, const N: usize> {
    #[cfg(feature = "alloc")]
    inner: Vec<T>,
    #[cfg(not(feature = "alloc"))]
    inner: [MaybeUninit<T>; N],
    #[cfg(not(feature = "alloc"))]
    len: usize,
}

impl<T, const N: usize> Default for FixedVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> FixedVec<T, N> {
    /// Create a new empty `FixedVec`.
    pub fn new() -> Self {
        #[cfg(feature = "alloc")]
        {
            Self {
                inner: Vec::with_capacity(N),
            }
        }
        #[cfg(not(feature = "alloc"))]
        {
            Self {
                // SAFETY: MaybeUninit<T> does not require initialization.
                inner: unsafe { MaybeUninit::<[MaybeUninit<T>; N]>::uninit().assume_init() },
                len: 0,
            }
        }
    }

    /// Push an item. Panics on no_alloc if capacity `N` is exceeded.
    pub fn push(&mut self, item: T) {
        #[cfg(feature = "alloc")]
        {
            self.inner.push(item);
        }
        #[cfg(not(feature = "alloc"))]
        {
            assert!(self.len < N, "FixedVec overflow: capacity {N}");
            self.inner[self.len] = MaybeUninit::new(item);
            self.len += 1;
        }
    }

    /// Number of initialized elements.
    pub fn len(&self) -> usize {
        #[cfg(feature = "alloc")]
        {
            self.inner.len()
        }
        #[cfg(not(feature = "alloc"))]
        {
            self.len
        }
    }

    /// Whether the container is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterate over initialized elements.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        #[cfg(feature = "alloc")]
        {
            self.inner.iter()
        }
        #[cfg(not(feature = "alloc"))]
        {
            self.inner[..self.len]
                .iter()
                .map(|slot| unsafe { slot.assume_init_ref() })
        }
    }

    /// Iterate mutably over initialized elements.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        #[cfg(feature = "alloc")]
        {
            self.inner.iter_mut()
        }
        #[cfg(not(feature = "alloc"))]
        {
            self.inner[..self.len]
                .iter_mut()
                .map(|slot| unsafe { slot.assume_init_mut() })
        }
    }
}

impl<T, const N: usize> Index<usize> for FixedVec<T, N> {
    type Output = T;
    fn index(&self, idx: usize) -> &T {
        #[cfg(feature = "alloc")]
        {
            &self.inner[idx]
        }
        #[cfg(not(feature = "alloc"))]
        {
            assert!(
                idx < self.len,
                "FixedVec index {idx} out of bounds (len {})",
                self.len
            );
            unsafe { self.inner[idx].assume_init_ref() }
        }
    }
}

impl<T, const N: usize> IndexMut<usize> for FixedVec<T, N> {
    fn index_mut(&mut self, idx: usize) -> &mut T {
        #[cfg(feature = "alloc")]
        {
            &mut self.inner[idx]
        }
        #[cfg(not(feature = "alloc"))]
        {
            assert!(
                idx < self.len,
                "FixedVec index {idx} out of bounds (len {})",
                self.len
            );
            unsafe { self.inner[idx].assume_init_mut() }
        }
    }
}

#[cfg(not(feature = "alloc"))]
impl<T, const N: usize> Drop for FixedVec<T, N> {
    fn drop(&mut self) {
        for i in 0..self.len {
            unsafe { self.inner[i].assume_init_drop() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_index() {
        let mut v = FixedVec::<i32, 4>::new();
        v.push(10);
        v.push(20);
        v.push(30);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 10);
        assert_eq!(v[1], 20);
        assert_eq!(v[2], 30);
    }

    #[test]
    fn test_iter() {
        let mut v = FixedVec::<i32, 4>::new();
        v.push(1);
        v.push(2);
        v.push(3);
        let sum: i32 = v.iter().sum();
        assert_eq!(sum, 6);
    }

    #[test]
    fn test_iter_mut() {
        let mut v = FixedVec::<i32, 4>::new();
        v.push(1);
        v.push(2);
        for x in v.iter_mut() {
            *x *= 10;
        }
        assert_eq!(v[0], 10);
        assert_eq!(v[1], 20);
    }

    #[test]
    fn test_empty() {
        let v = FixedVec::<i32, 4>::new();
        assert!(v.is_empty());
        assert_eq!(v.len(), 0);
    }
}
