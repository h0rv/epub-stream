use epub_stream::BlockRole;
use serde::{Deserialize, Serialize};

use crate::render_ir::{
    DrawCommand, ImageObjectCommand, JustifyMode, OverlayContent, OverlayItem, OverlayRect,
    OverlaySlot, PageAnnotation, PageChromeCommand, PageChromeKind, PageMetrics, RectCommand,
    RenderPage, ResolvedTextStyle, RuleCommand, TextCommand,
};

pub const CACHE_SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedCacheEnvelope {
    pub version: u8,
    pub pages: Vec<PersistedRenderPage>,
}

impl PersistedCacheEnvelope {
    pub fn from_pages(pages: &[RenderPage]) -> Self {
        Self {
            version: CACHE_SCHEMA_VERSION,
            pages: pages.iter().map(PersistedRenderPage::from).collect(),
        }
    }

    pub fn into_render_pages(self) -> Option<Vec<RenderPage>> {
        if self.version != CACHE_SCHEMA_VERSION {
            return None;
        }
        Some(self.pages.into_iter().map(RenderPage::from).collect())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedRenderPage {
    pub page_number: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<PersistedDrawCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_commands: Vec<PersistedDrawCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chrome_commands: Vec<PersistedDrawCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlay_commands: Vec<PersistedDrawCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlay_items: Vec<PersistedOverlayItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<PersistedPageAnnotation>,
    #[serde(default)]
    pub metrics: PersistedPageMetrics,
}

/// Binary-cache-friendly page payload.
///
/// This mirrors `PersistedRenderPage` but intentionally avoids
/// `skip_serializing_if` so compact binary formats like postcard keep a stable
/// field layout even when vectors are empty.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BinaryPersistedRenderPage {
    pub page_number: usize,
    pub commands: Vec<PersistedDrawCommand>,
    pub content_commands: Vec<PersistedDrawCommand>,
    pub chrome_commands: Vec<PersistedDrawCommand>,
    pub overlay_commands: Vec<PersistedDrawCommand>,
    pub overlay_items: Vec<PersistedOverlayItem>,
    pub annotations: Vec<PersistedPageAnnotation>,
    pub metrics: PersistedPageMetrics,
}

impl From<&RenderPage> for PersistedRenderPage {
    fn from(value: &RenderPage) -> Self {
        Self {
            page_number: value.page_number,
            commands: value
                .commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            content_commands: value
                .content_commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            chrome_commands: value
                .chrome_commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            overlay_commands: value
                .overlay_commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            overlay_items: value
                .overlay_items
                .iter()
                .map(PersistedOverlayItem::from)
                .collect(),
            annotations: value
                .annotations
                .iter()
                .map(PersistedPageAnnotation::from)
                .collect(),
            metrics: value.metrics.into(),
        }
    }
}

impl From<PersistedRenderPage> for BinaryPersistedRenderPage {
    fn from(value: PersistedRenderPage) -> Self {
        Self {
            page_number: value.page_number,
            commands: value.commands,
            content_commands: value.content_commands,
            chrome_commands: value.chrome_commands,
            overlay_commands: value.overlay_commands,
            overlay_items: value.overlay_items,
            annotations: value.annotations,
            metrics: value.metrics,
        }
    }
}

impl From<BinaryPersistedRenderPage> for PersistedRenderPage {
    fn from(value: BinaryPersistedRenderPage) -> Self {
        Self {
            page_number: value.page_number,
            commands: value.commands,
            content_commands: value.content_commands,
            chrome_commands: value.chrome_commands,
            overlay_commands: value.overlay_commands,
            overlay_items: value.overlay_items,
            annotations: value.annotations,
            metrics: value.metrics,
        }
    }
}

impl From<&RenderPage> for BinaryPersistedRenderPage {
    fn from(value: &RenderPage) -> Self {
        PersistedRenderPage::from(value).into()
    }
}

impl From<PersistedRenderPage> for RenderPage {
    fn from(value: PersistedRenderPage) -> Self {
        Self {
            page_number: value.page_number,
            commands: value.commands.into_iter().map(DrawCommand::from).collect(),
            content_commands: value
                .content_commands
                .into_iter()
                .map(DrawCommand::from)
                .collect(),
            chrome_commands: value
                .chrome_commands
                .into_iter()
                .map(DrawCommand::from)
                .collect(),
            overlay_commands: value
                .overlay_commands
                .into_iter()
                .map(DrawCommand::from)
                .collect(),
            overlay_items: value
                .overlay_items
                .into_iter()
                .map(OverlayItem::from)
                .collect(),
            annotations: value
                .annotations
                .into_iter()
                .map(PageAnnotation::from)
                .collect(),
            metrics: value.metrics.into(),
        }
    }
}

impl From<BinaryPersistedRenderPage> for RenderPage {
    fn from(value: BinaryPersistedRenderPage) -> Self {
        PersistedRenderPage::from(value).into()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedOverlayItem {
    pub slot: PersistedOverlaySlot,
    pub z: i32,
    pub content: PersistedOverlayContent,
}

impl From<&OverlayItem> for PersistedOverlayItem {
    fn from(value: &OverlayItem) -> Self {
        Self {
            slot: (&value.slot).into(),
            z: value.z,
            content: (&value.content).into(),
        }
    }
}

impl From<PersistedOverlayItem> for OverlayItem {
    fn from(value: PersistedOverlayItem) -> Self {
        Self {
            slot: value.slot.into(),
            z: value.z,
            content: value.content.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PersistedOverlaySlot {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    Custom(PersistedOverlayRect),
}

impl From<&OverlaySlot> for PersistedOverlaySlot {
    fn from(value: &OverlaySlot) -> Self {
        match value {
            OverlaySlot::TopLeft => Self::TopLeft,
            OverlaySlot::TopCenter => Self::TopCenter,
            OverlaySlot::TopRight => Self::TopRight,
            OverlaySlot::BottomLeft => Self::BottomLeft,
            OverlaySlot::BottomCenter => Self::BottomCenter,
            OverlaySlot::BottomRight => Self::BottomRight,
            OverlaySlot::Custom(rect) => Self::Custom((*rect).into()),
        }
    }
}

impl From<PersistedOverlaySlot> for OverlaySlot {
    fn from(value: PersistedOverlaySlot) -> Self {
        match value {
            PersistedOverlaySlot::TopLeft => Self::TopLeft,
            PersistedOverlaySlot::TopCenter => Self::TopCenter,
            PersistedOverlaySlot::TopRight => Self::TopRight,
            PersistedOverlaySlot::BottomLeft => Self::BottomLeft,
            PersistedOverlaySlot::BottomCenter => Self::BottomCenter,
            PersistedOverlaySlot::BottomRight => Self::BottomRight,
            PersistedOverlaySlot::Custom(rect) => Self::Custom(rect.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PersistedOverlayRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl From<OverlayRect> for PersistedOverlayRect {
    fn from(value: OverlayRect) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

impl From<PersistedOverlayRect> for OverlayRect {
    fn from(value: PersistedOverlayRect) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PersistedOverlayContent {
    Text(String),
    Command(PersistedDrawCommand),
}

impl From<&OverlayContent> for PersistedOverlayContent {
    fn from(value: &OverlayContent) -> Self {
        match value {
            OverlayContent::Text(text) => Self::Text(text.clone()),
            OverlayContent::Command(cmd) => Self::Command(cmd.into()),
        }
    }
}

impl From<PersistedOverlayContent> for OverlayContent {
    fn from(value: PersistedOverlayContent) -> Self {
        match value {
            PersistedOverlayContent::Text(text) => Self::Text(text),
            PersistedOverlayContent::Command(cmd) => Self::Command(cmd.into()),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedPageAnnotation {
    pub kind: String,
    pub value: Option<String>,
}

impl From<&PageAnnotation> for PersistedPageAnnotation {
    fn from(value: &PageAnnotation) -> Self {
        Self {
            kind: String::from(&value.kind),
            value: value.value.clone(),
        }
    }
}

impl From<PersistedPageAnnotation> for PageAnnotation {
    fn from(value: PersistedPageAnnotation) -> Self {
        Self {
            kind: value.kind.into(),
            value: value.value,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PersistedPageMetrics {
    pub chapter_index: usize,
    pub chapter_page_index: usize,
    pub chapter_page_count: Option<usize>,
    pub global_page_index: Option<usize>,
    pub global_page_count_estimate: Option<usize>,
    pub progress_chapter: f32,
    pub progress_book: Option<f32>,
}

impl From<PageMetrics> for PersistedPageMetrics {
    fn from(value: PageMetrics) -> Self {
        Self {
            chapter_index: value.chapter_index,
            chapter_page_index: value.chapter_page_index,
            chapter_page_count: value.chapter_page_count,
            global_page_index: value.global_page_index,
            global_page_count_estimate: value.global_page_count_estimate,
            progress_chapter: normalize_progress(value.progress_chapter),
            progress_book: value.progress_book.map(normalize_progress),
        }
    }
}

impl From<PersistedPageMetrics> for PageMetrics {
    fn from(value: PersistedPageMetrics) -> Self {
        Self {
            chapter_index: value.chapter_index,
            chapter_page_index: value.chapter_page_index,
            chapter_page_count: value.chapter_page_count,
            global_page_index: value.global_page_index,
            global_page_count_estimate: value.global_page_count_estimate,
            progress_chapter: normalize_progress(value.progress_chapter),
            progress_book: value.progress_book.map(normalize_progress),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PersistedDrawCommand {
    Text(PersistedTextCommand),
    Rule(PersistedRuleCommand),
    ImageObject(PersistedImageObjectCommand),
    Rect(PersistedRectCommand),
    PageChrome(PersistedPageChromeCommand),
}

impl From<&DrawCommand> for PersistedDrawCommand {
    fn from(value: &DrawCommand) -> Self {
        match value {
            DrawCommand::Text(cmd) => Self::Text(cmd.into()),
            DrawCommand::Rule(cmd) => Self::Rule((*cmd).into()),
            DrawCommand::ImageObject(cmd) => Self::ImageObject(cmd.into()),
            DrawCommand::Rect(cmd) => Self::Rect((*cmd).into()),
            DrawCommand::PageChrome(cmd) => Self::PageChrome(cmd.into()),
        }
    }
}

impl From<PersistedDrawCommand> for DrawCommand {
    fn from(value: PersistedDrawCommand) -> Self {
        match value {
            PersistedDrawCommand::Text(cmd) => Self::Text(cmd.into()),
            PersistedDrawCommand::Rule(cmd) => Self::Rule(cmd.into()),
            PersistedDrawCommand::ImageObject(cmd) => Self::ImageObject(cmd.into()),
            PersistedDrawCommand::Rect(cmd) => Self::Rect(cmd.into()),
            PersistedDrawCommand::PageChrome(cmd) => Self::PageChrome(cmd.into()),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedTextCommand {
    pub x: i32,
    pub baseline_y: i32,
    pub text: String,
    pub font_id: Option<u32>,
    pub style: PersistedResolvedTextStyle,
}

impl From<&TextCommand> for PersistedTextCommand {
    fn from(value: &TextCommand) -> Self {
        Self {
            x: value.x,
            baseline_y: value.baseline_y,
            text: value.text.clone(),
            font_id: value.font_id,
            style: (&value.style).into(),
        }
    }
}

impl From<PersistedTextCommand> for TextCommand {
    fn from(value: PersistedTextCommand) -> Self {
        Self {
            x: value.x,
            baseline_y: value.baseline_y,
            text: value.text,
            font_id: value.font_id,
            style: value.style.into(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedResolvedTextStyle {
    pub font_id: Option<u32>,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub size_px: f32,
    pub line_height: f32,
    pub letter_spacing: f32,
    pub role: PersistedBlockRole,
    pub justify_mode: PersistedJustifyMode,
}

impl From<&ResolvedTextStyle> for PersistedResolvedTextStyle {
    fn from(value: &ResolvedTextStyle) -> Self {
        Self {
            font_id: value.font_id,
            family: value.family.to_string(),
            weight: value.weight,
            italic: value.italic,
            size_px: value.size_px,
            line_height: value.line_height,
            letter_spacing: value.letter_spacing,
            role: value.role.into(),
            justify_mode: value.justify_mode.into(),
        }
    }
}

impl From<PersistedResolvedTextStyle> for ResolvedTextStyle {
    fn from(value: PersistedResolvedTextStyle) -> Self {
        Self {
            font_id: value.font_id,
            family: value.family.into(),
            weight: value.weight,
            italic: value.italic,
            size_px: value.size_px,
            line_height: value.line_height,
            letter_spacing: value.letter_spacing,
            role: value.role.into(),
            justify_mode: value.justify_mode.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum PersistedBlockRole {
    #[default]
    Body,
    Paragraph,
    Heading(u8),
    ListItem,
    FigureCaption,
    Preformatted,
}

impl From<BlockRole> for PersistedBlockRole {
    fn from(value: BlockRole) -> Self {
        match value {
            BlockRole::Body => Self::Body,
            BlockRole::Paragraph => Self::Paragraph,
            BlockRole::Heading(level) => Self::Heading(level),
            BlockRole::ListItem => Self::ListItem,
            BlockRole::FigureCaption => Self::FigureCaption,
            BlockRole::Preformatted => Self::Preformatted,
        }
    }
}

impl From<PersistedBlockRole> for BlockRole {
    fn from(value: PersistedBlockRole) -> Self {
        match value {
            PersistedBlockRole::Body => Self::Body,
            PersistedBlockRole::Paragraph => Self::Paragraph,
            PersistedBlockRole::Heading(level) => Self::Heading(level),
            PersistedBlockRole::ListItem => Self::ListItem,
            PersistedBlockRole::FigureCaption => Self::FigureCaption,
            PersistedBlockRole::Preformatted => Self::Preformatted,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum PersistedJustifyMode {
    #[default]
    None,
    InterWord {
        extra_px_total: i32,
    },
    AlignRight {
        offset_px: i32,
    },
    AlignCenter {
        offset_px: i32,
    },
}

impl From<JustifyMode> for PersistedJustifyMode {
    fn from(value: JustifyMode) -> Self {
        match value {
            JustifyMode::None => Self::None,
            JustifyMode::InterWord { extra_px_total } => Self::InterWord { extra_px_total },
            JustifyMode::AlignRight { offset_px } => Self::AlignRight { offset_px },
            JustifyMode::AlignCenter { offset_px } => Self::AlignCenter { offset_px },
        }
    }
}

impl From<PersistedJustifyMode> for JustifyMode {
    fn from(value: PersistedJustifyMode) -> Self {
        match value {
            PersistedJustifyMode::None => Self::None,
            PersistedJustifyMode::InterWord { extra_px_total } => {
                Self::InterWord { extra_px_total }
            }
            PersistedJustifyMode::AlignRight { offset_px } => Self::AlignRight { offset_px },
            PersistedJustifyMode::AlignCenter { offset_px } => Self::AlignCenter { offset_px },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PersistedRuleCommand {
    pub x: i32,
    pub y: i32,
    pub length: u32,
    pub thickness: u32,
    pub horizontal: bool,
}

impl From<RuleCommand> for PersistedRuleCommand {
    fn from(value: RuleCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            length: value.length,
            thickness: value.thickness,
            horizontal: value.horizontal,
        }
    }
}

impl From<PersistedRuleCommand> for RuleCommand {
    fn from(value: PersistedRuleCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            length: value.length,
            thickness: value.thickness,
            horizontal: value.horizontal,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedImageObjectCommand {
    pub src: String,
    pub alt: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl From<&ImageObjectCommand> for PersistedImageObjectCommand {
    fn from(value: &ImageObjectCommand) -> Self {
        Self {
            src: value.src.clone(),
            alt: value.alt.clone(),
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

impl From<PersistedImageObjectCommand> for ImageObjectCommand {
    fn from(value: PersistedImageObjectCommand) -> Self {
        Self {
            src: value.src,
            alt: value.alt,
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PersistedRectCommand {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub fill: bool,
}

impl From<RectCommand> for PersistedRectCommand {
    fn from(value: RectCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
            fill: value.fill,
        }
    }
}

impl From<PersistedRectCommand> for RectCommand {
    fn from(value: PersistedRectCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
            fill: value.fill,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistedPageChromeCommand {
    pub kind: PersistedPageChromeKind,
    pub text: Option<String>,
    pub current: Option<usize>,
    pub total: Option<usize>,
}

impl From<&PageChromeCommand> for PersistedPageChromeCommand {
    fn from(value: &PageChromeCommand) -> Self {
        Self {
            kind: value.kind.into(),
            text: value.text.clone(),
            current: value.current,
            total: value.total,
        }
    }
}

impl From<PersistedPageChromeCommand> for PageChromeCommand {
    fn from(value: PersistedPageChromeCommand) -> Self {
        Self {
            kind: value.kind.into(),
            text: value.text,
            current: value.current,
            total: value.total,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedPageChromeKind {
    #[default]
    Header,
    Footer,
    Progress,
}

impl From<PageChromeKind> for PersistedPageChromeKind {
    fn from(value: PageChromeKind) -> Self {
        match value {
            PageChromeKind::Header => Self::Header,
            PageChromeKind::Footer => Self::Footer,
            PageChromeKind::Progress => Self::Progress,
        }
    }
}

impl From<PersistedPageChromeKind> for PageChromeKind {
    fn from(value: PersistedPageChromeKind) -> Self {
        match value {
            PersistedPageChromeKind::Header => Self::Header,
            PersistedPageChromeKind::Footer => Self::Footer,
            PersistedPageChromeKind::Progress => Self::Progress,
        }
    }
}

fn normalize_progress(progress: f32) -> f32 {
    if !progress.is_finite() {
        return 0.0;
    }
    progress.clamp(0.0, 1.0)
}
