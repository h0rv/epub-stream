use core::fmt;
use epub_stream::BlockRole;
use std::sync::Arc;

type SplitLayerCommandIter<'a> = core::iter::Chain<
    core::iter::Chain<core::slice::Iter<'a, DrawCommand>, core::slice::Iter<'a, DrawCommand>>,
    core::slice::Iter<'a, DrawCommand>,
>;

/// Iterator over merged page commands.
pub enum MergedCommandIter<'a> {
    /// Iterates split layers in content/chrome/overlay order.
    Split(SplitLayerCommandIter<'a>),
    /// Iterates legacy merged command vector.
    Legacy(core::slice::Iter<'a, DrawCommand>),
}

impl<'a> Iterator for MergedCommandIter<'a> {
    type Item = &'a DrawCommand;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Split(iter) => iter.next(),
            Self::Legacy(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Split(iter) => iter.size_hint(),
            Self::Legacy(iter) => iter.size_hint(),
        }
    }
}

/// Page represented as backend-agnostic draw commands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderPage {
    /// 1-based page number.
    pub page_number: usize,
    /// Legacy merged command stream.
    ///
    /// This remains for compatibility. It is populated only when
    /// [`sync_commands`](Self::sync_commands) is called.
    pub commands: Vec<DrawCommand>,
    /// Content-layer draw commands (deterministic pagination output).
    pub content_commands: Vec<DrawCommand>,
    /// Chrome-layer draw commands (header/footer/progress and similar).
    pub chrome_commands: Vec<DrawCommand>,
    /// Overlay draw commands attached after content/chrome layout.
    pub overlay_commands: Vec<DrawCommand>,
    /// Structured overlay items attached by composer APIs.
    pub overlay_items: Vec<OverlayItem>,
    /// Structured non-draw annotations associated with this page.
    pub annotations: Vec<PageAnnotation>,
    /// Per-page metrics for navigation/progress consumers.
    pub metrics: PageMetrics,
}

impl RenderPage {
    const INITIAL_CONTENT_COMMAND_CAPACITY: usize = 8;
    const INITIAL_CHROME_COMMAND_CAPACITY: usize = 2;
    const INITIAL_OVERLAY_COMMAND_CAPACITY: usize = 2;

    /// Create an empty page.
    pub fn new(page_number: usize) -> Self {
        Self {
            page_number,
            // Keep command-layer defaults lazy so empty pages avoid baseline heap traffic.
            commands: Vec::with_capacity(0),
            content_commands: Vec::with_capacity(0),
            chrome_commands: Vec::with_capacity(0),
            overlay_commands: Vec::with_capacity(0),
            overlay_items: Vec::with_capacity(0),
            annotations: Vec::with_capacity(0),
            metrics: PageMetrics {
                chapter_page_index: page_number.saturating_sub(1),
                ..PageMetrics::default()
            },
        }
    }

    /// Clear all commands and reset for reuse, preserving allocated capacity.
    pub fn clear_for_reuse(&mut self, new_page_number: usize) {
        self.page_number = new_page_number;
        self.commands.clear();
        self.content_commands.clear();
        self.chrome_commands.clear();
        self.overlay_commands.clear();
        self.overlay_items.clear();
        self.annotations.clear();
        self.metrics = PageMetrics {
            chapter_page_index: new_page_number.saturating_sub(1),
            ..PageMetrics::default()
        };
    }

    /// Push a content-layer command.
    pub fn push_content_command(&mut self, cmd: DrawCommand) {
        if self.content_commands.capacity() == 0 {
            self.content_commands
                .reserve(Self::INITIAL_CONTENT_COMMAND_CAPACITY);
        }
        self.content_commands.push(cmd);
    }

    /// Push a chrome-layer command.
    pub fn push_chrome_command(&mut self, cmd: DrawCommand) {
        if self.chrome_commands.capacity() == 0 {
            self.chrome_commands
                .reserve(Self::INITIAL_CHROME_COMMAND_CAPACITY);
        }
        self.chrome_commands.push(cmd);
    }

    /// Push an overlay-layer command.
    pub fn push_overlay_command(&mut self, cmd: DrawCommand) {
        if self.overlay_commands.capacity() == 0 {
            self.overlay_commands
                .reserve(Self::INITIAL_OVERLAY_COMMAND_CAPACITY);
        }
        self.overlay_commands.push(cmd);
    }

    /// Number of merged commands visible to consumers.
    ///
    /// When split layers are populated this returns their combined count.
    /// Otherwise it falls back to the legacy merged command stream.
    pub fn merged_commands_len(&self) -> usize {
        let split =
            self.content_commands.len() + self.chrome_commands.len() + self.overlay_commands.len();
        if split > 0 {
            split
        } else {
            self.commands.len()
        }
    }

    /// Iterate merged commands without allocating.
    ///
    /// Split layers are preferred when present. If all split layers are empty,
    /// this yields legacy `commands` entries.
    pub fn merged_commands_iter(&self) -> MergedCommandIter<'_> {
        if self.content_commands.is_empty()
            && self.chrome_commands.is_empty()
            && self.overlay_commands.is_empty()
        {
            MergedCommandIter::Legacy(self.commands.iter())
        } else {
            MergedCommandIter::Split(
                self.content_commands
                    .iter()
                    .chain(self.chrome_commands.iter())
                    .chain(self.overlay_commands.iter()),
            )
        }
    }

    #[cfg(not(target_os = "espidf"))]
    fn merged_command_at(&self, idx: usize) -> Option<&DrawCommand> {
        let content_len = self.content_commands.len();
        if idx < content_len {
            return self.content_commands.get(idx);
        }
        let chrome_len = self.chrome_commands.len();
        let chrome_idx = idx.saturating_sub(content_len);
        if chrome_idx < chrome_len {
            return self.chrome_commands.get(chrome_idx);
        }
        self.overlay_commands
            .get(chrome_idx.saturating_sub(chrome_len))
    }

    #[cfg(not(target_os = "espidf"))]
    fn append_synced_tail_from(&mut self, mut cursor: usize) {
        let content_len = self.content_commands.len();
        let chrome_len = self.chrome_commands.len();
        let content_and_chrome = content_len + chrome_len;

        if cursor < content_len {
            self.commands
                .extend(self.content_commands[cursor..].iter().cloned());
            cursor = content_len;
        }
        if cursor < content_and_chrome {
            let chrome_start = cursor.saturating_sub(content_len);
            self.commands
                .extend(self.chrome_commands[chrome_start..].iter().cloned());
            cursor = content_and_chrome;
        }
        let overlay_start = cursor.saturating_sub(content_and_chrome);
        if overlay_start < self.overlay_commands.len() {
            self.commands
                .extend(self.overlay_commands[overlay_start..].iter().cloned());
        }
    }

    /// Rebuild legacy merged `commands` from split layers.
    pub fn sync_commands(&mut self) {
        #[cfg(target_os = "espidf")]
        {
            // On constrained targets, avoid duplicating command vectors.
            // Embedded consumers render from split command layers directly.
            self.commands.clear();
            return;
        }
        #[cfg(not(target_os = "espidf"))]
        {
            let expected = self.content_commands.len()
                + self.chrome_commands.len()
                + self.overlay_commands.len();
            let cursor = self.commands.len();
            if cursor == expected {
                return;
            }

            if cursor < expected {
                // Fast path: if merged stream is a valid prefix, append only the missing tail.
                let prefix_is_valid = cursor == 0
                    || self
                        .merged_command_at(cursor - 1)
                        .is_some_and(|cmd| self.commands[cursor - 1] == *cmd);
                if prefix_is_valid {
                    self.commands.reserve(expected - cursor);
                    self.append_synced_tail_from(cursor);
                    return;
                }
            }

            self.commands.clear();
            self.commands.reserve(expected);
            self.commands.extend(self.content_commands.iter().cloned());
            self.commands.extend(self.chrome_commands.iter().cloned());
            self.commands.extend(self.overlay_commands.iter().cloned());
        }
    }

    /// Backward-compatible accessor alias for page metadata.
    pub fn page_meta(&self) -> &PageMeta {
        &self.metrics
    }
}

/// Structured page annotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageAnnotation {
    /// Stable annotation kind/tag.
    pub kind: PageAnnotationKind,
    /// Optional annotation payload.
    pub value: Option<String>,
}

/// Structured page annotation kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PageAnnotationKind {
    /// Generic note/marker annotation.
    Note,
    /// Inline image source annotation emitted by layout.
    InlineImageSrc,
    /// Forward-compatible fallback for unknown/legacy string tags.
    Unknown(String),
}

impl PageAnnotationKind {
    /// Canonical string form used by persisted payloads and compatibility paths.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Note => "note",
            Self::InlineImageSrc => "inline_image_src",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

impl AsRef<str> for PageAnnotationKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for PageAnnotationKind {
    fn from(value: &str) -> Self {
        match value {
            "note" => Self::Note,
            "inline_image_src" => Self::InlineImageSrc,
            _ => Self::Unknown(value.to_string()),
        }
    }
}

impl From<String> for PageAnnotationKind {
    fn from(value: String) -> Self {
        match value.as_str() {
            "note" => Self::Note,
            "inline_image_src" => Self::InlineImageSrc,
            _ => Self::Unknown(value),
        }
    }
}

impl From<PageAnnotationKind> for String {
    fn from(value: PageAnnotationKind) -> Self {
        match value {
            PageAnnotationKind::Note => "note".to_string(),
            PageAnnotationKind::InlineImageSrc => "inline_image_src".to_string(),
            PageAnnotationKind::Unknown(value) => value,
        }
    }
}

impl From<&PageAnnotationKind> for String {
    fn from(value: &PageAnnotationKind) -> Self {
        value.as_str().to_string()
    }
}

impl PartialEq<&str> for PageAnnotationKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for PageAnnotationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured page metrics for progress and navigation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PageMetrics {
    /// Chapter index in the spine (0-based), when known.
    pub chapter_index: usize,
    /// Page index in chapter (0-based).
    pub chapter_page_index: usize,
    /// Total pages in chapter, when known.
    pub chapter_page_count: Option<usize>,
    /// Global page index across rendered stream (0-based), when known.
    pub global_page_index: Option<usize>,
    /// Estimated global page count, when known.
    pub global_page_count_estimate: Option<usize>,
    /// Chapter progress in range `[0.0, 1.0]`.
    pub progress_chapter: f32,
    /// Book progress in range `[0.0, 1.0]`, when known.
    pub progress_book: Option<f32>,
}

/// Backward-compatible alias for page-level metadata.
pub type PageMeta = PageMetrics;

/// Stable pagination profile id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaginationProfileId(pub [u8; 32]);

impl PaginationProfileId {
    /// Build a deterministic profile id from arbitrary payload bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        fn fnv64(seed: u64, payload: &[u8]) -> u64 {
            let mut hash = seed;
            for b in payload {
                hash ^= *b as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash
        }
        let mut out = [0u8; 32];
        let h0 = fnv64(0xcbf29ce484222325, bytes).to_le_bytes();
        let h1 = fnv64(0x9e3779b97f4a7c15, bytes).to_le_bytes();
        let h2 = fnv64(0xd6e8feb86659fd93, bytes).to_le_bytes();
        let h3 = fnv64(0xa0761d6478bd642f, bytes).to_le_bytes();
        out[0..8].copy_from_slice(&h0);
        out[8..16].copy_from_slice(&h1);
        out[16..24].copy_from_slice(&h2);
        out[24..32].copy_from_slice(&h3);
        Self(out)
    }
}

/// Logical overlay slots for app/UI composition.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlaySlot {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    Custom(OverlayRect),
}

/// Logical viewport size for overlay composition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlaySize {
    pub width: u32,
    pub height: u32,
}

/// Rectangle for custom overlay slot coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Overlay content payload.
#[derive(Clone, Debug, PartialEq)]
pub enum OverlayContent {
    /// Text payload (resolved by the app/backend).
    Text(String),
    /// Backend-agnostic draw command payload.
    Command(DrawCommand),
}

/// Overlay item attached to a page.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayItem {
    /// Destination slot.
    pub slot: OverlaySlot,
    /// Z-order.
    pub z: i32,
    /// Overlay payload.
    pub content: OverlayContent,
}

/// Overlay composer API for app-driven overlay placement/content.
pub trait OverlayComposer {
    fn compose(&self, metrics: &PageMetrics, viewport: OverlaySize) -> Vec<OverlayItem>;
}

#[cfg(test)]
mod tests {
    use super::{
        DrawCommand, PageAnnotationKind, PageChromeCommand, PageChromeKind, RectCommand,
        RenderPage, RuleCommand,
    };

    #[test]
    fn page_annotation_kind_maps_known_tags() {
        assert_eq!(PageAnnotationKind::from("note"), PageAnnotationKind::Note);
        assert_eq!(
            PageAnnotationKind::from("inline_image_src"),
            PageAnnotationKind::InlineImageSrc
        );
        assert_eq!(String::from(PageAnnotationKind::Note), "note");
        assert_eq!(
            String::from(PageAnnotationKind::InlineImageSrc),
            "inline_image_src"
        );
    }

    #[test]
    fn page_annotation_kind_preserves_unknown_tags() {
        let kind = PageAnnotationKind::from("custom_annotation");
        assert_eq!(
            kind,
            PageAnnotationKind::Unknown("custom_annotation".to_string())
        );
        assert_eq!(kind.as_str(), "custom_annotation");
        assert_eq!(kind, "custom_annotation");
        assert_eq!(kind.to_string(), "custom_annotation");
        assert_eq!(String::from(&kind), "custom_annotation".to_string());
    }

    #[test]
    fn render_page_merged_commands_iter_reads_split_layers_without_sync() {
        let mut page = RenderPage::new(1);
        page.push_content_command(DrawCommand::Rule(RuleCommand {
            x: 0,
            y: 0,
            length: 10,
            thickness: 1,
            horizontal: true,
        }));
        page.push_chrome_command(DrawCommand::Rect(RectCommand {
            x: 0,
            y: 0,
            width: 5,
            height: 6,
            fill: false,
        }));
        page.push_overlay_command(DrawCommand::PageChrome(PageChromeCommand {
            kind: PageChromeKind::Footer,
            text: Some("f".to_string()),
            current: None,
            total: None,
        }));

        assert_eq!(page.commands.len(), 0);
        assert_eq!(page.merged_commands_len(), 3);
        assert_eq!(page.merged_commands_iter().count(), 3);
    }

    #[test]
    fn render_page_sync_commands_remains_explicit_compatibility_path() {
        let mut page = RenderPage::new(2);
        page.push_content_command(DrawCommand::Rule(RuleCommand {
            x: 0,
            y: 0,
            length: 10,
            thickness: 1,
            horizontal: true,
        }));
        assert!(page.commands.is_empty());
        page.sync_commands();
        assert_eq!(page.commands.len(), 1);
    }

    #[test]
    fn render_page_new_defers_less_used_vector_allocations() {
        let page = RenderPage::new(1);
        assert_eq!(page.commands.capacity(), 0);
        assert_eq!(page.content_commands.capacity(), 0);
        assert_eq!(page.chrome_commands.capacity(), 0);
        assert_eq!(page.overlay_commands.capacity(), 0);
        assert_eq!(page.overlay_items.capacity(), 0);
        assert_eq!(page.annotations.capacity(), 0);
    }
}

/// Layout output commands.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawCommand {
    /// Draw text.
    Text(TextCommand),
    /// Draw a line rule.
    Rule(RuleCommand),
    /// Draw an inline image object box.
    ImageObject(ImageObjectCommand),
    /// Draw rectangle.
    Rect(RectCommand),
    /// Draw page metadata/chrome.
    PageChrome(PageChromeCommand),
}

/// Theme-aware render intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderIntent {
    /// Convert output to grayscale mode.
    pub grayscale_mode: GrayscaleMode,
    /// Optional dithering algorithm.
    pub dither: DitherMode,
    /// Contrast multiplier in percent (100 = neutral).
    pub contrast_boost: u8,
}

impl Default for RenderIntent {
    fn default() -> Self {
        Self {
            grayscale_mode: GrayscaleMode::Off,
            dither: DitherMode::None,
            contrast_boost: 100,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrayscaleMode {
    Off,
    Luminosity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DitherMode {
    None,
    Ordered,
    ErrorDiffusion,
}

/// Resolved style passed to renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTextStyle {
    /// Stable font identifier for this style.
    pub font_id: Option<u32>,
    /// Chosen family.
    pub family: Arc<str>,
    /// Numeric weight.
    pub weight: u16,
    /// Italic flag.
    pub italic: bool,
    /// Size in pixels.
    pub size_px: f32,
    /// Line height multiplier.
    pub line_height: f32,
    /// Letter spacing in px.
    pub letter_spacing: f32,
    /// Semantic role.
    pub role: BlockRole,
    /// Justification mode from layout.
    pub justify_mode: JustifyMode,
}

/// Justification mode determined during layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JustifyMode {
    /// Left/no justification.
    None,
    /// Inter-word with total extra px to distribute.
    InterWord { extra_px_total: i32 },
    /// Right alignment with horizontal offset.
    AlignRight { offset_px: i32 },
    /// Center alignment with horizontal offset.
    AlignCenter { offset_px: i32 },
}

/// Text draw command.
#[derive(Clone, Debug, PartialEq)]
pub struct TextCommand {
    /// Left x.
    pub x: i32,
    /// Baseline y.
    pub baseline_y: i32,
    /// Content.
    pub text: String,
    /// Font identifier for direct command-level lookup.
    pub font_id: Option<u32>,
    /// Resolved style.
    pub style: ResolvedTextStyle,
}

/// Rule draw command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleCommand {
    /// Start x.
    pub x: i32,
    /// Start y.
    pub y: i32,
    /// Length.
    pub length: u32,
    /// Thickness.
    pub thickness: u32,
    /// Horizontal if true; vertical if false.
    pub horizontal: bool,
}

/// Rectangle command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectCommand {
    /// Left x.
    pub x: i32,
    /// Top y.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Fill rectangle when true.
    pub fill: bool,
}

/// Inline image object command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageObjectCommand {
    /// Resource href (OPF-relative).
    pub src: String,
    /// Optional alt/caption text.
    pub alt: String,
    /// Left x.
    pub x: i32,
    /// Top y.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Page-level metadata/chrome marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageChromeCommand {
    /// Semantic chrome kind.
    pub kind: PageChromeKind,
    /// Optional text payload (e.g. footer text).
    pub text: Option<String>,
    /// Optional current value (e.g. for progress).
    pub current: Option<usize>,
    /// Optional total value (e.g. for progress).
    pub total: Option<usize>,
}

/// Kind of page-level metadata/chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageChromeKind {
    /// Header marker.
    Header,
    /// Footer marker.
    Footer,
    /// Progress marker.
    Progress,
}

/// Text style for header/footer chrome rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageChromeTextStyle {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

/// Shared page-chrome policy and geometry configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageChromeConfig {
    /// Emit/draw page header text.
    pub header_enabled: bool,
    /// Emit/draw page footer text.
    pub footer_enabled: bool,
    /// Emit/draw page progress bar.
    pub progress_enabled: bool,
    /// Header text left x.
    pub header_x: i32,
    /// Header text baseline y.
    pub header_baseline_y: i32,
    /// Header text style.
    pub header_style: PageChromeTextStyle,
    /// Footer text left x.
    pub footer_x: i32,
    /// Footer text baseline offset from bottom edge.
    pub footer_baseline_from_bottom: i32,
    /// Footer text style.
    pub footer_style: PageChromeTextStyle,
    /// Progress bar left/right inset.
    pub progress_x_inset: i32,
    /// Progress bar top y offset from bottom edge.
    pub progress_y_from_bottom: i32,
    /// Progress bar height.
    pub progress_height: u32,
    /// Progress bar outline thickness.
    pub progress_stroke_width: u32,
}

impl PageChromeConfig {
    /// Default chrome geometry matching historical renderer behavior.
    pub const fn geometry_defaults() -> Self {
        Self {
            header_enabled: true,
            footer_enabled: true,
            progress_enabled: true,
            header_x: 8,
            header_baseline_y: 16,
            header_style: PageChromeTextStyle::Bold,
            footer_x: 8,
            footer_baseline_from_bottom: 8,
            footer_style: PageChromeTextStyle::Regular,
            progress_x_inset: 8,
            progress_y_from_bottom: 20,
            progress_height: 4,
            progress_stroke_width: 1,
        }
    }

    /// Defaults used by layout so chrome markers are opt-in.
    pub const fn layout_defaults() -> Self {
        let mut cfg = Self::geometry_defaults();
        cfg.header_enabled = false;
        cfg.footer_enabled = false;
        cfg.progress_enabled = false;
        cfg
    }
}

impl Default for PageChromeConfig {
    fn default() -> Self {
        Self::layout_defaults()
    }
}

/// Typography policy knobs for layout behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TypographyConfig {
    /// Hyphenation policy.
    pub hyphenation: HyphenationConfig,
    /// Widow/orphan control policy.
    pub widow_orphan_control: WidowOrphanControl,
    /// Justification policy.
    pub justification: JustificationConfig,
    /// Hanging punctuation policy.
    pub hanging_punctuation: HangingPunctuationConfig,
}

/// Hyphenation behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HyphenationConfig {
    /// Soft-hyphen handling policy.
    pub soft_hyphen_policy: HyphenationMode,
}

impl Default for HyphenationConfig {
    fn default() -> Self {
        Self {
            soft_hyphen_policy: HyphenationMode::Discretionary,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HyphenationMode {
    Ignore,
    Discretionary,
}

/// Widow/orphan policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WidowOrphanControl {
    /// Keep at least this many lines at paragraph start/end when possible.
    pub min_lines: u8,
    /// Enable widow/orphan controls.
    pub enabled: bool,
}

impl Default for WidowOrphanControl {
    fn default() -> Self {
        Self {
            min_lines: 2,
            enabled: true,
        }
    }
}

/// Justification policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JustificationConfig {
    /// Enable justification/alignment policy.
    pub enabled: bool,
    /// Justification strategy for body/paragraph lines.
    pub strategy: JustificationStrategy,
    /// Minimum words required for justification.
    pub min_words: usize,
    /// Minimum fill ratio required for justification.
    pub min_fill_ratio: f32,
    /// Maximum stretch per space as a multiplier of measured space width.
    ///
    /// Used by adaptive inter-word mode to avoid visually noisy spacing.
    /// Full inter-word mode ignores this cap.
    pub max_space_stretch_ratio: f32,
}

impl Default for JustificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strategy: JustificationStrategy::AdaptiveInterWord,
            min_words: 7,
            min_fill_ratio: 0.75,
            max_space_stretch_ratio: 0.45,
        }
    }
}

/// Justification/alignment strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JustificationStrategy {
    /// Adaptive inter-word justification with quality thresholds.
    AdaptiveInterWord,
    /// Full inter-word justification that uses all line slack.
    FullInterWord,
    /// Left alignment (no inter-word expansion).
    AlignLeft,
    /// Right alignment.
    AlignRight,
    /// Center alignment.
    AlignCenter,
}

/// Hanging punctuation policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HangingPunctuationConfig {
    /// Enable hanging punctuation (currently informational).
    pub enabled: bool,
}

impl Default for HangingPunctuationConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Non-text object layout policy knobs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectLayoutConfig {
    /// Max inline-image height ratio relative to content height.
    pub max_inline_image_height_ratio: f32,
    /// Policy for cover-like first-page images.
    pub cover_page_mode: CoverPageMode,
    /// Enable/disable float placement.
    pub float_support: FloatSupport,
    /// SVG placement mode.
    pub svg_mode: SvgMode,
    /// Emit alt-text fallback when object drawing is unavailable.
    pub alt_text_fallback: bool,
}

impl Default for ObjectLayoutConfig {
    fn default() -> Self {
        Self {
            max_inline_image_height_ratio: 0.5,
            cover_page_mode: CoverPageMode::Contain,
            float_support: FloatSupport::None,
            svg_mode: SvgMode::RasterizeFallback,
            alt_text_fallback: true,
        }
    }
}

/// Cover-image placement mode for cover-like first-page image resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverPageMode {
    /// Fit image within content area, preserve aspect ratio, no cropping.
    Contain,
    /// Fill viewport while preserving aspect ratio; crop overflow by viewport clip.
    FullBleed,
    /// Respect normal CSS/object layout behavior (no special cover handling).
    RespectCss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatSupport {
    None,
    Basic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgMode {
    Ignore,
    RasterizeFallback,
    Native,
}
