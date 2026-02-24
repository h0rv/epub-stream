use std::env;
use std::process::ExitCode;

use epub_stream::metadata::EpubMetadata;
use epub_stream::navigation::NavPoint;
use epub_stream::validate::{validate_epub_file, ValidationDiagnostic, ValidationSeverity};
use epub_stream::{ChapterRef, EpubBook, EpubError};

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool(bool),
    Num(usize),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn render(&self, pretty: bool) -> String {
        let mut out = String::with_capacity(256);
        self.write_into(&mut out, pretty, 0);
        out
    }

    fn write_into(&self, out: &mut String, pretty: bool, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Json::Num(v) => out.push_str(&v.to_string()),
            Json::Str(v) => write_json_string(out, v),
            Json::Arr(items) => {
                out.push('[');
                if !items.is_empty() && pretty {
                    out.push('\n');
                }
                for (idx, item) in items.iter().enumerate() {
                    if pretty {
                        write_indent(out, depth + 1);
                    }
                    item.write_into(out, pretty, depth + 1);
                    if idx + 1 != items.len() {
                        out.push(',');
                    }
                    if pretty {
                        out.push('\n');
                    }
                }
                if !items.is_empty() && pretty {
                    write_indent(out, depth);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                if !fields.is_empty() && pretty {
                    out.push('\n');
                }
                for (idx, (key, value)) in fields.iter().enumerate() {
                    if pretty {
                        write_indent(out, depth + 1);
                    }
                    write_json_string(out, key);
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    value.write_into(out, pretty, depth + 1);
                    if idx + 1 != fields.len() {
                        out.push(',');
                    }
                    if pretty {
                        out.push('\n');
                    }
                }
                if !fields.is_empty() && pretty {
                    write_indent(out, depth);
                }
                out.push('}');
            }
        }
    }
}

fn write_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn main() -> ExitCode {
    match run(env::args().collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {}", msg);
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut rest = args.into_iter().skip(1).collect::<Vec<_>>();
    let pretty = pop_flag(&mut rest, "--pretty");

    if rest.is_empty() || rest[0] == "--help" || rest[0] == "-h" {
        print_help();
        return Ok(());
    }

    let cmd = rest.remove(0);
    match cmd.as_str() {
        "metadata" => {
            let path = first_arg(&rest, "metadata requires <epub_path>")?;
            let book = EpubBook::open(&path).map_err(display_err)?;
            let output = Json::Obj(vec![
                ("epub".to_string(), Json::Str(path)),
                ("metadata".to_string(), metadata_json(book.metadata())),
            ]);
            println!("{}", output.render(pretty));
        }
        "spine" => {
            let path = first_arg(&rest, "spine requires <epub_path>")?;
            let book = EpubBook::open(&path).map_err(display_err)?;
            let items = book
                .spine()
                .items()
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    Json::Obj(vec![
                        ("index".to_string(), Json::Num(index)),
                        ("idref".to_string(), Json::Str(item.idref.clone())),
                        (
                            "id".to_string(),
                            item.id.clone().map_or(Json::Null, Json::Str),
                        ),
                        ("linear".to_string(), Json::Bool(item.linear)),
                        (
                            "properties".to_string(),
                            item.properties.clone().map_or(Json::Null, Json::Str),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            let output = Json::Obj(vec![
                ("epub".to_string(), Json::Str(path)),
                ("count".to_string(), Json::Num(items.len())),
                ("spine".to_string(), Json::Arr(items)),
            ]);
            println!("{}", output.render(pretty));
        }
        "toc" => {
            let mut args = rest;
            let flat = pop_flag(&mut args, "--flat");
            let path = first_arg(&args, "toc requires <epub_path>")?;
            let book = EpubBook::open(&path).map_err(display_err)?;
            let toc = if flat {
                Json::Arr(
                    book.navigation()
                        .map(|n| n.toc_flat())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(depth, point)| {
                            Json::Obj(vec![
                                ("depth".to_string(), Json::Num(depth)),
                                ("label".to_string(), Json::Str(point.label.clone())),
                                ("href".to_string(), Json::Str(point.href.clone())),
                                (
                                    "children_count".to_string(),
                                    Json::Num(point.children.len()),
                                ),
                            ])
                        })
                        .collect(),
                )
            } else {
                Json::Arr(
                    book.toc()
                        .unwrap_or(&[])
                        .iter()
                        .map(nav_point_json)
                        .collect(),
                )
            };
            let output = Json::Obj(vec![
                ("epub".to_string(), Json::Str(path)),
                ("toc".to_string(), toc),
            ]);
            println!("{}", output.render(pretty));
        }
        "chapters" => {
            let mut args = rest;
            let ndjson = pop_flag(&mut args, "--ndjson");
            let path = first_arg(&args, "chapters requires <epub_path>")?;
            let book = EpubBook::open(&path).map_err(display_err)?;
            let chapters = book.chapters().collect::<Vec<_>>();
            if ndjson {
                for chapter in &chapters {
                    let obj = chapter_json(chapter);
                    println!("{}", obj.render(false));
                }
            } else {
                let output = Json::Obj(vec![
                    ("epub".to_string(), Json::Str(path)),
                    ("count".to_string(), Json::Num(chapters.len())),
                    (
                        "chapters".to_string(),
                        Json::Arr(chapters.iter().map(chapter_json).collect()),
                    ),
                ]);
                println!("{}", output.render(pretty));
            }
        }
        "chapter-text" => {
            let (path, selector, raw) = parse_chapter_args(rest, "chapter-text")?;
            let mut book = EpubBook::open(&path).map_err(display_err)?;
            let chapter = resolve_chapter(&book, selector)?;
            let text = book.chapter_text(chapter.index).map_err(display_err)?;
            if raw {
                print!("{}", text);
            } else {
                let output = Json::Obj(vec![
                    ("epub".to_string(), Json::Str(path)),
                    ("chapter".to_string(), chapter_json(&chapter)),
                    ("text".to_string(), Json::Str(text)),
                ]);
                println!("{}", output.render(pretty));
            }
        }
        "chapter-html" => {
            let (path, selector, raw) = parse_chapter_args(rest, "chapter-html")?;
            let mut book = EpubBook::open(&path).map_err(display_err)?;
            let chapter = resolve_chapter(&book, selector)?;
            let html = book.chapter_html(chapter.index).map_err(display_err)?;
            if raw {
                print!("{}", html);
            } else {
                let output = Json::Obj(vec![
                    ("epub".to_string(), Json::Str(path)),
                    ("chapter".to_string(), chapter_json(&chapter)),
                    ("html".to_string(), Json::Str(html)),
                ]);
                println!("{}", output.render(pretty));
            }
        }
        "validate" => {
            let mut args = rest;
            let strict = pop_flag(&mut args, "--strict");
            let path = first_arg(&args, "validate requires <epub_path>")?;
            let report = validate_epub_file(&path).map_err(display_err)?;

            let output = Json::Obj(vec![
                ("epub".to_string(), Json::Str(path)),
                ("valid".to_string(), Json::Bool(report.is_valid())),
                ("error_count".to_string(), Json::Num(report.error_count())),
                (
                    "warning_count".to_string(),
                    Json::Num(report.warning_count()),
                ),
                (
                    "diagnostics".to_string(),
                    Json::Arr(report.diagnostics().iter().map(diagnostic_json).collect()),
                ),
            ]);
            println!("{}", output.render(pretty));

            let has_failures = if strict {
                report.error_count() > 0 || report.warning_count() > 0
            } else {
                report.error_count() > 0
            };
            if has_failures {
                return Err(if strict {
                    "validation failed (strict mode)".to_string()
                } else {
                    "validation failed".to_string()
                });
            }
        }
        _ => {
            return Err(format!(
                "unknown command '{}'; run `epub-stream --help` for usage",
                cmd
            ));
        }
    }

    Ok(())
}

#[derive(Clone, Debug)]
enum ChapterSelector {
    Index(usize),
    Id(String),
}

fn parse_chapter_args(
    mut args: Vec<String>,
    command: &str,
) -> Result<(String, ChapterSelector, bool), String> {
    let raw = pop_flag(&mut args, "--raw");
    let path = first_arg(&args, &format!("{} requires <epub_path>", command))?;

    let mut index = None;
    let mut id = None;
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--index requires a value".to_string())?;
                index = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| format!("invalid --index value '{}'", value))?,
                );
                i += 2;
            }
            "--id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--id requires a value".to_string())?;
                id = Some(value.clone());
                i += 2;
            }
            _ => i += 1,
        }
    }

    let selector = match (index, id) {
        (Some(_), Some(_)) => {
            return Err("use only one selector: --index <n> or --id <idref>".to_string());
        }
        (Some(v), None) => ChapterSelector::Index(v),
        (None, Some(v)) => ChapterSelector::Id(v),
        (None, None) => {
            return Err(format!(
                "{} requires chapter selector: --index <n> or --id <idref>",
                command
            ));
        }
    };

    Ok((path, selector, raw))
}

fn resolve_chapter(
    book: &EpubBook<std::fs::File>,
    selector: ChapterSelector,
) -> Result<ChapterRef, String> {
    match selector {
        ChapterSelector::Index(v) => book.chapter(v).map_err(display_err),
        ChapterSelector::Id(v) => book.chapter_by_id(&v).map_err(display_err),
    }
}

fn first_arg(args: &[String], msg: &str) -> Result<String, String> {
    args.first().cloned().ok_or_else(|| msg.to_string())
}

fn pop_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        args.remove(pos);
        true
    } else {
        false
    }
}

fn nav_point_json(point: &NavPoint) -> Json {
    Json::Obj(vec![
        ("label".to_string(), Json::Str(point.label.clone())),
        ("href".to_string(), Json::Str(point.href.clone())),
        (
            "children".to_string(),
            Json::Arr(point.children.iter().map(nav_point_json).collect()),
        ),
    ])
}

fn chapter_json(chapter: &ChapterRef) -> Json {
    Json::Obj(vec![
        ("index".to_string(), Json::Num(chapter.index)),
        ("idref".to_string(), Json::Str(chapter.idref.clone())),
        ("href".to_string(), Json::Str(chapter.href.clone())),
        (
            "media_type".to_string(),
            Json::Str(chapter.media_type.clone()),
        ),
    ])
}

fn metadata_json(metadata: &EpubMetadata) -> Json {
    let manifest = metadata
        .manifest
        .iter()
        .map(|item| {
            Json::Obj(vec![
                ("id".to_string(), Json::Str(item.id.clone())),
                ("href".to_string(), Json::Str(item.href.clone())),
                ("media_type".to_string(), Json::Str(item.media_type.clone())),
                (
                    "properties".to_string(),
                    item.properties.clone().map_or(Json::Null, Json::Str),
                ),
            ])
        })
        .collect::<Vec<_>>();
    let guide = metadata
        .guide
        .iter()
        .map(|item| {
            Json::Obj(vec![
                ("type".to_string(), Json::Str(item.guide_type.clone())),
                (
                    "title".to_string(),
                    item.title.clone().map_or(Json::Null, Json::Str),
                ),
                ("href".to_string(), Json::Str(item.href.clone())),
            ])
        })
        .collect::<Vec<_>>();

    Json::Obj(vec![
        ("title".to_string(), Json::Str(metadata.title.clone())),
        ("author".to_string(), Json::Str(metadata.author.clone())),
        ("language".to_string(), Json::Str(metadata.language.clone())),
        (
            "identifier".to_string(),
            metadata.identifier.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "date".to_string(),
            metadata.date.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "publisher".to_string(),
            metadata.publisher.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "rights".to_string(),
            metadata.rights.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "description".to_string(),
            metadata.description.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "subjects".to_string(),
            Json::Arr(
                metadata
                    .subjects
                    .iter()
                    .map(|subject| Json::Str(subject.clone()))
                    .collect(),
            ),
        ),
        (
            "modified".to_string(),
            metadata.modified.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "rendition_layout".to_string(),
            metadata
                .rendition_layout
                .clone()
                .map_or(Json::Null, Json::Str),
        ),
        (
            "cover_id".to_string(),
            metadata.cover_id.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "opf_path".to_string(),
            metadata.opf_path.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "manifest_count".to_string(),
            Json::Num(metadata.manifest.len()),
        ),
        ("manifest".to_string(), Json::Arr(manifest)),
        ("guide".to_string(), Json::Arr(guide)),
    ])
}

fn diagnostic_json(diag: &ValidationDiagnostic) -> Json {
    let severity = match diag.severity {
        ValidationSeverity::Error => "error",
        ValidationSeverity::Warning => "warning",
    };
    Json::Obj(vec![
        ("code".to_string(), Json::Str(diag.code.to_string())),
        ("severity".to_string(), Json::Str(severity.to_string())),
        ("message".to_string(), Json::Str(diag.message.clone())),
        (
            "path".to_string(),
            diag.path.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "location".to_string(),
            diag.location.clone().map_or(Json::Null, Json::Str),
        ),
        (
            "spec_ref".to_string(),
            diag.spec_ref
                .map(|v| Json::Str(v.to_string()))
                .unwrap_or(Json::Null),
        ),
        (
            "hint".to_string(),
            diag.hint.clone().map_or(Json::Null, Json::Str),
        ),
    ])
}

fn display_err(err: EpubError) -> String {
    err.to_string()
}

fn print_help() {
    let help = r#"epub-stream (epμb) - inspect EPUB files

USAGE:
  epub-stream [--pretty] <command> [args...]

COMMANDS:
  metadata <epub_path>
  validate <epub_path> [--strict]
  spine <epub_path>
  toc <epub_path> [--flat]
  chapters <epub_path> [--ndjson]
  chapter-text <epub_path> (--index <n> | --id <idref>) [--raw]
  chapter-html <epub_path> (--index <n> | --id <idref>) [--raw]

NOTES:
  - Output is JSON by default.
  - `chapters --ndjson` emits one JSON object per line.
  - `chapter-text --raw` and `chapter-html --raw` emit raw content.
"#;
    println!("{}", help);
}
