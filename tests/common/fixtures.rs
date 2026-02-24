use std::path::{Path, PathBuf};

pub const FUNDAMENTAL_FIXTURE: &str =
    "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub";

pub const BENCH_FIXTURES: &[&str] = &[
    "tests/fixtures/bench/pg84-frankenstein.epub",
    "tests/fixtures/bench/pg1342-pride-and-prejudice.epub",
    "tests/fixtures/bench/pg1661-sherlock-holmes.epub",
    "tests/fixtures/bench/pg2701-moby-dick.epub",
];

pub fn core_fixtures() -> Vec<&'static str> {
    let mut out = Vec::with_capacity(BENCH_FIXTURES.len() + 1);
    if Path::new(FUNDAMENTAL_FIXTURE).exists() {
        out.push(FUNDAMENTAL_FIXTURE);
    }
    for path in BENCH_FIXTURES {
        if Path::new(path).exists() {
            out.push(path);
        }
    }
    out
}

pub fn discover_optional_corpus() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in ["tests/fixtures/corpus", "tests/datasets/wild/gutenberg"] {
        let root_path = Path::new(root);
        let Ok(entries) = std::fs::read_dir(root_path) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}
