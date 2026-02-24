mod common;

use common::budget_alloc::BudgetAlloc;
use common::fixtures::core_fixtures;
use epub_stream::EpubBook;

// Current fixtures peak around 318KiB in tokenize paths.
// Keep a guardrail at 512KiB and tighten after allocator optimizations land.
const TOKENIZE_BUDGET_BYTES: usize = 512 * 1024;

#[global_allocator]
static ALLOC: BudgetAlloc = BudgetAlloc::new();

fn choose_tokenized_chapter(path: &str) -> usize {
    let mut book = EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path, e));
    let chapter_count = book.chapter_count();
    if chapter_count == 0 {
        return 0;
    }
    for idx in 0..chapter_count.min(12) {
        if let Ok(tokens) = book.tokenize_spine_item(idx) {
            if !tokens.is_empty() {
                return idx;
            }
        }
    }
    0
}

#[test]
fn tokenize_chapter_under_budget_for_core_fixtures() {
    let fixtures = core_fixtures();
    assert!(
        !fixtures.is_empty(),
        "No fixtures found under tests/fixtures. Cannot run tokenize budget test."
    );

    for path in fixtures {
        let chapter_index = choose_tokenized_chapter(path);

        ALLOC.reset();
        let mut book = EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path, e));
        let tokens = book
            .tokenize_spine_item(chapter_index)
            .unwrap_or_else(|e| panic!("tokenize {} chapter {}: {}", path, chapter_index, e));
        assert!(
            !tokens.is_empty(),
            "fixture {} chapter {} produced no tokens",
            path,
            chapter_index
        );

        let peak = ALLOC.peak_bytes();
        assert!(
            peak <= TOKENIZE_BUDGET_BYTES,
            "tokenize peak over budget for {} chapter {}: {} bytes ({:.1}KB), budget: {}KB",
            path,
            chapter_index,
            peak,
            peak as f64 / 1024.0,
            TOKENIZE_BUDGET_BYTES / 1024
        );
        println!(
            "tokenize fixture={} chapter={} peak_kib={:.1} allocs={}",
            path,
            chapter_index,
            peak as f64 / 1024.0,
            ALLOC.alloc_count()
        );
    }
}
