use std::collections::VecDeque;
use std::sync::Arc;

use crate::budget::{MemoryBudget, Reservation};

/// A screenshot kept **encoded** (JPEG/WebP/PNG bytes) plus a 64-bit perceptual hash.
///
/// A raw 1920x1080 RGBA frame is 8,294,400 bytes; the encoded form is decoded only when sent to a model.
#[derive(Debug, Clone)]
pub struct Frame {
    pub hash: u64,
    pub encoded: Arc<[u8]>,
}

/// Result of [`FrameRing::push`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// Stored (possibly evicting older frames).
    Stored,
    /// Perceptually identical to the newest frame: not stored, and callers should not send it again.
    Duplicate,
    /// The memory budget refused the frame even after evicting everything older.
    OverBudget,
}

/// Hamming distance between two perceptual hashes.
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 64-bit difference hash of a grayscale image (`width * height` bytes, row-major).
///
/// The image is box-downsampled to 9x8 and each pixel is compared with its right neighbour. Robust to rescaling and
/// to small colour changes; cheap enough to run on every step.
///
/// # Panics
/// If the buffer length is not `width * height`, or either dimension is smaller than 9x8.
pub fn dhash_gray(pixels: &[u8], width: usize, height: usize) -> u64 {
    assert_eq!(pixels.len(), width * height, "buffer size mismatch");
    assert!(width >= 9 && height >= 8, "image too small to hash");
    let mut cell = [[0u32; 9]; 8];
    for (row, cells) in cell.iter_mut().enumerate() {
        let (y0, y1) = (row * height / 8, ((row + 1) * height / 8).max(row * height / 8 + 1));
        for (col, out) in cells.iter_mut().enumerate() {
            let (x0, x1) = (col * width / 9, ((col + 1) * width / 9).max(col * width / 9 + 1));
            let mut sum = 0u64;
            let mut n = 0u64;
            for y in y0..y1 {
                for &p in &pixels[y * width + x0..y * width + x1] {
                    sum += u64::from(p);
                    n += 1;
                }
            }
            *out = (sum / n.max(1)) as u32;
        }
    }
    let mut hash = 0u64;
    for (row, cells) in cell.iter().enumerate() {
        for col in 0..8 {
            if cells[col] > cells[col + 1] {
                hash |= 1 << (row * 8 + col);
            }
        }
    }
    hash
}

#[derive(Debug)]
struct Slot {
    frame: Frame,
    _reservation: Option<Reservation>,
}

/// Bounded ring of encoded frames with near-duplicate suppression and optional budget accounting.
#[derive(Debug)]
pub struct FrameRing {
    capacity: usize,
    threshold: u32,
    budget: Option<MemoryBudget>,
    slots: VecDeque<Slot>,
}

impl FrameRing {
    /// `capacity` is clamped to at least 1. Frames whose hash differs from the newest by at most `threshold` bits are
    /// treated as duplicates.
    pub fn new(capacity: usize, threshold: u32) -> Self {
        Self { capacity: capacity.max(1), threshold, budget: None, slots: VecDeque::new() }
    }

    /// Charge stored frames against `budget`; the oldest frames are evicted to make room.
    #[must_use]
    pub fn with_budget(mut self, budget: MemoryBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    pub fn push(&mut self, hash: u64, encoded: impl Into<Arc<[u8]>>) -> PushOutcome {
        let encoded = encoded.into();
        if self.slots.back().is_some_and(|s| hamming(s.frame.hash, hash) <= self.threshold) {
            return PushOutcome::Duplicate;
        }
        while self.slots.len() >= self.capacity {
            self.slots.pop_front();
        }
        let reservation = match &self.budget {
            None => None,
            Some(b) => loop {
                if let Some(r) = b.reserve(encoded.len()) {
                    break Some(r);
                }
                if self.slots.pop_front().is_none() {
                    return PushOutcome::OverBudget;
                }
            },
        };
        self.slots.push_back(Slot { frame: Frame { hash, encoded }, _reservation: reservation });
        PushOutcome::Stored
    }

    /// Shrink to at most `keep` newest frames (used when memory pressure turns amber).
    pub fn shrink_to(&mut self, keep: usize) {
        while self.slots.len() > keep {
            self.slots.pop_front();
        }
    }

    pub fn latest(&self) -> Option<&Frame> {
        self.slots.back().map(|s| &s.frame)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Encoded bytes currently held.
    pub fn bytes(&self) -> usize {
        self.slots.iter().map(|s| s.frame.encoded.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect()
    }

    #[test]
    fn dhash_is_stable_under_small_noise_and_differs_for_different_images() {
        let a = gradient(90, 80, |x, _| (x * 2) as u8);
        let noisy = gradient(90, 80, |x, y| ((x * 2) as u8).saturating_add((y % 2) as u8));
        let other = gradient(90, 80, |x, _| 255 - (x * 2) as u8);
        assert!(hamming(dhash_gray(&a, 90, 80), dhash_gray(&noisy, 90, 80)) <= 4);
        assert!(hamming(dhash_gray(&a, 90, 80), dhash_gray(&other, 90, 80)) > 20);
    }

    #[test]
    fn dhash_handles_exact_minimum_size_and_rescaling() {
        let small = gradient(9, 8, |x, _| (x * 20) as u8);
        let big = gradient(900, 800, |x, _| ((x / 100) * 20) as u8);
        assert_eq!(dhash_gray(&small, 9, 8), dhash_gray(&big, 900, 800));
    }

    #[test]
    #[should_panic(expected = "too small")]
    fn dhash_rejects_tiny_images() {
        dhash_gray(&[0; 4], 2, 2);
    }

    #[test]
    fn ring_skips_near_duplicates_and_bounds_memory() {
        let mut r = FrameRing::new(2, 4);
        assert_eq!(r.push(0, vec![0; 10]), PushOutcome::Stored);
        assert_eq!(r.push(0b111, vec![0; 10]), PushOutcome::Duplicate);
        assert_eq!(r.push(u64::MAX, vec![0; 10]), PushOutcome::Stored);
        assert_eq!(r.push(0xF0F0_F0F0_F0F0_F0F0, vec![0; 10]), PushOutcome::Stored);
        assert_eq!((r.len(), r.bytes()), (2, 20));
        r.shrink_to(1);
        assert_eq!(r.len(), 1);
        assert_eq!(r.latest().unwrap().hash, 0xF0F0_F0F0_F0F0_F0F0);
    }

    #[test]
    fn budget_is_charged_and_oldest_evicted_to_fit() {
        let budget = MemoryBudget::new(100, 100);
        let mut r = FrameRing::new(10, 0).with_budget(budget.clone());
        assert_eq!(r.push(1, vec![0; 60]), PushOutcome::Stored);
        assert_eq!(r.push(u64::MAX, vec![0; 60]), PushOutcome::Stored, "evicts the first to fit the second");
        assert_eq!((r.len(), budget.used()), (1, 60));
        assert_eq!(r.push(0x5555_5555_5555_5555, vec![0; 200]), PushOutcome::OverBudget);
        drop(r);
        assert_eq!(budget.used(), 0, "dropping the ring returns every byte");
    }
}
