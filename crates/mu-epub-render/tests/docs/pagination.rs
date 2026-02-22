use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mu_epub::{EpubBook, MemoryBudget, RenderPrepOptions};
use mu_epub_render::{
    CancelToken, OverlayComposer, OverlayContent, OverlayItem, OverlaySize, OverlaySlot,
    PageChromeConfig, PaginationProfileId, RenderBookPageMap, RenderCacheStore, RenderConfig,
    RenderDiagnostic, RenderEngine, RenderEngineError, RenderEngineOptions,
    RenderLocatorTargetKind, RenderPage, RenderReadingPositionToken,
};

fn fixture_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push(
        "../../tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
    );
    path
}

fn open_fixture_book() -> EpubBook<std::fs::File> {
    EpubBook::open(fixture_path()).expect("fixture EPUB should open")
}

fn build_engine() -> RenderEngine {
    let mut opts = RenderEngineOptions::for_display(420, 180);
    opts.layout.page_chrome = PageChromeConfig {
        progress_enabled: true,
        footer_enabled: true,
        ..PageChromeConfig::default()
    };
    RenderEngine::new(opts)
}

fn chapter_with_min_pages(
    engine: &RenderEngine,
    book: &mut EpubBook<std::fs::File>,
    min_pages: usize,
) -> Option<(usize, Vec<RenderPage>)> {
    for chapter in 0..book.chapter_count() {
        let pages = engine
            .prepare_chapter(book, chapter)
            .expect("full chapter render should succeed");
        if pages.len() >= min_pages {
            return Some((chapter, pages));
        }
    }
    None
}

fn render_chapter_page_counts(
    engine: &RenderEngine,
    book: &mut EpubBook<std::fs::File>,
) -> Vec<usize> {
    let mut out = Vec::with_capacity(book.chapter_count());
    for chapter in 0..book.chapter_count() {
        let pages = engine
            .prepare_chapter(book, chapter)
            .expect("chapter render should succeed");
        out.push(pages.len());
    }
    out
}

fn normalize_pages_for_stream_compare(mut pages: Vec<RenderPage>) -> Vec<RenderPage> {
    for page in &mut pages {
        page.metrics.chapter_page_count = None;
        page.metrics.global_page_index = None;
        page.metrics.global_page_count_estimate = None;
        page.metrics.progress_chapter = 0.0;
        page.metrics.progress_book = None;
    }
    pages
}

#[test]
fn prepare_chapter_page_range_matches_full_slice() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let (chapter, full) = chapter_with_min_pages(&engine, &mut book, 3)
        .expect("fixture should contain a chapter with at least 3 pages");

    let start = 1usize;
    let end = (start + 2).min(full.len());
    let expected = full[start..end].to_vec();

    let actual = engine
        .prepare_chapter_page_range(&mut book, chapter, start, end)
        .expect("range render should succeed");
    assert_eq!(actual, expected);
}

#[test]
fn prepare_chapter_iter_matches_full_render() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let chapter = 0usize;

    let full = engine
        .prepare_chapter(&mut book, chapter)
        .expect("full chapter render should succeed");
    let iterated: Vec<RenderPage> = engine
        .prepare_chapter_iter(&mut book, chapter)
        .expect("iterator render should succeed")
        .collect();

    assert_eq!(iterated, full);
}

#[test]
fn prepare_chapter_iter_streaming_matches_full_render() {
    let engine = build_engine();
    let mut book_for_full = open_fixture_book();
    let chapter = 0usize;

    let full = engine
        .prepare_chapter(&mut book_for_full, chapter)
        .expect("full chapter render should succeed");

    let streaming: Vec<RenderPage> = engine
        .prepare_chapter_iter_streaming(open_fixture_book(), chapter)
        .collect::<Result<Vec<_>, _>>()
        .expect("streaming iterator should succeed");

    assert_eq!(
        normalize_pages_for_stream_compare(streaming),
        normalize_pages_for_stream_compare(full)
    );
}

#[test]
fn prepare_chapter_bytes_with_matches_full_render() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let chapter = 0usize;

    let expected = engine
        .prepare_chapter(&mut book, chapter)
        .expect("full chapter render should succeed");

    let chapter_href = book.chapter(chapter).expect("chapter should exist").href;
    let mut chapter_buf = Vec::with_capacity(128 * 1024);
    book.read_resource_into_with_hard_cap(
        &chapter_href,
        &mut chapter_buf,
        RenderPrepOptions::default().memory.max_entry_bytes,
    )
    .expect("chapter bytes should load");

    let mut actual = Vec::new();
    engine
        .prepare_chapter_bytes_with(&mut book, chapter, &chapter_buf, |page| actual.push(page))
        .expect("chapter-bytes render should succeed");

    assert_eq!(
        normalize_pages_for_stream_compare(actual),
        normalize_pages_for_stream_compare(expected)
    );
}

#[test]
fn prepare_chapter_iter_streaming_reports_errors() {
    let engine = build_engine();
    let invalid_chapter = usize::MAX;
    let mut iter = engine.prepare_chapter_iter_streaming(open_fixture_book(), invalid_chapter);
    let first = iter
        .next()
        .expect("streaming iterator should produce terminal error");
    assert!(first.is_err());
    assert!(iter.next().is_none());
}

#[test]
fn pagination_profile_id_is_stable_for_same_options() {
    let e1 = build_engine();
    let e2 = build_engine();
    assert_eq!(e1.pagination_profile_id(), e2.pagination_profile_id());
}

#[test]
fn toc_href_resolves_to_expected_chapter_first_page() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let navigation = book
        .ensure_navigation()
        .expect("navigation parse should succeed")
        .cloned()
        .expect("fixture should include navigation");
    let chapters: Vec<_> = book.chapters().collect();
    let page_counts = render_chapter_page_counts(&engine, &mut book);
    let page_map = RenderBookPageMap::from_chapter_page_counts(&chapters, &page_counts);

    let (toc_href, target) = navigation
        .toc_flat()
        .into_iter()
        .find_map(|(_, point)| {
            let target = page_map.resolve_toc_href(&point.href)?;
            if page_counts.get(target.chapter_index).copied().unwrap_or(0) == 0 {
                return None;
            }
            Some((point.href.clone(), target))
        })
        .expect("fixture should include a TOC href that resolves to rendered chapter pages");

    let expected_chapter = chapters
        .iter()
        .find(|chapter| chapter.href == target.chapter_href)
        .map(|chapter| chapter.index)
        .expect("resolved href should map to an existing chapter");
    let expected_page = page_map
        .chapter_start_page_index(expected_chapter)
        .expect("resolved chapter should have a start page");

    let resolved = page_map
        .resolve_toc_href(&toc_href)
        .expect("TOC href should resolve through page map");
    assert_eq!(resolved.chapter_index, expected_chapter);
    assert_eq!(resolved.page_index, expected_page);
}

#[test]
fn fragment_href_uses_explicit_chapter_start_fallback() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let chapters: Vec<_> = book.chapters().collect();
    let page_counts = render_chapter_page_counts(&engine, &mut book);
    let page_map = RenderBookPageMap::from_chapter_page_counts(&chapters, &page_counts);
    let chapter = page_counts
        .iter()
        .position(|count| *count > 0)
        .expect("fixture should include at least one rendered chapter");
    let href = format!("{}#__missing_anchor__", chapters[chapter].href);
    let expected = page_map
        .chapter_start_page_index(chapter)
        .expect("chapter should have a start page");

    let first = page_map
        .resolve_href(&href)
        .expect("fragment href should resolve with fallback");
    let second = page_map
        .resolve_href(&href)
        .expect("fragment href should deterministically resolve");

    assert_eq!(first, second);
    assert_eq!(first.chapter_index, chapter);
    assert_eq!(first.page_index, expected);
    assert_eq!(
        first.kind,
        RenderLocatorTargetKind::FragmentFallbackChapterStart
    );
}

#[test]
fn reading_position_token_remaps_after_reflow_and_preserves_chapter_when_possible() {
    let baseline_engine = build_engine();
    let mut baseline_book = open_fixture_book();
    let chapters: Vec<_> = baseline_book.chapters().collect();
    let baseline_counts = render_chapter_page_counts(&baseline_engine, &mut baseline_book);
    let baseline_map = RenderBookPageMap::from_chapter_page_counts(&chapters, &baseline_counts);

    let mut reflow_opts = RenderEngineOptions::for_display(300, 180);
    reflow_opts.layout.paragraph_gap_px = reflow_opts.layout.paragraph_gap_px.saturating_add(2);
    let reflow_engine = RenderEngine::new(reflow_opts);
    let mut reflow_book = open_fixture_book();
    let reflow_counts = render_chapter_page_counts(&reflow_engine, &mut reflow_book);
    let reflow_map = RenderBookPageMap::from_chapter_page_counts(&chapters, &reflow_counts);

    let chapter = (0..chapters.len())
        .find(|idx| baseline_counts[*idx] > 2 && reflow_counts[*idx] > 0)
        .or_else(|| {
            (0..chapters.len()).find(|idx| baseline_counts[*idx] > 0 && reflow_counts[*idx] > 0)
        })
        .expect("fixture should include a chapter rendered by both profiles");

    let baseline_start = baseline_map
        .chapter_start_page_index(chapter)
        .expect("baseline chapter start should exist");
    let reflow_start = reflow_map
        .chapter_start_page_index(chapter)
        .expect("reflow chapter start should exist");
    let baseline_mid = baseline_start.saturating_add(baseline_counts[chapter] / 2);
    let token = baseline_map
        .reading_position_token_for_page_index(baseline_mid)
        .expect("token should be created for baseline page");
    let remapped = reflow_map
        .remap_reading_position_token(&token)
        .expect("token should remap on reflow profile");

    assert!(remapped >= reflow_start);
    assert!(remapped < reflow_start.saturating_add(reflow_counts[chapter]));

    let expected_chapter_offset = if reflow_counts[chapter] <= 1 {
        0
    } else {
        (token.chapter_progress.clamp(0.0, 1.0) * (reflow_counts[chapter] - 1) as f32).round()
            as usize
    }
    .min(reflow_counts[chapter].saturating_sub(1));
    assert_eq!(
        remapped,
        reflow_start.saturating_add(expected_chapter_offset),
        "chapter progress remap should target nearest logical page"
    );

    let mut previous = reflow_start;
    for old_offset in 0..baseline_counts[chapter] {
        let old_index = baseline_start.saturating_add(old_offset);
        let token = baseline_map
            .reading_position_token_for_page_index(old_index)
            .expect("token should be created");
        let remapped = reflow_map
            .remap_reading_position_token(&token)
            .expect("token should remap");
        assert!(remapped >= reflow_start);
        assert!(remapped < reflow_start.saturating_add(reflow_counts[chapter]));
        assert!(remapped >= previous);
        previous = remapped;
    }
}

#[test]
fn reading_position_token_handles_invalid_or_changed_chapter_counts_safely() {
    let book = open_fixture_book();
    let chapters: Vec<_> = book.chapters().take(3).collect();
    assert!(
        chapters.len() >= 2,
        "fixture should provide enough chapters for remap safety coverage"
    );

    let old_map = RenderBookPageMap::from_chapter_page_counts(&chapters, &[3, 4, 2]);
    let token = old_map
        .reading_position_token_for_page_index(4)
        .expect("old token should be created");

    let changed_map = RenderBookPageMap::from_chapter_page_counts(&chapters, &[1]);
    let changed = changed_map
        .remap_reading_position_token(&token)
        .expect("remap should fall back safely for changed chapter counts");
    assert!(changed < changed_map.total_pages());

    let invalid = RenderReadingPositionToken {
        chapter_index: usize::MAX,
        chapter_href: Some("missing.xhtml".to_string()),
        chapter_page_index: usize::MAX,
        chapter_page_count: 0,
        chapter_progress: f32::NAN,
        global_page_index: usize::MAX,
        global_page_count: 5,
    };
    let invalid_mapped = changed_map
        .remap_reading_position_token(&invalid)
        .expect("invalid chapter hints should still map via bounded global progress");
    assert_eq!(invalid_mapped, changed_map.total_pages().saturating_sub(1));

    let empty_map = RenderBookPageMap::from_chapter_page_counts(&chapters, &[]);
    assert!(
        empty_map.remap_reading_position_token(&invalid).is_none(),
        "empty target map should not produce an index"
    );
}

#[derive(Clone, Copy, Debug, Default)]
struct AlreadyCancelled;

impl CancelToken for AlreadyCancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn prepare_chapter_with_cancel_stops_early() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let mut saw_pages = 0usize;
    let result =
        engine.prepare_chapter_with_cancel(&mut book, 0, &AlreadyCancelled, |_page| saw_pages += 1);
    assert!(result.is_err());
    assert_eq!(saw_pages, 0);
}

#[test]
fn prepare_chapter_with_config_can_disable_embedded_fonts() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let (chapter, _) = chapter_with_min_pages(&engine, &mut book, 1)
        .expect("fixture should contain at least one renderable chapter");
    let mut pages = Vec::new();
    engine
        .prepare_chapter_with_config(
            &mut book,
            chapter,
            RenderConfig::default().with_embedded_fonts(false),
            |page| pages.push(page),
        )
        .expect("render without embedded fonts should succeed");
    assert!(!pages.is_empty());
}

#[derive(Clone, Copy, Debug, Default)]
struct FooterOverlay;

impl OverlayComposer for FooterOverlay {
    fn compose(
        &self,
        metrics: &mu_epub_render::PageMetrics,
        _viewport: OverlaySize,
    ) -> Vec<OverlayItem> {
        vec![OverlayItem {
            slot: OverlaySlot::BottomCenter,
            z: 1,
            content: OverlayContent::Text(format!("p{}", metrics.chapter_page_index + 1)),
        }]
    }
}

#[test]
fn overlay_composer_attaches_overlay_items() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let (chapter, _) = chapter_with_min_pages(&engine, &mut book, 1)
        .expect("fixture should contain at least one renderable chapter");
    let mut pages = Vec::new();
    engine
        .prepare_chapter_with_overlay_composer(
            &mut book,
            chapter,
            OverlaySize {
                width: 420,
                height: 180,
            },
            &FooterOverlay,
            |p| pages.push(p),
        )
        .expect("overlay composer path should succeed");
    assert!(!pages.is_empty());
    assert!(pages.iter().all(|p| !p.overlay_items.is_empty()));
}

#[test]
fn diagnostic_sink_receives_reflow_timing() {
    let mut engine = build_engine();
    let seen = Arc::new(Mutex::new(Vec::<RenderDiagnostic>::new()));
    let seen_clone = Arc::clone(&seen);
    engine.set_diagnostic_sink(move |d| {
        if let Ok(mut sink) = seen_clone.lock() {
            sink.push(d);
        }
    });
    let mut book = open_fixture_book();
    let _ = engine
        .prepare_chapter(&mut book, 0)
        .expect("prepare should pass");
    let diagnostics = seen.lock().expect("diag lock").clone();
    assert!(diagnostics
        .iter()
        .any(|d| matches!(d, RenderDiagnostic::ReflowTimeMs(_))));
}

#[derive(Default)]
struct CacheSpy {
    loads: Mutex<usize>,
    stores: Mutex<usize>,
    cached_pages: Mutex<Option<Vec<RenderPage>>>,
}

impl CacheSpy {
    fn load_count(&self) -> usize {
        *self.loads.lock().expect("load lock")
    }

    fn store_count(&self) -> usize {
        *self.stores.lock().expect("store lock")
    }
}

impl RenderCacheStore for CacheSpy {
    fn load_chapter_pages(
        &self,
        _profile: PaginationProfileId,
        _chapter_index: usize,
    ) -> Option<Vec<RenderPage>> {
        let mut loads = self.loads.lock().expect("load lock");
        *loads += 1;
        self.cached_pages.lock().expect("pages lock").clone()
    }

    fn store_chapter_pages(
        &self,
        _profile: PaginationProfileId,
        _chapter_index: usize,
        pages: &[RenderPage],
    ) {
        let mut stores = self.stores.lock().expect("store lock");
        *stores += 1;
        *self.cached_pages.lock().expect("pages lock") = Some(pages.to_vec());
    }
}

#[test]
fn prepare_chapter_with_config_stores_pages_in_cache() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let cache = CacheSpy::default();
    let (chapter, _) = chapter_with_min_pages(&engine, &mut book, 1)
        .expect("fixture should contain at least one renderable chapter");

    let pages = engine
        .prepare_chapter_with_config_collect(
            &mut book,
            chapter,
            RenderConfig::default().with_cache(&cache),
        )
        .expect("prepare with cache should pass");

    assert!(!pages.is_empty());
    assert_eq!(cache.load_count(), 1);
    assert_eq!(cache.store_count(), 1);
    let cached = cache
        .cached_pages
        .lock()
        .expect("pages lock")
        .clone()
        .expect("cache should store pages");
    assert_eq!(cached, pages);
}

#[test]
fn prepare_chapter_with_config_uses_cache_hit() {
    let engine = build_engine();
    let mut book = open_fixture_book();
    let (chapter, expected) = chapter_with_min_pages(&engine, &mut book, 1)
        .expect("fixture should contain at least one renderable chapter");

    let cache = CacheSpy::default();
    *cache.cached_pages.lock().expect("pages lock") = Some(expected.clone());
    let mut book_from_cache = open_fixture_book();

    let actual = engine
        .prepare_chapter_with_config_collect(
            &mut book_from_cache,
            chapter,
            RenderConfig::default().with_cache(&cache),
        )
        .expect("cached prepare should pass");

    assert_eq!(actual, expected);
    assert_eq!(cache.load_count(), 1);
    assert_eq!(cache.store_count(), 0);
}

#[test]
fn prepare_chapter_collect_enforces_max_pages_in_memory() {
    let baseline_engine = build_engine();
    let mut baseline_book = open_fixture_book();
    let (chapter, _) = chapter_with_min_pages(&baseline_engine, &mut baseline_book, 2)
        .expect("fixture should contain a chapter with at least 2 pages");

    let mut opts = RenderEngineOptions::for_display(420, 180);
    opts.prep = RenderPrepOptions {
        memory: MemoryBudget {
            max_pages_in_memory: 1,
            ..MemoryBudget::default()
        },
        ..RenderPrepOptions::default()
    };
    let engine = RenderEngine::new(opts);
    let mut book = open_fixture_book();

    let err = engine
        .prepare_chapter(&mut book, chapter)
        .expect_err("collect path should enforce max_pages_in_memory");
    assert!(matches!(
        err,
        RenderEngineError::LimitExceeded {
            kind: "max_pages_in_memory",
            ..
        }
    ));
}
