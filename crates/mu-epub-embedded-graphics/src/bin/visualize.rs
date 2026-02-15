use std::env;
use std::process::ExitCode;

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{OriginDimensions, Size};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::Pixel;
use mu_epub::EpubBook;
use mu_epub_embedded_graphics::EgRenderer;
use mu_epub_render::{RenderConfig, RenderEngine, RenderEngineOptions};

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
    opts.layout.margin_left = 10;
    opts.layout.margin_right = 10;
    opts.layout.margin_top = 4;
    opts.layout.margin_bottom = 24;
    opts.layout.first_line_indent_px = 0;
    opts.layout.paragraph_gap_px = 8;
    opts.layout.line_gap_px = 4;
    opts.layout.typography.justification.enabled = cfg.justify;
    opts.layout.typography.justification.min_words = 6;
    opts.layout.typography.justification.min_fill_ratio = 0.78;
    opts.prep.layout_hints.base_font_size_px = 22.0;
    opts.prep.layout_hints.text_scale = 1.0;
    opts.prep.layout_hints.min_font_size_px = 14.0;
    opts.prep.layout_hints.max_font_size_px = 40.0;
    opts.prep.layout_hints.min_line_height = 1.05;
    opts.prep.layout_hints.max_line_height = 1.35;
    opts.prep.style.hints = opts.prep.layout_hints;

    let engine = RenderEngine::new(opts);
    let renderer: EgRenderer = EgRenderer::default();

    let mut chapter_total_pages = 0usize;
    engine
        .prepare_chapter_with(&mut book, cfg.chapter, |_| {
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
                RenderConfig::default().with_page_range(page_range),
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
            other => return Err(format!("unknown option '{}'", other)),
        }
    }

    if cfg.pages == 0 {
        return Err("--pages must be > 0".to_string());
    }
    if cfg.width == 0 || cfg.height == 0 {
        return Err("--width and --height must be > 0".to_string());
    }

    Ok(cfg)
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
