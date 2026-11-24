//! Memory regression guard: heap use of one distillation, measured with dhat in its own test binary.
//! Peak heap is compared with the size of the input, so the bound is meaningful on any machine.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use ibos_dom::{DistillOptions, distill};
use ibos_mem::StepArena;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn read(name: &str, ext: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}.{ext}.json", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn distilling_a_large_page_stays_within_a_small_multiple_of_its_input() {
    let _profiler = dhat::Profiler::builder().testing().build();
    let (ax, dom) = (read("products", "ax"), read("products", "dom"));
    let input = ax.len() + dom.len();
    let before = dhat::HeapStats::get();

    let mut arena = StepArena::with_capacity(64 * 1024);
    let mut kept = 0;
    let mut live_after = Vec::new();
    for _ in 0..30 {
        let d = distill("u", "t", &ax, Some(&dom), (1280.0, 800.0), &DistillOptions::default(), &arena).unwrap();
        kept = d.stats.kept;
        drop(d);
        arena.reset(); // freed in one operation; its largest chunk is kept for the next step
        live_after.push(dhat::HeapStats::get().curr_bytes);
    }
    let after = dhat::HeapStats::get();
    let growth = after.max_bytes.saturating_sub(before.curr_bytes);
    let retained = after.curr_bytes.saturating_sub(before.curr_bytes);
    println!(
        "input {input} B; peak growth {growth} B ({:.2}x input); retained after 30 steps {retained} B (arena capacity {} B); kept {kept}",
        growth as f64 / input as f64,
        arena.allocated_bytes()
    );

    assert!(growth < input * 2, "peak heap growth {growth} B exceeds 2x the {input} B input");
    // no leak: steady state is flat from step 3 to step 30
    let (early, late) = (live_after[2], live_after[29]);
    assert!(late <= early + 4 * 1024, "live heap grew from {early} B to {late} B over 27 further steps");
    // what stays allocated is the arena's reusable chunk, not accumulated results
    assert!(
        retained <= arena.allocated_bytes() + 32 * 1024,
        "retained {retained} B is more than the arena's {} B capacity",
        arena.allocated_bytes()
    );
    assert!(kept > 0);
}
