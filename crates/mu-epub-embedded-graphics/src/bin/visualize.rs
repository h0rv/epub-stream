use std::env;
use std::process::ExitCode;

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{OriginDimensions, Size};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::Pixel;
use mu_epub::EpubBook;
use mu_epub_embedded_graphics::{with_embedded_text_measurer, EgRenderer};
use mu_epub_render::{CoverPageMode, RenderConfig, RenderEngine, RenderEngineOptions};

const DEFAULT_EPUB_PATH: &str = "tests/fixtures/bench/pg84-frankenstein.epub";

#[derive(Clone, Debug)]
struct Args {
    epub_path: String,
    chapter: usize,
    start_page: usize,
    pages: usize,
    out_dir: String,
    width: u32,
    height: u32,
    justify: bool,
    font_size_px: f32,
    line_gap_px: i32,
    paragraph_gap_px: i32,
    margin_left: i32,
    margin_right: i32,
    margin_top: i32,
    margin_bottom: i32,
    justify_min_words: usize,
    justify_min_fill_ratio: f32,
    cover_page_mode: CoverPageMode,
    widow_orphan: bool,
    widow_orphan_min_lines: u8,
    hanging_punctuation: bool,
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
    let cfg = parse_args(args)?;
    std::fs::create_dir_all(&cfg.out_dir).map_err(|e| e.to_string())?;
    clear_previous_outputs(&cfg.out_dir)?;

    let mut book = EpubBook::open(&cfg.epub_path).map_err(|e| e.to_string())?;
    let chapter_count = book.chapter_count();
    if chapter_count == 0 {
        return Err("EPUB has no chapters".to_string());
    }
    if cfg.chapter >= chapter_count {
        return Err(format!(
            "chapter {} out of range (chapter_count={})",
            cfg.chapter, chapter_count
        ));
    }

    let mut opts = RenderEngineOptions::for_display(cfg.width as i32, cfg.height as i32);
    // Xteink-like layout defaults so snapshots resemble on-device output.
    opts.layout.margin_left = cfg.margin_left;
    opts.layout.margin_right = cfg.margin_right;
    opts.layout.margin_top = cfg.margin_top;
    opts.layout.margin_bottom = cfg.margin_bottom;
    opts.layout.first_line_indent_px = 0;
    opts.layout.paragraph_gap_px = cfg.paragraph_gap_px;
    opts.layout.line_gap_px = cfg.line_gap_px;
    opts.layout.typography.justification.enabled = cfg.justify;
    opts.layout.typography.justification.min_words = cfg.justify_min_words;
    opts.layout.typography.justification.min_fill_ratio = cfg.justify_min_fill_ratio;
    opts.layout.object_layout.cover_page_mode = cfg.cover_page_mode;
    opts.layout.typography.widow_orphan_control.enabled = cfg.widow_orphan;
    opts.layout.typography.widow_orphan_control.min_lines = cfg.widow_orphan_min_lines.max(1);
    opts.layout.typography.hanging_punctuation.enabled = cfg.hanging_punctuation;
    opts.prep.layout_hints.base_font_size_px = cfg.font_size_px;
    opts.prep.layout_hints.text_scale = 1.0;
    opts.prep.layout_hints.min_font_size_px = 14.0;
    opts.prep.layout_hints.max_font_size_px = 40.0;
    opts.prep.layout_hints.min_line_height = 1.05;
    opts.prep.layout_hints.max_line_height = 1.35;
    opts.prep.style.hints = opts.prep.layout_hints;

    let engine = RenderEngine::new(opts);
    let renderer: EgRenderer = EgRenderer::default();
    let base_render_cfg = with_embedded_text_measurer(RenderConfig::default());

    let mut chapter_total_pages = 0usize;
    engine
        .prepare_chapter_with_config(&mut book, cfg.chapter, base_render_cfg.clone(), |_| {
            chapter_total_pages += 1;
        })
        .map_err(|e| format!("unable to count chapter pages: {}", e))?;

    let mut rendered = 0usize;
    let mut manifest = String::from(
        "file\tchapter_idx\tpage_idx\tpage_number\tchapter_page_count\tprogress_chapter\n",
    );
    for offset in 0..cfg.pages {
        let page_idx = cfg.start_page + offset;
        let page_range = page_idx..page_idx + 1;
        let pages = engine
            .prepare_chapter_with_config_collect(
                &mut book,
                cfg.chapter,
                base_render_cfg.clone().with_page_range(page_range),
            )
            .map_err(|e| {
                format!(
                    "render failed at chapter={} page={}: {}",
                    cfg.chapter, page_idx, e
                )
            })?;
        if pages.is_empty() {
            break;
        }

        let page = &pages[0];
        let mut display = BitmapDisplay::new(cfg.width, cfg.height);
        renderer
            .render_page(page, &mut display)
            .map_err(|_| "render backend failed".to_string())?;

        let file_name = format!(
            "chapter_{:03}_page_{:04}.pgm",
            cfg.chapter + 1,
            page_idx + 1
        );
        let file_path = format!("{}/{}", cfg.out_dir, file_name);
        display.save_pgm(&file_path)?;

        manifest.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{:.4}\n",
            file_name,
            cfg.chapter,
            page_idx,
            page.page_number,
            chapter_total_pages,
            (page_idx as f32 / chapter_total_pages.max(1) as f32).clamp(0.0, 1.0),
        ));
        rendered += 1;
    }

    if rendered == 0 {
        return Err("no pages rendered for requested chapter/page range".to_string());
    }
    let manifest_path = format!("{}/manifest.tsv", cfg.out_dir);
    std::fs::write(&manifest_path, manifest).map_err(|e| e.to_string())?;
    println!(
        "rendered {} page(s) to {} (manifest: {})",
        rendered, cfg.out_dir, manifest_path
    );
    Ok(())
}

fn clear_previous_outputs(out_dir: &str) -> Result<(), String> {
    let entries = std::fs::read_dir(out_dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if (name.starts_with("chapter_") && (name.ends_with(".pgm") || name.ends_with(".png")))
                || name == "manifest.tsv"
            {
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
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
        chapter: 5,
        start_page: 0,
        pages: 12,
        out_dir: "target/visualize-default".to_string(),
        width: 480,
        height: 800,
        justify: false,
        font_size_px: 22.0,
        line_gap_px: 4,
        paragraph_gap_px: 8,
        margin_left: 10,
        margin_right: 10,
        margin_top: 8,
        margin_bottom: 24,
        justify_min_words: 6,
        justify_min_fill_ratio: 0.78,
        cover_page_mode: CoverPageMode::Contain,
        widow_orphan: true,
        widow_orphan_min_lines: 2,
        hanging_punctuation: true,
    };

    let mut i = if has_positional_epub { 2usize } else { 1usize };
    while i < args.len() {
        match args[i].as_str() {
            "--chapter" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--chapter requires a value".to_string())?;
                cfg.chapter = v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --chapter value '{}'", v))?;
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
                cfg.pages = v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --pages value '{}'", v))?;
                i += 2;
            }
            "--out" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--out requires a value".to_string())?;
                cfg.out_dir = v.clone();
                i += 2;
            }
            "--width" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--width requires a value".to_string())?;
                cfg.width = v
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --width value '{}'", v))?;
                i += 2;
            }
            "--height" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--height requires a value".to_string())?;
                cfg.height = v
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --height value '{}'", v))?;
                i += 2;
            }
            "--justify" => {
                cfg.justify = true;
                i += 1;
            }
            "--font-size" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--font-size requires a value".to_string())?;
                cfg.font_size_px = v
                    .parse::<f32>()
                    .map_err(|_| format!("invalid --font-size value '{}'", v))?;
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
            "--cover-page-mode" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--cover-page-mode requires a value".to_string())?;
                cfg.cover_page_mode = parse_cover_page_mode(v)
                    .ok_or_else(|| format!("invalid --cover-page-mode value '{}'", v))?;
                i += 2;
            }
            "--no-widow-orphan" => {
                cfg.widow_orphan = false;
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
                cfg.hanging_punctuation = false;
                i += 1;
            }
            other => return Err(format!("unknown option '{}'", other)),
        }
    }

    if cfg.pages == 0 {
        return Err("--pages must be > 0".to_string());
    }
    if cfg.width == 0 || cfg.height == 0 {
        return Err("--width and --height must be > 0".to_string());
    }
    if cfg.font_size_px <= 0.0 {
        return Err("--font-size must be > 0".to_string());
    }
    if !(0.0..=1.0).contains(&cfg.justify_min_fill_ratio) {
        return Err("--justify-min-fill must be between 0.0 and 1.0".to_string());
    }

    Ok(cfg)
}

fn parse_cover_page_mode(value: &str) -> Option<CoverPageMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "contain" => Some(CoverPageMode::Contain),
        "full-bleed" | "full_bleed" | "fullbleed" => Some(CoverPageMode::FullBleed),
        "respect-css" | "respect_css" | "respectcss" | "css" => Some(CoverPageMode::RespectCss),
        _ => None,
    }
}

fn help_text() -> &'static str {
    r#"visualize - render EPUB pages to PGM snapshots

USAGE:
  cargo run -p mu-epub-embedded-graphics --bin visualize -- [epub_path] [options]

OPTIONS:
  --chapter <n>       0-based chapter index (default: 5)
  --start-page <n>    0-based first page index (default: 0)
  --pages <n>         number of pages to render (default: 12)
  --out <dir>         output directory (default: target/visualize-default)
  --width <px>        viewport width (default: 480)
  --height <px>       viewport height (default: 800)
  --justify           enable inter-word justification (default: off)
  --font-size <px>    base font size in px (default: 22)
  --line-gap <px>     extra line gap in px (default: 4)
  --paragraph-gap <px> paragraph gap in px (default: 8)
  --margin-left <px>  left margin (default: 10)
  --margin-right <px> right margin (default: 10)
  --margin-top <px>   top margin (default: 8)
  --margin-bottom <px> bottom margin (default: 24)
  --justify-min-words <n> minimum words for justification (default: 6)
  --justify-min-fill <r> minimum fill ratio for justification (default: 0.78)
  --cover-page-mode <mode> contain|full-bleed|respect-css (default: contain)
  --no-widow-orphan   disable widow/orphan control
  --widow-orphan-min-lines <n> min lines for widow/orphan (default: 2)
  --no-hanging-punctuation disable hanging punctuation

DEFAULT EPUB:
  tests/fixtures/bench/pg84-frankenstein.epub
"#
}

#[derive(Clone, Debug)]
struct BitmapDisplay {
    size: Size,
    pixels: Vec<BinaryColor>,
}

impl BitmapDisplay {
    fn new(width: u32, height: u32) -> Self {
        let len = width.saturating_mul(height) as usize;
        Self {
            size: Size::new(width, height),
            pixels: vec![BinaryColor::Off; len],
        }
    }

    fn save_pgm(&self, path: &str) -> Result<(), String> {
        let mut data = Vec::with_capacity(self.pixels.len() + 64);
        data.extend_from_slice(
            format!("P5\n{} {}\n255\n", self.size.width, self.size.height).as_bytes(),
        );
        for color in &self.pixels {
            data.push(match color {
                BinaryColor::On => 0u8,
                BinaryColor::Off => 255u8,
            });
        }
        std::fs::write(path, data).map_err(|e| e.to_string())
    }
}

impl OriginDimensions for BitmapDisplay {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for BitmapDisplay {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let w = self.size.width as i32;
        let h = self.size.height as i32;
        for Pixel(point, color) in pixels {
            if point.x < 0 || point.y < 0 || point.x >= w || point.y >= h {
                continue;
            }
            let idx = (point.y as u32 * self.size.width + point.x as u32) as usize;
            self.pixels[idx] = color;
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        for pixel in &mut self.pixels {
            *pixel = color;
        }
        Ok(())
    }
}
