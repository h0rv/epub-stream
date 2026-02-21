use core::convert::Infallible;
use std::path::{Path, PathBuf};

use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
    Pixel,
};
use mu_epub::{EpubBook, MemoryBudget};
use mu_epub_embedded_graphics::{
    with_embedded_text_measurer, EgRenderer, ImageRegistryLimits, MonochromeBitmap,
};
use mu_epub_render::{
    BlockRole, DrawCommand, ImageObjectCommand, JustifyMode, RenderConfig, RenderEngine,
    RenderEngineError, RenderEngineOptions, RenderPage, ResolvedTextStyle, TextCommand,
    TextMeasurer,
};

const DISPLAY_WIDTH: i32 = 480;
const DISPLAY_HEIGHT: i32 = 800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FamilyOverride {
    Auto,
    Monospace,
}

impl FamilyOverride {
    fn as_forced_family(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Monospace => Some("monospace"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Monospace => "monospace",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Scenario {
    base_font_size_px: f32,
    text_scale: f32,
    justify: bool,
    family_override: FamilyOverride,
    line_gap_px: i32,
    paragraph_gap_px: i32,
}

impl Scenario {
    fn describe(self) -> String {
        format!(
            "family={} justify={} base_font={} text_scale={} line_gap={} paragraph_gap={}",
            self.family_override.label(),
            self.justify,
            self.base_font_size_px,
            self.text_scale,
            self.line_gap_px,
            self.paragraph_gap_px
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct ScenarioResult {
    scenario: Scenario,
    page_count: usize,
}

#[derive(Default)]
struct PixelCaptureDisplay {
    size: Size,
    on_pixels: usize,
}

impl PixelCaptureDisplay {
    fn new(width: u32, height: u32) -> Self {
        Self {
            size: Size::new(width, height),
            on_pixels: 0,
        }
    }
}

impl OriginDimensions for PixelCaptureDisplay {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for PixelCaptureDisplay {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(_point, color) in pixels {
            if color == BinaryColor::On {
                self.on_pixels += 1;
            }
        }
        Ok(())
    }
}

#[test]
fn embedded_reflow_regression_matrix_is_monotonic_and_bounded() {
    let mut fixtures = available_bench_fixtures();
    if fixtures.is_empty() {
        eprintln!(
            "embedded reflow regression skipped: no bench fixtures found under {}",
            bench_fixture_root().display()
        );
        return;
    }
    fixtures.truncate(1);
    for fixture in fixtures {
        run_matrix_for_fixture(&fixture);
    }
}

#[test]
fn embedded_low_ram_reflow_and_page_turn_loops_are_stable() {
    const LOW_RAM_BUDGET: MemoryBudget = MemoryBudget {
        max_entry_bytes: 2 * 1024 * 1024,
        max_css_bytes: 192 * 1024,
        max_nav_bytes: 192 * 1024,
        max_inline_style_bytes: 8 * 1024,
        max_pages_in_memory: 96,
    };

    let Some(fixture) = available_bench_fixtures().into_iter().next() else {
        eprintln!(
            "embedded low-ram regression skipped: no bench fixtures found under {}",
            bench_fixture_root().display()
        );
        return;
    };

    let baseline = Scenario {
        base_font_size_px: 22.0,
        text_scale: 1.0,
        justify: true,
        family_override: FamilyOverride::Auto,
        line_gap_px: 4,
        paragraph_gap_px: 8,
    };

    let Some((chapter_index, baseline_pages)) =
        pick_multi_page_text_chapter_with_budget(&fixture, baseline, LOW_RAM_BUDGET)
    else {
        eprintln!(
            "embedded low-ram regression skipped for {}: no suitable chapter found within budget",
            fixture.display()
        );
        return;
    };

    let scenarios = [
        baseline,
        Scenario {
            base_font_size_px: 24.0,
            text_scale: 1.10,
            justify: true,
            family_override: FamilyOverride::Auto,
            line_gap_px: 5,
            paragraph_gap_px: 10,
        },
        Scenario {
            base_font_size_px: 20.0,
            text_scale: 1.0,
            justify: false,
            family_override: FamilyOverride::Monospace,
            line_gap_px: 3,
            paragraph_gap_px: 6,
        },
    ];

    let mut expected = Vec::new();
    expected.push((
        baseline_pages.len(),
        sampled_embedded_signature(&baseline_pages),
    ));
    for scenario in scenarios.iter().skip(1).copied() {
        let pages =
            render_fixture_chapter_with_budget(&fixture, chapter_index, scenario, LOW_RAM_BUDGET)
                .unwrap_or_else(|msg| panic!("{}", msg));
        expected.push((pages.len(), sampled_embedded_signature(&pages)));
    }

    for _loop_idx in 0..6 {
        for (scenario_idx, scenario) in scenarios.iter().copied().enumerate() {
            let pages = render_fixture_chapter_with_budget(
                &fixture,
                chapter_index,
                scenario,
                LOW_RAM_BUDGET,
            )
            .unwrap_or_else(|msg| panic!("{}", msg));
            let (expected_page_count, expected_signature) = expected[scenario_idx];
            assert_eq!(
                pages.len(),
                expected_page_count,
                "fixture={} scenario='{}' low-ram page count drifted",
                fixture.display(),
                scenario.describe()
            );
            let signature = sampled_embedded_signature(&pages);
            assert_eq!(
                signature,
                expected_signature,
                "fixture={} scenario='{}' low-ram render signature drifted",
                fixture.display(),
                scenario.describe()
            );
        }
    }
}

#[test]
fn embedded_renderer_budget_diagnostics_cover_limit_and_fallback_paths() {
    let mut renderer = EgRenderer::with_image_registry_limits(
        mu_epub_embedded_graphics::EgRenderConfig {
            clear_first: false,
            ..mu_epub_embedded_graphics::EgRenderConfig::default()
        },
        ImageRegistryLimits {
            max_images: 1,
            max_total_pixels: 4,
        },
    );
    renderer
        .register_image_bitmap(
            "images/a.bin",
            MonochromeBitmap::from_packed_bits(2, 2, vec![0b1111_0000]).expect("valid bitmap"),
        )
        .expect("first registration should succeed");
    let err = renderer.register_image_bitmap(
        "images/b.bin",
        MonochromeBitmap::from_packed_bits(1, 1, vec![0b1000_0000]).expect("valid bitmap"),
    );
    assert_eq!(
        err,
        Err(mu_epub_embedded_graphics::ImageRegistryError::MaxImagesExceeded)
    );

    let text = TextCommand {
        x: 0,
        baseline_y: 10,
        text: "fallback telemetry".to_string(),
        font_id: None,
        style: ResolvedTextStyle {
            font_id: None,
            family: "fantasy".to_string(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.3,
            letter_spacing: 0.0,
            role: BlockRole::Body,
            justify_mode: JustifyMode::None,
        },
    };
    let image = ImageObjectCommand {
        src: "images/missing.bin".to_string(),
        alt: String::new(),
        x: 4,
        y: 6,
        width: 8,
        height: 8,
    };
    let page = RenderPage {
        page_number: 1,
        commands: vec![DrawCommand::Text(text), DrawCommand::ImageObject(image)],
        ..RenderPage::new(1)
    };
    let mut display = PixelCaptureDisplay::new(96, 96);
    let diagnostics = renderer
        .render_page_with_diagnostics(&page, &mut display)
        .expect("render should succeed");

    assert_eq!(diagnostics.image_fallback_draws, 1);
    assert_eq!(diagnostics.text_fallbacks.unknown_family, 1);
    assert_eq!(diagnostics.text_fallbacks.total(), 1);
    assert_eq!(diagnostics.image_registry.registered_images, 1);
    assert_eq!(diagnostics.image_registry.max_images_errors, 1);
    assert_eq!(
        diagnostics.image_registry.image_slot_pressure_percent(),
        100
    );
}

fn run_matrix_for_fixture(fixture: &Path) {
    let baseline = Scenario {
        base_font_size_px: 22.0,
        text_scale: 1.0,
        justify: true,
        family_override: FamilyOverride::Auto,
        line_gap_px: 4,
        paragraph_gap_px: 8,
    };

    let Some((chapter_index, baseline_pages)) = pick_multi_page_text_chapter(fixture, baseline)
    else {
        eprintln!(
            "embedded reflow regression skipped for {}: no multi-page body-text chapter in baseline scenario",
            fixture.display()
        );
        return;
    };

    assert_scenario_invariants(
        fixture,
        chapter_index,
        &baseline_pages,
        baseline,
        "baseline chapter invariants",
    );
    assert_embedded_renderer_draws_sample_pages(fixture, &baseline_pages, baseline);

    let base_font_sizes = [22.0f32, 28.0];
    let text_scales = [1.0f32, 1.30];
    let justify_modes = [false, true];
    let families = [FamilyOverride::Auto, FamilyOverride::Monospace];

    let mut results = Vec::new();
    results.push(ScenarioResult {
        scenario: baseline,
        page_count: baseline_pages.len(),
    });

    for family in families {
        for justify in justify_modes {
            for (base_idx, base_font_size_px) in base_font_sizes.iter().copied().enumerate() {
                for (scale_idx, text_scale) in text_scales.iter().copied().enumerate() {
                    let scenario = Scenario {
                        base_font_size_px,
                        text_scale,
                        justify,
                        family_override: family,
                        line_gap_px: 4,
                        paragraph_gap_px: 8,
                    };

                    if family == FamilyOverride::Auto && justify && base_idx == 0 && scale_idx == 0
                    {
                        continue;
                    }

                    let pages = render_fixture_chapter(fixture, chapter_index, scenario)
                        .unwrap_or_else(|msg| panic!("{}", msg));
                    assert_scenario_invariants(
                        fixture,
                        chapter_index,
                        &pages,
                        scenario,
                        "matrix chapter invariants",
                    );
                    assert_embedded_renderer_draws_sample_pages(fixture, &pages, scenario);
                    results.push(ScenarioResult {
                        scenario,
                        page_count: pages.len(),
                    });
                }
            }
        }
    }

    let expanded_spacing = Scenario {
        base_font_size_px: 22.0,
        text_scale: 1.0,
        justify: true,
        family_override: FamilyOverride::Auto,
        line_gap_px: 8,
        paragraph_gap_px: 14,
    };
    let expanded_spacing_pages = render_fixture_chapter(fixture, chapter_index, expanded_spacing)
        .unwrap_or_else(|msg| panic!("{}", msg));
    assert_scenario_invariants(
        fixture,
        chapter_index,
        &expanded_spacing_pages,
        expanded_spacing,
        "expanded-spacing chapter invariants",
    );
    assert_embedded_renderer_draws_sample_pages(fixture, &expanded_spacing_pages, expanded_spacing);
    results.push(ScenarioResult {
        scenario: expanded_spacing,
        page_count: expanded_spacing_pages.len(),
    });

    assert_page_count_monotonicity(fixture, &results);
}

fn assert_page_count_monotonicity(fixture: &Path, results: &[ScenarioResult]) {
    for lhs in results {
        for rhs in results {
            if lhs.scenario.family_override != rhs.scenario.family_override {
                continue;
            }
            if lhs.scenario.justify != rhs.scenario.justify {
                continue;
            }
            if rhs.scenario.base_font_size_px < lhs.scenario.base_font_size_px {
                continue;
            }
            if rhs.scenario.text_scale < lhs.scenario.text_scale {
                continue;
            }
            if rhs.scenario.line_gap_px < lhs.scenario.line_gap_px {
                continue;
            }
            if rhs.scenario.paragraph_gap_px < lhs.scenario.paragraph_gap_px {
                continue;
            }
            let strictly_larger = rhs.scenario.base_font_size_px > lhs.scenario.base_font_size_px
                || rhs.scenario.text_scale > lhs.scenario.text_scale
                || rhs.scenario.line_gap_px > lhs.scenario.line_gap_px
                || rhs.scenario.paragraph_gap_px > lhs.scenario.paragraph_gap_px;
            if !strictly_larger {
                continue;
            }
            assert!(
                rhs.page_count >= lhs.page_count,
                "fixture={} monotonic page count violated: '{}' ({}) -> '{}' ({})",
                fixture.display(),
                lhs.scenario.describe(),
                lhs.page_count,
                rhs.scenario.describe(),
                rhs.page_count
            );
        }
    }
}

fn assert_scenario_invariants(
    fixture: &Path,
    chapter_index: usize,
    pages: &[RenderPage],
    scenario: Scenario,
    context: &str,
) {
    assert!(
        !pages.is_empty(),
        "fixture={} {} scenario='{}' should render at least one page",
        fixture.display(),
        context,
        scenario.describe()
    );
    assert_metrics_are_monotonic(fixture, chapter_index, pages, scenario, context);
    assert_no_right_edge_body_overrun(fixture, pages, scenario, context);
}

fn assert_metrics_are_monotonic(
    fixture: &Path,
    chapter_index: usize,
    pages: &[RenderPage],
    scenario: Scenario,
    context: &str,
) {
    let total = pages.len();
    let mut last_progress = 0.0f32;
    for (idx, page) in pages.iter().enumerate() {
        assert_eq!(
            page.page_number,
            idx + 1,
            "fixture={} {} scenario='{}' page_number should increase by one",
            fixture.display(),
            context,
            scenario.describe()
        );
        let metrics = &page.metrics;
        assert_eq!(
            metrics.chapter_index,
            chapter_index,
            "fixture={} {} scenario='{}' unexpected chapter index",
            fixture.display(),
            context,
            scenario.describe()
        );
        assert_eq!(
            metrics.chapter_page_index,
            idx,
            "fixture={} {} scenario='{}' chapter_page_index should be monotonic",
            fixture.display(),
            context,
            scenario.describe()
        );
        assert_eq!(
            metrics.chapter_page_count,
            Some(total),
            "fixture={} {} scenario='{}' chapter_page_count should be complete",
            fixture.display(),
            context,
            scenario.describe()
        );
        assert!(
            (0.0..=1.0).contains(&metrics.progress_chapter),
            "fixture={} {} scenario='{}' progress_chapter should be in [0,1], got {}",
            fixture.display(),
            context,
            scenario.describe(),
            metrics.progress_chapter
        );
        assert!(
            metrics.progress_chapter >= last_progress,
            "fixture={} {} scenario='{}' progress_chapter should be monotonic ({} < {})",
            fixture.display(),
            context,
            scenario.describe(),
            metrics.progress_chapter,
            last_progress
        );
        last_progress = metrics.progress_chapter;
    }
    if total > 1 {
        let first = &pages[0].metrics;
        let last = &pages[total - 1].metrics;
        assert!(
            first.progress_chapter <= 0.05,
            "fixture={} {} scenario='{}' first-page progress should start near zero, got {}",
            fixture.display(),
            context,
            scenario.describe(),
            first.progress_chapter
        );
        assert!(
            last.progress_chapter >= 0.95,
            "fixture={} {} scenario='{}' last-page progress should be near complete, got {}",
            fixture.display(),
            context,
            scenario.describe(),
            last.progress_chapter
        );
    }
}

fn assert_no_right_edge_body_overrun(
    fixture: &Path,
    pages: &[RenderPage],
    scenario: Scenario,
    context: &str,
) {
    let right_limit = (DISPLAY_WIDTH - 2) as f32;
    let mut sampled = 0usize;
    'pages: for (page_idx, page) in pages.iter().enumerate().take(3) {
        for cmd in &page.commands {
            let DrawCommand::Text(text) = cmd else {
                continue;
            };
            if !matches!(
                text.style.role,
                BlockRole::Body | BlockRole::Paragraph | BlockRole::ListItem
            ) {
                continue;
            }
            if text.text.trim().is_empty() || is_uppercase_heavy(&text.text) {
                continue;
            }
            assert!(
                text.x >= 0,
                "fixture={} {} scenario='{}' negative x for body text on page {}",
                fixture.display(),
                context,
                scenario.describe(),
                page_idx
            );
            let est_right = text.x as f32 + measured_text_width_px(&text.text, &text.style);
            assert!(
                est_right <= (right_limit + 1.0),
                "fixture={} {} scenario='{}' page={} right-edge overrun: line='{}' est_right={} limit={}",
                fixture.display(),
                context,
                scenario.describe(),
                page_idx,
                text.text,
                est_right,
                right_limit
            );
            sampled += 1;
            if sampled >= 64 {
                break 'pages;
            }
        }
    }
    assert!(
        sampled > 0,
        "fixture={} {} scenario='{}' expected sampled body lines",
        fixture.display(),
        context,
        scenario.describe()
    );
}

fn assert_embedded_renderer_draws_sample_pages(
    fixture: &Path,
    pages: &[RenderPage],
    scenario: Scenario,
) {
    let renderer: EgRenderer = EgRenderer::default();
    let mut sampled = 0usize;
    for page in pages.iter().take(2) {
        let mut display = PixelCaptureDisplay::new(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32);
        renderer
            .render_page(page, &mut display)
            .unwrap_or_else(|_| {
                panic!(
                    "fixture={} embedded renderer should draw scenario='{}'",
                    fixture.display(),
                    scenario.describe()
                )
            });
        assert!(
            display.on_pixels > 0,
            "fixture={} scenario='{}' expected rendered pixels on sampled page",
            fixture.display(),
            scenario.describe()
        );
        sampled += 1;
    }
    assert!(
        sampled > 0,
        "fixture={} scenario='{}' expected sampled pages for embedded renderer",
        fixture.display(),
        scenario.describe()
    );
}

fn sampled_embedded_signature(pages: &[RenderPage]) -> u64 {
    let renderer: EgRenderer = EgRenderer::default();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    hash = hash_mix(hash, pages.len() as u64);
    for page in pages.iter().take(6) {
        hash = hash_mix(hash, page.page_number as u64);
        hash = hash_mix(hash, page.commands.len() as u64);
        hash = hash_mix(hash, page.metrics.chapter_page_index as u64);
        hash = hash_mix(hash, page.metrics.progress_chapter.to_bits() as u64);
        let mut display = PixelCaptureDisplay::new(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32);
        renderer
            .render_page(page, &mut display)
            .expect("embedded renderer should draw sampled page");
        hash = hash_mix(hash, display.on_pixels as u64);
    }
    hash
}

fn hash_mix(mut hash: u64, value: u64) -> u64 {
    hash ^= value;
    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    hash
}

fn pick_multi_page_text_chapter(
    fixture: &Path,
    scenario: Scenario,
) -> Option<(usize, Vec<RenderPage>)> {
    let mut book = EpubBook::open(fixture).ok()?;
    let engine = build_engine(scenario);
    for chapter_index in 0..book.chapter_count() {
        let Ok(pages) = render_chapter(&engine, &mut book, chapter_index, scenario) else {
            continue;
        };
        if pages.len() < 3 {
            continue;
        }
        if sampled_body_line_count(&pages) >= 8 {
            return Some((chapter_index, pages));
        }
    }
    None
}

fn pick_multi_page_text_chapter_with_budget(
    fixture: &Path,
    scenario: Scenario,
    budget: MemoryBudget,
) -> Option<(usize, Vec<RenderPage>)> {
    let mut book = EpubBook::open(fixture).ok()?;
    let engine = build_engine_with_budget(scenario, budget);
    for chapter_index in 0..book.chapter_count() {
        let Ok(pages) = render_chapter(&engine, &mut book, chapter_index, scenario) else {
            continue;
        };
        if !(3..=48).contains(&pages.len()) {
            continue;
        }
        if sampled_body_line_count(&pages) >= 8 {
            return Some((chapter_index, pages));
        }
    }
    None
}

fn sampled_body_line_count(pages: &[RenderPage]) -> usize {
    let mut sampled = 0usize;
    for page in pages.iter().take(3) {
        for cmd in &page.commands {
            let DrawCommand::Text(text) = cmd else {
                continue;
            };
            if !matches!(
                text.style.role,
                BlockRole::Body | BlockRole::Paragraph | BlockRole::ListItem
            ) {
                continue;
            }
            if text.text.trim().is_empty() || is_uppercase_heavy(&text.text) {
                continue;
            }
            sampled += 1;
        }
    }
    sampled
}

fn render_fixture_chapter(
    fixture: &Path,
    chapter_index: usize,
    scenario: Scenario,
) -> Result<Vec<RenderPage>, String> {
    let mut book = EpubBook::open(fixture).map_err(|e| {
        format!(
            "unable to open fixture {} for scenario '{}': {}",
            fixture.display(),
            scenario.describe(),
            e
        )
    })?;
    let engine = build_engine(scenario);
    render_chapter(&engine, &mut book, chapter_index, scenario).map_err(|e| {
        format!(
            "render failed for fixture={} chapter={} scenario='{}': {}",
            fixture.display(),
            chapter_index,
            scenario.describe(),
            e
        )
    })
}

fn render_fixture_chapter_with_budget(
    fixture: &Path,
    chapter_index: usize,
    scenario: Scenario,
    budget: MemoryBudget,
) -> Result<Vec<RenderPage>, String> {
    let mut book = EpubBook::open(fixture).map_err(|e| {
        format!(
            "unable to open fixture {} for scenario '{}': {}",
            fixture.display(),
            scenario.describe(),
            e
        )
    })?;
    let engine = build_engine_with_budget(scenario, budget);
    render_chapter(&engine, &mut book, chapter_index, scenario).map_err(|e| {
        format!(
            "low-ram render failed for fixture={} chapter={} scenario='{}': {}",
            fixture.display(),
            chapter_index,
            scenario.describe(),
            e
        )
    })
}

fn render_chapter<R: std::io::Read + std::io::Seek>(
    engine: &RenderEngine,
    book: &mut EpubBook<R>,
    chapter_index: usize,
    scenario: Scenario,
) -> Result<Vec<RenderPage>, RenderEngineError> {
    let mut config = RenderConfig::default();
    if let Some(family) = scenario.family_override.as_forced_family() {
        config = config.with_forced_font_family(family);
    }
    config = with_embedded_text_measurer(config);
    engine.prepare_chapter_with_config_collect(book, chapter_index, config)
}

fn build_engine(scenario: Scenario) -> RenderEngine {
    build_engine_with_budget(scenario, MemoryBudget::default())
}

fn build_engine_with_budget(scenario: Scenario, budget: MemoryBudget) -> RenderEngine {
    let mut opts = RenderEngineOptions::for_display(DISPLAY_WIDTH, DISPLAY_HEIGHT);
    opts.layout.margin_left = 10;
    opts.layout.margin_right = 10;
    opts.layout.margin_top = 10;
    opts.layout.margin_bottom = 24;
    opts.layout.first_line_indent_px = 0;
    opts.layout.line_gap_px = scenario.line_gap_px;
    opts.layout.paragraph_gap_px = scenario.paragraph_gap_px;
    opts.layout.typography.justification.enabled = scenario.justify;
    opts.layout.typography.justification.min_words = 6;
    opts.layout.typography.justification.min_fill_ratio = 0.78;
    opts.prep.layout_hints.base_font_size_px = scenario.base_font_size_px;
    opts.prep.layout_hints.text_scale = scenario.text_scale;
    opts.prep.layout_hints.min_font_size_px = 14.0;
    opts.prep.layout_hints.max_font_size_px = 72.0;
    opts.prep.layout_hints.min_line_height = 1.05;
    opts.prep.layout_hints.max_line_height = 1.40;
    opts.prep.memory = budget;
    opts.prep.style.hints = opts.prep.layout_hints;
    RenderEngine::new(opts)
}

fn bench_fixture_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("../../tests/fixtures/bench");
    root
}

fn available_bench_fixtures() -> Vec<PathBuf> {
    let root = bench_fixture_root();
    let preferred = [
        "pg84-frankenstein.epub",
        "pg1342-pride-and-prejudice.epub",
        "pg1661-sherlock-holmes.epub",
        "pg2701-moby-dick.epub",
    ];
    let mut fixtures = Vec::new();
    for fixture in preferred {
        let path = root.join(fixture);
        if path.exists() {
            fixtures.push(path);
        }
    }
    if !fixtures.is_empty() {
        return fixtures;
    }
    let Ok(entries) = std::fs::read_dir(&root) else {
        return fixtures;
    };
    let mut discovered: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
        })
        .collect();
    discovered.sort();
    discovered
}

fn measured_text_width_px(text: &str, style: &ResolvedTextStyle) -> f32 {
    let measurer = mu_epub_embedded_graphics::EgTextMeasurer::new();
    measurer.measure_text_px(text, style)
}

fn is_uppercase_heavy(text: &str) -> bool {
    let mut alpha = 0usize;
    let mut upper = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_alphabetic() {
            alpha += 1;
            if ch.is_ascii_uppercase() {
                upper += 1;
            }
        }
    }
    alpha >= 8 && (upper as f32 / alpha as f32) >= 0.75
}
