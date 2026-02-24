mod common;

use common::budget_alloc::BudgetAlloc;
use common::fixtures::core_fixtures;
use epub_stream::EpubBook;

// Current fixtures peak around 318KiB in metadata/open paths.
// Keep a guardrail at 384KiB and tighten as allocator churn is reduced.
const METADATA_BUDGET_BYTES: usize = 384 * 1024;

#[global_allocator]
static ALLOC: BudgetAlloc = BudgetAlloc::new();

#[test]
fn metadata_under_budget_for_core_fixtures() {
    let fixtures = core_fixtures();
    assert!(
        !fixtures.is_empty(),
        "No fixtures found under tests/fixtures. Cannot run metadata budget test."
    );

    for path in fixtures {
        ALLOC.reset();
        let book = EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path, e));
        let _title = book.title();
        let _author = book.author();
        let _chapter_count = book.chapter_count();
        let _manifest_entries = book.metadata().manifest.len();

        let peak = ALLOC.peak_bytes();
        assert!(
            peak <= METADATA_BUDGET_BYTES,
            "metadata peak over budget for {}: {} bytes ({:.1}KB), budget: {}KB",
            path,
            peak,
            peak as f64 / 1024.0,
            METADATA_BUDGET_BYTES / 1024
        );
        println!(
            "metadata fixture={} peak_kib={:.1} allocs={}",
            path,
            peak as f64 / 1024.0,
            ALLOC.alloc_count()
        );
    }
}
