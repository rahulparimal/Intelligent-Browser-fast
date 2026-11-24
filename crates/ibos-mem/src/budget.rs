use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How loaded the process is relative to its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pressure {
    /// Normal operation.
    Green,
    /// Past the soft limit: shed optional load (caches, extra frames, parallelism).
    Amber,
    /// At the hard limit: refuse work that needs large buffers.
    Red,
}

#[derive(Debug)]
struct Inner {
    used: AtomicUsize,
    soft: usize,
    hard: usize,
}

/// Process-wide memory accounting.
///
/// Subsystems [`reserve`](Self::reserve) before they allocate large buffers. A reservation is released on drop, on every
/// path including panics. The hard limit is never exceeded: when a reservation would pass it, `reserve` returns `None`
/// and the caller must degrade instead of allocating anyway.
#[derive(Debug, Clone)]
pub struct MemoryBudget(Arc<Inner>);

/// RAII handle for reserved bytes. Dropping it returns the bytes to the budget.
#[derive(Debug)]
pub struct Reservation {
    budget: MemoryBudget,
    bytes: usize,
}

impl Reservation {
    /// Number of bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

impl MemoryBudget {
    /// # Panics
    /// If `soft > hard`.
    pub fn new(soft: usize, hard: usize) -> Self {
        assert!(soft <= hard, "soft limit must not exceed hard limit");
        Self(Arc::new(Inner { used: AtomicUsize::new(0), soft, hard }))
    }

    /// Bytes currently reserved.
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }

    pub fn soft_limit(&self) -> usize {
        self.0.soft
    }

    pub fn hard_limit(&self) -> usize {
        self.0.hard
    }

    pub fn pressure(&self) -> Pressure {
        let used = self.used();
        if used >= self.0.hard {
            Pressure::Red
        } else if used >= self.0.soft {
            Pressure::Amber
        } else {
            Pressure::Green
        }
    }

    /// Reserve `bytes`, or `None` if that would exceed the hard limit (also on arithmetic overflow).
    pub fn reserve(&self, bytes: usize) -> Option<Reservation> {
        let mut cur = self.0.used.load(Ordering::Acquire);
        loop {
            let next = cur.checked_add(bytes).filter(|n| *n <= self.0.hard)?;
            match self.0.used.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Reservation { budget: self.clone(), bytes }),
                Err(actual) => cur = actual,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reservation_is_released_and_hard_limit_enforced() {
        let b = MemoryBudget::new(50, 100);
        let r1 = b.reserve(60).unwrap();
        assert_eq!(b.pressure(), Pressure::Amber);
        assert!(b.reserve(41).is_none());
        let r2 = b.reserve(40).unwrap();
        assert_eq!(b.pressure(), Pressure::Red);
        drop(r2);
        drop(r1);
        assert_eq!((b.used(), b.pressure()), (0, Pressure::Green));
        assert!(b.reserve(usize::MAX).is_none(), "overflow must be refused, not wrapped");
    }

    #[test]
    fn released_on_panic() {
        let b = MemoryBudget::new(10, 10);
        let b2 = b.clone();
        let _ = std::panic::catch_unwind(move || {
            let _held = b2.reserve(10).unwrap();
            panic!("boom");
        });
        assert_eq!(b.used(), 0);
    }

    #[test]
    fn threadsafe_and_never_over_limit() {
        let b = MemoryBudget::new(900, 1000);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let b = b.clone();
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        if let Some(_held) = b.reserve(100) {
                            assert!(b.used() <= 1000);
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(b.used(), 0);
    }

    proptest! {
        #[test]
        fn accounting_always_returns_to_zero(sizes in proptest::collection::vec(0usize..400, 0..60)) {
            let b = MemoryBudget::new(300, 500);
            let mut held = Vec::new();
            for s in sizes {
                if let Some(r) = b.reserve(s) { held.push(r); }
                prop_assert!(b.used() <= 500);
                prop_assert_eq!(b.used(), held.iter().map(Reservation::bytes).sum::<usize>());
            }
            drop(held);
            prop_assert_eq!(b.used(), 0);
        }
    }
}
