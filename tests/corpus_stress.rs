mod common;

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use common::fixtures::{core_fixtures, discover_optional_corpus};
use epub_stream::EpubBook;

fn discover_corpus() -> Vec<PathBuf> {
    let mut corpus: Vec<PathBuf> = core_fixtures().into_iter().map(PathBuf::from).collect();
    corpus.extend(discover_optional_corpus());
    corpus.sort();
    corpus.dedup();
    corpus
}

#[test]
fn corpus_open_and_tokenize_without_panics() {
    let corpus = discover_corpus();
    assert!(
        !corpus.is_empty(),
        "No EPUB fixtures found. Expected files under tests/fixtures or tests/datasets."
    );

    let mut failures = Vec::new();
    for path in corpus {
        let run = panic::catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let mut book = EpubBook::open(&path).map_err(|e| format!("open failed: {}", e))?;
            let _ = book.title();
            let _ = book.author();
            let chapter_count = book.chapter_count();
            if chapter_count > 0 {
                for idx in 0..chapter_count.min(6) {
                    let _ = book.tokenize_spine_item(idx);
                }
            }
            Ok(())
        }));

        match run {
            Ok(Ok(())) => {}
            Ok(Err(err)) => failures.push(format!("{} -> {}", path.display(), err)),
            Err(_) => failures.push(format!("{} -> panic", path.display())),
        }
    }

    assert!(
        failures.is_empty(),
        "Corpus stress failures ({}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
