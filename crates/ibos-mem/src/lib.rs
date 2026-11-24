//! Memory budgeting and bounded buffers.
//!
//! The goal is *predictable* memory: every large allocation has an owner, a bound and a way to shed load.
//!
//! * [`MemoryBudget`] - process-wide accounting with soft and hard limits and RAII [`Reservation`]s.
//! * [`FrameRing`] - bounded ring of *encoded* screenshots with perceptual-hash de-duplication.
//! * [`StepArena`] - per-step scratch arena freed in one operation.
//! * [`Interner`] - string interning for tag and attribute names.
//! * [`ProcessMemory`] - resident-size sampling for a process tree (used by the browser supervisor).

mod arena;
mod budget;
mod frames;
mod intern;
mod proc;

pub use arena::StepArena;
pub use budget::{MemoryBudget, Pressure, Reservation};
pub use frames::{Frame, FrameRing, PushOutcome, dhash_gray, hamming};
pub use intern::{Interner, Sym};
pub use proc::ProcessMemory;
