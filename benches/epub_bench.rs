use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use epub::doc::EpubDoc;
use epub_parser::Epub as EpubParser;
use epub_stream::metadata::{parse_container_xml, parse_opf};
use epub_stream::spine::parse_spine;
use epub_stream::tokenizer::tokenize_html;
use epub_stream::zip::StreamingZip;
use epub_stream::EpubBook;

#[derive(Clone, Copy)]
struct Fixture {
    key: &'static str,
    filename: &'static str,
    bytes: &'static [u8],
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        key: "fundamental-a11y",
        filename: "Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        bytes: include_bytes!(
            "../tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub"
        ),
    },
    Fixture {
        key: "pg84-frankenstein",
        filename: "pg84-frankenstein.epub",
        bytes: include_bytes!("../tests/fixtures/bench/pg84-frankenstein.epub"),
    },
    Fixture {
        key: "pg1661-sherlock-holmes",
        filename: "pg1661-sherlock-holmes.epub",
        bytes: include_bytes!("../tests/fixtures/bench/pg1661-sherlock-holmes.epub"),
    },
    Fixture {
        key: "pg2701-moby-dick",
        filename: "pg2701-moby-dick.epub",
        bytes: include_bytes!("../tests/fixtures/bench/pg2701-moby-dick.epub"),
    },
    Fixture {
        key: "pg1342-pride-and-prejudice",
        filename: "pg1342-pride-and-prejudice.epub",
        bytes: include_bytes!("../tests/fixtures/bench/pg1342-pride-and-prejudice.epub"),
    },
];

const WARMUP_ITERS: usize = 2;
const MEASURE_ITERS: usize = 10;

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
    min: u128,
    median: u128,
    p90: u128,
    mean: u128,
    max: u128,
    min_peak_heap_bytes: usize,
    median_peak_heap_bytes: usize,
    p90_peak_heap_bytes: usize,
    mean_peak_heap_bytes: usize,
    max_peak_heap_bytes: usize,
}

fn read_entry(zip: &mut StreamingZip<Cursor<&[u8]>>, path: &str) -> Vec<u8> {
    let entry = zip
        .get_entry(path)
        .unwrap_or_else(|| panic!("missing archive entry: {}", path))
        .clone();
    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let n = zip
        .read_file(&entry, &mut buf)
        .unwrap_or_else(|e| panic!("failed to read '{}': {:?}", path, e));
    buf.truncate(n);
    buf
}

fn open_zip(bytes: &'static [u8]) -> StreamingZip<Cursor<&'static [u8]>> {
    StreamingZip::new(Cursor::new(bytes)).expect("failed to open fixture zip")
}

fn resolve_spine_hrefs(bytes: &'static [u8]) -> (String, Vec<String>) {
    let mut zip = open_zip(bytes);
    let container_xml = read_entry(&mut zip, "META-INF/container.xml");
    let opf_path = parse_container_xml(&container_xml).expect("failed to parse container.xml");
    let opf = read_entry(&mut zip, &opf_path);
    let metadata = parse_opf(&opf).expect("failed to parse OPF");
    let spine = parse_spine(&opf).expect("failed to parse spine");

    let hrefs = spine
        .items()
        .iter()
        .filter_map(|item| metadata.get_item(&item.idref))
        .map(|item| item.href.clone())
        .collect();

    (opf_path, hrefs)
}

fn percentile(sorted: &[u128], percentile: f64) -> u128 {
    let idx = ((sorted.len().saturating_sub(1) as f64) * percentile).round() as usize;
    sorted[idx]
}

fn run_case<F>(fixture: &str, case: &str, mut op: F) -> CaseResult
where
    F: FnMut() -> usize,
{
    for _ in 0..WARMUP_ITERS {
        black_box(op());
    }

    let mut samples = Vec::with_capacity(MEASURE_ITERS);
    let mut mem_samples = Vec::with_capacity(MEASURE_ITERS);
    for _ in 0..MEASURE_ITERS {
        let baseline_alloc = current_alloc_bytes();
        reset_peak_alloc_bytes();
        let start = Instant::now();
        black_box(op());
        samples.push(start.elapsed().as_nanos());
        let peak_extra = peak_alloc_bytes().saturating_sub(baseline_alloc);
        mem_samples.push(peak_extra);
    }

    samples.sort_unstable();
    mem_samples.sort_unstable();
    let sum: u128 = samples.iter().copied().sum();
    let mem_sum: usize = mem_samples.iter().copied().sum();
    let mean = sum / samples.len() as u128;
    let min = samples[0];
    let median = percentile(&samples, 0.5);
    let p90 = percentile(&samples, 0.9);
    let max = samples[samples.len() - 1];
    let mem_mean = mem_sum / mem_samples.len();
    let mem_min = mem_samples[0];
    let mem_median =
        mem_samples[((mem_samples.len().saturating_sub(1) as f64) * 0.5).round() as usize];
    let mem_p90 =
        mem_samples[((mem_samples.len().saturating_sub(1) as f64) * 0.9).round() as usize];
    let mem_max = mem_samples[mem_samples.len() - 1];

    CaseResult {
        fixture: fixture.to_string(),
        case: case.to_string(),
        iterations: MEASURE_ITERS,
        min,
        median,
        p90,
        mean,
        max,
        min_peak_heap_bytes: mem_min,
        median_peak_heap_bytes: mem_median,
        p90_peak_heap_bytes: mem_p90,
        mean_peak_heap_bytes: mem_mean,
        max_peak_heap_bytes: mem_max,
    }
}

fn main() {
    println!("# epub-stream benchmark corpus");
    println!(
        "# warmup_iters={}, measure_iters={}",
        WARMUP_ITERS, MEASURE_ITERS
    );
    println!("# fixture_count={}", FIXTURES.len());
    println!(
        "fixture,case,iterations,min_ns,median_ns,p90_ns,mean_ns,max_ns,min_peak_heap_bytes,median_peak_heap_bytes,p90_peak_heap_bytes,mean_peak_heap_bytes,max_peak_heap_bytes"
    );

    let mut results: Vec<CaseResult> = Vec::new();

    for fixture in FIXTURES {
        let (opf_path, spine_hrefs) = resolve_spine_hrefs(fixture.bytes);
        let base = opf_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");

        let first_path = {
            let href = spine_hrefs
                .first()
                .unwrap_or_else(|| panic!("no spine href in fixture {}", fixture.key));
            if base.is_empty() {
                href.clone()
            } else {
                format!("{}/{}", base, href)
            }
        };

        results.push(run_case(fixture.key, "high_level/open_book", || {
            let book = EpubBook::from_reader(Cursor::new(fixture.bytes)).expect("open failed");
            black_box(book.chapter_count())
        }));

        results.push(run_case(
            fixture.key,
            "high_level/open_and_tokenize_first",
            || {
                let mut book =
                    EpubBook::from_reader(Cursor::new(fixture.bytes)).expect("open failed");
                let tokens = book
                    .tokenize_spine_item(0)
                    .expect("tokenize first chapter failed");
                black_box(tokens.len())
            },
        ));

        results.push(run_case(fixture.key, "compare/epub-rs/open_book", || {
            let doc = EpubDoc::from_reader(Cursor::new(fixture.bytes.to_vec()))
                .expect("epub-rs open failed");
            black_box(doc.get_num_chapters())
        }));

        results.push(run_case(
            fixture.key,
            "compare/epub-rs/open_and_get_current",
            || {
                let mut doc = EpubDoc::from_reader(Cursor::new(fixture.bytes.to_vec()))
                    .expect("epub-rs open failed");
                let (bytes, _mime) = doc
                    .get_current()
                    .expect("epub-rs failed to read current chapter");
                black_box(bytes.len())
            },
        ));

        results.push(run_case(fixture.key, "compare/epub-parser/parse", || {
            let parsed = EpubParser::parse_from_buffer(fixture.bytes).expect("epub-parser failed");
            black_box(parsed.pages.len())
        }));

        results.push(run_case(
            fixture.key,
            "compare/epub-parser/parse_and_first_page_len",
            || {
                let parsed =
                    EpubParser::parse_from_buffer(fixture.bytes).expect("epub-parser failed");
                let first_page_len = parsed.pages.first().map(|p| p.content.len()).unwrap_or(0);
                black_box(first_page_len)
            },
        ));

        results.push(run_case(fixture.key, "zip/open_archive", || {
            let zip = open_zip(fixture.bytes);
            black_box(zip.num_entries())
        }));

        results.push(run_case(fixture.key, "parse/package", || {
            let mut zip = open_zip(fixture.bytes);
            let container_xml = read_entry(&mut zip, "META-INF/container.xml");
            let opf_path = parse_container_xml(&container_xml).expect("container parse failed");
            let opf = read_entry(&mut zip, &opf_path);
            let metadata = parse_opf(&opf).expect("opf parse failed");
            let spine = parse_spine(&opf).expect("spine parse failed");
            black_box(metadata.manifest.len() + spine.len())
        }));

        results.push(run_case(fixture.key, "tokenize/first_spine_item", || {
            let mut zip = open_zip(fixture.bytes);
            let chapter = read_entry(&mut zip, &first_path);
            let html = std::str::from_utf8(&chapter).expect("chapter utf-8");
            let tokens = tokenize_html(html).expect("tokenize failed");
            black_box(tokens.len())
        }));
    }

    for result in &results {
        println!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{}",
            result.fixture,
            result.case,
            result.iterations,
            result.min,
            result.median,
            result.p90,
            result.mean,
            result.max,
            result.min_peak_heap_bytes,
            result.median_peak_heap_bytes,
            result.p90_peak_heap_bytes,
            result.mean_peak_heap_bytes,
            result.max_peak_heap_bytes
        );
    }

    println!("# summary");
    println!("fixture,metric,epub-stream_median_ns,other_median_ns,ratio_x,delta_percent");

    let find_median = |fixture: &str, case: &str| -> u128 {
        results
            .iter()
            .find(|r| r.fixture == fixture && r.case == case)
            .unwrap_or_else(|| panic!("missing case in results: fixture={} case={}", fixture, case))
            .median
    };
    let find_median_peak_heap = |fixture: &str, case: &str| -> usize {
        results
            .iter()
            .find(|r| r.fixture == fixture && r.case == case)
            .unwrap_or_else(|| panic!("missing case in results: fixture={} case={}", fixture, case))
            .median_peak_heap_bytes
    };

    for fixture in FIXTURES {
        let print_compare = |label: &str, base: &str, other: &str| {
            let base_median = find_median(fixture.key, base) as f64;
            let other_median = find_median(fixture.key, other) as f64;
            let ratio = other_median / base_median;
            let delta_percent = ((other_median - base_median) / base_median) * 100.0;
            println!(
                "{},{},{:.0},{:.0},{:.2},{:.1}",
                fixture.key, label, base_median, other_median, ratio, delta_percent
            );
        };

        print_compare(
            "open_book: epub-stream vs epub-rs",
            "high_level/open_book",
            "compare/epub-rs/open_book",
        );
        print_compare(
            "open+read_first: epub-stream vs epub-rs",
            "high_level/open_and_tokenize_first",
            "compare/epub-rs/open_and_get_current",
        );
        print_compare(
            "open_book: epub-stream vs epub-parser(parse)",
            "high_level/open_book",
            "compare/epub-parser/parse",
        );
    }

    println!("# memory_summary");
    println!(
        "fixture,metric,epub-stream_median_peak_heap_bytes,other_median_peak_heap_bytes,ratio_x,delta_percent"
    );
    for fixture in FIXTURES {
        let print_mem_compare = |label: &str, base: &str, other: &str| {
            let base_median = find_median_peak_heap(fixture.key, base) as f64;
            let other_median = find_median_peak_heap(fixture.key, other) as f64;
            let ratio = other_median / base_median;
            let delta_percent = ((other_median - base_median) / base_median) * 100.0;
            println!(
                "{},{},{:.0},{:.0},{:.2},{:.1}",
                fixture.key, label, base_median, other_median, ratio, delta_percent
            );
        };

        print_mem_compare(
            "open_book: epub-stream vs epub-rs",
            "high_level/open_book",
            "compare/epub-rs/open_book",
        );
        print_mem_compare(
            "open+read_first: epub-stream vs epub-rs",
            "high_level/open_and_tokenize_first",
            "compare/epub-rs/open_and_get_current",
        );
        print_mem_compare(
            "open_book: epub-stream vs epub-parser(parse)",
            "high_level/open_book",
            "compare/epub-parser/parse",
        );
    }

    println!("# fixtures");
    println!("key,filename,size_bytes");
    for fixture in FIXTURES {
        println!(
            "{},{},{}",
            fixture.key,
            fixture.filename,
            fixture.bytes.len()
        );
    }
}
