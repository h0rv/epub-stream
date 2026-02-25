//! Render IR, layout engine, and orchestration for `epub-stream`.

#![cfg_attr(
    not(test),
    deny(
        clippy::disallowed_methods,
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::todo,
        clippy::unimplemented
    )
)]

mod render_engine;
mod render_ir;
mod render_layout;

pub use epub_stream::BlockRole;
pub use render_engine::{
    estimate_fragment_progress_in_html, remap_page_index_by_chapter_progress,
    resolve_page_index_for_chapter_progress, CancelToken, ChapterLayoutSummary,
    FileRenderCacheStore, LayoutSession, NeverCancel, PageRange, RenderBookPageMap,
    RenderBookPageMapEntry, RenderCacheStore, RenderConfig, RenderDiagnostic, RenderEngine,
    RenderEngineError, RenderEngineOptions, RenderLocatorPageTarget, RenderLocatorTargetKind,
    RenderPageIter, RenderPageStreamIter, RenderReadingPositionToken,
};
pub use render_ir::{
    CoverPageMode, DitherMode, DrawCommand, FloatSupport, GrayscaleMode, HangingPunctuationConfig,
    HyphenationConfig, HyphenationMode, ImageObjectCommand, JustificationConfig,
    JustificationStrategy, JustifyMode, ObjectLayoutConfig, OverlayComposer, OverlayContent,
    OverlayItem, OverlayRect, OverlaySize, OverlaySlot, PageAnnotation, PageChromeCommand,
    PageChromeConfig, PageChromeKind, PageChromeTextStyle, PageMeta, PageMetrics,
    PaginationProfileId, RectCommand, RenderIntent, RenderPage, ResolvedTextStyle, RuleCommand,
    SvgMode, TextCommand, TypographyConfig, WidowOrphanControl,
};
pub use render_layout::{LayoutConfig, LayoutEngine, SoftHyphenPolicy, TextMeasurer};
