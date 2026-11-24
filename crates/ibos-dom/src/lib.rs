//! Page distillation.
//!
//! Input: the browser's accessibility tree and layout rectangles, as raw JSON (parsed by borrowing, not copying).
//! Output: an [`ibos_ctx::Snapshot`] with only what a model needs to act: interactive elements, headings and visible text,
//! repeated structures collapsed, text capped, offscreen content counted instead of listed.
//!
//! Element handles are the browser's backend node ids: stable across steps and identical to what actions use, so no mapping
//! table is needed.

mod distill;
mod parse;
mod roles;

pub use distill::{DistillError, DistillOptions, Distilled, Stats, distill};
