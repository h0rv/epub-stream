//! Render IR, layout engine, and orchestration for `mu-epub`.

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

pub use mu_epub::BlockRole;
pub use render_engine::{
    CancelToken, LayoutSession, NeverCancel, PageRange, RenderCacheStore, RenderConfig,
    RenderDiagnostic, RenderEngine, RenderEngineError, RenderEngineOptions, RenderPageIter,
    RenderPageStreamIter,
};
pub use render_ir::{
    DitherMode, DrawCommand, FloatSupport, GrayscaleMode, HangingPunctuationConfig,
    HyphenationConfig, HyphenationMode, ImageObjectCommand, JustificationConfig, JustifyMode,
    ObjectLayoutConfig, OverlayComposer, OverlayContent, OverlayItem, OverlayRect, OverlaySize,
    OverlaySlot, PageAnnotation, PageChromeCommand, PageChromeConfig, PageChromeKind,
    PageChromeTextStyle, PageMeta, PageMetrics, PaginationProfileId, RectCommand, RenderIntent,
    RenderPage, ResolvedTextStyle, RuleCommand, SvgMode, TextCommand, TypographyConfig,
    WidowOrphanControl,
};
pub use render_layout::{LayoutConfig, LayoutEngine, SoftHyphenPolicy, TextMeasurer};
