use bumpalo::Bump;

/// Per-step scratch arena. Everything transient while handling one step (adjacency lists, temporary strings) goes here
/// and is released in one operation by [`reset`](Self::reset), instead of being freed allocation by allocation.
#[derive(Debug, Default)]
pub struct StepArena {
    bump: Bump,
}

impl StepArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(bytes: usize) -> Self {
        Self { bump: Bump::with_capacity(bytes) }
    }

    /// The underlying allocator, for `bumpalo::collections`.
    pub fn bump(&self) -> &Bump {
        &self.bump
    }

    pub fn alloc_str(&self, s: &str) -> &str {
        self.bump.alloc_str(s)
    }

    /// Bytes handed out since the last reset (including slack inside chunks).
    pub fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes()
    }

    /// Release everything at once, keeping the largest chunk for reuse.
    pub fn reset(&mut self) {
        self.bump.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_reuses_capacity() {
        let mut a = StepArena::with_capacity(4096);
        for _ in 0..3 {
            for i in 0..100 {
                let s = a.alloc_str(&format!("element-{i}"));
                assert!(s.starts_with("element-"));
            }
            let before = a.allocated_bytes();
            a.reset();
            assert!(a.allocated_bytes() <= before);
        }
    }
}
