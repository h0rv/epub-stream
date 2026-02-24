mod common;

use std::panic::{self, AssertUnwindSafe};

use common::budget_alloc::BudgetAlloc;
use epub_stream::EpubBook;

const FIXTURE: &str = "tests/fixtures/bench/pg84-frankenstein.epub";

#[global_allocator]
static ALLOC: BudgetAlloc = BudgetAlloc::new();

fn fragment_heap() -> Vec<Box<[u8]>> {
    let mut fragments: Vec<Option<Box<[u8]>>> = Vec::with_capacity(240);
    for idx in 0..240usize {
        let size = match idx % 6 {
            0 => 64,
            1 => 192,
            2 => 512,
            3 => 1024,
            4 => 1536,
            _ => 2048,
        };
        fragments.push(Some(vec![0xA5; size].into_boxed_slice()));
    }
    for (idx, slot) in fragments.iter_mut().enumerate() {
        if idx % 2 == 1 || idx % 7 == 0 {
            *slot = None;
        }
    }
    fragments.into_iter().flatten().collect()
}

#[test]
fn epub_open_and_tokenize_survive_fragmented_heap() {
    let kept_fragments = fragment_heap();
    ALLOC.reset();

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut book = EpubBook::open(FIXTURE).expect("open fixture");
        let _ = book.title().to_string();
        let chapter_count = book.chapter_count();
        if chapter_count > 0 {
            let chapter_idx = if chapter_count > 1 { 1 } else { 0 };
            let _tokens = book
                .tokenize_spine_item(chapter_idx)
                .expect("tokenize chapter on fragmented heap");
        }
    }));

    drop(kept_fragments);

    assert!(
        result.is_ok(),
        "EPUB open/tokenize panicked under fragmented allocator pressure"
    );
    let peak = ALLOC.peak_bytes();
    assert!(
        peak <= 160 * 1024,
        "fragmented open/tokenize peak too high: {} bytes ({:.1}KB)",
        peak,
        peak as f64 / 1024.0
    );
}
