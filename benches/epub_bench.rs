use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use epub_stream::EpubBook;
use epub_stream_embedded_graphics::with_embedded_text_measurer;
use epub_stream_render::{RenderConfig, RenderEngine, RenderEngineOptions};

const DISPLAY_WIDTH: i32 = 480;
const DISPLAY_HEIGHT: i32 = 800;

const FIXTURES: &[(&str, &str)] = &[
    (
        "fundamental-a11y",
        "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
    ),
    (
        "pg84-frankenstein",
        "tests/fixtures/bench/pg84-frankenstein.epub",
    ),
    (
        "pg1342-pride-and-prejudice",
        "tests/fixtures/bench/pg1342-pride-and-prejudice.epub",
    ),
    (
        "pg1661-sherlock-holmes",
        "tests/fixtures/bench/pg1661-sherlock-holmes.epub",
    ),
    (
        "pg2701-moby-dick",
        "tests/fixtures/bench/pg2701-moby-dick.epub",
    ),
];

struct TrackingAllocator;

static CURRENT_ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL_ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn current_alloc_bytes() -> usize {
    CURRENT_ALLOC_BYTES.load(Ordering::Relaxed)
}

fn peak_alloc_bytes() -> usize {
    PEAK_ALLOC_BYTES.load(Ordering::Relaxed)
}

fn reset_peak_alloc_bytes() {
    let current = current_alloc_bytes();
    PEAK_ALLOC_BYTES.store(current, Ordering::Relaxed);
}

fn update_peak_alloc_bytes(current: usize) {
    let mut peak = PEAK_ALLOC_BYTES.load(Ordering::Relaxed);
    while current > peak {
        match PEAK_ALLOC_BYTES.compare_exchange_weak(
            peak,
            current,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(next) => peak = next,
        }
    }
}

fn add_current_alloc_bytes(delta: usize) {
    let current = CURRENT_ALLOC_BYTES.fetch_add(delta, Ordering::Relaxed) + delta;
    update_peak_alloc_bytes(current);
}

fn sub_current_alloc_bytes(delta: usize) {
    let mut current = CURRENT_ALLOC_BYTES.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_sub(delta);
        match CURRENT_ALLOC_BYTES.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            add_current_alloc_bytes(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        sub_current_alloc_bytes(layout.size());
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            add_current_alloc_bytes(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                add_current_alloc_bytes(new_size - layout.size());
            } else {
                sub_current_alloc_bytes(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

#[derive(Clone, Debug)]
struct CaseResult {
    fixture: String,
    case: String,
    iterations: usize,
    min_ns: u128,
    median_ns: u128,
    mean_ns: u128,
    max_ns: u128,
    min_peak_heap_bytes: usize,
    median_peak_heap_bytes: usize,
    mean_peak_heap_bytes: usize,
    max_peak_heap_bytes: usize,
}

fn percentile_u128(sorted: &[u128], percentile: f64) -> u128 {
    let idx = ((sorted.len().saturating_sub(1) as f64) * percentile).round() as usize;
    sorted[idx]
}

fn percentile_usize(sorted: &[usize], percentile: f64) -> usize {
    let idx = ((sorted.len().saturating_sub(1) as f64) * percentile).round() as usize;
    sorted[idx]
}

fn pick_text_chapter(path: &str) -> usize {
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

fn run_case<F>(
    fixture: &str,
    case: &str,
    warmup_iters: usize,
    measure_iters: usize,
    mut op: F,
) -> CaseResult
where
    F: FnMut() -> usize,
{
    for _ in 0..warmup_iters {
        black_box(op());
    }

    let mut time_samples = Vec::with_capacity(measure_iters);
    let mut mem_samples = Vec::with_capacity(measure_iters);
    for _ in 0..measure_iters {
        let baseline_alloc = current_alloc_bytes();
        reset_peak_alloc_bytes();
        let start = Instant::now();
        black_box(op());
        time_samples.push(start.elapsed().as_nanos());
        let peak_extra = peak_alloc_bytes().saturating_sub(baseline_alloc);
        mem_samples.push(peak_extra);
    }

    time_samples.sort_unstable();
    mem_samples.sort_unstable();

    let time_sum: u128 = time_samples.iter().copied().sum();
    let mem_sum: usize = mem_samples.iter().copied().sum();

    CaseResult {
        fixture: fixture.to_string(),
        case: case.to_string(),
        iterations: measure_iters,
        min_ns: time_samples[0],
        median_ns: percentile_u128(&time_samples, 0.5),
        mean_ns: time_sum / time_samples.len() as u128,
        max_ns: time_samples[time_samples.len() - 1],
        min_peak_heap_bytes: mem_samples[0],
        median_peak_heap_bytes: percentile_usize(&mem_samples, 0.5),
        mean_peak_heap_bytes: mem_sum / mem_samples.len(),
        max_peak_heap_bytes: mem_samples[mem_samples.len() - 1],
    }
}

fn main() {
    let quick = std::env::args().any(|arg| arg == "--quick");
    let warmup_iters = if quick { 1 } else { 2 };
    let measure_iters = if quick { 3 } else { 10 };

    println!("# epub-stream benchmark");
    println!(
        "# mode={} warmup_iters={} measure_iters={}",
        if quick { "quick" } else { "full" },
        warmup_iters,
        measure_iters
    );
    println!(
        "fixture,case,iterations,min_ns,median_ns,mean_ns,max_ns,min_peak_heap_bytes,median_peak_heap_bytes,mean_peak_heap_bytes,max_peak_heap_bytes"
    );

    let mut results = Vec::new();
    for (fixture_key, fixture_path) in FIXTURES {
        let text_chapter = pick_text_chapter(fixture_path);

        results.push(run_case(
            fixture_key,
            "open_epub",
            warmup_iters,
            measure_iters,
            || {
                let book =
                    EpubBook::open(fixture_path).unwrap_or_else(|e| panic!("open failed: {}", e));
                book.chapter_count()
            },
        ));

        results.push(run_case(
            fixture_key,
            "parse_metadata",
            warmup_iters,
            measure_iters,
            || {
                let book =
                    EpubBook::open(fixture_path).unwrap_or_else(|e| panic!("open failed: {}", e));
                black_box(book.title());
                black_box(book.author());
                book.chapter_count()
            },
        ));

        results.push(run_case(
            fixture_key,
            "tokenize_text_chapter",
            warmup_iters,
            measure_iters,
            || {
                let mut book =
                    EpubBook::open(fixture_path).unwrap_or_else(|e| panic!("open failed: {}", e));
                let tokens = book
                    .tokenize_spine_item(text_chapter)
                    .unwrap_or_else(|e| panic!("tokenize failed: {}", e));
                tokens.len()
            },
        ));

        results.push(run_case(
            fixture_key,
            "render_text_chapter",
            warmup_iters,
            measure_iters,
            || {
                let mut book =
                    EpubBook::open(fixture_path).unwrap_or_else(|e| panic!("open failed: {}", e));
                let engine = RenderEngine::new(RenderEngineOptions::for_display(
                    DISPLAY_WIDTH,
                    DISPLAY_HEIGHT,
                ));
                let config = with_embedded_text_measurer(RenderConfig::default());
                let pages = engine
                    .prepare_chapter_with_config_collect(&mut book, text_chapter, config)
                    .unwrap_or_else(|e| panic!("render failed: {}", e));
                pages.len()
            },
        ));

        results.push(run_case(
            fixture_key,
            "full_open_to_first_page",
            warmup_iters,
            measure_iters,
            || {
                let mut book =
                    EpubBook::open(fixture_path).unwrap_or_else(|e| panic!("open failed: {}", e));
                let engine = RenderEngine::new(RenderEngineOptions::for_display(
                    DISPLAY_WIDTH,
                    DISPLAY_HEIGHT,
                ));
                let config = with_embedded_text_measurer(RenderConfig::default());
                let pages = engine
                    .prepare_chapter_with_config_collect(&mut book, text_chapter, config)
                    .unwrap_or_else(|e| panic!("render failed: {}", e));
                pages.first().map(|page| page.commands.len()).unwrap_or(0)
            },
        ));
    }

    for result in &results {
        println!(
            "{},{},{},{},{},{},{},{},{},{},{}",
            result.fixture,
            result.case,
            result.iterations,
            result.min_ns,
            result.median_ns,
            result.mean_ns,
            result.max_ns,
            result.min_peak_heap_bytes,
            result.median_peak_heap_bytes,
            result.mean_peak_heap_bytes,
            result.max_peak_heap_bytes
        );
    }
}
