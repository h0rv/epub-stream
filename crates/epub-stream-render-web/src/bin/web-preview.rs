use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use epub_stream::{
    navigation::NavPoint, EmbeddedFontFace, EmbeddedFontStyle, EpubBook, Locator, Navigation,
    ReadingSession,
};
use epub_stream_render::{
    BlockRole, CoverPageMode, DitherMode, DrawCommand, FloatSupport, GrayscaleMode,
    HyphenationMode, JustificationStrategy, JustifyMode, PageChromeConfig, PageChromeKind,
    PageChromeTextStyle, RenderConfig, RenderEngine, RenderEngineOptions, RenderPage,
    SoftHyphenPolicy, SvgMode,
};
use serde::{Deserialize, Serialize};

const DEFAULT_EPUB_PATH: &str = "tests/fixtures/bench/pg84-frankenstein.epub";
const DEFAULT_OUT_PATH: &str = "target/web-preview/index.html";
const DEFAULT_PORT: u16 = 42817;

#[derive(Clone, Debug)]
struct Args {
    epub_path: String,
    out_path: String,
    serve: bool,
    open_browser: bool,
    port: u16,
    chapter: Option<usize>,
    start_page: usize,
    pages_per_chapter: Option<usize>,
    display_width: u32,
    display_height: u32,
    justify_enabled: bool,
    justify_strategy: String,
    cover_page_mode: String,
    prep_base_font_size_px: f32,
    prep_text_scale: f32,
    line_gap_px: i32,
    paragraph_gap_px: i32,
    margin_left: i32,
    margin_right: i32,
    margin_top: i32,
    margin_bottom: i32,
    justify_min_words: usize,
    justify_min_fill_ratio: f32,
    justify_max_space_stretch_ratio: f32,
    widow_orphan_enabled: bool,
    widow_orphan_min_lines: u8,
    hanging_punctuation_enabled: bool,
    max_image_bytes: usize,
    max_font_bytes: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct RenderUiConfig {
    chapter: Option<usize>,
    start_page: usize,
    pages_per_chapter: Option<usize>,

    display_width: u32,
    display_height: u32,

    margin_left: i32,
    margin_right: i32,
    margin_top: i32,
    margin_bottom: i32,
    line_gap_px: i32,
    paragraph_gap_px: i32,
    heading_gap_px: i32,
    heading_keep_with_next_lines: u8,
    list_indent_px: i32,
    first_line_indent_px: i32,
    suppress_indent_after_heading: bool,

    justify_enabled: bool,
    justify_strategy: String,
    justify_min_words: usize,
    justify_min_fill_ratio: f32,
    justify_max_space_stretch_ratio: f32,

    widow_orphan_enabled: bool,
    widow_orphan_min_lines: u8,
    hanging_punctuation_enabled: bool,

    soft_hyphen_policy: String,
    hyphenation_mode: String,

    min_line_height_px: i32,
    max_line_height_px: i32,

    object_max_inline_image_height_ratio: f32,
    object_cover_page_mode: String,
    object_float_support: String,
    object_svg_mode: String,
    object_alt_text_fallback: bool,

    render_grayscale_mode: String,
    render_dither_mode: String,
    render_contrast_boost: u8,

    page_chrome_header_enabled: bool,
    page_chrome_footer_enabled: bool,
    page_chrome_progress_enabled: bool,
    page_chrome_header_x: i32,
    page_chrome_header_baseline_y: i32,
    page_chrome_header_style: String,
    page_chrome_footer_x: i32,
    page_chrome_footer_baseline_from_bottom: i32,
    page_chrome_footer_style: String,
    page_chrome_progress_x_inset: i32,
    page_chrome_progress_y_from_bottom: i32,
    page_chrome_progress_height: u32,
    page_chrome_progress_stroke_width: u32,

    prep_base_font_size_px: f32,
    prep_min_font_size_px: f32,
    prep_max_font_size_px: f32,
    prep_min_line_height: f32,
    prep_max_line_height: f32,
    prep_text_scale: f32,

    embedded_fonts: bool,
    max_image_bytes: usize,
    max_font_bytes: usize,
    style_max_selectors: usize,
    style_max_css_bytes: usize,
    style_max_nesting: usize,
    font_limit_max_faces: usize,
    font_limit_max_bytes_per_font: usize,
    font_limit_max_total_font_bytes: usize,
    memory_max_entry_bytes: usize,
    memory_max_css_bytes: usize,
    memory_max_nav_bytes: usize,
    memory_max_inline_style_bytes: usize,
    memory_max_pages_in_memory: usize,

    ui_font_family: String,
    ui_font_scale: f32,
}

impl Default for RenderUiConfig {
    fn default() -> Self {
        Self {
            chapter: None,
            start_page: 0,
            pages_per_chapter: None,
            display_width: 900,
            display_height: 1200,
            margin_left: 24,
            margin_right: 24,
            margin_top: 18,
            margin_bottom: 30,
            line_gap_px: 4,
            paragraph_gap_px: 8,
            heading_gap_px: 10,
            heading_keep_with_next_lines: 2,
            list_indent_px: 12,
            first_line_indent_px: 0,
            suppress_indent_after_heading: true,
            justify_enabled: true,
            justify_strategy: "adaptive-inter-word".to_string(),
            justify_min_words: 6,
            justify_min_fill_ratio: 0.78,
            justify_max_space_stretch_ratio: 0.45,
            widow_orphan_enabled: true,
            widow_orphan_min_lines: 2,
            hanging_punctuation_enabled: true,
            soft_hyphen_policy: "discretionary".to_string(),
            hyphenation_mode: "discretionary".to_string(),
            min_line_height_px: 14,
            max_line_height_px: 56,
            object_max_inline_image_height_ratio: 0.5,
            object_cover_page_mode: "contain".to_string(),
            object_float_support: "none".to_string(),
            object_svg_mode: "rasterize-fallback".to_string(),
            object_alt_text_fallback: true,
            render_grayscale_mode: "off".to_string(),
            render_dither_mode: "none".to_string(),
            render_contrast_boost: 100,
            page_chrome_header_enabled: false,
            page_chrome_footer_enabled: false,
            page_chrome_progress_enabled: false,
            page_chrome_header_x: 8,
            page_chrome_header_baseline_y: 16,
            page_chrome_header_style: "bold".to_string(),
            page_chrome_footer_x: 8,
            page_chrome_footer_baseline_from_bottom: 8,
            page_chrome_footer_style: "regular".to_string(),
            page_chrome_progress_x_inset: 8,
            page_chrome_progress_y_from_bottom: 20,
            page_chrome_progress_height: 4,
            page_chrome_progress_stroke_width: 1,
            prep_base_font_size_px: 22.0,
            prep_min_font_size_px: 12.0,
            prep_max_font_size_px: 72.0,
            prep_min_line_height: 1.05,
            prep_max_line_height: 1.6,
            prep_text_scale: 1.0,
            embedded_fonts: true,
            max_image_bytes: 16 * 1024 * 1024,
            max_font_bytes: 24 * 1024 * 1024,
            style_max_selectors: 4096,
            style_max_css_bytes: 512 * 1024,
            style_max_nesting: 32,
            font_limit_max_faces: 64,
            font_limit_max_bytes_per_font: 8 * 1024 * 1024,
            font_limit_max_total_font_bytes: 64 * 1024 * 1024,
            memory_max_entry_bytes: 4 * 1024 * 1024,
            memory_max_css_bytes: 512 * 1024,
            memory_max_nav_bytes: 512 * 1024,
            memory_max_inline_style_bytes: 16 * 1024,
            memory_max_pages_in_memory: 128,
            ui_font_family: "auto".to_string(),
            ui_font_scale: 1.0,
        }
    }
}

impl RenderUiConfig {
    fn from_args(args: &Args) -> Self {
        Self {
            chapter: args.chapter,
            start_page: args.start_page,
            pages_per_chapter: args.pages_per_chapter,
            display_width: args.display_width,
            display_height: args.display_height,
            margin_left: args.margin_left,
            margin_right: args.margin_right,
            margin_top: args.margin_top,
            margin_bottom: args.margin_bottom,
            line_gap_px: args.line_gap_px,
            paragraph_gap_px: args.paragraph_gap_px,
            justify_enabled: args.justify_enabled,
            justify_strategy: args.justify_strategy.clone(),
            object_cover_page_mode: args.cover_page_mode.clone(),
            justify_min_words: args.justify_min_words,
            justify_min_fill_ratio: args.justify_min_fill_ratio,
            justify_max_space_stretch_ratio: args.justify_max_space_stretch_ratio,
            widow_orphan_enabled: args.widow_orphan_enabled,
            widow_orphan_min_lines: args.widow_orphan_min_lines,
            hanging_punctuation_enabled: args.hanging_punctuation_enabled,
            prep_base_font_size_px: args.prep_base_font_size_px,
            prep_text_scale: args.prep_text_scale,
            max_image_bytes: args.max_image_bytes,
            max_font_bytes: args.max_font_bytes,
            ..Self::default()
        }
    }

    fn normalized(mut self) -> Self {
        self.display_width = self.display_width.clamp(64, 4096);
        self.display_height = self.display_height.clamp(64, 4096);
        self.justify_min_words = self.justify_min_words.max(1);
        self.justify_min_fill_ratio = self.justify_min_fill_ratio.clamp(0.0, 1.0);
        self.justify_max_space_stretch_ratio = self.justify_max_space_stretch_ratio.clamp(0.0, 8.0);
        self.justify_strategy =
            justification_strategy_to_string(parse_justification_strategy(&self.justify_strategy))
                .to_string();
        self.widow_orphan_min_lines = self.widow_orphan_min_lines.max(1);
        self.heading_keep_with_next_lines = self.heading_keep_with_next_lines.max(1);
        self.prep_base_font_size_px = self.prep_base_font_size_px.clamp(1.0, 240.0);
        self.prep_min_font_size_px = self.prep_min_font_size_px.clamp(1.0, 240.0);
        self.prep_max_font_size_px = self.prep_max_font_size_px.clamp(1.0, 320.0);
        if self.prep_min_font_size_px > self.prep_max_font_size_px {
            std::mem::swap(
                &mut self.prep_min_font_size_px,
                &mut self.prep_max_font_size_px,
            );
        }
        self.prep_text_scale = self.prep_text_scale.clamp(0.25, 4.0);
        self.prep_min_line_height = self.prep_min_line_height.clamp(0.5, 4.0);
        self.prep_max_line_height = self.prep_max_line_height.clamp(0.5, 4.0);
        if self.prep_min_line_height > self.prep_max_line_height {
            std::mem::swap(
                &mut self.prep_min_line_height,
                &mut self.prep_max_line_height,
            );
        }
        self.min_line_height_px = self.min_line_height_px.clamp(4, 512);
        self.max_line_height_px = self.max_line_height_px.clamp(4, 768);
        if self.min_line_height_px > self.max_line_height_px {
            std::mem::swap(&mut self.min_line_height_px, &mut self.max_line_height_px);
        }
        self.object_max_inline_image_height_ratio =
            self.object_max_inline_image_height_ratio.clamp(0.05, 4.0);
        self.object_cover_page_mode =
            cover_page_mode_to_string(parse_cover_page_mode(&self.object_cover_page_mode))
                .to_string();
        self.render_contrast_boost = self.render_contrast_boost.clamp(10, 255);
        self.page_chrome_progress_height = self.page_chrome_progress_height.max(1);
        self.page_chrome_progress_stroke_width = self.page_chrome_progress_stroke_width.max(1);
        self.max_image_bytes = self.max_image_bytes.max(1024);
        self.max_font_bytes = self.max_font_bytes.max(1024);
        self.style_max_selectors = self.style_max_selectors.max(1);
        self.style_max_css_bytes = self.style_max_css_bytes.max(1024);
        self.style_max_nesting = self.style_max_nesting.max(1);
        self.font_limit_max_faces = self.font_limit_max_faces.max(1);
        self.font_limit_max_bytes_per_font = self.font_limit_max_bytes_per_font.max(1024);
        self.font_limit_max_total_font_bytes = self.font_limit_max_total_font_bytes.max(1024);
        self.memory_max_entry_bytes = self.memory_max_entry_bytes.max(1024);
        self.memory_max_css_bytes = self.memory_max_css_bytes.max(1024);
        self.memory_max_nav_bytes = self.memory_max_nav_bytes.max(1024);
        self.memory_max_inline_style_bytes = self.memory_max_inline_style_bytes.max(256);
        self.memory_max_pages_in_memory = self.memory_max_pages_in_memory.max(1);
        self.ui_font_scale = self.ui_font_scale.clamp(0.25, 4.0);
        self.ui_font_family = if self.ui_font_family.trim().is_empty() {
            "auto".to_string()
        } else {
            self.ui_font_family.trim().to_string()
        };
        self
    }

    fn to_engine_options(&self) -> RenderEngineOptions {
        let mut opts =
            RenderEngineOptions::for_display(self.display_width as i32, self.display_height as i32);

        let layout = &mut opts.layout;
        layout.display_width = self.display_width as i32;
        layout.display_height = self.display_height as i32;
        layout.margin_left = self.margin_left;
        layout.margin_right = self.margin_right;
        layout.margin_top = self.margin_top;
        layout.margin_bottom = self.margin_bottom;
        layout.line_gap_px = self.line_gap_px;
        layout.paragraph_gap_px = self.paragraph_gap_px;
        layout.heading_gap_px = self.heading_gap_px;
        layout.heading_keep_with_next_lines = self.heading_keep_with_next_lines;
        layout.list_indent_px = self.list_indent_px;
        layout.first_line_indent_px = self.first_line_indent_px;
        layout.suppress_indent_after_heading = self.suppress_indent_after_heading;
        layout.justify_min_words = self.justify_min_words;
        layout.justify_min_fill_ratio = self.justify_min_fill_ratio;
        layout.min_line_height_px = self.min_line_height_px;
        layout.max_line_height_px = self.max_line_height_px;
        layout.soft_hyphen_policy = parse_soft_hyphen_policy(&self.soft_hyphen_policy);

        layout.typography.hyphenation.soft_hyphen_policy =
            parse_hyphenation_mode(&self.hyphenation_mode);
        layout.typography.widow_orphan_control.enabled = self.widow_orphan_enabled;
        layout.typography.widow_orphan_control.min_lines = self.widow_orphan_min_lines;
        layout.typography.justification.enabled = self.justify_enabled;
        layout.typography.justification.strategy =
            parse_justification_strategy(&self.justify_strategy);
        layout.typography.justification.min_words = self.justify_min_words;
        layout.typography.justification.min_fill_ratio = self.justify_min_fill_ratio;
        layout.typography.justification.max_space_stretch_ratio =
            self.justify_max_space_stretch_ratio;
        layout.typography.hanging_punctuation.enabled = self.hanging_punctuation_enabled;

        layout.object_layout.max_inline_image_height_ratio =
            self.object_max_inline_image_height_ratio;
        layout.object_layout.cover_page_mode = parse_cover_page_mode(&self.object_cover_page_mode);
        layout.object_layout.float_support = parse_float_support(&self.object_float_support);
        layout.object_layout.svg_mode = parse_svg_mode(&self.object_svg_mode);
        layout.object_layout.alt_text_fallback = self.object_alt_text_fallback;

        layout.render_intent.grayscale_mode = parse_grayscale_mode(&self.render_grayscale_mode);
        layout.render_intent.dither = parse_dither_mode(&self.render_dither_mode);
        layout.render_intent.contrast_boost = self.render_contrast_boost;

        layout.page_chrome = PageChromeConfig {
            header_enabled: self.page_chrome_header_enabled,
            footer_enabled: self.page_chrome_footer_enabled,
            progress_enabled: self.page_chrome_progress_enabled,
            header_x: self.page_chrome_header_x,
            header_baseline_y: self.page_chrome_header_baseline_y,
            header_style: parse_chrome_text_style(&self.page_chrome_header_style),
            footer_x: self.page_chrome_footer_x,
            footer_baseline_from_bottom: self.page_chrome_footer_baseline_from_bottom,
            footer_style: parse_chrome_text_style(&self.page_chrome_footer_style),
            progress_x_inset: self.page_chrome_progress_x_inset,
            progress_y_from_bottom: self.page_chrome_progress_y_from_bottom,
            progress_height: self.page_chrome_progress_height,
            progress_stroke_width: self.page_chrome_progress_stroke_width,
        };

        opts.prep.layout_hints.base_font_size_px = self.prep_base_font_size_px;
        opts.prep.layout_hints.min_font_size_px = self.prep_min_font_size_px;
        opts.prep.layout_hints.max_font_size_px = self.prep_max_font_size_px;
        opts.prep.layout_hints.min_line_height = self.prep_min_line_height;
        opts.prep.layout_hints.max_line_height = self.prep_max_line_height;
        let effective_text_scale = (self.prep_text_scale * self.ui_font_scale).clamp(0.25, 8.0);
        opts.prep.layout_hints.text_scale = effective_text_scale;
        opts.prep.style.limits.max_selectors = self.style_max_selectors;
        opts.prep.style.limits.max_css_bytes = self.style_max_css_bytes;
        opts.prep.style.limits.max_nesting = self.style_max_nesting;
        opts.prep.style.hints = opts.prep.layout_hints;
        opts.prep.fonts.max_faces = self.font_limit_max_faces;
        opts.prep.fonts.max_bytes_per_font = self.font_limit_max_bytes_per_font;
        opts.prep.fonts.max_total_font_bytes = self.font_limit_max_total_font_bytes;
        opts.prep.memory.max_entry_bytes = self.memory_max_entry_bytes;
        opts.prep.memory.max_css_bytes = self.memory_max_css_bytes;
        opts.prep.memory.max_nav_bytes = self.memory_max_nav_bytes;
        opts.prep.memory.max_inline_style_bytes = self.memory_max_inline_style_bytes;
        opts.prep.memory.max_pages_in_memory = self.memory_max_pages_in_memory;

        opts
    }
}

#[derive(Serialize)]
struct PreviewPayload {
    meta: PreviewMeta,
    chapters: Vec<ChapterSummary>,
    pages: Vec<PagePayload>,
    toc: Vec<TocEntryPayload>,
    images: BTreeMap<String, String>,
    fonts: Vec<FontFacePayload>,
    style_families: Vec<String>,
    warnings: Vec<String>,
    config: RenderUiConfig,
    option_lists: OptionLists,
}

#[derive(Serialize)]
struct PreviewMeta {
    title: String,
    author: String,
    language: String,
    viewport: Viewport,
    chapter_count: usize,
    page_count: usize,
}

#[derive(Serialize)]
struct Viewport {
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct ChapterSummary {
    index: usize,
    idref: String,
    href: String,
    title: Option<String>,
    media_type: String,
}

#[derive(Serialize)]
struct PagePayload {
    id: String,
    chapter_index: usize,
    chapter_href: String,
    page_index: usize,
    page_number: usize,
    metrics: MetricsPayload,
    commands: Vec<CommandPayload>,
    annotations: Vec<AnnotationPayload>,
}

#[derive(Serialize)]
struct MetricsPayload {
    chapter_index: usize,
    chapter_page_index: usize,
    chapter_page_count: Option<usize>,
    global_page_index: Option<usize>,
    global_page_count_estimate: Option<usize>,
    progress_chapter: f32,
    progress_book: Option<f32>,
}

#[derive(Serialize)]
struct AnnotationPayload {
    kind: String,
    value: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CommandPayload {
    Text {
        x: i32,
        baseline_y: i32,
        text: String,
        style: TextStylePayload,
        font_id: Option<u32>,
    },
    Rule {
        x: i32,
        y: i32,
        length: u32,
        thickness: u32,
        horizontal: bool,
    },
    ImageObject {
        src: String,
        alt: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    Rect {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        fill: bool,
    },
    PageChrome {
        #[serde(rename = "kind_detail")]
        kind: String,
        text: Option<String>,
        current: Option<usize>,
        total: Option<usize>,
    },
}

#[derive(Serialize)]
struct TextStylePayload {
    family: String,
    weight: u16,
    italic: bool,
    size_px: f32,
    line_height: f32,
    letter_spacing: f32,
    role: String,
    font_id: Option<u32>,
    justify: JustifyPayload,
}

#[derive(Serialize)]
struct JustifyPayload {
    mode: String,
    extra_px_total: Option<i32>,
    offset_px: Option<i32>,
}

#[derive(Serialize)]
struct TocEntryPayload {
    label: String,
    href: String,
    depth: usize,
    target_chapter: Option<usize>,
    target_page_global: Option<usize>,
    fragment: Option<String>,
}

#[derive(Serialize)]
struct FontFacePayload {
    family: String,
    weight: u16,
    style: String,
    href: String,
    format: Option<String>,
    data_uri: String,
}

#[derive(Serialize)]
struct OptionLists {
    justification_strategy: Vec<&'static str>,
    cover_page_mode: Vec<&'static str>,
    soft_hyphen_policy: Vec<&'static str>,
    hyphenation_mode: Vec<&'static str>,
    object_float_support: Vec<&'static str>,
    object_svg_mode: Vec<&'static str>,
    render_grayscale_mode: Vec<&'static str>,
    render_dither_mode: Vec<&'static str>,
    chrome_text_style: Vec<&'static str>,
}

impl Default for OptionLists {
    fn default() -> Self {
        Self {
            justification_strategy: vec![
                "adaptive-inter-word",
                "full-inter-word",
                "align-left",
                "align-right",
                "align-center",
            ],
            cover_page_mode: vec!["contain", "full-bleed", "respect-css"],
            soft_hyphen_policy: vec!["discretionary", "ignore"],
            hyphenation_mode: vec!["discretionary", "ignore"],
            object_float_support: vec!["none", "basic"],
            object_svg_mode: vec!["ignore", "rasterize-fallback", "native"],
            render_grayscale_mode: vec!["off", "luminosity"],
            render_dither_mode: vec!["none", "ordered", "error-diffusion"],
            chrome_text_style: vec!["regular", "bold", "italic", "bold-italic"],
        }
    }
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn main() -> ExitCode {
    match run(env::args().collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {}", msg);
            eprintln!("{}", help_text());
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let cli = parse_args(args)?;
    let initial_cfg = RenderUiConfig::from_args(&cli).normalized();

    if cli.serve {
        return run_server(&cli.epub_path, initial_cfg, cli.port, cli.open_browser);
    }

    if cli.out_path.is_empty() {
        return Err("--out must not be empty".to_string());
    }
    if let Some(parent) = Path::new(&cli.out_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }

    let payload = render_preview_payload(&cli.epub_path, &initial_cfg)?;
    let data_json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let html = build_html(&data_json, false);
    std::fs::write(&cli.out_path, html).map_err(|e| e.to_string())?;

    println!(
        "wrote web preview to {} (pages={}, toc_entries={}, images={}, fonts={})",
        cli.out_path,
        payload.meta.page_count,
        payload.toc.len(),
        payload.images.len(),
        payload.fonts.len(),
    );
    Ok(())
}

fn run_server(
    epub_path: &str,
    initial_cfg: RenderUiConfig,
    port: u16,
    open_browser: bool,
) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(false).map_err(|e| e.to_string())?;

    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let initial_payload = render_preview_payload(epub_path, &initial_cfg)?;
    let initial_json = serde_json::to_string(&initial_payload).map_err(|e| e.to_string())?;
    let html = build_html(&initial_json, true);
    let url = format!("http://{}:{}/", addr.ip(), addr.port());

    println!("serving web preview at {}", url);

    if open_browser {
        try_open_browser(&url);
    }

    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("accept error: {}", err);
                continue;
            }
        };
        if let Err(err) = stream.set_read_timeout(Some(Duration::from_secs(15))) {
            eprintln!("set timeout failed: {}", err);
        }
        if let Err(err) = handle_connection(&mut stream, epub_path, &initial_cfg, &html) {
            eprintln!("request error: {}", err);
        }
    }

    Ok(())
}

fn try_open_browser(url: &str) {
    let mut opened = false;

    if let Ok(status) = Command::new("open").arg(url).status() {
        if status.success() {
            opened = true;
        }
    }

    if !opened {
        let _ = Command::new("xdg-open").arg(url).status();
    }
}

fn handle_connection(
    stream: &mut TcpStream,
    epub_path: &str,
    initial_cfg: &RenderUiConfig,
    html: &str,
) -> Result<(), String> {
    let req = read_http_request(stream)?;
    let path = req.path.split('?').next().unwrap_or(&req.path);

    match (req.method.as_str(), path) {
        ("GET", "/") => write_http_response(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            html.as_bytes(),
        ),
        ("GET", "/api/default-config") => {
            let body = serde_json::to_vec(initial_cfg).map_err(|e| e.to_string())?;
            write_http_response(stream, "200 OK", "application/json", &body)
        }
        ("POST", "/api/render") => {
            let parsed = serde_json::from_slice::<RenderUiConfig>(&req.body)
                .map_err(|e| format!("invalid render config: {}", e))?;
            let payload = render_preview_payload(epub_path, &parsed.normalized())?;
            let body = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
            write_http_response(stream, "200 OK", "application/json", &body)
        }
        ("GET", "/favicon.ico") => write_http_response(stream, "204 No Content", "text/plain", &[]),
        _ => write_http_response(
            stream,
            "404 Not Found",
            "application/json",
            br#"{"error":"not_found"}"#,
        ),
    }
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 1024];
    let mut header_end = None;

    while header_end.is_none() {
        let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(idx) = find_header_end(&buf) {
            header_end = Some(idx);
            break;
        }
        if buf.len() > 1024 * 1024 {
            return Err("request header too large".to_string());
        }
    }

    let header_end = header_end.ok_or_else(|| "incomplete http request".to_string())?;
    let header = &buf[..header_end];
    let header_text = String::from_utf8_lossy(header);
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;

    let mut req_parts = request_line.split_whitespace();
    let method = req_parts
        .next()
        .ok_or_else(|| "missing method".to_string())?
        .to_string();
    let path = req_parts
        .next()
        .ok_or_else(|| "missing path".to_string())?
        .to_string();

    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }

    let mut body = Vec::with_capacity(content_length);
    if buf.len() > header_end + 4 {
        body.extend_from_slice(&buf[(header_end + 4)..]);
    }

    while body.len() < content_length {
        let n = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }

    if body.len() > content_length {
        body.truncate(content_length);
    }

    Ok(HttpRequest { method, path, body })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    for idx in 0..=(buf.len() - 4) {
        if &buf[idx..idx + 4] == b"\r\n\r\n" {
            return Some(idx);
        }
    }
    None
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status,
        content_type,
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())
}

fn render_preview_payload(epub_path: &str, cfg: &RenderUiConfig) -> Result<PreviewPayload, String> {
    let cfg = cfg.clone().normalized();

    let mut book = EpubBook::open(epub_path).map_err(|e| e.to_string())?;
    let chapter_refs: Vec<_> = book.chapters().collect();
    if chapter_refs.is_empty() {
        return Err("EPUB has no chapters".to_string());
    }

    if let Some(chapter) = cfg.chapter {
        if chapter >= chapter_refs.len() {
            return Err(format!(
                "chapter {} out of range (chapter_count={})",
                chapter,
                chapter_refs.len()
            ));
        }
    }

    let opts = cfg.to_engine_options();
    let engine = RenderEngine::new(opts);

    let chapters_to_render: Vec<usize> = match cfg.chapter {
        Some(chapter) => vec![chapter],
        None => (0..chapter_refs.len()).collect(),
    };

    let mut pages = Vec::with_capacity(256);
    let mut image_sources = BTreeSet::new();
    let mut font_families = BTreeSet::new();
    let mut chapter_first_global_page = HashMap::new();

    for chapter_index in chapters_to_render {
        let mut render_cfg = RenderConfig::default().with_embedded_fonts(cfg.embedded_fonts);
        if cfg.ui_font_family != "auto" {
            render_cfg = render_cfg.with_forced_font_family(cfg.ui_font_family.clone());
        }
        let rendered = engine
            .prepare_chapter_with_config_collect(&mut book, chapter_index, render_cfg)
            .map_err(|e| format!("render failed for chapter {}: {}", chapter_index, e))?;

        let chapter_start = cfg.start_page.min(rendered.len());
        let chapter_limit = cfg.pages_per_chapter.unwrap_or(usize::MAX);

        if chapter_start < rendered.len() {
            chapter_first_global_page
                .entry(chapter_index)
                .or_insert_with(|| pages.len());
        }

        for page in rendered.into_iter().skip(chapter_start).take(chapter_limit) {
            let chapter_href = chapter_refs
                .get(chapter_index)
                .map(|c| c.href.clone())
                .unwrap_or_default();
            pages.push(convert_page(
                &page,
                chapter_index,
                chapter_href,
                &mut image_sources,
                &mut font_families,
            ));
        }
    }

    if pages.is_empty() {
        return Err("no pages rendered for requested chapter/page range".to_string());
    }

    let mut warnings = Vec::with_capacity(0);
    let images = load_image_data_uris(
        &mut book,
        &image_sources,
        cfg.max_image_bytes,
        &mut warnings,
    );
    let fonts = if cfg.embedded_fonts {
        load_embedded_fonts(&mut book, cfg.max_font_bytes, &mut warnings)
    } else {
        warnings.push("embedded font loading disabled by configuration".to_string());
        Vec::with_capacity(0)
    };

    let navigation = book
        .ensure_navigation()
        .map_err(|e| e.to_string())?
        .cloned();

    let mut reading_session = book.reading_session();
    let toc = build_toc_payload(
        navigation.as_ref(),
        &mut reading_session,
        &chapter_refs,
        &chapter_first_global_page,
    );
    let chapter_titles = build_chapter_title_lookup(&toc, chapter_refs.len());

    Ok(PreviewPayload {
        meta: PreviewMeta {
            title: book.title().to_string(),
            author: book.author().to_string(),
            language: book.language().to_string(),
            viewport: Viewport {
                width: cfg.display_width,
                height: cfg.display_height,
            },
            chapter_count: chapter_refs.len(),
            page_count: pages.len(),
        },
        chapters: chapter_refs
            .iter()
            .map(|c| ChapterSummary {
                index: c.index,
                idref: c.idref.clone(),
                href: c.href.clone(),
                title: chapter_titles.get(c.index).cloned().flatten(),
                media_type: c.media_type.clone(),
            })
            .collect(),
        pages,
        toc,
        images,
        fonts,
        style_families: font_families.into_iter().collect(),
        warnings,
        config: cfg,
        option_lists: OptionLists::default(),
    })
}

fn convert_page(
    page: &RenderPage,
    chapter_index: usize,
    chapter_href: String,
    image_sources: &mut BTreeSet<String>,
    font_families: &mut BTreeSet<String>,
) -> PagePayload {
    let commands = merged_commands(page);
    let mut converted = Vec::with_capacity(commands.len());

    for cmd in commands {
        converted.push(convert_command(&cmd, image_sources, font_families));
    }

    PagePayload {
        id: format!("{}:{}", chapter_index, page.metrics.chapter_page_index),
        chapter_index,
        chapter_href,
        page_index: page.metrics.chapter_page_index,
        page_number: page.page_number,
        metrics: MetricsPayload {
            chapter_index: page.metrics.chapter_index,
            chapter_page_index: page.metrics.chapter_page_index,
            chapter_page_count: page.metrics.chapter_page_count,
            global_page_index: page.metrics.global_page_index,
            global_page_count_estimate: page.metrics.global_page_count_estimate,
            progress_chapter: page.metrics.progress_chapter,
            progress_book: page.metrics.progress_book,
        },
        commands: converted,
        annotations: page
            .annotations
            .iter()
            .map(|annotation| AnnotationPayload {
                kind: annotation.kind.clone(),
                value: annotation.value.clone(),
            })
            .collect(),
    }
}

fn merged_commands(page: &RenderPage) -> Vec<DrawCommand> {
    let mut merged = Vec::with_capacity(
        page.content_commands.len() + page.chrome_commands.len() + page.overlay_commands.len(),
    );
    merged.extend(page.content_commands.iter().cloned());
    merged.extend(page.chrome_commands.iter().cloned());
    merged.extend(page.overlay_commands.iter().cloned());
    if merged.is_empty() {
        merged.extend(page.commands.iter().cloned());
    }
    merged
}

fn convert_command(
    cmd: &DrawCommand,
    image_sources: &mut BTreeSet<String>,
    font_families: &mut BTreeSet<String>,
) -> CommandPayload {
    match cmd {
        DrawCommand::Text(text) => {
            font_families.insert(text.style.family.clone());
            CommandPayload::Text {
                x: text.x,
                baseline_y: text.baseline_y,
                text: text.text.clone(),
                style: TextStylePayload {
                    family: text.style.family.clone(),
                    weight: text.style.weight,
                    italic: text.style.italic,
                    size_px: text.style.size_px,
                    line_height: text.style.line_height,
                    letter_spacing: text.style.letter_spacing,
                    role: block_role_to_string(text.style.role),
                    font_id: text.style.font_id,
                    justify: justify_mode_to_payload(text.style.justify_mode),
                },
                font_id: text.font_id,
            }
        }
        DrawCommand::Rule(rule) => CommandPayload::Rule {
            x: rule.x,
            y: rule.y,
            length: rule.length,
            thickness: rule.thickness,
            horizontal: rule.horizontal,
        },
        DrawCommand::ImageObject(image) => {
            if !image.src.is_empty() {
                image_sources.insert(image.src.clone());
            }
            CommandPayload::ImageObject {
                src: image.src.clone(),
                alt: image.alt.clone(),
                x: image.x,
                y: image.y,
                width: image.width,
                height: image.height,
            }
        }
        DrawCommand::Rect(rect) => CommandPayload::Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            fill: rect.fill,
        },
        DrawCommand::PageChrome(chrome) => CommandPayload::PageChrome {
            kind: page_chrome_kind_to_string(chrome.kind),
            text: chrome.text.clone(),
            current: chrome.current,
            total: chrome.total,
        },
    }
}

fn load_image_data_uris<R: std::io::Read + std::io::Seek>(
    book: &mut EpubBook<R>,
    image_sources: &BTreeSet<String>,
    max_image_bytes: usize,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();

    for src in image_sources {
        match book.read_resource(src) {
            Ok(bytes) => {
                if bytes.len() > max_image_bytes {
                    warnings.push(format!(
                        "skipped image '{}' because it exceeded max bytes ({} > {})",
                        src,
                        bytes.len(),
                        max_image_bytes
                    ));
                    continue;
                }
                let mime = mime_from_path(src).unwrap_or("application/octet-stream");
                out.insert(
                    src.clone(),
                    format!("data:{};base64,{}", mime, BASE64.encode(bytes)),
                );
            }
            Err(err) => warnings.push(format!("unable to read image '{}': {}", src, err)),
        }
    }

    out
}

fn load_embedded_fonts<R: std::io::Read + std::io::Seek>(
    book: &mut EpubBook<R>,
    max_font_bytes: usize,
    warnings: &mut Vec<String>,
) -> Vec<FontFacePayload> {
    let mut out = Vec::with_capacity(0);
    let mut seen = BTreeSet::new();

    let faces = match book.embedded_fonts() {
        Ok(faces) => faces,
        Err(err) => {
            warnings.push(format!("unable to enumerate embedded fonts: {}", err));
            return out;
        }
    };

    for face in faces {
        let dedupe = format!(
            "{}|{}|{}|{}",
            face.family,
            face.weight,
            embedded_style_to_css(face.style),
            face.href
        );
        if !seen.insert(dedupe) {
            continue;
        }
        if let Some(payload) = load_font_face(book, &face, max_font_bytes, warnings) {
            out.push(payload);
        }
    }

    out
}

fn load_font_face<R: std::io::Read + std::io::Seek>(
    book: &mut EpubBook<R>,
    face: &EmbeddedFontFace,
    max_font_bytes: usize,
    warnings: &mut Vec<String>,
) -> Option<FontFacePayload> {
    let bytes = match book.read_resource(&face.href) {
        Ok(bytes) => bytes,
        Err(err) => {
            warnings.push(format!(
                "unable to load embedded font '{}': {}",
                face.href, err
            ));
            return None;
        }
    };

    if bytes.len() > max_font_bytes {
        warnings.push(format!(
            "skipped font '{}' because it exceeded max bytes ({} > {})",
            face.href,
            bytes.len(),
            max_font_bytes
        ));
        return None;
    }

    let format_hint = face
        .format
        .clone()
        .or_else(|| font_format_from_path(&face.href).map(ToString::to_string));
    let mime = font_mime_type(format_hint.as_deref(), &face.href)
        .unwrap_or("application/octet-stream")
        .to_string();

    Some(FontFacePayload {
        family: face.family.clone(),
        weight: face.weight,
        style: embedded_style_to_css(face.style).to_string(),
        href: face.href.clone(),
        format: format_hint,
        data_uri: format!("data:{};base64,{}", mime, BASE64.encode(bytes)),
    })
}

fn build_toc_payload(
    navigation: Option<&Navigation>,
    reading_session: &mut ReadingSession,
    chapters: &[epub_stream::ChapterRef],
    chapter_first_global_page: &HashMap<usize, usize>,
) -> Vec<TocEntryPayload> {
    let Some(navigation) = navigation else {
        return Vec::with_capacity(0);
    };

    let chapter_lookup = build_chapter_lookup(chapters);
    let basename_lookup = build_chapter_basename_lookup(chapters);
    let mut out = Vec::with_capacity(0);

    flatten_toc(
        &navigation.toc,
        0,
        reading_session,
        &chapter_lookup,
        &basename_lookup,
        chapter_first_global_page,
        &mut out,
    );
    out
}

fn flatten_toc(
    points: &[NavPoint],
    depth: usize,
    reading_session: &mut ReadingSession,
    chapter_lookup: &HashMap<String, usize>,
    basename_lookup: &HashMap<String, usize>,
    chapter_first_global_page: &HashMap<usize, usize>,
    out: &mut Vec<TocEntryPayload>,
) {
    for point in points {
        let (base_href, fragment) = split_href_fragment(&point.href);
        let target_chapter = resolve_chapter_index(
            reading_session,
            &point.href,
            &base_href,
            chapter_lookup,
            basename_lookup,
        );
        let target_page_global = target_chapter
            .and_then(|chapter_index| chapter_first_global_page.get(&chapter_index).copied());

        out.push(TocEntryPayload {
            label: point.label.clone(),
            href: point.href.clone(),
            depth,
            target_chapter,
            target_page_global,
            fragment,
        });

        flatten_toc(
            &point.children,
            depth + 1,
            reading_session,
            chapter_lookup,
            basename_lookup,
            chapter_first_global_page,
            out,
        );
    }
}

fn build_chapter_title_lookup(
    toc: &[TocEntryPayload],
    chapter_count: usize,
) -> Vec<Option<String>> {
    let mut best: Vec<Option<(usize, String)>> = vec![None; chapter_count];

    for entry in toc {
        let Some(chapter_index) = entry.target_chapter else {
            continue;
        };
        if chapter_index >= chapter_count {
            continue;
        }
        let label = entry.label.trim();
        if label.is_empty() {
            continue;
        }

        let should_replace = match &best[chapter_index] {
            None => true,
            Some((best_depth, best_label)) => {
                entry.depth < *best_depth
                    || (entry.depth == *best_depth && label.len() < best_label.len())
            }
        };

        if should_replace {
            best[chapter_index] = Some((entry.depth, label.to_string()));
        }
    }

    best.into_iter()
        .map(|item| item.map(|(_, label)| label))
        .collect()
}

fn resolve_chapter_index(
    reading_session: &mut ReadingSession,
    href: &str,
    base_href: &str,
    chapter_lookup: &HashMap<String, usize>,
    basename_lookup: &HashMap<String, usize>,
) -> Option<usize> {
    if let Ok(resolved) = reading_session.resolve_locator(Locator::Href(href.to_string())) {
        return Some(resolved.chapter.index);
    }

    let normalized = normalize_rel_path(base_href);
    if let Some(index) = chapter_lookup.get(&normalized) {
        return Some(*index);
    }

    let basename = basename_of(&normalized);
    basename_lookup.get(&basename).copied()
}

fn build_chapter_lookup(chapters: &[epub_stream::ChapterRef]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for chapter in chapters {
        out.insert(chapter.href.clone(), chapter.index);
        out.insert(normalize_rel_path(&chapter.href), chapter.index);
    }
    out
}

fn build_chapter_basename_lookup(chapters: &[epub_stream::ChapterRef]) -> HashMap<String, usize> {
    let mut counts = HashMap::<String, usize>::new();
    for chapter in chapters {
        let base = basename_of(&normalize_rel_path(&chapter.href));
        *counts.entry(base).or_insert(0) += 1;
    }

    let mut out = HashMap::new();
    for chapter in chapters {
        let base = basename_of(&normalize_rel_path(&chapter.href));
        if counts.get(&base).copied().unwrap_or(0) == 1 {
            out.insert(base, chapter.index);
        }
    }
    out
}

fn split_href_fragment(href: &str) -> (String, Option<String>) {
    match href.split_once('#') {
        Some((base, fragment)) => (base.to_string(), Some(fragment.to_string())),
        None => (href.to_string(), None),
    }
}

fn normalize_rel_path(path: &str) -> String {
    let mut normalized_parts: Vec<&str> = Vec::with_capacity(0);
    let base = path
        .split_once('?')
        .map_or(path, |(without_query, _)| without_query);
    let slash_normalized = base.replace('\\', "/");

    for part in slash_normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let _ = normalized_parts.pop();
            continue;
        }
        normalized_parts.push(part);
    }

    normalized_parts.join("/")
}

fn basename_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn parse_soft_hyphen_policy(value: &str) -> SoftHyphenPolicy {
    match value.trim().to_ascii_lowercase().as_str() {
        "ignore" => SoftHyphenPolicy::Ignore,
        _ => SoftHyphenPolicy::Discretionary,
    }
}

fn parse_hyphenation_mode(value: &str) -> HyphenationMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "ignore" => HyphenationMode::Ignore,
        _ => HyphenationMode::Discretionary,
    }
}

fn parse_cover_page_mode(value: &str) -> CoverPageMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "full-bleed" | "full_bleed" | "fullbleed" => CoverPageMode::FullBleed,
        "respect-css" | "respect_css" | "respectcss" | "css" => CoverPageMode::RespectCss,
        _ => CoverPageMode::Contain,
    }
}

fn cover_page_mode_to_string(mode: CoverPageMode) -> &'static str {
    match mode {
        CoverPageMode::Contain => "contain",
        CoverPageMode::FullBleed => "full-bleed",
        CoverPageMode::RespectCss => "respect-css",
    }
}

fn parse_float_support(value: &str) -> FloatSupport {
    match value.trim().to_ascii_lowercase().as_str() {
        "basic" => FloatSupport::Basic,
        _ => FloatSupport::None,
    }
}

fn parse_svg_mode(value: &str) -> SvgMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "ignore" => SvgMode::Ignore,
        "native" => SvgMode::Native,
        _ => SvgMode::RasterizeFallback,
    }
}

fn parse_grayscale_mode(value: &str) -> GrayscaleMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "luminosity" => GrayscaleMode::Luminosity,
        _ => GrayscaleMode::Off,
    }
}

fn parse_dither_mode(value: &str) -> DitherMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "ordered" => DitherMode::Ordered,
        "error-diffusion" => DitherMode::ErrorDiffusion,
        _ => DitherMode::None,
    }
}

fn parse_chrome_text_style(value: &str) -> PageChromeTextStyle {
    match value.trim().to_ascii_lowercase().as_str() {
        "bold" => PageChromeTextStyle::Bold,
        "italic" => PageChromeTextStyle::Italic,
        "bold-italic" => PageChromeTextStyle::BoldItalic,
        _ => PageChromeTextStyle::Regular,
    }
}

fn page_chrome_kind_to_string(kind: PageChromeKind) -> String {
    match kind {
        PageChromeKind::Header => "header".to_string(),
        PageChromeKind::Footer => "footer".to_string(),
        PageChromeKind::Progress => "progress".to_string(),
    }
}

fn block_role_to_string(role: BlockRole) -> String {
    match role {
        BlockRole::Body => "body".to_string(),
        BlockRole::Paragraph => "paragraph".to_string(),
        BlockRole::Heading(level) => format!("heading-{}", level),
        BlockRole::ListItem => "list-item".to_string(),
        BlockRole::FigureCaption => "figure-caption".to_string(),
    }
}

fn parse_justification_strategy(value: &str) -> JustificationStrategy {
    match value.trim().to_ascii_lowercase().as_str() {
        "full-inter-word" | "full" => JustificationStrategy::FullInterWord,
        "align-left" | "left" => JustificationStrategy::AlignLeft,
        "align-right" | "right" => JustificationStrategy::AlignRight,
        "align-center" | "center" | "centre" => JustificationStrategy::AlignCenter,
        _ => JustificationStrategy::AdaptiveInterWord,
    }
}

fn justification_strategy_to_string(strategy: JustificationStrategy) -> &'static str {
    match strategy {
        JustificationStrategy::AdaptiveInterWord => "adaptive-inter-word",
        JustificationStrategy::FullInterWord => "full-inter-word",
        JustificationStrategy::AlignLeft => "align-left",
        JustificationStrategy::AlignRight => "align-right",
        JustificationStrategy::AlignCenter => "align-center",
    }
}

fn justify_mode_to_payload(mode: JustifyMode) -> JustifyPayload {
    match mode {
        JustifyMode::None => JustifyPayload {
            mode: "none".to_string(),
            extra_px_total: None,
            offset_px: None,
        },
        JustifyMode::InterWord { extra_px_total } => JustifyPayload {
            mode: "inter-word".to_string(),
            extra_px_total: Some(extra_px_total),
            offset_px: None,
        },
        JustifyMode::AlignRight { offset_px } => JustifyPayload {
            mode: "align-right".to_string(),
            extra_px_total: None,
            offset_px: Some(offset_px),
        },
        JustifyMode::AlignCenter { offset_px } => JustifyPayload {
            mode: "align-center".to_string(),
            extra_px_total: None,
            offset_px: Some(offset_px),
        },
    }
}

fn embedded_style_to_css(style: EmbeddedFontStyle) -> &'static str {
    match style {
        EmbeddedFontStyle::Normal => "normal",
        EmbeddedFontStyle::Italic => "italic",
        EmbeddedFontStyle::Oblique => "oblique",
    }
}

fn mime_from_path(path: &str) -> Option<&'static str> {
    let ext = extension_of(path)?;
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

fn font_format_from_path(path: &str) -> Option<&'static str> {
    let ext = extension_of(path)?;
    match ext.as_str() {
        "ttf" => Some("truetype"),
        "otf" => Some("opentype"),
        "woff" => Some("woff"),
        "woff2" => Some("woff2"),
        _ => None,
    }
}

fn font_mime_type(format_hint: Option<&str>, path: &str) -> Option<&'static str> {
    if let Some(format_hint) = format_hint {
        let lower = format_hint.to_ascii_lowercase();
        let normalized = lower.trim();
        if normalized.contains("woff2") {
            return Some("font/woff2");
        }
        if normalized.contains("woff") {
            return Some("font/woff");
        }
        if normalized.contains("opentype") || normalized.contains("otf") {
            return Some("font/otf");
        }
        if normalized.contains("truetype") || normalized.contains("ttf") {
            return Some("font/ttf");
        }
    }

    let ext = extension_of(path)?;
    match ext.as_str() {
        "woff2" => Some("font/woff2"),
        "woff" => Some("font/woff"),
        "otf" => Some("font/otf"),
        "ttf" => Some("font/ttf"),
        _ => None,
    }
}

fn extension_of(path: &str) -> Option<String> {
    let no_fragment = path.split_once('#').map_or(path, |(base, _)| base);
    let no_query = no_fragment
        .split_once('?')
        .map_or(no_fragment, |(base, _)| base);
    let ext = no_query.rsplit_once('.')?.1;
    Some(ext.to_ascii_lowercase())
}

fn build_html(initial_payload_json: &str, server_mode: bool) -> String {
    let safe_json = initial_payload_json.replace("</script>", "<\\/script>");
    let server_mode_literal = if server_mode { "true" } else { "false" };

    let template = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>epub-stream interactive preview</title>
  <style>
    :root {
      --bg: #f2efe8;
      --panel: #fdfbf7;
      --panel-alt: #f8f4eb;
      --ink: #252016;
      --muted: #675f50;
      --accent: #226a52;
      --accent-strong: #164b3a;
      --line: #d7cebc;
      --line-strong: #c4b79f;
      --shadow: rgba(28, 21, 9, 0.14);
    }

    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      color: var(--ink);
      background:
        radial-gradient(circle at 18% -24%, #f7dfb9 0%, transparent 44%),
        radial-gradient(circle at 92% -10%, #d9e7dc 0%, transparent 38%),
        var(--bg);
      font-family: "Alegreya Sans", "Source Sans 3", "Segoe UI", sans-serif;
    }

    .layout {
      display: grid;
      grid-template-columns: minmax(360px, 420px) minmax(0, 1fr);
      gap: 20px;
      padding: 20px;
      min-height: 100vh;
      align-items: start;
    }

    .panel {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 18px;
      box-shadow: 0 12px 30px var(--shadow);
    }

    .sidebar {
      padding: 16px;
      display: flex;
      flex-direction: column;
      gap: 12px;
      max-height: calc(100vh - 40px);
      overflow: auto;
      position: sticky;
      top: 20px;
    }

    .section-card {
      border: 1px solid var(--line);
      border-radius: 12px;
      background: var(--panel-alt);
      padding: 10px;
      display: grid;
      gap: 8px;
    }

    .title {
      margin: 0;
      font-size: 2rem;
      line-height: 1.08;
      letter-spacing: -0.01em;
      font-family: "Alegreya", "Source Serif 4", Georgia, serif;
    }

    .meta {
      color: var(--muted);
      font-size: 0.95rem;
      margin-top: 2px;
    }

    .quick-grid {
      display: grid;
      grid-template-columns: 1fr;
      gap: 8px;
    }

    label {
      display: grid;
      gap: 6px;
      font-size: 0.85rem;
      color: var(--muted);
      padding: 8px;
      border: 1px solid var(--line);
      border-radius: 10px;
      background: #fff;
    }

    .label-title {
      font-size: 0.74rem;
      text-transform: uppercase;
      letter-spacing: 0.04em;
      color: #5f5646;
      font-weight: 600;
    }

    .range-row {
      display: flex;
      align-items: center;
      gap: 8px;
    }

    .range-row input[type="range"] {
      flex: 1;
    }

    .range-value {
      min-width: 52px;
      text-align: right;
      font-variant-numeric: tabular-nums;
      color: var(--accent-strong);
      font-weight: 600;
      font-size: 0.84rem;
    }

    input, select, textarea, button {
      font: inherit;
      color: var(--ink);
    }

    input, select, textarea {
      border: 1px solid var(--line);
      border-radius: 8px;
      padding: 6px 8px;
      background: #fff;
      width: 100%;
    }

    textarea {
      min-height: 240px;
      resize: vertical;
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
      font-size: 12px;
      line-height: 1.4;
      white-space: pre;
      overflow-wrap: normal;
      overflow-x: auto;
    }

    .control-row {
      display: flex;
      align-items: center;
      gap: 8px;
      flex-wrap: wrap;
    }

    .btn {
      border: 1px solid var(--line-strong);
      border-radius: 999px;
      padding: 7px 14px;
      background: #fff;
      cursor: pointer;
      font-size: 0.92rem;
      transition: 120ms ease;
    }

    .btn:hover {
      border-color: var(--accent);
      color: var(--accent);
      transform: translateY(-1px);
    }

    .btn.btn-primary {
      background: var(--accent);
      color: #fff;
      border-color: var(--accent);
    }

    .btn.btn-primary:hover {
      background: var(--accent-strong);
      border-color: var(--accent-strong);
      color: #fff;
    }

    .status {
      font-size: 0.87rem;
      color: #1e5f49;
      border: 1px dashed #bddccf;
      border-radius: 8px;
      padding: 7px 8px;
      min-height: 2.2em;
      background: #eef8f3;
    }

    details {
      border: 1px solid var(--line);
      border-radius: 10px;
      padding: 6px 8px;
      background: #fff;
    }

    summary {
      cursor: pointer;
      font-size: 0.92rem;
      color: var(--ink);
      font-weight: 600;
    }

    .toc {
      overflow: auto;
      max-height: min(34vh, 360px);
      padding: 4px;
      border: 1px solid var(--line);
      border-radius: 12px;
      background: #fff;
    }

    .toc-item {
      width: 100%;
      text-align: left;
      border: 1px solid transparent;
      background: transparent;
      border-radius: 8px;
      cursor: pointer;
      padding: 6px 8px;
      font-size: 0.9rem;
      color: var(--ink);
    }

    .toc-item:hover {
      border-color: var(--line);
      background: #f8f6ef;
    }

    .toc-item[data-missing="true"] {
      color: #9c7f45;
    }

    .warnings {
      font-size: 0.84rem;
      color: #7b2f2f;
      max-height: 140px;
      overflow: auto;
      border: 1px solid #e8c9c9;
      border-radius: 10px;
      padding: 8px;
      background: #fff5f5;
      display: grid;
      gap: 4px;
    }

    .viewer {
      padding: 16px;
      display: grid;
      grid-template-rows: auto minmax(0, 1fr);
      gap: 12px;
    }

    .toolbar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      flex-wrap: wrap;
      gap: 10px;
      border-bottom: 1px solid var(--line);
      padding-bottom: 10px;
    }

    .canvas-wrap {
      display: grid;
      place-items: center;
      overflow: auto;
      border: 1px solid var(--line);
      border-radius: 14px;
      background: linear-gradient(180deg, #fefcf8, #f5efe2);
      padding: 20px;
    }

    canvas {
      display: block;
      background: #fff;
      box-shadow: 0 16px 40px var(--shadow);
      max-width: 100%;
      height: auto;
      border-radius: 4px;
    }

    #page-hint {
      color: #6f4f12;
      font-weight: 600;
    }

    @media (max-width: 1200px) {
      .layout {
        grid-template-columns: minmax(320px, 370px) minmax(0, 1fr);
        gap: 14px;
        padding: 14px;
      }
      .sidebar {
        max-height: calc(100vh - 28px);
        top: 14px;
      }
    }

    @media (max-width: 1040px) {
      .layout { grid-template-columns: 1fr; }
      .sidebar {
        position: static;
        max-height: none;
      }
      .toc { max-height: 260px; }
      .title { font-size: 1.5rem; }
    }
  </style>
</head>
<body>
  <div class="layout">
    <aside class="panel sidebar">
      <section class="section-card">
        <h1 id="book-title" class="title"></h1>
        <div id="book-meta" class="meta"></div>
      </section>

      <section class="section-card">
        <div class="quick-grid">
          <label><span class="label-title">Chapter</span>
            <select id="quick-chapter"></select>
          </label>
          <label><span class="label-title">Viewport width</span>
            <input id="quick-width" type="number" min="64" max="4096" />
          </label>
          <label><span class="label-title">Viewport height</span>
            <input id="quick-height" type="number" min="64" max="4096" />
          </label>
          <label><span class="label-title">Base font size</span>
            <input id="quick-base-font" type="number" min="1" step="0.1" />
          </label>
          <label><span class="label-title">Text scale</span>
            <input id="quick-text-scale" type="number" min="0.25" max="4" step="0.05" />
          </label>
          <label><span class="label-title">Font family override</span>
            <select id="quick-ui-font-family"></select>
          </label>
          <label><span class="label-title">UI font scale</span>
            <div class="range-row">
              <input id="quick-ui-font-scale" type="range" min="0.25" max="4" step="0.05" />
              <output id="quick-ui-font-scale-value" class="range-value">1.00x</output>
            </div>
          </label>
          <label><span class="label-title">Justification</span>
            <select id="quick-justify">
              <option value="true">enabled</option>
              <option value="false">disabled</option>
            </select>
          </label>
          <label><span class="label-title">Justify strategy</span>
            <select id="quick-justify-mode"></select>
          </label>
          <label><span class="label-title">Cover page mode</span>
            <select id="quick-cover-mode"></select>
          </label>
        </div>
        <div class="control-row" style="margin-top:8px;">
          <button id="apply-btn" class="btn btn-primary">Apply (Re-render)</button>
          <button id="reset-btn" class="btn">Reset Defaults</button>
        </div>
        <div id="status" class="status"></div>
      </section>

      <section class="section-card">
        <details>
          <summary>Advanced Config JSON (all options)</summary>
          <label style="margin-top:8px;"><span class="label-title">Render config</span>
            <textarea id="config-json"></textarea>
          </label>
          <div class="meta">Any field in this JSON can be changed. Apply will re-render using exactly this payload.</div>
        </details>
      </section>

      <section class="section-card">
        <div class="meta">Table of contents</div>
        <div class="toc" id="toc-list"></div>
      </section>
      <section class="warnings" id="warnings"></section>
    </aside>

    <main class="panel viewer">
      <div class="toolbar">
        <div class="control-row">
          <button id="prev-btn" class="btn">Previous</button>
          <button id="next-btn" class="btn">Next</button>
        </div>
        <div class="control-row">
          <div id="page-hint" class="meta"></div>
          <div id="page-label" class="meta"></div>
        </div>
      </div>
      <div class="canvas-wrap">
        <canvas id="page-canvas"></canvas>
      </div>
    </main>
  </div>

  <script>
    const SERVER_MODE = __SERVER_MODE__;
    const INITIAL_PAYLOAD = __INITIAL_PAYLOAD__;

    const state = {
      payload: INITIAL_PAYLOAD,
      pageOffset: 0,
      defaults: JSON.parse(JSON.stringify(INITIAL_PAYLOAD.config || {})),
      imageCache: new Map(),
      styleTag: null,
    };

    const el = {
      title: document.getElementById('book-title'),
      meta: document.getElementById('book-meta'),
      status: document.getElementById('status'),
      chapter: document.getElementById('quick-chapter'),
      width: document.getElementById('quick-width'),
      height: document.getElementById('quick-height'),
      baseFont: document.getElementById('quick-base-font'),
      textScale: document.getElementById('quick-text-scale'),
      uiFontFamily: document.getElementById('quick-ui-font-family'),
      uiFontScale: document.getElementById('quick-ui-font-scale'),
      uiFontScaleValue: document.getElementById('quick-ui-font-scale-value'),
      justify: document.getElementById('quick-justify'),
      justifyMode: document.getElementById('quick-justify-mode'),
      coverMode: document.getElementById('quick-cover-mode'),
      apply: document.getElementById('apply-btn'),
      reset: document.getElementById('reset-btn'),
      configJson: document.getElementById('config-json'),
      toc: document.getElementById('toc-list'),
      warnings: document.getElementById('warnings'),
      pageLabel: document.getElementById('page-label'),
      pageHint: document.getElementById('page-hint'),
      prev: document.getElementById('prev-btn'),
      next: document.getElementById('next-btn'),
      canvas: document.getElementById('page-canvas'),
    };
    const ctx = el.canvas.getContext('2d');

    wireEvents();
    hydrateUIFromPayload();
    applyPayload(INITIAL_PAYLOAD, true);

    function wireEvents() {
      el.apply.addEventListener('click', async () => {
        const cfg = readConfigFromEditorAndQuick();
        if (!cfg) return;

        if (!SERVER_MODE) {
          state.payload.config = cfg;
          updateConfigEditor(cfg);
          renderCurrentPage();
          setStatus('Static mode: UI-only options applied locally. Run with --serve for full reflow.');
          return;
        }

        await rerenderFromServer(cfg);
      });

      el.reset.addEventListener('click', async () => {
        const cfg = JSON.parse(JSON.stringify(state.defaults));
        updateQuickControls(cfg);
        updateConfigEditor(cfg);
        if (!SERVER_MODE) {
          state.payload.config = cfg;
          renderCurrentPage();
          setStatus('Reset to defaults in static mode.');
          return;
        }
        await rerenderFromServer(cfg);
      });

      el.prev.addEventListener('click', () => {
        const pages = visiblePages();
        if (pages.length === 0) return;
        state.pageOffset = Math.max(0, state.pageOffset - 1);
        renderCurrentPage();
      });

      el.next.addEventListener('click', () => {
        const pages = visiblePages();
        if (pages.length === 0) return;
        state.pageOffset = Math.min(pages.length - 1, state.pageOffset + 1);
        renderCurrentPage();
      });

      el.uiFontFamily.addEventListener('change', () => {
        const cfg = state.payload.config || {};
        cfg.ui_font_family = String(el.uiFontFamily.value || 'auto');
        state.payload.config = cfg;
        updateConfigEditor(cfg);
        if (SERVER_MODE) {
          setStatus('Font family updated. Click Apply (Re-render) to re-layout with this family.');
          return;
        }
        renderCurrentPage();
      });
      el.uiFontScale.addEventListener('input', () => {
        const cfg = state.payload.config || {};
        cfg.ui_font_scale = Number(el.uiFontScale.value || 1);
        state.payload.config = cfg;
        updateUiFontScaleValue(cfg.ui_font_scale);
        updateConfigEditor(cfg);
        if (SERVER_MODE) {
          setStatus('UI font scale updated. Click Apply (Re-render) to re-layout at this scale.');
          return;
        }
        renderCurrentPage();
      });
    }

    function hydrateUIFromPayload() {
      el.title.textContent = state.payload.meta?.title || 'Untitled EPUB';
      const author = state.payload.meta?.author || 'Unknown author';
      const lang = state.payload.meta?.language || 'unknown language';
      const pageCount = Number(state.payload.meta?.page_count || 0);
      el.meta.textContent = `${author} • ${lang} • ${pageCount} rendered page(s)`;

      const cfg = state.payload.config || {};
      rebuildChapterSelect(cfg.chapter);
      rebuildFontFamilySelect(cfg.ui_font_family);
      rebuildJustifyStrategySelect(cfg.justify_strategy);
      rebuildCoverModeSelect(cfg.object_cover_page_mode);
      updateQuickControls(cfg);
      updateConfigEditor(cfg);
      renderToc();
      renderWarnings();
      setStatus(SERVER_MODE ? 'Ready. Change settings and apply to re-render.' : 'Static mode loaded.');
    }

    function rebuildChapterSelect(selectedChapter) {
      el.chapter.innerHTML = '';
      const all = document.createElement('option');
      all.value = 'all';
      all.textContent = 'All chapters';
      el.chapter.appendChild(all);

      for (const chapter of state.payload.chapters || []) {
        const option = document.createElement('option');
        option.value = String(chapter.index);
        const label = String(chapter.title || chapter.href || '').trim();
        option.textContent = `#${chapter.index + 1} ${label || '(untitled chapter)'}`;
        el.chapter.appendChild(option);
      }

      if (Number.isInteger(selectedChapter)) {
        el.chapter.value = String(selectedChapter);
      } else {
        el.chapter.value = 'all';
      }
    }

    function rebuildFontFamilySelect(selectedFamily) {
      const families = new Set(['auto', 'serif', 'sans-serif', 'monospace']);
      for (const f of state.payload.style_families || []) families.add(f);
      for (const face of state.payload.fonts || []) if (face?.family) families.add(face.family);

      el.uiFontFamily.innerHTML = '';
      const ordered = [
        'auto',
        ...Array.from(families)
          .filter((family) => family !== 'auto')
          .sort((a, b) => String(a).localeCompare(String(b))),
      ];
      for (const family of ordered) {
        const option = document.createElement('option');
        option.value = family;
        option.textContent = family === 'auto' ? 'Book-resolved (auto)' : family;
        el.uiFontFamily.appendChild(option);
      }

      el.uiFontFamily.value = selectedFamily && families.has(selectedFamily) ? selectedFamily : 'auto';
    }

    function rebuildJustifyStrategySelect(selectedStrategy) {
      const options = (state.payload.option_lists && state.payload.option_lists.justification_strategy) || [
        'adaptive-inter-word',
        'full-inter-word',
        'align-left',
        'align-right',
        'align-center',
      ];
      el.justifyMode.innerHTML = '';
      for (const value of options) {
        const option = document.createElement('option');
        option.value = String(value);
        option.textContent = String(value);
        el.justifyMode.appendChild(option);
      }
      const fallback = 'adaptive-inter-word';
      el.justifyMode.value = options.includes(selectedStrategy) ? selectedStrategy : fallback;
    }

    function rebuildCoverModeSelect(selectedMode) {
      const options = (state.payload.option_lists && state.payload.option_lists.cover_page_mode) || [
        'contain',
        'full-bleed',
        'respect-css',
      ];
      el.coverMode.innerHTML = '';
      for (const value of options) {
        const option = document.createElement('option');
        option.value = String(value);
        option.textContent = String(value);
        el.coverMode.appendChild(option);
      }
      const fallback = 'contain';
      el.coverMode.value = options.includes(selectedMode) ? selectedMode : fallback;
    }

    function updateQuickControls(cfg) {
      el.width.value = cfg.display_width;
      el.height.value = cfg.display_height;
      el.baseFont.value = cfg.prep_base_font_size_px;
      el.textScale.value = cfg.prep_text_scale;
      el.justify.value = String(Boolean(cfg.justify_enabled));
      el.justifyMode.value = cfg.justify_strategy || 'adaptive-inter-word';
      el.coverMode.value = cfg.object_cover_page_mode || 'contain';
      el.uiFontScale.value = cfg.ui_font_scale;
      updateUiFontScaleValue(cfg.ui_font_scale);
      if (Number.isInteger(cfg.chapter)) {
        el.chapter.value = String(cfg.chapter);
      } else {
        el.chapter.value = 'all';
      }
      if (cfg.ui_font_family) {
        el.uiFontFamily.value = cfg.ui_font_family;
      }
    }

    function updateUiFontScaleValue(value) {
      const numeric = Number(value || 1);
      el.uiFontScaleValue.textContent = `${numeric.toFixed(2)}x`;
    }

    function updateConfigEditor(cfg) {
      el.configJson.value = JSON.stringify(cfg, null, 2);
    }

    function readConfigFromEditorAndQuick() {
      let cfg;
      try {
        cfg = JSON.parse(el.configJson.value);
      } catch (err) {
        setStatus(`Invalid JSON: ${err.message}`);
        return null;
      }

      cfg.display_width = Number(el.width.value || cfg.display_width || 900);
      cfg.display_height = Number(el.height.value || cfg.display_height || 1200);
      cfg.prep_base_font_size_px = Number(el.baseFont.value || cfg.prep_base_font_size_px || 22);
      cfg.prep_text_scale = Number(el.textScale.value || cfg.prep_text_scale || 1);
      cfg.justify_enabled = el.justify.value === 'true';
      cfg.justify_strategy = String(el.justifyMode.value || cfg.justify_strategy || 'adaptive-inter-word');
      cfg.object_cover_page_mode = String(el.coverMode.value || cfg.object_cover_page_mode || 'contain');
      cfg.ui_font_scale = Number(el.uiFontScale.value || cfg.ui_font_scale || 1);
      cfg.ui_font_family = String(el.uiFontFamily.value || cfg.ui_font_family || 'auto');
      cfg.chapter = el.chapter.value === 'all' ? null : Number(el.chapter.value);

      updateConfigEditor(cfg);
      return cfg;
    }

    async function rerenderFromServer(cfg) {
      setStatus('Rendering...');
      const anchor = currentPageAnchor();
      const fallback = currentPageFallback();
      try {
        const res = await fetch('/api/render', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(cfg),
        });
        if (!res.ok) {
          const text = await res.text();
          throw new Error(`HTTP ${res.status}: ${text}`);
        }
        const payload = await res.json();
        applyPayload(payload, false, anchor, fallback);
        setStatus(`Rendered ${payload.meta?.page_count || 0} page(s).`);
      } catch (err) {
        setStatus(`Render failed: ${err.message}`);
      }
    }

    function applyPayload(payload, isInitial, anchor = null, fallback = null) {
      state.payload = payload;
      hydrateUIFromPayload();
      applyEmbeddedFonts();
      preloadImages().finally(() => {
        restorePageOffset(anchor, fallback);
        renderCurrentPage();
      });
      if (!isInitial) {
        updateConfigEditor(payload.config || {});
      }
    }

    function renderToc() {
      el.toc.innerHTML = '';
      const entries = Array.isArray(state.payload.toc) ? state.payload.toc : [];
      if (entries.length === 0) {
        const note = document.createElement('div');
        note.className = 'meta';
        note.style.padding = '0 8px';
        note.textContent = 'No TOC entries found.';
        el.toc.appendChild(note);
        return;
      }

      for (const entry of entries) {
        const btn = document.createElement('button');
        btn.className = 'toc-item';
        btn.style.paddingLeft = `${8 + (Number(entry.depth || 0) * 14)}px`;
        btn.textContent = entry.label || entry.href || '(untitled)';
        const hasTarget = Number.isInteger(entry.target_chapter) && Number.isInteger(entry.target_page_global);
        if (!hasTarget) {
          btn.dataset.missing = 'true';
        }
        btn.addEventListener('click', () => {
          if (!hasTarget) return;
          const page = visiblePages().find((p) => p._globalIndex === entry.target_page_global);
          if (!page) return;
          const idx = visiblePages().findIndex((p) => p._globalIndex === entry.target_page_global);
          if (idx < 0) return;
          state.pageOffset = idx;
          renderCurrentPage();
        });
        el.toc.appendChild(btn);
      }
    }

    function renderWarnings() {
      el.warnings.innerHTML = '';
      const warnings = Array.isArray(state.payload.warnings) ? state.payload.warnings : [];
      if (warnings.length === 0) {
        const msg = document.createElement('div');
        msg.style.color = 'var(--muted)';
        msg.textContent = 'No preview warnings.';
        el.warnings.appendChild(msg);
        return;
      }
      const heading = document.createElement('strong');
      heading.textContent = `Warnings (${warnings.length})`;
      el.warnings.appendChild(heading);
      for (const w of warnings) {
        const line = document.createElement('div');
        line.textContent = w;
        el.warnings.appendChild(line);
      }
    }

    function setStatus(text) {
      el.status.textContent = text;
    }

    function applyEmbeddedFonts() {
      if (state.styleTag) state.styleTag.remove();
      const fonts = Array.isArray(state.payload.fonts) ? state.payload.fonts : [];
      if (fonts.length === 0) {
        state.styleTag = null;
        return;
      }
      let css = '';
      for (const font of fonts) {
        if (!font?.family || !font?.data_uri) continue;
        const fmt = font.format ? ` format('${String(font.format).replace(/'/g, '')}')` : '';
        css += `@font-face { font-family: "${escapeCss(font.family)}"; src: url('${font.data_uri}')${fmt}; font-style: ${font.style || 'normal'}; font-weight: ${Number(font.weight || 400)}; font-display: swap; }\n`;
      }
      if (!css) return;
      state.styleTag = document.createElement('style');
      state.styleTag.textContent = css;
      document.head.appendChild(state.styleTag);
    }

    async function preloadImages() {
      state.imageCache.clear();
      const entries = Object.entries(state.payload.images || {});
      await Promise.all(entries.map(([src, uri]) => {
        return new Promise((resolve) => {
          const img = new Image();
          img.onload = () => {
            state.imageCache.set(src, img);
            resolve();
          };
          img.onerror = () => resolve();
          img.src = uri;
        });
      }));
    }

    function visiblePages() {
      const pages = Array.isArray(state.payload.pages) ? state.payload.pages : [];
      return pages.map((p, idx) => ({ ...p, _globalIndex: idx }));
    }

    function currentPageAnchor() {
      const pages = visiblePages();
      if (pages.length === 0) return null;
      const idx = Math.max(0, Math.min(state.pageOffset, pages.length - 1));
      const page = pages[idx];
      return {
        chapter_index: page.chapter_index,
        page_index: page.page_index,
        page_number: page.page_number,
      };
    }

    function currentPageFallback() {
      const pages = visiblePages();
      if (pages.length === 0) return null;
      const idx = Math.max(0, Math.min(state.pageOffset, pages.length - 1));
      const page = pages[idx];
      const chapterPages = pages.filter((p) => p.chapter_index === page.chapter_index);
      const chapterOffset = chapterPages.findIndex((p) => p._globalIndex === page._globalIndex);
      return {
        global_offset: idx,
        global_count: pages.length,
        chapter_index: page.chapter_index,
        chapter_offset: Math.max(0, chapterOffset),
        chapter_count: chapterPages.length,
      };
    }

    function restorePageOffset(anchor, fallback = null) {
      const pages = visiblePages();
      if (pages.length === 0) {
        state.pageOffset = 0;
        return;
      }
      if (!anchor) {
        state.pageOffset = Math.max(0, Math.min(state.pageOffset, pages.length - 1));
        return;
      }

      let idx = pages.findIndex((p) =>
        p.chapter_index === anchor.chapter_index &&
        p.page_index === anchor.page_index
      );
      if (idx < 0) {
        idx = pages.findIndex((p) =>
          p.chapter_index === anchor.chapter_index &&
          p.page_number === anchor.page_number
        );
      }
      if (idx < 0) {
        idx = pages.findIndex((p) => p.chapter_index === anchor.chapter_index);
      }
      if (idx < 0) {
        idx = fallbackIndexFromProgress(pages, fallback);
      }
      if (idx < 0) {
        idx = Math.max(0, Math.min(state.pageOffset, pages.length - 1));
      }
      state.pageOffset = idx;
    }

    function fallbackIndexFromProgress(pages, fallback) {
      if (!fallback || pages.length === 0) return -1;

      if (Number.isInteger(fallback.chapter_index)) {
        const chapterPages = pages.filter((p) => p.chapter_index === fallback.chapter_index);
        if (chapterPages.length > 0) {
          let chapterOffset = Number(fallback.chapter_offset || 0);
          if (Number(fallback.chapter_count || 0) > 1 && chapterPages.length > 1) {
            const ratio = chapterOffset / Math.max(1, Number(fallback.chapter_count) - 1);
            chapterOffset = Math.round(ratio * (chapterPages.length - 1));
          }
          chapterOffset = Math.max(0, Math.min(chapterOffset, chapterPages.length - 1));
          return chapterPages[chapterOffset]._globalIndex;
        }
      }

      const oldCount = Number(fallback.global_count || 0);
      const oldOffset = Number(fallback.global_offset || 0);
      if (oldCount > 0) {
        let idx;
        if (oldCount === 1 || pages.length === 1) {
          idx = 0;
        } else {
          const ratio = oldOffset / Math.max(1, oldCount - 1);
          idx = Math.round(ratio * (pages.length - 1));
        }
        return Math.max(0, Math.min(idx, pages.length - 1));
      }
      return -1;
    }

    function renderCurrentPage() {
      const pages = visiblePages();
      if (pages.length === 0) {
        drawEmpty();
        return;
      }
      state.pageOffset = Math.max(0, Math.min(state.pageOffset, pages.length - 1));
      const page = pages[state.pageOffset];
      const cfg = state.payload.config || {};

      el.canvas.width = Number(cfg.display_width || state.payload.meta?.viewport?.width || 800);
      el.canvas.height = Number(cfg.display_height || state.payload.meta?.viewport?.height || 1200);

      ctx.clearRect(0, 0, el.canvas.width, el.canvas.height);
      ctx.fillStyle = '#fff';
      ctx.fillRect(0, 0, el.canvas.width, el.canvas.height);

      for (const cmd of page.commands || []) {
        drawCommand(cmd, page, cfg);
      }

      applyRenderIntent(cfg);

      const hasText = (page.commands || []).some((cmd) => cmd.kind === 'text');
      el.pageHint.textContent = hasText
        ? ''
        : 'Image-only page: font changes show on text pages.';
      el.pageLabel.textContent = `page ${page.page_number} (${state.pageOffset + 1}/${pages.length}) • chapter ${page.chapter_index + 1}`;
    }

    function drawEmpty() {
      el.canvas.width = Number(state.payload.meta?.viewport?.width || 800);
      el.canvas.height = Number(state.payload.meta?.viewport?.height || 1200);
      ctx.clearRect(0, 0, el.canvas.width, el.canvas.height);
      ctx.fillStyle = '#f8f8f8';
      ctx.fillRect(0, 0, el.canvas.width, el.canvas.height);
      ctx.fillStyle = '#666';
      ctx.font = '16px sans-serif';
      ctx.fillText('No pages available for current configuration.', 16, 36);
      el.pageLabel.textContent = 'No pages';
    }

    function drawCommand(cmd, page, cfg) {
      switch (cmd.kind) {
        case 'text':
          drawText(cmd, cfg);
          break;
        case 'rule':
          drawRule(cmd);
          break;
        case 'rect':
          drawRect(cmd);
          break;
        case 'image_object':
          drawImage(cmd);
          break;
        case 'page_chrome':
          drawPageChrome(cmd, page, cfg);
          break;
        default:
          break;
      }
    }

    function drawText(cmd, cfg) {
      const style = cmd.style || {};
      const scale = SERVER_MODE ? 1 : Number(cfg.ui_font_scale || 1);
      const size = Math.max(8, Number(style.size_px || 16) * scale);
      const family = (cfg.ui_font_family && cfg.ui_font_family !== 'auto')
        ? cfg.ui_font_family
        : (style.family || 'serif');
      const familySpec = canvasFamilySpec(family, 'serif');
      const italic = style.italic ? 'italic' : 'normal';
      const weight = Number(style.weight || 400);
      const letterSpacing = Number(style.letter_spacing || 0) * scale;

      ctx.fillStyle = '#111';
      ctx.textBaseline = 'alphabetic';
      ctx.font = `${italic} ${weight} ${size}px ${familySpec}`;

      const text = String(cmd.text || '');
      const justify = style.justify || { mode: 'none', extra_px_total: null, offset_px: null };
      const applyJustify = Boolean(cfg.justify_enabled) && justify.mode === 'inter-word' && Number.isFinite(justify.extra_px_total);
      let x = Number(cmd.x || 0);
      const y = Number(cmd.baseline_y || 0);
      const contentRight = el.canvas.width - Number(cfg.margin_right || 0);
      const availableWidth = Math.max(0, contentRight - x);
      const chars = Array.from(text);
      const measuredBaseWidth = ctx.measureText(text).width + (Math.max(0, chars.length - 1) * letterSpacing);
      const spaces = chars.filter((ch) => ch === ' ').length;

      if (Boolean(cfg.justify_enabled) && (justify.mode === 'align-right' || justify.mode === 'align-center')) {
        if (Number.isFinite(justify.offset_px)) {
          x += Math.max(0, Number(justify.offset_px));
        } else {
          const slack = Math.max(0, availableWidth - measuredBaseWidth);
          x += justify.mode === 'align-center' ? slack / 2 : slack;
        }
      }

      if (!applyJustify && letterSpacing <= 0.01) {
        ctx.fillText(text, x, y);
        return;
      }

      let extraSpace = 0;
      if (applyJustify) {
        if (spaces > 0) {
          const requestedExtraTotal = Math.max(0, Number(justify.extra_px_total || 0) * scale);
          extraSpace = requestedExtraTotal / spaces;
        }
      }

      const renderedWidth = measuredBaseWidth + (spaces * extraSpace);
      const squeeze = availableWidth > 0 && renderedWidth > availableWidth
        ? Math.max(0.85, availableWidth / renderedWidth)
        : 1;
      if (squeeze < 0.999) {
        ctx.save();
        ctx.translate(x, y);
        ctx.scale(squeeze, 1);
        if (applyJustify && letterSpacing <= 0.01) {
          drawJustifiedText(text, 0, 0, extraSpace);
        } else {
          drawTrackedText(text, 0, 0, letterSpacing, extraSpace);
        }
        ctx.restore();
        return;
      }

      if (applyJustify && letterSpacing <= 0.01) {
        drawJustifiedText(text, x, y, extraSpace);
        return;
      }

      drawTrackedText(text, x, y, letterSpacing, extraSpace);
    }

    function drawTrackedText(text, x, y, letterSpacing, extraSpace) {
      let cursor = x;
      for (const ch of Array.from(text)) {
        ctx.fillText(ch, cursor, y);
        cursor += ctx.measureText(ch).width + letterSpacing + (ch === ' ' ? extraSpace : 0);
      }
    }

    function drawJustifiedText(text, x, y, extraPerSpace) {
      let cursor = x;
      const tokens = String(text).split(/(\s+)/).filter((token) => token.length > 0);
      for (const token of tokens) {
        if (/^\s+$/.test(token)) {
          const spaces = Array.from(token).filter((ch) => ch === ' ').length;
          cursor += ctx.measureText(token).width + (spaces * extraPerSpace);
          continue;
        }
        ctx.fillText(token, cursor, y);
        cursor += ctx.measureText(token).width;
      }
    }

    function drawRule(cmd) {
      const x = Number(cmd.x || 0);
      const y = Number(cmd.y || 0);
      const length = Number(cmd.length || 0);
      const thickness = Math.max(1, Number(cmd.thickness || 1));
      ctx.strokeStyle = '#171717';
      ctx.lineWidth = thickness;
      ctx.beginPath();
      if (cmd.horizontal) {
        ctx.moveTo(x, y);
        ctx.lineTo(x + length, y);
      } else {
        ctx.moveTo(x, y);
        ctx.lineTo(x, y + length);
      }
      ctx.stroke();
    }

    function drawRect(cmd) {
      const x = Number(cmd.x || 0);
      const y = Number(cmd.y || 0);
      const width = Number(cmd.width || 0);
      const height = Number(cmd.height || 0);
      if (cmd.fill) {
        ctx.fillStyle = '#111';
        ctx.fillRect(x, y, width, height);
      } else {
        ctx.strokeStyle = '#111';
        ctx.lineWidth = 1;
        ctx.strokeRect(x, y, width, height);
      }
    }

    function drawImage(cmd) {
      const x = Number(cmd.x || 0);
      const y = Number(cmd.y || 0);
      const width = Number(cmd.width || 0);
      const height = Number(cmd.height || 0);
      const img = state.imageCache.get(cmd.src);
      if (img) {
        ctx.drawImage(img, x, y, width, height);
        return;
      }
      ctx.fillStyle = '#efefef';
      ctx.fillRect(x, y, width, height);
      ctx.strokeStyle = '#bbb';
      ctx.strokeRect(x, y, width, height);
      ctx.fillStyle = '#666';
      ctx.font = '12px sans-serif';
      ctx.fillText(cmd.alt || cmd.src || 'image', x + 6, y + 18);
    }

    function drawPageChrome(cmd, page, cfg) {
      const kind = cmd.kind_detail || cmd.kind;
      const family = (cfg.ui_font_family && cfg.ui_font_family !== 'auto') ? cfg.ui_font_family : 'serif';
      const h = el.canvas.height;
      const w = el.canvas.width;

      if (kind === 'header' && cfg.page_chrome_header_enabled) {
        const style = String(cfg.page_chrome_header_style || 'bold');
        const font = chromeFont(style, 14, family);
        ctx.fillStyle = '#444';
        ctx.font = font;
        ctx.textBaseline = 'alphabetic';
        const text = cmd.text || '';
        ctx.fillText(String(text), Number(cfg.page_chrome_header_x || 8), Number(cfg.page_chrome_header_baseline_y || 16));
      }

      if (kind === 'footer' && cfg.page_chrome_footer_enabled) {
        const style = String(cfg.page_chrome_footer_style || 'regular');
        const font = chromeFont(style, 12, family);
        ctx.fillStyle = '#444';
        ctx.font = font;
        ctx.textBaseline = 'alphabetic';
        const text = cmd.text || `Page ${page.page_number}`;
        const y = h - Number(cfg.page_chrome_footer_baseline_from_bottom || 8);
        ctx.fillText(String(text), Number(cfg.page_chrome_footer_x || 8), y);
      }

      if (kind === 'progress' && cfg.page_chrome_progress_enabled) {
        const current = Number(cmd.current || 0);
        const total = Math.max(1, Number(cmd.total || page.metrics?.chapter_page_count || 1));
        const ratio = Math.max(0, Math.min(1, current / total));
        const xInset = Number(cfg.page_chrome_progress_x_inset || 8);
        const yFromBottom = Number(cfg.page_chrome_progress_y_from_bottom || 20);
        const y = h - yFromBottom;
        const barWidth = Math.max(12, w - xInset * 2);
        const barHeight = Math.max(1, Number(cfg.page_chrome_progress_height || 4));
        const stroke = Math.max(1, Number(cfg.page_chrome_progress_stroke_width || 1));

        ctx.lineWidth = stroke;
        ctx.strokeStyle = '#555';
        ctx.strokeRect(xInset, y, barWidth, barHeight);
        ctx.fillStyle = '#1f8f6a';
        ctx.fillRect(xInset, y, barWidth * ratio, barHeight);
      }
    }

    function chromeFont(style, size, family) {
      const familySpec = canvasFamilySpec(family, 'serif');
      const lower = String(style || 'regular').toLowerCase();
      if (lower === 'bold') return `normal 700 ${size}px ${familySpec}`;
      if (lower === 'italic') return `italic 400 ${size}px ${familySpec}`;
      if (lower === 'bold-italic') return `italic 700 ${size}px ${familySpec}`;
      return `normal 400 ${size}px ${familySpec}`;
    }

    function canvasFamilySpec(family, fallback = 'serif') {
      const raw = String(family || '').trim();
      const parts = (raw ? raw.split(',') : [fallback])
        .map((part) => stripOuterQuotes(part.trim()))
        .filter((part) => part.length > 0);

      const normalized = parts.length > 0 ? parts : [fallback];
      if (!normalized.some((part) => isGenericFamily(part))) {
        normalized.push(fallback);
      }

      return normalized
        .map((part) => (isGenericFamily(part) ? part : `"${escapeCss(part)}"`))
        .join(', ');
    }

    function stripOuterQuotes(value) {
      if (value.length < 2) return value;
      const first = value[0];
      const last = value[value.length - 1];
      if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
        return value.slice(1, -1);
      }
      return value;
    }

    function isGenericFamily(value) {
      const lower = String(value || '').toLowerCase();
      return (
        lower === 'serif' ||
        lower === 'sans-serif' ||
        lower === 'monospace' ||
        lower === 'cursive' ||
        lower === 'fantasy' ||
        lower === 'system-ui' ||
        lower === 'ui-serif' ||
        lower === 'ui-sans-serif' ||
        lower === 'ui-monospace' ||
        lower === 'emoji' ||
        lower === 'math' ||
        lower === 'fangsong'
      );
    }

    function applyRenderIntent(cfg) {
      const grayscale = String(cfg.render_grayscale_mode || 'off').toLowerCase();
      const dither = String(cfg.render_dither_mode || 'none').toLowerCase();
      const contrast = Number(cfg.render_contrast_boost || 100);

      if (grayscale === 'off' && dither === 'none' && contrast === 100) {
        return;
      }

      const image = ctx.getImageData(0, 0, el.canvas.width, el.canvas.height);
      const data = image.data;

      for (let i = 0; i < data.length; i += 4) {
        let r = data[i + 0];
        let g = data[i + 1];
        let b = data[i + 2];

        if (grayscale === 'luminosity') {
          const y = (0.2126 * r) + (0.7152 * g) + (0.0722 * b);
          r = y; g = y; b = y;
        }

        if (contrast !== 100) {
          const factor = contrast / 100;
          r = Math.max(0, Math.min(255, ((r - 128) * factor) + 128));
          g = Math.max(0, Math.min(255, ((g - 128) * factor) + 128));
          b = Math.max(0, Math.min(255, ((b - 128) * factor) + 128));
        }

        data[i + 0] = r;
        data[i + 1] = g;
        data[i + 2] = b;
      }

      if (dither === 'ordered') {
        const matrix = [
          [0, 8, 2, 10],
          [12, 4, 14, 6],
          [3, 11, 1, 9],
          [15, 7, 13, 5],
        ];
        for (let y = 0; y < el.canvas.height; y++) {
          for (let x = 0; x < el.canvas.width; x++) {
            const idx = (y * el.canvas.width + x) * 4;
            const lum = data[idx];
            const threshold = (matrix[y % 4][x % 4] / 16) * 255;
            const v = lum >= threshold ? 255 : 0;
            data[idx + 0] = v;
            data[idx + 1] = v;
            data[idx + 2] = v;
          }
        }
      } else if (dither === 'error-diffusion') {
        for (let y = 0; y < el.canvas.height; y++) {
          for (let x = 0; x < el.canvas.width; x++) {
            const idx = (y * el.canvas.width + x) * 4;
            const old = data[idx];
            const nw = old < 128 ? 0 : 255;
            const err = old - nw;
            data[idx + 0] = nw;
            data[idx + 1] = nw;
            data[idx + 2] = nw;

            diffuse(x + 1, y, err * 7 / 16);
            diffuse(x - 1, y + 1, err * 3 / 16);
            diffuse(x, y + 1, err * 5 / 16);
            diffuse(x + 1, y + 1, err * 1 / 16);
          }
        }

        function diffuse(x, y, delta) {
          if (x < 0 || y < 0 || x >= el.canvas.width || y >= el.canvas.height) return;
          const idx = (y * el.canvas.width + x) * 4;
          const v = Math.max(0, Math.min(255, data[idx] + delta));
          data[idx + 0] = v;
          data[idx + 1] = v;
          data[idx + 2] = v;
        }
      }

      ctx.putImageData(image, 0, 0);
    }

    function escapeCss(value) {
      return String(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
    }
  </script>
</body>
</html>
"#;

    template
        .replace("__INITIAL_PAYLOAD__", &safe_json)
        .replace("__SERVER_MODE__", server_mode_literal)
}

fn parse_args(args: Vec<String>) -> Result<Args, String> {
    if args.len() >= 2 && (args[1] == "--help" || args[1] == "-h") {
        return Err("help requested".to_string());
    }

    let has_positional_epub = args.get(1).is_some_and(|v| !v.starts_with("--"));

    let mut cfg = Args {
        epub_path: if has_positional_epub {
            args[1].clone()
        } else {
            DEFAULT_EPUB_PATH.to_string()
        },
        out_path: DEFAULT_OUT_PATH.to_string(),
        serve: false,
        open_browser: false,
        port: DEFAULT_PORT,
        chapter: None,
        start_page: 0,
        pages_per_chapter: None,
        display_width: 900,
        display_height: 1200,
        justify_enabled: true,
        justify_strategy: "adaptive-inter-word".to_string(),
        cover_page_mode: "contain".to_string(),
        prep_base_font_size_px: 22.0,
        prep_text_scale: 1.0,
        line_gap_px: 4,
        paragraph_gap_px: 8,
        margin_left: 24,
        margin_right: 24,
        margin_top: 18,
        margin_bottom: 30,
        justify_min_words: 6,
        justify_min_fill_ratio: 0.78,
        justify_max_space_stretch_ratio: 0.45,
        widow_orphan_enabled: true,
        widow_orphan_min_lines: 2,
        hanging_punctuation_enabled: true,
        max_image_bytes: 16 * 1024 * 1024,
        max_font_bytes: 24 * 1024 * 1024,
    };

    let mut i = if has_positional_epub { 2usize } else { 1usize };
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--out requires a value".to_string())?;
                cfg.out_path = v.clone();
                i += 2;
            }
            "--serve" => {
                cfg.serve = true;
                i += 1;
            }
            "--open" => {
                cfg.open_browser = true;
                i += 1;
            }
            "--port" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--port requires a value".to_string())?;
                cfg.port = v
                    .parse::<u16>()
                    .map_err(|_| format!("invalid --port value '{}'", v))?;
                i += 2;
            }
            "--chapter" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--chapter requires a value".to_string())?;
                cfg.chapter = Some(
                    v.parse::<usize>()
                        .map_err(|_| format!("invalid --chapter value '{}'", v))?,
                );
                i += 2;
            }
            "--start-page" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--start-page requires a value".to_string())?;
                cfg.start_page = v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --start-page value '{}'", v))?;
                i += 2;
            }
            "--pages" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--pages requires a value".to_string())?;
                cfg.pages_per_chapter = Some(
                    v.parse::<usize>()
                        .map_err(|_| format!("invalid --pages value '{}'", v))?,
                );
                i += 2;
            }
            "--width" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--width requires a value".to_string())?;
                cfg.display_width = v
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --width value '{}'", v))?;
                i += 2;
            }
            "--height" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--height requires a value".to_string())?;
                cfg.display_height = v
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --height value '{}'", v))?;
                i += 2;
            }
            "--justify" => {
                cfg.justify_enabled = true;
                i += 1;
            }
            "--no-justify" => {
                cfg.justify_enabled = false;
                i += 1;
            }
            "--justify-mode" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--justify-mode requires a value".to_string())?;
                cfg.justify_strategy = v.clone();
                i += 2;
            }
            "--cover-page-mode" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--cover-page-mode requires a value".to_string())?;
                cfg.cover_page_mode = v.clone();
                i += 2;
            }
            "--font-size" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--font-size requires a value".to_string())?;
                cfg.prep_base_font_size_px = v
                    .parse::<f32>()
                    .map_err(|_| format!("invalid --font-size value '{}'", v))?;
                i += 2;
            }
            "--text-scale" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--text-scale requires a value".to_string())?;
                cfg.prep_text_scale = v
                    .parse::<f32>()
                    .map_err(|_| format!("invalid --text-scale value '{}'", v))?;
                i += 2;
            }
            "--line-gap" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--line-gap requires a value".to_string())?;
                cfg.line_gap_px = v
                    .parse::<i32>()
                    .map_err(|_| format!("invalid --line-gap value '{}'", v))?;
                i += 2;
            }
            "--paragraph-gap" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--paragraph-gap requires a value".to_string())?;
                cfg.paragraph_gap_px = v
                    .parse::<i32>()
                    .map_err(|_| format!("invalid --paragraph-gap value '{}'", v))?;
                i += 2;
            }
            "--margin-left" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--margin-left requires a value".to_string())?;
                cfg.margin_left = v
                    .parse::<i32>()
                    .map_err(|_| format!("invalid --margin-left value '{}'", v))?;
                i += 2;
            }
            "--margin-right" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--margin-right requires a value".to_string())?;
                cfg.margin_right = v
                    .parse::<i32>()
                    .map_err(|_| format!("invalid --margin-right value '{}'", v))?;
                i += 2;
            }
            "--margin-top" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--margin-top requires a value".to_string())?;
                cfg.margin_top = v
                    .parse::<i32>()
                    .map_err(|_| format!("invalid --margin-top value '{}'", v))?;
                i += 2;
            }
            "--margin-bottom" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--margin-bottom requires a value".to_string())?;
                cfg.margin_bottom = v
                    .parse::<i32>()
                    .map_err(|_| format!("invalid --margin-bottom value '{}'", v))?;
                i += 2;
            }
            "--justify-min-words" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--justify-min-words requires a value".to_string())?;
                cfg.justify_min_words = v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --justify-min-words value '{}'", v))?;
                i += 2;
            }
            "--justify-min-fill" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--justify-min-fill requires a value".to_string())?;
                cfg.justify_min_fill_ratio = v
                    .parse::<f32>()
                    .map_err(|_| format!("invalid --justify-min-fill value '{}'", v))?;
                i += 2;
            }
            "--justify-max-space-stretch" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--justify-max-space-stretch requires a value".to_string())?;
                cfg.justify_max_space_stretch_ratio = v
                    .parse::<f32>()
                    .map_err(|_| format!("invalid --justify-max-space-stretch value '{}'", v))?;
                i += 2;
            }
            "--no-widow-orphan" => {
                cfg.widow_orphan_enabled = false;
                i += 1;
            }
            "--widow-orphan-min-lines" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--widow-orphan-min-lines requires a value".to_string())?;
                cfg.widow_orphan_min_lines = v
                    .parse::<u8>()
                    .map_err(|_| format!("invalid --widow-orphan-min-lines value '{}'", v))?;
                i += 2;
            }
            "--no-hanging-punctuation" => {
                cfg.hanging_punctuation_enabled = false;
                i += 1;
            }
            "--max-image-bytes" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-image-bytes requires a value".to_string())?;
                cfg.max_image_bytes = v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --max-image-bytes value '{}'", v))?;
                i += 2;
            }
            "--max-font-bytes" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--max-font-bytes requires a value".to_string())?;
                cfg.max_font_bytes = v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --max-font-bytes value '{}'", v))?;
                i += 2;
            }
            other => return Err(format!("unknown option '{}'", other)),
        }
    }

    if cfg.display_width == 0 || cfg.display_height == 0 {
        return Err("--width and --height must be > 0".to_string());
    }
    if cfg.prep_base_font_size_px <= 0.0 {
        return Err("--font-size must be > 0".to_string());
    }
    if cfg.justify_min_words == 0 {
        return Err("--justify-min-words must be > 0".to_string());
    }
    if !(0.0..=1.0).contains(&cfg.justify_min_fill_ratio) {
        return Err("--justify-min-fill must be between 0.0 and 1.0".to_string());
    }
    if cfg.justify_max_space_stretch_ratio < 0.0 {
        return Err("--justify-max-space-stretch must be >= 0.0".to_string());
    }
    cfg.justify_strategy =
        justification_strategy_to_string(parse_justification_strategy(&cfg.justify_strategy))
            .to_string();
    cfg.cover_page_mode =
        cover_page_mode_to_string(parse_cover_page_mode(&cfg.cover_page_mode)).to_string();

    Ok(cfg)
}

fn help_text() -> &'static str {
    r#"web-preview - interactive web renderer preview for epub-stream

USAGE:
  cargo run -p epub-stream-render-web --bin web-preview -- [epub_path] [options]

MODES:
  default: generate standalone HTML file at --out
  --serve: start local preview server with live re-render API

OPTIONS:
  --out <file>                output HTML path (default: target/web-preview/index.html)
  --serve                     start local server mode for live re-render
  --open                      open browser automatically (use with --serve)
  --port <n>                  server port in --serve mode (default: 42817)

  --chapter <n>               initial 0-based chapter index (default: all)
  --start-page <n>            initial 0-based page start per chapter (default: 0)
  --pages <n>                 initial pages per chapter limit

  --width <px>                initial viewport width (default: 900)
  --height <px>               initial viewport height (default: 1200)
  --justify / --no-justify    initial justification state (default: on)
  --justify-mode <mode>       adaptive-inter-word|full-inter-word|align-left|align-right|align-center
  --cover-page-mode <mode>    contain|full-bleed|respect-css
  --font-size <px>            initial base font size (default: 22)
  --text-scale <x>            initial text scale (default: 1.0)
  --line-gap <px>             initial line gap (default: 4)
  --paragraph-gap <px>        initial paragraph gap (default: 8)
  --margin-left <px>          initial left margin (default: 24)
  --margin-right <px>         initial right margin (default: 24)
  --margin-top <px>           initial top margin (default: 18)
  --margin-bottom <px>        initial bottom margin (default: 30)
  --justify-min-words <n>     initial min words for justify (default: 6)
  --justify-min-fill <r>      initial min fill ratio for justify (default: 0.78)
  --justify-max-space-stretch <r> max adaptive stretch ratio per space (default: 0.45)
  --no-widow-orphan           disable widow/orphan control initially
  --widow-orphan-min-lines <n> initial widow/orphan min lines (default: 2)
  --no-hanging-punctuation    disable hanging punctuation initially
  --max-image-bytes <n>       max image bytes embedded in payload (default: 16777216)
  --max-font-bytes <n>        max font bytes embedded in payload (default: 25165824)

DEFAULT EPUB:
  tests/fixtures/bench/pg84-frankenstein.epub
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    #[test]
    fn normalize_rel_path_collapses_dot_segments() {
        assert_eq!(
            normalize_rel_path("./OPS/../OPS/text/./ch1.xhtml"),
            "OPS/text/ch1.xhtml"
        );
    }

    #[test]
    fn split_href_fragment_returns_fragment() {
        let (base, fragment) = split_href_fragment("text/ch1.xhtml#intro");
        assert_eq!(base, "text/ch1.xhtml");
        assert_eq!(fragment.as_deref(), Some("intro"));
    }

    #[test]
    fn mime_from_path_detects_svg() {
        assert_eq!(mime_from_path("images/figure.svg"), Some("image/svg+xml"));
    }

    #[test]
    fn font_mime_type_prefers_format_hint() {
        assert_eq!(
            font_mime_type(Some("woff2"), "fonts/book.ttf"),
            Some("font/woff2")
        );
    }

    #[test]
    fn normalized_config_clamps_values() {
        let cfg = RenderUiConfig {
            justify_min_words: 0,
            justify_min_fill_ratio: 9.0,
            justify_max_space_stretch_ratio: 99.0,
            justify_strategy: "bogus".to_string(),
            object_cover_page_mode: "weird-mode".to_string(),
            ui_font_scale: 0.01,
            style_max_selectors: 0,
            memory_max_pages_in_memory: 0,
            ..RenderUiConfig::default()
        }
        .normalized();

        assert_eq!(cfg.justify_min_words, 1);
        assert_eq!(cfg.justify_min_fill_ratio, 1.0);
        assert_eq!(cfg.justify_max_space_stretch_ratio, 8.0);
        assert_eq!(cfg.justify_strategy, "adaptive-inter-word");
        assert_eq!(cfg.object_cover_page_mode, "contain");
        assert_eq!(cfg.ui_font_scale, 0.25);
        assert_eq!(cfg.style_max_selectors, 1);
        assert_eq!(cfg.memory_max_pages_in_memory, 1);
    }

    #[test]
    fn to_engine_options_maps_extended_render_prep_limits() {
        let cfg = RenderUiConfig {
            display_width: 777,
            display_height: 999,
            justify_enabled: false,
            prep_text_scale: 1.2,
            ui_font_scale: 1.4,
            object_cover_page_mode: "full-bleed".to_string(),
            style_max_selectors: 123,
            style_max_css_bytes: 222_222,
            style_max_nesting: 9,
            font_limit_max_faces: 7,
            font_limit_max_bytes_per_font: 333_333,
            font_limit_max_total_font_bytes: 444_444,
            memory_max_entry_bytes: 555_555,
            memory_max_css_bytes: 666_666,
            memory_max_nav_bytes: 777_777,
            memory_max_inline_style_bytes: 8_888,
            memory_max_pages_in_memory: 11,
            ..RenderUiConfig::default()
        };

        let opts = cfg.to_engine_options();
        assert_eq!(opts.layout.display_width, 777);
        assert_eq!(opts.layout.display_height, 999);
        assert!(!opts.layout.typography.justification.enabled);
        assert!((opts.prep.layout_hints.text_scale - 1.68).abs() < 0.0001);
        assert_eq!(
            opts.layout.object_layout.cover_page_mode,
            CoverPageMode::FullBleed
        );
        assert_eq!(opts.prep.style.limits.max_selectors, 123);
        assert_eq!(opts.prep.style.limits.max_css_bytes, 222_222);
        assert_eq!(opts.prep.style.limits.max_nesting, 9);
        assert_eq!(opts.prep.fonts.max_faces, 7);
        assert_eq!(opts.prep.fonts.max_bytes_per_font, 333_333);
        assert_eq!(opts.prep.fonts.max_total_font_bytes, 444_444);
        assert_eq!(opts.prep.memory.max_entry_bytes, 555_555);
        assert_eq!(opts.prep.memory.max_css_bytes, 666_666);
        assert_eq!(opts.prep.memory.max_nav_bytes, 777_777);
        assert_eq!(opts.prep.memory.max_inline_style_bytes, 8_888);
        assert_eq!(opts.prep.memory.max_pages_in_memory, 11);
    }

    #[test]
    fn render_ui_config_json_exposes_reader_and_budget_knobs() {
        let value =
            serde_json::to_value(RenderUiConfig::default()).expect("config should serialize");
        let obj = value
            .as_object()
            .expect("config should serialize to JSON object");
        let required = [
            "display_width",
            "display_height",
            "margin_left",
            "margin_right",
            "line_gap_px",
            "paragraph_gap_px",
            "justify_enabled",
            "justify_strategy",
            "justify_max_space_stretch_ratio",
            "object_cover_page_mode",
            "prep_base_font_size_px",
            "prep_text_scale",
            "ui_font_scale",
            "ui_font_family",
            "embedded_fonts",
            "style_max_selectors",
            "style_max_css_bytes",
            "style_max_nesting",
            "font_limit_max_faces",
            "font_limit_max_bytes_per_font",
            "font_limit_max_total_font_bytes",
            "memory_max_entry_bytes",
            "memory_max_css_bytes",
            "memory_max_nav_bytes",
            "memory_max_inline_style_bytes",
            "memory_max_pages_in_memory",
            "page_chrome_progress_enabled",
            "render_grayscale_mode",
            "render_dither_mode",
            "render_contrast_boost",
        ];
        for key in required {
            assert!(obj.contains_key(key), "missing config key '{}'", key);
        }
    }

    fn fixture_path(name: &str) -> String {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("../../tests/fixtures/bench");
        path.push(name);
        path.to_string_lossy().into_owned()
    }

    fn payload_text_right_px(text: &str, style: &TextStylePayload) -> f32 {
        let chars = text.chars().count();
        if chars == 0 {
            return 0.0;
        }
        let family = style.family.to_ascii_lowercase();
        let proportional = !(family.contains("mono") || family.contains("fixed"));
        let mut em_sum = 0.0f32;
        if proportional {
            for ch in text.chars() {
                em_sum += match ch {
                    ' ' | '\u{00A0}' => 0.32,
                    '\t' => 1.28,
                    'i' | 'l' | 'I' | '|' | '!' => 0.24,
                    '.' | ',' | ':' | ';' | '\'' | '"' | '`' => 0.23,
                    '-' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' => 0.34,
                    '(' | ')' | '[' | ']' | '{' | '}' => 0.30,
                    'f' | 't' | 'j' | 'r' => 0.34,
                    'm' | 'w' | 'M' | 'W' | '@' | '%' | '&' | '#' => 0.74,
                    c if c.is_ascii_digit() => 0.52,
                    c if c.is_ascii_uppercase() => 0.61,
                    c if c.is_ascii_lowercase() => 0.50,
                    c if c.is_whitespace() => 0.32,
                    c if c.is_ascii_punctuation() => 0.42,
                    _ => 0.56,
                };
            }
        } else {
            for ch in text.chars() {
                em_sum += if ch == ' ' { 0.52 } else { 0.58 };
            }
        }
        let mut scale = if proportional { 1.05 } else { 1.02 };
        if style.weight >= 700 {
            scale += 0.02;
        }
        if style.italic {
            scale += 0.01;
        }
        if style.size_px >= 24.0 {
            scale += 0.01;
        }
        let mut width = em_sum * style.size_px * scale;
        if chars > 1 {
            width += (chars as f32 - 1.0) * style.letter_spacing.max(0.0);
        }
        width
    }

    fn assert_payload_text_bounds(payload: &PreviewPayload, max_pages: usize) {
        let content_right =
            (payload.config.display_width as i32 - payload.config.margin_right).max(1) as f32;
        let mut sampled = 0usize;
        for page in payload.pages.iter().take(max_pages) {
            for cmd in &page.commands {
                let CommandPayload::Text { x, text, style, .. } = cmd else {
                    continue;
                };
                if !matches!(style.role.as_str(), "body" | "paragraph" | "list-item") {
                    continue;
                }
                if text.trim().is_empty() {
                    continue;
                }
                let offset = if style.justify.mode == "align-right"
                    || style.justify.mode == "align-center"
                {
                    style.justify.offset_px.unwrap_or(0).max(0) as f32
                } else {
                    0.0
                };
                let right = *x as f32 + offset + payload_text_right_px(text, style);
                assert!(
                    right <= content_right + 2.0,
                    "text likely clipped: '{}' right={} content_right={}",
                    text,
                    right,
                    content_right
                );
                sampled += 1;
            }
        }
        assert!(sampled > 0, "expected to sample body text lines");
    }

    fn assert_payload_single_chapter_metrics(payload: &PreviewPayload) {
        assert!(
            !payload.pages.is_empty(),
            "expected non-empty payload pages"
        );
        let chapter_index = payload.pages[0].chapter_index;
        let mut last_progress = 0.0f32;
        for (idx, page) in payload.pages.iter().enumerate() {
            assert_eq!(page.chapter_index, chapter_index);
            assert_eq!(page.page_index, idx);
            assert_eq!(page.page_number, idx + 1);
            assert_eq!(page.metrics.chapter_page_index, idx);
            assert_eq!(page.metrics.chapter_page_count, Some(payload.pages.len()));
            assert!(page.metrics.progress_chapter >= last_progress);
            last_progress = page.metrics.progress_chapter;
        }
        assert!(
            payload
                .pages
                .last()
                .expect("pages should include last")
                .metrics
                .progress_chapter
                >= 0.90
        );
    }

    fn inter_word_count(payload: &PreviewPayload) -> usize {
        payload
            .pages
            .iter()
            .flat_map(|page| page.commands.iter())
            .filter_map(|cmd| match cmd {
                CommandPayload::Text { style, .. } => Some(style.justify.mode.as_str()),
                _ => None,
            })
            .filter(|mode| *mode == "inter-word")
            .count()
    }

    fn families_in_payload(payload: &PreviewPayload) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for cmd in payload.pages.iter().flat_map(|page| page.commands.iter()) {
            if let CommandPayload::Text { style, .. } = cmd {
                out.insert(style.family.clone());
            }
        }
        out
    }

    #[test]
    fn preview_reflow_matrix_validates_dynamic_reader_knobs() {
        let epub = fixture_path("pg84-frankenstein.epub");
        let baseline_cfg = RenderUiConfig {
            chapter: Some(4),
            justify_enabled: true,
            ui_font_family: "auto".to_string(),
            ..RenderUiConfig::default()
        };
        let baseline =
            render_preview_payload(&epub, &baseline_cfg).expect("baseline render should succeed");
        assert_payload_text_bounds(&baseline, 3);
        assert_payload_single_chapter_metrics(&baseline);
        let baseline_pages = baseline.meta.page_count;
        let baseline_inter_word = inter_word_count(&baseline);
        assert!(
            baseline_inter_word > 0,
            "expected baseline inter-word justification"
        );

        let mut scaled_cfg = baseline_cfg.clone();
        scaled_cfg.ui_font_scale = 1.35;
        let scaled =
            render_preview_payload(&epub, &scaled_cfg).expect("scaled render should succeed");
        assert!(
            scaled.meta.page_count >= baseline_pages,
            "larger reader scale should not reduce page count"
        );
        assert_payload_text_bounds(&scaled, 3);
        assert_payload_single_chapter_metrics(&scaled);

        let mut spaced_cfg = baseline_cfg.clone();
        spaced_cfg.line_gap_px = 8;
        spaced_cfg.paragraph_gap_px = 14;
        let spaced =
            render_preview_payload(&epub, &spaced_cfg).expect("spaced render should succeed");
        assert!(
            spaced.meta.page_count >= baseline_pages,
            "larger line/paragraph spacing should not reduce page count"
        );
        assert_payload_text_bounds(&spaced, 3);
        assert_payload_single_chapter_metrics(&spaced);

        let mut no_justify_cfg = baseline_cfg.clone();
        no_justify_cfg.justify_enabled = false;
        let no_justify = render_preview_payload(&epub, &no_justify_cfg)
            .expect("no-justify render should succeed");
        assert_eq!(inter_word_count(&no_justify), 0);
        assert_payload_text_bounds(&no_justify, 3);
        assert_payload_single_chapter_metrics(&no_justify);

        let mut mono_cfg = baseline_cfg.clone();
        mono_cfg.ui_font_family = "monospace".to_string();
        let mono = render_preview_payload(&epub, &mono_cfg)
            .expect("forced monospace render should succeed");
        assert!(
            families_in_payload(&mono)
                .iter()
                .any(|family| family.to_ascii_lowercase().contains("mono")),
            "expected monospace family in text payload"
        );
        assert_payload_single_chapter_metrics(&mono);

        let mut constrained_cfg = baseline_cfg.clone();
        constrained_cfg.memory_max_pages_in_memory = 1;
        let constrained = render_preview_payload(&epub, &constrained_cfg);
        assert!(
            constrained.is_err(),
            "tight page memory limit should fail for multi-page chapter"
        );
    }

    #[test]
    fn frankenstein_chapter_titles_prefer_toc_labels_over_href_filenames() {
        let epub = fixture_path("pg84-frankenstein.epub");
        let cfg = RenderUiConfig {
            chapter: Some(4),
            ..RenderUiConfig::default()
        };
        let payload = render_preview_payload(&epub, &cfg).expect("render should succeed");

        let chapter = payload
            .chapters
            .iter()
            .find(|c| c.href.contains("84-h-1.htm.html"))
            .expect("frankenstein chapter href should exist");
        let title = chapter
            .title
            .as_deref()
            .map(str::trim)
            .expect("chapter should have TOC-derived title");

        assert!(!title.is_empty(), "title should be non-empty");
        assert_ne!(title, chapter.href, "title should not mirror href");
        assert!(
            !title.contains(".htm"),
            "title should not look like an html file name: {}",
            title
        );
    }
}
