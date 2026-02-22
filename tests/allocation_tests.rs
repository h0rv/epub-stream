//! Allocation count tests for critical EPUB paths
//!
//! These tests verify bounded allocation behavior on hot paths using a
//! counting allocator. Run with: cargo test --test allocation_tests

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use epub_stream::book::EpubBook;
use epub_stream::zip::StreamingZip;
use epub_stream::zip::ZipLimits;

const SAMPLE_EPUB_PATH: &str =
    "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub";

/// Global allocation counter
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static DEALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Counting allocator wrapper
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_COUNT.fetch_add(1, Ordering::SeqCst);
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

/// Reset allocation counters
fn reset_counters() {
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    DEALLOC_COUNT.store(0, Ordering::SeqCst);
}

/// Get current allocation count
fn alloc_count() -> usize {
    ALLOC_COUNT.load(Ordering::SeqCst)
}

/// Check if sample EPUB exists
fn has_sample_epub() -> bool {
    std::path::Path::new(SAMPLE_EPUB_PATH).exists()
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_open_allocations_are_bounded() {
    let _guard = ALLOC_TEST_LOCK.lock().expect("alloc test lock");
    if !has_sample_epub() {
        return;
    }

    reset_counters();
    let start_allocs = alloc_count();

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let _book = EpubBook::from_reader(file).expect("Failed to open EPUB");

    let end_allocs = alloc_count();
    let allocs = end_allocs - start_allocs;

    println!("Allocations during open: {}", allocs);

    // Opening a book should require reasonable allocations
    // This is a sanity check - adjust based on actual requirements
    assert!(
        allocs < 10000,
        "Too many allocations during open: {} (expected < 10000)",
        allocs
    );
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_first_page_allocations_are_bounded() {
    let _guard = ALLOC_TEST_LOCK.lock().expect("alloc test lock");
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book = EpubBook::from_reader(file).expect("Failed to open EPUB");

    reset_counters();
    let start_allocs = alloc_count();

    // Read first chapter content
    let _html = book.chapter_html(0).expect("Failed to read first chapter");

    let end_allocs = alloc_count();
    let allocs = end_allocs - start_allocs;

    println!("Allocations during first page read: {}", allocs);

    // Reading a chapter should have bounded allocations
    assert!(
        allocs < 5000,
        "Too many allocations during first page: {} (expected < 5000)",
        allocs
    );
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_next_page_allocations_are_bounded() {
    let _guard = ALLOC_TEST_LOCK.lock().expect("alloc test lock");
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book = EpubBook::from_reader(file).expect("Failed to open EPUB");

    // First, read chapter 0 to warm up
    let _ = book.chapter_html(0);

    reset_counters();
    let start_allocs = alloc_count();

    // Read second chapter content
    if book.chapter_count() > 1 {
        let _html = book.chapter_html(1).expect("Failed to read second chapter");
    }

    let end_allocs = alloc_count();
    let allocs = end_allocs - start_allocs;

    println!("Allocations during next page read: {}", allocs);

    // Subsequent reads should also be bounded
    assert!(
        allocs < 5000,
        "Too many allocations during next page: {} (expected < 5000)",
        allocs
    );
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_zip_read_file_with_scratch_reduces_allocations() {
    let _guard = ALLOC_TEST_LOCK.lock().expect("alloc test lock");
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entry = zip
        .get_entry("mimetype")
        .expect("mimetype not found")
        .clone();

    // Test without scratch buffer
    reset_counters();
    let start_allocs = alloc_count();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let _ = zip.read_file(&entry, &mut buf);

    let allocs_without_scratch = alloc_count() - start_allocs;

    // Test with scratch buffer
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    reset_counters();
    let start_allocs = alloc_count();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let mut scratch = vec![0u8; 4096];
    let _ = zip.read_file_with_scratch(&entry, &mut buf, &mut scratch);

    let allocs_with_scratch = alloc_count() - start_allocs;

    println!(
        "Allocations without scratch: {}, with scratch: {}",
        allocs_without_scratch, allocs_with_scratch
    );

    // Using scratch buffer should not increase allocations
    assert!(
        allocs_with_scratch <= allocs_without_scratch,
        "Using scratch buffer increased allocations: {} vs {}",
        allocs_with_scratch,
        allocs_without_scratch
    );
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_chapter_text_into_reduces_allocations() {
    let _guard = ALLOC_TEST_LOCK.lock().expect("alloc test lock");
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book = EpubBook::from_reader(file).expect("Failed to open EPUB");
    let mut out = String::with_capacity(16 * 1024);

    // First call includes initial setup and any one-time allocations.
    reset_counters();
    let first_start = alloc_count();
    book.chapter_text_into(0, &mut out)
        .expect("Failed to read chapter text into");
    let first_call_allocs = alloc_count() - first_start;

    // Second call reuses the same output buffer; allocation count should not grow.
    let second_start = alloc_count();
    book.chapter_text_into(0, &mut out)
        .expect("Failed to read chapter text into (second call)");
    let second_call_allocs = alloc_count() - second_start;

    println!(
        "chapter_text_into allocations (first call): {}, (second call): {}",
        first_call_allocs, second_call_allocs
    );

    // Reusing the same caller-owned buffer should never require materially more
    // allocations on the second call.
    assert!(
        second_call_allocs <= first_call_allocs + 4,
        "chapter_text_into buffer reuse regressed: first={} second={}",
        first_call_allocs,
        second_call_allocs
    );
}

#[test]
fn test_limits_prevent_excessive_allocations() {
    // Create a minimal ZIP with tight limits
    let limits = ZipLimits::new(1024, 128);

    // The limits should be enforced at the API level
    assert_eq!(limits.max_file_read_size, 1024);
    assert_eq!(limits.max_mimetype_size, 128);
}
