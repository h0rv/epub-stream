//! DHAT heap profiler for epub-stream.
//!
//! Profiles allocation patterns across the full EPUB pipeline:
//! open -> metadata -> tokenize -> style -> render.
//!
//! Usage:
//!   cargo run -p epub-stream-heap-profile --release -- [OPTIONS] [EPUB_FILES...]
//!
//! Or via just:
//!   just analysis heap-profile [phase] [files...]
//!
//! Outputs dhat-<phase>.json files in the output directory (default: target/memory).
//! Open in https://nnethercote.github.io/dh_view/dh_view.html

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use std::path::{Path, PathBuf};
use std::process::Command;

use epub_stream::book::OpenConfig;
use epub_stream::metadata::MetadataLimits;
use epub_stream::navigation::NavigationLimits;
use epub_stream::{
    CoverImageOptions, EpubBook, EpubBookOptions, ImageReadOptions, ValidationMode, ZipLimits,
};
use epub_stream_render::{RenderConfig, RenderEngine, RenderEngineOptions};

const DISPLAY_WIDTH: i32 = 480;
const DISPLAY_HEIGHT: i32 = 800;

const DEFAULT_FIXTURES: &[&str] = &[
    "tests/fixtures/bench/pg84-frankenstein.epub",
    "tests/fixtures/bench/pg1342-pride-and-prejudice.epub",
    "tests/fixtures/bench/pg1661-sherlock-holmes.epub",
    "tests/fixtures/bench/pg2701-moby-dick.epub",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Open,
    OpenTemp,
    Cover,
    Tokenize,
    Render,
    Full,
    SessionOnce,
    Session,
}

impl Phase {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "open-temp" | "open_temp" => Some(Self::OpenTemp),
            "cover" => Some(Self::Cover),
            "tokenize" => Some(Self::Tokenize),
            "render" => Some(Self::Render),
            "full" => Some(Self::Full),
            "session-once" | "session_once" => Some(Self::SessionOnce),
            "session" => Some(Self::Session),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::OpenTemp => "open_temp",
            Self::Cover => "cover",
            Self::Tokenize => "tokenize",
            Self::Render => "render",
            Self::Full => "full",
            Self::SessionOnce => "session_once",
            Self::Session => "session",
        }
    }
}

fn pick_text_chapter(book: &mut EpubBook<std::fs::File>) -> usize {
    let count = book.chapter_count();
    for idx in 0..count.min(12) {
        if let Ok(tokens) = book.tokenize_spine_item(idx) {
            if !tokens.is_empty() {
                return idx;
            }
        }
    }
    0
}

fn profile_file(path: &Path, phase: Phase) {
    let path_str = path.to_string_lossy();

    match phase {
        Phase::Open => {
            let _book = EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path_str, e));
        }
        Phase::OpenTemp => {
            let temp_dir = std::env::temp_dir().join("xteink-epub-temp-profile");
            std::fs::create_dir_all(&temp_dir)
                .unwrap_or_else(|e| panic!("create temp dir {}: {}", temp_dir.display(), e));
            let _book = EpubBook::open_with_temp_storage(
                path,
                &temp_dir,
                OpenConfig {
                    options: EpubBookOptions {
                        zip_limits: Some(
                            ZipLimits::new(256 * 1024, 128).with_max_eocd_scan(8 * 1024),
                        ),
                        validation_mode: ValidationMode::Lenient,
                        max_nav_bytes: Some(32 * 1024),
                        navigation_limits: NavigationLimits::embedded(),
                        metadata_limits: MetadataLimits::embedded(),
                    },
                    lazy_navigation: true,
                },
            )
            .unwrap_or_else(|e| panic!("open_temp {}: {}", path_str, e));
        }
        Phase::Cover => {
            let mut book =
                EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path_str, e));
            let mut cover_buf = Vec::with_capacity(8);
            let cover_opts = CoverImageOptions {
                image: ImageReadOptions {
                    max_bytes: 4 * 1024 * 1024,
                    allow_svg: true,
                    allow_unknown_images: true,
                },
                ..CoverImageOptions::default()
            };
            for _ in 0..5 {
                let _ = book.read_cover_image_into_with_options(&mut cover_buf, cover_opts);
            }
        }
        Phase::Tokenize => {
            let mut book =
                EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path_str, e));
            let ch = pick_text_chapter(&mut book);
            let _tokens = book
                .tokenize_spine_item(ch)
                .unwrap_or_else(|e| panic!("tokenize {}: {}", path_str, e));
        }
        Phase::Render => {
            let mut book =
                EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path_str, e));
            let ch = pick_text_chapter(&mut book);
            let engine = RenderEngine::new(RenderEngineOptions::for_display(
                DISPLAY_WIDTH,
                DISPLAY_HEIGHT,
            ));
            let config =
                epub_stream_embedded_graphics::with_embedded_text_measurer(RenderConfig::default());
            let _pages = engine
                .prepare_chapter_with_config_collect(&mut book, ch, config)
                .unwrap_or_else(|e| panic!("render {}: {}", path_str, e));
        }
        Phase::Full => {
            let mut book =
                EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path_str, e));
            let engine = RenderEngine::new(RenderEngineOptions::for_display(
                DISPLAY_WIDTH,
                DISPLAY_HEIGHT,
            ));
            let config =
                epub_stream_embedded_graphics::with_embedded_text_measurer(RenderConfig::default());
            let count = book.chapter_count();
            for ch in 0..count {
                let _ = engine.prepare_chapter_with_config_collect(&mut book, ch, config.clone());
            }
        }
        Phase::SessionOnce | Phase::Session => {
            let mut book =
                EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path_str, e));
            let mut cover_buf = Vec::with_capacity(8);
            let cover_opts = CoverImageOptions {
                image: ImageReadOptions {
                    max_bytes: 4 * 1024 * 1024,
                    allow_svg: true,
                    allow_unknown_images: true,
                },
                ..CoverImageOptions::default()
            };
            for _ in 0..3 {
                let _ = book.read_cover_image_into_with_options(&mut cover_buf, cover_opts);
            }

            // Stream-like page flip simulation: render chapter-by-chapter without retaining page vecs.
            let engine = RenderEngine::new(RenderEngineOptions::for_display(
                DISPLAY_WIDTH,
                DISPLAY_HEIGHT,
            ));
            let config =
                epub_stream_embedded_graphics::with_embedded_text_measurer(RenderConfig::default());
            let mut flips = 0usize;
            let count = book.chapter_count();
            let pass_count = if matches!(phase, Phase::Session) {
                2
            } else {
                1
            };
            for _pass in 0..pass_count {
                for ch in 0..count {
                    let result = engine.prepare_chapter_with_config_page_refs(
                        &mut book,
                        ch,
                        config.clone(),
                        |_| {
                            flips = flips.saturating_add(1);
                        },
                    );
                    let _ = result;
                }
            }
            if flips == 0 {
                panic!("session {} produced zero pages", path_str);
            }
        }
    }
}

/// Extract a short name from a file path for use in output filenames.
fn short_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string())
}

fn usage() {
    eprintln!("Usage: heap-profile [OPTIONS] [EPUB_FILES...]");
    eprintln!();
    eprintln!("Options:");
    eprintln!(
        "  --phase <open|open_temp|cover|tokenize|render|full|session_once|session>  Pipeline phase to profile (default: render)"
    );
    eprintln!("  --out-dir <DIR>                      Output directory for dhat JSON (default: target/memory)");
    eprintln!(
        "  --aggregate                          Single profile for all files (default: per-file)"
    );
    eprintln!();
    eprintln!("By default, each EPUB gets its own clean DHAT profile (separate process).");
    eprintln!("With --aggregate, all files share one profile.");
    eprintln!();
    eprintln!("If no EPUB files are given, profiles the default Gutenberg fixtures.");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut phase = Phase::Render;
    let mut out_dir = PathBuf::from("target/memory");
    let mut files: Vec<PathBuf> = Vec::with_capacity(8);
    let mut aggregate = false;
    // Internal flag: when set, we're a child process profiling a single file.
    let mut single_file_mode = false;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--phase" => {
                i += 1;
                phase = Phase::from_str(&args[i]).unwrap_or_else(|| {
                    eprintln!("Unknown phase: {}", args[i]);
                    usage();
                    std::process::exit(1);
                });
            }
            "--out-dir" => {
                i += 1;
                out_dir = PathBuf::from(&args[i]);
            }
            "--aggregate" => {
                aggregate = true;
            }
            "--single-file" => {
                // Internal: child process mode — profile exactly one file with DHAT active.
                single_file_mode = true;
            }
            "--help" | "-h" => {
                usage();
                std::process::exit(0);
            }
            other => {
                files.push(PathBuf::from(other));
            }
        }
        i += 1;
    }

    if files.is_empty() {
        for f in DEFAULT_FIXTURES {
            let p = PathBuf::from(f);
            if p.exists() {
                files.push(p);
            }
        }
    }

    if files.is_empty() {
        eprintln!("No EPUB files found. Provide paths or ensure test fixtures exist.");
        std::process::exit(1);
    }

    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| {
        eprintln!("Failed to create output dir {}: {}", out_dir.display(), e);
        std::process::exit(1);
    });

    let phase_name = phase.name();

    // Child process mode: profile exactly the one file with DHAT active.
    if single_file_mode {
        assert!(files.len() == 1, "--single-file expects exactly one file");
        let file = &files[0];
        let name = short_name(file);
        let json_path = out_dir.join(format!("dhat-{phase_name}-{name}.json"));

        let _profiler = dhat::Profiler::builder().file_name(json_path).build();

        profile_file(file, phase);
        // _profiler drops here, writes JSON
        return;
    }

    // Aggregate mode: single DHAT session for all files.
    if aggregate {
        let json_path = out_dir.join(format!("dhat-{phase_name}.json"));
        eprintln!(
            "heap-profile: phase={}, files={} (aggregate), out={}",
            phase_name,
            files.len(),
            out_dir.display()
        );

        let _profiler = dhat::Profiler::builder()
            .file_name(json_path.clone())
            .build();

        for file in &files {
            eprintln!("  profiling: {}", file.display());
            profile_file(file, phase);
        }

        // _profiler drops here, writes JSON
        eprintln!(
            "Done. Open {} in https://nnethercote.github.io/dh_view/dh_view.html",
            json_path.display()
        );
        return;
    }

    // Per-file mode (default): spawn a child process per file for clean DHAT sessions.
    let self_exe = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("Failed to determine own executable path: {}", e);
        std::process::exit(1);
    });

    eprintln!(
        "heap-profile: phase={}, files={} (per-file), out={}",
        phase_name,
        files.len(),
        out_dir.display()
    );

    let mut any_failed = false;
    for file in &files {
        let name = short_name(file);
        eprintln!(
            "  profiling: {} -> dhat-{}-{}.json",
            file.display(),
            phase_name,
            name
        );

        let status = Command::new(&self_exe)
            .arg("--single-file")
            .arg("--phase")
            .arg(phase_name)
            .arg("--out-dir")
            .arg(&out_dir)
            .arg(file)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("    FAILED (exit {})", s.code().unwrap_or(-1));
                any_failed = true;
            }
            Err(e) => {
                eprintln!("    FAILED to spawn: {}", e);
                any_failed = true;
            }
        }
    }

    eprintln!();
    eprintln!("Profiles saved to {}:", out_dir.display());
    for file in &files {
        let name = short_name(file);
        let json_path = out_dir.join(format!("dhat-{phase_name}-{name}.json"));
        if json_path.exists() {
            eprintln!("  {}", json_path.display());
        }
    }
    eprintln!();
    eprintln!("Open in https://nnethercote.github.io/dh_view/dh_view.html");

    if any_failed {
        std::process::exit(1);
    }
}
