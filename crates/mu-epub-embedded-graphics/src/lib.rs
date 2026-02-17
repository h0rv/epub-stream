//! embedded-graphics renderer for `mu-epub-render` pages.

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

use embedded_graphics::{
    mono_font::{
        ascii::{
            FONT_10X20, FONT_6X13_BOLD, FONT_6X13_ITALIC, FONT_6X9, FONT_7X13_BOLD,
            FONT_7X13_ITALIC, FONT_7X14, FONT_7X14_BOLD, FONT_8X13, FONT_8X13_BOLD,
            FONT_8X13_ITALIC, FONT_9X15_BOLD, FONT_9X18, FONT_9X18_BOLD,
        },
        MonoFont, MonoTextStyle,
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use mu_epub_render::{
    DrawCommand, ImageObjectCommand, JustifyMode, PageChromeCommand, PageChromeConfig,
    PageChromeKind, PageChromeTextStyle, RenderPage, ResolvedTextStyle, TextCommand,
};
use std::borrow::Cow;

/// Backend-local font identifier used for metrics and rasterization dispatch.
pub type FontId = u8;

/// Why style-to-font mapping had to fallback to a default face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontFallbackReason {
    UnknownFamily,
    UnknownFontId,
    UnsupportedWeightItalic,
    BackendUnavailable,
}

/// Resolved font selection for a text style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontSelection {
    pub font_id: FontId,
    pub fallback_reason: Option<FontFallbackReason>,
}

/// Backend-provided metrics for a specific font id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontMetrics {
    pub char_width: i32,
    pub space_width: i32,
}

/// Face registration descriptor for dynamic font backends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFaceRegistration<'a> {
    pub family: &'a str,
    pub weight: u16,
    pub italic: bool,
    pub data: &'a [u8],
}

/// Backend rendering capabilities used by callers for graceful degradation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub ttf: bool,
    pub images: bool,
    pub svg: bool,
    pub justification: bool,
}

/// Font abstraction used by the renderer's text paths.
pub trait FontBackend {
    fn register_faces(&mut self, faces: &[FontFaceRegistration<'_>]) -> usize;
    fn resolve_font(&self, style: &ResolvedTextStyle, font_id: Option<u32>) -> FontSelection;
    fn metrics(&self, font_id: FontId) -> FontMetrics;
    fn draw_text_run<D>(
        &self,
        display: &mut D,
        font_id: FontId,
        text: &str,
        origin: Point,
    ) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>;

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            ttf: false,
            images: false,
            svg: false,
            justification: true,
        }
    }
}

/// Mono-font backend used by default and matching previous behavior.
#[derive(Clone, Copy, Debug, Default)]
pub struct MonoFontBackend;

impl MonoFontBackend {
    const SIZE_SMALL: FontId = 0;
    const SIZE_MEDIUM: FontId = 1;
    const SIZE_LARGE: FontId = 2;
    const SIZE_XL: FontId = 3;

    const VARIANT_REGULAR: FontId = 0;
    const VARIANT_ITALIC: FontId = 1;
    const VARIANT_BOLD: FontId = 2;
    const VARIANT_BOLD_ITALIC: FontId = 3;

    fn encode_font_id(size_bucket: FontId, variant: FontId) -> FontId {
        (size_bucket << 2) | (variant & 0x03)
    }

    fn decode_font_id(font_id: FontId) -> (FontId, FontId) {
        ((font_id >> 2) & 0x03, font_id & 0x03)
    }

    fn size_bucket_for(style: &ResolvedTextStyle) -> FontId {
        if style.size_px >= 24.0 {
            Self::SIZE_XL
        } else if style.size_px >= 20.0 {
            Self::SIZE_LARGE
        } else if style.size_px >= 16.0 {
            Self::SIZE_MEDIUM
        } else {
            Self::SIZE_SMALL
        }
    }

    fn style_variant_for(style: &ResolvedTextStyle) -> FontId {
        if style.weight >= 700 && style.italic {
            Self::VARIANT_BOLD_ITALIC
        } else if style.weight >= 700 {
            Self::VARIANT_BOLD
        } else if style.italic {
            Self::VARIANT_ITALIC
        } else {
            Self::VARIANT_REGULAR
        }
    }

    fn font_for(font_id: FontId) -> (&'static MonoFont<'static>, Option<FontFallbackReason>) {
        let (size_bucket, variant) = Self::decode_font_id(font_id);
        match (size_bucket, variant) {
            (Self::SIZE_SMALL, Self::VARIANT_REGULAR) => (&FONT_6X9, None),
            (Self::SIZE_SMALL, Self::VARIANT_ITALIC) => (&FONT_6X13_ITALIC, None),
            (Self::SIZE_SMALL, Self::VARIANT_BOLD) => (&FONT_6X13_BOLD, None),
            (Self::SIZE_SMALL, Self::VARIANT_BOLD_ITALIC) => (
                &FONT_6X13_BOLD,
                Some(FontFallbackReason::UnsupportedWeightItalic),
            ),
            (Self::SIZE_MEDIUM, Self::VARIANT_REGULAR) => (&FONT_7X14, None),
            (Self::SIZE_MEDIUM, Self::VARIANT_ITALIC) => (&FONT_7X13_ITALIC, None),
            (Self::SIZE_MEDIUM, Self::VARIANT_BOLD) => (&FONT_7X14_BOLD, None),
            (Self::SIZE_MEDIUM, Self::VARIANT_BOLD_ITALIC) => (
                &FONT_7X14_BOLD,
                Some(FontFallbackReason::UnsupportedWeightItalic),
            ),
            (Self::SIZE_LARGE, Self::VARIANT_REGULAR) => (&FONT_8X13, None),
            (Self::SIZE_LARGE, Self::VARIANT_ITALIC) => (&FONT_8X13_ITALIC, None),
            (Self::SIZE_LARGE, Self::VARIANT_BOLD) => (&FONT_8X13_BOLD, None),
            (Self::SIZE_LARGE, Self::VARIANT_BOLD_ITALIC) => (
                &FONT_8X13_BOLD,
                Some(FontFallbackReason::UnsupportedWeightItalic),
            ),
            (Self::SIZE_XL, Self::VARIANT_REGULAR) => (&FONT_10X20, None),
            (Self::SIZE_XL, Self::VARIANT_ITALIC) => (
                &FONT_9X18,
                Some(FontFallbackReason::UnsupportedWeightItalic),
            ),
            (Self::SIZE_XL, Self::VARIANT_BOLD) => (&FONT_9X18_BOLD, None),
            (Self::SIZE_XL, Self::VARIANT_BOLD_ITALIC) => (
                &FONT_9X18_BOLD,
                Some(FontFallbackReason::UnsupportedWeightItalic),
            ),
            _ => (&FONT_8X13, Some(FontFallbackReason::UnknownFontId)),
        }
    }

    fn style_for(font_id: FontId) -> MonoTextStyle<'static, BinaryColor> {
        let (font, _) = Self::font_for(font_id);
        MonoTextStyle::new(font, BinaryColor::On)
    }

    fn family_supported(family: &str) -> bool {
        matches!(
            family.trim().to_ascii_lowercase().as_str(),
            "monospace" | "mono" | "fixed" | "serif" | "sans-serif"
        )
    }
}

impl FontBackend for MonoFontBackend {
    fn register_faces(&mut self, _faces: &[FontFaceRegistration<'_>]) -> usize {
        0
    }

    fn resolve_font(&self, style: &ResolvedTextStyle, font_id: Option<u32>) -> FontSelection {
        let mut fallback_reason =
            (!Self::family_supported(&style.family)).then_some(FontFallbackReason::UnknownFamily);

        if font_id.is_some_and(|id| id > u8::MAX as u32) {
            fallback_reason = Some(FontFallbackReason::UnknownFontId);
        }

        let mapped_by_style =
            Self::encode_font_id(Self::size_bucket_for(style), Self::style_variant_for(style));
        let (_, style_fallback) = Self::font_for(mapped_by_style);
        if style_fallback.is_some() {
            fallback_reason = style_fallback;
        }

        FontSelection {
            font_id: mapped_by_style,
            fallback_reason,
        }
    }

    fn metrics(&self, font_id: FontId) -> FontMetrics {
        let style = Self::style_for(font_id);
        let width = style.font.character_size.width as i32;
        FontMetrics {
            char_width: width,
            space_width: width,
        }
    }

    fn draw_text_run<D>(
        &self,
        display: &mut D,
        font_id: FontId,
        text: &str,
        origin: Point,
    ) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let style = Self::style_for(font_id);
        let normalized = normalize_text_for_mono(text);
        Text::with_baseline(normalized.as_ref(), origin, style, Baseline::Top).draw(display)?;
        Ok((normalized.chars().count() as i32) * (style.font.character_size.width as i32))
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            ttf: false,
            images: false,
            svg: false,
            justification: true,
        }
    }
}

fn normalize_text_for_mono(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|ch| {
        matches!(
            ch,
            '\u{00A0}' // nbsp
                | '\u{2013}' // en dash
                | '\u{2014}' // em dash
                | '\u{2018}' // left single quote
                | '\u{2019}' // right single quote
                | '\u{201C}' // left double quote
                | '\u{201D}' // right double quote
                | '\u{2026}' // ellipsis
        )
    }) {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\u{00A0}' => out.push(' '),
            '\u{2013}' | '\u{2014}' => out.push('-'),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201C}' | '\u{201D}' => out.push('"'),
            '\u{2026}' => out.push_str("..."),
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

/// Optional TTF backend feature gate.
#[cfg(feature = "ttf-backend")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TtfFallbackPolicy {
    /// Always fallback to mono-font rendering when TTF shaping/raster is unavailable.
    #[default]
    MonoOnly,
}

/// Options for the experimental `ttf-backend` path.
///
/// Note: the current backend remains fallback-oriented and routes drawing
/// through mono rendering until full TTF rasterization support is implemented.
#[cfg(feature = "ttf-backend")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TtfBackendOptions {
    /// Maximum number of faces accepted via registration.
    pub max_faces: usize,
    /// Maximum bytes accepted for a single face payload.
    pub max_face_bytes: usize,
    /// Maximum aggregate bytes accepted across all registered faces.
    pub max_total_face_bytes: usize,
    /// Policy for unresolved/unsupported faces.
    pub fallback_policy: TtfFallbackPolicy,
}

#[cfg(feature = "ttf-backend")]
impl Default for TtfBackendOptions {
    fn default() -> Self {
        Self {
            max_faces: 64,
            max_face_bytes: 8 * 1024 * 1024,
            max_total_face_bytes: 64 * 1024 * 1024,
            fallback_policy: TtfFallbackPolicy::MonoOnly,
        }
    }
}

/// Optional TTF backend feature gate.
#[cfg(feature = "ttf-backend")]
#[derive(Clone, Copy, Debug)]
pub struct TtfFontBackend {
    mono_fallback: MonoFontBackend,
    options: TtfBackendOptions,
    accepted_faces: usize,
    accepted_total_bytes: usize,
}

#[cfg(feature = "ttf-backend")]
impl Default for TtfFontBackend {
    fn default() -> Self {
        Self::new(TtfBackendOptions::default())
    }
}

#[cfg(feature = "ttf-backend")]
impl TtfFontBackend {
    /// Create a TTF backend with explicit options.
    pub fn new(options: TtfBackendOptions) -> Self {
        Self {
            mono_fallback: MonoFontBackend,
            options,
            accepted_faces: 0,
            accepted_total_bytes: 0,
        }
    }

    /// Returns options currently used by the backend.
    pub fn options(&self) -> TtfBackendOptions {
        self.options
    }

    /// Text status describing current feature maturity.
    pub fn status(&self) -> &'static str {
        "fallback_only"
    }
}

#[cfg(feature = "ttf-backend")]
impl FontBackend for TtfFontBackend {
    fn register_faces(&mut self, faces: &[FontFaceRegistration<'_>]) -> usize {
        let mut accepted = 0usize;
        for face in faces {
            if self.accepted_faces >= self.options.max_faces {
                break;
            }
            let bytes = face.data.len();
            if bytes > self.options.max_face_bytes {
                continue;
            }
            if self.accepted_total_bytes.saturating_add(bytes) > self.options.max_total_face_bytes {
                continue;
            }
            self.accepted_faces += 1;
            self.accepted_total_bytes += bytes;
            accepted += 1;
        }
        accepted
    }

    fn resolve_font(&self, style: &ResolvedTextStyle, font_id: Option<u32>) -> FontSelection {
        let mut selection = self.mono_fallback.resolve_font(style, font_id);
        selection.fallback_reason = Some(FontFallbackReason::BackendUnavailable);
        selection
    }

    fn metrics(&self, font_id: FontId) -> FontMetrics {
        self.mono_fallback.metrics(font_id)
    }

    fn draw_text_run<D>(
        &self,
        display: &mut D,
        font_id: FontId,
        text: &str,
        origin: Point,
    ) -> Result<i32, D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        self.mono_fallback
            .draw_text_run(display, font_id, text, origin)
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            ttf: true,
            images: false,
            svg: false,
            justification: true,
        }
    }
}

/// embedded-graphics backend configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EgRenderConfig {
    /// Clear display before drawing page.
    pub clear_first: bool,
    /// Page chrome rendering policy and geometry.
    pub page_chrome: PageChromeConfig,
}

impl Default for EgRenderConfig {
    fn default() -> Self {
        Self {
            clear_first: true,
            page_chrome: PageChromeConfig::geometry_defaults(),
        }
    }
}

/// Draw-command executor for embedded-graphics targets.
#[derive(Clone, Copy, Debug)]
pub struct EgRenderer<B = MonoFontBackend> {
    cfg: EgRenderConfig,
    backend: B,
}

impl Default for EgRenderer<MonoFontBackend> {
    fn default() -> Self {
        Self {
            cfg: EgRenderConfig::default(),
            backend: MonoFontBackend,
        }
    }
}

impl<B> EgRenderer<B>
where
    B: FontBackend,
{
    /// Create renderer with config and backend.
    pub fn with_backend(cfg: EgRenderConfig, backend: B) -> Self {
        Self { cfg, backend }
    }

    /// Expose the configured font backend for direct mutation.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Register one or more font faces in the backend.
    pub fn register_faces(&mut self, faces: &[FontFaceRegistration<'_>]) -> usize {
        self.backend.register_faces(faces)
    }

    /// Report backend capabilities for graceful feature degradation.
    pub fn capabilities(&self) -> BackendCapabilities {
        self.backend.capabilities()
    }

    /// Render a page to a draw target.
    pub fn render_page<D>(&self, page: &RenderPage, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        self.render_content(page, display)?;
        self.render_overlay(page, display)?;
        Ok(())
    }

    /// Render content commands from the current single-stream page output.
    pub fn render_content<D>(&self, page: &RenderPage, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        if self.cfg.clear_first {
            display.clear(BinaryColor::Off)?;
        }
        let content_iter: Box<dyn Iterator<Item = &DrawCommand> + '_> =
            if !page.content_commands.is_empty() {
                Box::new(page.content_commands.iter())
            } else {
                Box::new(
                    page.commands
                        .iter()
                        .filter(|cmd| !matches!(cmd, DrawCommand::PageChrome(_))),
                )
            };
        for cmd in content_iter {
            self.draw_command(display, cmd)?;
        }
        Ok(())
    }

    /// Render overlay/chrome commands from the current single-stream page output.
    pub fn render_overlay<D>(&self, page: &RenderPage, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        if !page.chrome_commands.is_empty() || !page.overlay_commands.is_empty() {
            for cmd in page
                .chrome_commands
                .iter()
                .chain(page.overlay_commands.iter())
            {
                self.draw_command(display, cmd)?;
            }
            return Ok(());
        }
        for cmd in page
            .commands
            .iter()
            .filter(|cmd| matches!(cmd, DrawCommand::PageChrome(_)))
        {
            self.draw_command(display, cmd)?;
        }
        Ok(())
    }

    /// Render pre-split content commands (compatible with content/overlay page outputs).
    pub fn render_content_commands<D>(
        &self,
        commands: &[DrawCommand],
        display: &mut D,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        if self.cfg.clear_first {
            display.clear(BinaryColor::Off)?;
        }
        for cmd in commands {
            self.draw_command(display, cmd)?;
        }
        Ok(())
    }

    /// Render pre-split overlay commands (compatible with content/overlay page outputs).
    pub fn render_overlay_commands<D>(
        &self,
        commands: &[DrawCommand],
        display: &mut D,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        for cmd in commands {
            self.draw_command(display, cmd)?;
        }
        Ok(())
    }

    fn draw_command<D>(&self, display: &mut D, cmd: &DrawCommand) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        match cmd {
            DrawCommand::Text(text) => self.draw_text(display, text),
            DrawCommand::Rule(rule) => {
                let style = PrimitiveStyle::with_stroke(BinaryColor::On, rule.thickness);
                let end = if rule.horizontal {
                    Point::new(rule.x + rule.length as i32, rule.y)
                } else {
                    Point::new(rule.x, rule.y + rule.length as i32)
                };
                Line::new(Point::new(rule.x, rule.y), end)
                    .into_styled(style)
                    .draw(display)?;
                Ok(())
            }
            DrawCommand::Rect(rect) => {
                let shape = Rectangle::new(
                    Point::new(rect.x, rect.y),
                    Size::new(rect.width, rect.height),
                );
                if rect.fill {
                    shape
                        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                        .draw(display)?;
                } else {
                    shape
                        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                        .draw(display)?;
                }
                Ok(())
            }
            DrawCommand::ImageObject(image) => self.draw_image_fallback(display, image),
            DrawCommand::PageChrome(chrome) => self.draw_page_chrome(display, chrome),
        }
    }

    fn draw_image_fallback<D>(
        &self,
        display: &mut D,
        image: &ImageObjectCommand,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        Rectangle::new(
            Point::new(image.x, image.y),
            Size::new(image.width.max(1), image.height.max(1)),
        )
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
        Ok(())
    }

    fn draw_text<D>(&self, display: &mut D, cmd: &TextCommand) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let requested_font_id = cmd.font_id.or(cmd.style.font_id);
        let selection = self.backend.resolve_font(&cmd.style, requested_font_id);
        let metrics = self.backend.metrics(selection.font_id);
        let origin = Point::new(cmd.x, cmd.baseline_y);

        match cmd.style.justify_mode {
            JustifyMode::None => self
                .backend
                .draw_text_run(display, selection.font_id, &cmd.text, origin)
                .map(|_| ()),
            JustifyMode::InterWord { extra_px_total } => {
                let spaces = cmd.text.chars().filter(|c| *c == ' ').count() as i32;
                if spaces <= 0 || extra_px_total <= 0 {
                    self.backend
                        .draw_text_run(display, selection.font_id, &cmd.text, origin)?;
                    return Ok(());
                }

                // Guard against visually noisy justification when the layout
                // asks for too much inter-word expansion for the active font.
                let max_extra_per_space = (metrics.space_width / 2).max(1);
                let capped_total = extra_px_total.min(max_extra_per_space * spaces);
                let per_space = capped_total / spaces;
                let mut remainder = capped_total % spaces;
                let mut x = cmd.x;
                let mut run_start = 0usize;

                for (idx, ch) in cmd.text.char_indices() {
                    if ch == ' ' {
                        if run_start < idx {
                            let run = &cmd.text[run_start..idx];
                            x += self.backend.draw_text_run(
                                display,
                                selection.font_id,
                                run,
                                Point::new(x, cmd.baseline_y),
                            )?;
                        }

                        x += metrics.space_width + per_space;
                        if remainder > 0 {
                            x += 1;
                            remainder -= 1;
                        }
                        run_start = idx + ch.len_utf8();
                    }
                }

                if run_start < cmd.text.len() {
                    let run = &cmd.text[run_start..];
                    self.backend.draw_text_run(
                        display,
                        selection.font_id,
                        run,
                        Point::new(x, cmd.baseline_y),
                    )?;
                }
                Ok(())
            }
        }
    }

    fn draw_page_chrome<D>(
        &self,
        display: &mut D,
        chrome: &PageChromeCommand,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let bounds = display.bounding_box();
        let width = bounds.size.width as i32;
        let height = bounds.size.height as i32;
        let chrome_cfg = self.cfg.page_chrome;
        match chrome.kind {
            PageChromeKind::Header => {
                if !chrome_cfg.header_enabled {
                    return Ok(());
                }
                if let Some(text) = &chrome.text {
                    let style = mono_text_style(chrome_cfg.header_style);
                    Text::new(
                        text,
                        Point::new(chrome_cfg.header_x, chrome_cfg.header_baseline_y),
                        style,
                    )
                    .draw(display)?;
                }
            }
            PageChromeKind::Footer => {
                if !chrome_cfg.footer_enabled {
                    return Ok(());
                }
                if let Some(text) = &chrome.text {
                    let style = mono_text_style(chrome_cfg.footer_style);
                    Text::new(
                        text,
                        Point::new(
                            chrome_cfg.footer_x,
                            height.saturating_sub(chrome_cfg.footer_baseline_from_bottom),
                        ),
                        style,
                    )
                    .draw(display)?;
                }
            }
            PageChromeKind::Progress => {
                if !chrome_cfg.progress_enabled {
                    return Ok(());
                }
                let current = chrome.current.unwrap_or(0);
                let total = chrome.total.unwrap_or(1).max(1);
                let bar_x = chrome_cfg.progress_x_inset;
                let bar_y = height.saturating_sub(chrome_cfg.progress_y_from_bottom);
                let bar_w = (width - (chrome_cfg.progress_x_inset * 2)).max(1) as u32;
                let bar_h = chrome_cfg.progress_height.max(1);
                let filled = ((bar_w as usize * current.min(total)) / total) as u32;
                Rectangle::new(Point::new(bar_x, bar_y), Size::new(bar_w, bar_h))
                    .into_styled(PrimitiveStyle::with_stroke(
                        BinaryColor::On,
                        chrome_cfg.progress_stroke_width.max(1),
                    ))
                    .draw(display)?;
                Rectangle::new(Point::new(bar_x, bar_y), Size::new(filled, bar_h))
                    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                    .draw(display)?;
            }
        }
        Ok(())
    }
}

fn mono_text_style(style: PageChromeTextStyle) -> MonoTextStyle<'static, BinaryColor> {
    match style {
        PageChromeTextStyle::Regular => MonoTextStyle::new(&FONT_8X13, BinaryColor::On),
        PageChromeTextStyle::Bold => MonoTextStyle::new(&FONT_7X13_BOLD, BinaryColor::On),
        PageChromeTextStyle::Italic => MonoTextStyle::new(&FONT_6X13_ITALIC, BinaryColor::On),
        PageChromeTextStyle::BoldItalic => MonoTextStyle::new(&FONT_9X15_BOLD, BinaryColor::On),
    }
}

impl EgRenderer<MonoFontBackend> {
    /// Create renderer with config.
    pub fn new(cfg: EgRenderConfig) -> Self {
        Self {
            cfg,
            backend: MonoFontBackend,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embedded_graphics::mock_display::MockDisplay;
    use std::{cell::RefCell, rc::Rc};

    use mu_epub_render::{
        BlockRole, DrawCommand, JustifyMode, PageChromeCommand, PageChromeKind, RenderPage,
        ResolvedTextStyle, TextCommand,
    };

    #[derive(Default)]
    struct PixelCaptureDisplay {
        size: Size,
        on_pixels: Vec<Point>,
    }

    impl PixelCaptureDisplay {
        fn with_size(width: u32, height: u32) -> Self {
            Self {
                size: Size::new(width, height),
                on_pixels: Vec::new(),
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
            for Pixel(point, color) in pixels {
                if color == BinaryColor::On {
                    self.on_pixels.push(point);
                }
            }
            Ok(())
        }
    }

    #[derive(Clone, Debug, Default)]
    struct BackendSpy {
        state: Rc<RefCell<BackendSpyState>>,
    }

    fn page_with_commands(page_number: usize, commands: Vec<DrawCommand>) -> RenderPage {
        RenderPage {
            page_number,
            commands,
            ..RenderPage::new(page_number)
        }
    }

    #[derive(Debug, Default)]
    struct BackendSpyState {
        register_calls: usize,
        registered_face_counts: Vec<usize>,
        resolve_calls: usize,
        metrics_calls: usize,
        draw_runs: Vec<String>,
    }

    impl BackendSpy {
        fn state(&self) -> Rc<RefCell<BackendSpyState>> {
            Rc::clone(&self.state)
        }
    }

    impl FontBackend for BackendSpy {
        fn register_faces(&mut self, faces: &[FontFaceRegistration<'_>]) -> usize {
            let mut state = self.state.borrow_mut();
            state.register_calls += 1;
            state.registered_face_counts.push(faces.len());
            faces.len()
        }

        fn resolve_font(&self, _style: &ResolvedTextStyle, _font_id: Option<u32>) -> FontSelection {
            self.state.borrow_mut().resolve_calls += 1;
            FontSelection {
                font_id: 9,
                fallback_reason: Some(FontFallbackReason::UnknownFamily),
            }
        }

        fn metrics(&self, _font_id: FontId) -> FontMetrics {
            self.state.borrow_mut().metrics_calls += 1;
            FontMetrics {
                char_width: 1,
                space_width: 1,
            }
        }

        fn draw_text_run<D>(
            &self,
            _display: &mut D,
            _font_id: FontId,
            text: &str,
            _origin: Point,
        ) -> Result<i32, D::Error>
        where
            D: DrawTarget<Color = BinaryColor>,
        {
            self.state.borrow_mut().draw_runs.push(text.to_string());
            Ok(text.chars().count() as i32)
        }
    }

    #[test]
    fn renders_text_command_without_error() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let renderer = EgRenderer::default();
        let style = ResolvedTextStyle {
            font_id: None,
            family: "serif".to_string(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            role: BlockRole::Body,
            justify_mode: JustifyMode::None,
        };
        let page = page_with_commands(
            1,
            vec![DrawCommand::Text(TextCommand {
                x: 10,
                baseline_y: 20,
                text: "Hello".to_string(),
                font_id: None,
                style,
            })],
        );

        let result = renderer.render_page(&page, &mut display);
        assert!(result.is_ok());
    }

    #[test]
    fn text_command_execution_uses_backend_draw() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let backend = BackendSpy::default();
        let state = backend.state();
        let renderer = EgRenderer::with_backend(EgRenderConfig::default(), backend);
        let style = ResolvedTextStyle {
            font_id: None,
            family: "serif".to_string(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            role: BlockRole::Body,
            justify_mode: JustifyMode::None,
        };
        let page = page_with_commands(
            1,
            vec![DrawCommand::Text(TextCommand {
                x: 0,
                baseline_y: 10,
                text: "cmd".to_string(),
                font_id: None,
                style,
            })],
        );

        let result = renderer.render_page(&page, &mut display);
        assert!(result.is_ok());
        let snapshot = state.borrow();
        assert_eq!(snapshot.resolve_calls, 1);
        assert_eq!(snapshot.metrics_calls, 1);
        assert_eq!(snapshot.draw_runs, vec!["cmd".to_string()]);
    }

    #[test]
    fn renderer_register_faces_forwards_to_backend() {
        let backend = BackendSpy::default();
        let state = backend.state();
        let mut renderer = EgRenderer::with_backend(EgRenderConfig::default(), backend);
        let font_data_a = [0x00u8, 0x01];
        let font_data_b = [0x02u8];
        let faces = [
            FontFaceRegistration {
                family: "Body",
                weight: 400,
                italic: false,
                data: &font_data_a,
            },
            FontFaceRegistration {
                family: "Body",
                weight: 700,
                italic: true,
                data: &font_data_b,
            },
        ];

        let registered = renderer.register_faces(&faces);
        assert_eq!(registered, 2);
        let snapshot = state.borrow();
        assert_eq!(snapshot.register_calls, 1);
        assert_eq!(snapshot.registered_face_counts, vec![2]);
    }

    #[test]
    fn backend_mut_exposes_font_backend_registration() {
        let backend = BackendSpy::default();
        let state = backend.state();
        let mut renderer = EgRenderer::with_backend(EgRenderConfig::default(), backend);

        let registered = renderer.backend_mut().register_faces(&[]);
        assert_eq!(registered, 0);
        let snapshot = state.borrow();
        assert_eq!(snapshot.register_calls, 1);
        assert_eq!(snapshot.registered_face_counts, vec![0]);
    }

    #[test]
    fn mono_backend_capabilities_match_expected_flags() {
        let renderer = EgRenderer::default();
        assert_eq!(
            renderer.capabilities(),
            BackendCapabilities {
                ttf: false,
                images: false,
                svg: false,
                justification: true,
            }
        );
    }

    #[test]
    fn justification_and_non_justification_use_backend_paths() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let backend = BackendSpy::default();
        let state = backend.state();
        let renderer = EgRenderer::with_backend(EgRenderConfig::default(), backend);
        let base_style = ResolvedTextStyle {
            font_id: None,
            family: "serif".to_string(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            role: BlockRole::Body,
            justify_mode: JustifyMode::None,
        };

        let plain = TextCommand {
            x: 0,
            baseline_y: 10,
            text: "aa bb".to_string(),
            font_id: None,
            style: base_style.clone(),
        };
        let justified = TextCommand {
            x: 0,
            baseline_y: 20,
            text: "aa bb".to_string(),
            font_id: None,
            style: ResolvedTextStyle {
                justify_mode: JustifyMode::InterWord { extra_px_total: 2 },
                ..base_style
            },
        };
        let page = page_with_commands(
            1,
            vec![DrawCommand::Text(plain), DrawCommand::Text(justified)],
        );

        let result = renderer.render_page(&page, &mut display);
        assert!(result.is_ok());
        let snapshot = state.borrow();
        assert_eq!(snapshot.resolve_calls, 2);
        assert_eq!(snapshot.metrics_calls, 2);
        assert_eq!(snapshot.draw_runs, vec!["aa bb", "aa", "bb"]);
    }

    #[test]
    fn mono_backend_reports_fallback_reason_for_unknown_family() {
        let backend = MonoFontBackend;
        let style = ResolvedTextStyle {
            font_id: None,
            family: "fantasy".to_string(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            role: BlockRole::Body,
            justify_mode: JustifyMode::None,
        };

        let selection = backend.resolve_font(&style, None);
        assert_eq!(
            selection.fallback_reason,
            Some(FontFallbackReason::UnknownFamily)
        );
    }

    #[test]
    fn mono_backend_reports_unknown_font_id_fallback_reason() {
        let backend = MonoFontBackend;
        let style = ResolvedTextStyle {
            font_id: None,
            family: "monospace".to_string(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            role: BlockRole::Body,
            justify_mode: JustifyMode::None,
        };

        let selection = backend.resolve_font(&style, Some(999));
        assert_eq!(
            selection.fallback_reason,
            Some(FontFallbackReason::UnknownFontId)
        );
    }

    #[test]
    fn page_chrome_commands_are_rendered_not_dropped() {
        let mut display = MockDisplay::new();
        display.set_allow_overdraw(true);
        let renderer = EgRenderer::default();
        let page = page_with_commands(
            2,
            vec![
                DrawCommand::PageChrome(PageChromeCommand {
                    kind: PageChromeKind::Header,
                    text: Some("Header".to_string()),
                    current: None,
                    total: None,
                }),
                DrawCommand::PageChrome(PageChromeCommand {
                    kind: PageChromeKind::Footer,
                    text: Some("Footer".to_string()),
                    current: None,
                    total: None,
                }),
                DrawCommand::PageChrome(PageChromeCommand {
                    kind: PageChromeKind::Progress,
                    text: None,
                    current: Some(2),
                    total: Some(5),
                }),
            ],
        );
        let result = renderer.render_page(&page, &mut display);
        assert!(result.is_ok());
    }

    #[test]
    fn split_and_single_stream_render_paths_are_compatible() {
        let mut display_single = MockDisplay::new();
        display_single.set_allow_overdraw(true);
        let mut display_split = MockDisplay::new();
        display_split.set_allow_overdraw(true);
        let backend_single = BackendSpy::default();
        let backend_split = BackendSpy::default();
        let state_single = backend_single.state();
        let state_split = backend_split.state();
        let renderer_single = EgRenderer::with_backend(EgRenderConfig::default(), backend_single);
        let renderer_split = EgRenderer::with_backend(EgRenderConfig::default(), backend_split);
        let base_style = ResolvedTextStyle {
            font_id: None,
            family: "serif".to_string(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            role: BlockRole::Body,
            justify_mode: JustifyMode::None,
        };
        let content_commands = vec![
            DrawCommand::Text(TextCommand {
                x: 0,
                baseline_y: 10,
                text: "content".to_string(),
                font_id: None,
                style: base_style,
            }),
            DrawCommand::Rule(mu_epub_render::RuleCommand {
                x: 0,
                y: 12,
                length: 8,
                thickness: 1,
                horizontal: true,
            }),
        ];
        let overlay_commands = vec![DrawCommand::PageChrome(PageChromeCommand {
            kind: PageChromeKind::Footer,
            text: Some("footer".to_string()),
            current: None,
            total: None,
        })];
        let mut combined = content_commands.clone();
        combined.extend(overlay_commands.clone());
        let page = page_with_commands(1, combined);

        renderer_single
            .render_page(&page, &mut display_single)
            .expect("single-stream render should succeed");
        renderer_split
            .render_content_commands(&content_commands, &mut display_split)
            .expect("split content render should succeed");
        renderer_split
            .render_overlay_commands(&overlay_commands, &mut display_split)
            .expect("split overlay render should succeed");

        let snap_single = state_single.borrow();
        let snap_split = state_split.borrow();
        assert_eq!(snap_single.resolve_calls, snap_split.resolve_calls);
        assert_eq!(snap_single.metrics_calls, snap_split.metrics_calls);
        assert_eq!(snap_single.draw_runs, snap_split.draw_runs);
    }

    #[test]
    fn page_chrome_config_changes_progress_geometry() {
        let mut cfg = EgRenderConfig {
            clear_first: false,
            ..EgRenderConfig::default()
        };
        cfg.page_chrome.header_enabled = false;
        cfg.page_chrome.footer_enabled = false;
        cfg.page_chrome.progress_x_inset = 20;
        cfg.page_chrome.progress_y_from_bottom = 30;
        cfg.page_chrome.progress_height = 2;
        let renderer = EgRenderer::new(cfg);
        let page = page_with_commands(
            1,
            vec![DrawCommand::PageChrome(PageChromeCommand {
                kind: PageChromeKind::Progress,
                text: None,
                current: Some(1),
                total: Some(2),
            })],
        );
        let mut display = PixelCaptureDisplay::with_size(120, 80);

        let result = renderer.render_page(&page, &mut display);
        assert!(result.is_ok());

        let expected_y = 50;
        assert!(display
            .on_pixels
            .iter()
            .any(|p| p.y == expected_y && p.x >= 20));
        assert!(!display.on_pixels.iter().any(|p| p.y == 60));
    }

    #[test]
    fn page_chrome_config_can_suppress_renderer_chrome_drawing() {
        let mut cfg = EgRenderConfig {
            clear_first: false,
            ..EgRenderConfig::default()
        };
        cfg.page_chrome.header_enabled = false;
        cfg.page_chrome.footer_enabled = false;
        cfg.page_chrome.progress_enabled = false;
        let renderer = EgRenderer::new(cfg);
        let page = page_with_commands(
            1,
            vec![
                DrawCommand::PageChrome(PageChromeCommand {
                    kind: PageChromeKind::Header,
                    text: Some("Header".to_string()),
                    current: None,
                    total: None,
                }),
                DrawCommand::PageChrome(PageChromeCommand {
                    kind: PageChromeKind::Footer,
                    text: Some("Footer".to_string()),
                    current: None,
                    total: None,
                }),
                DrawCommand::PageChrome(PageChromeCommand {
                    kind: PageChromeKind::Progress,
                    text: None,
                    current: Some(1),
                    total: Some(3),
                }),
            ],
        );
        let mut display = PixelCaptureDisplay::with_size(120, 80);

        let result = renderer.render_page(&page, &mut display);
        assert!(result.is_ok());
        assert!(display.on_pixels.is_empty());
    }

    #[cfg(feature = "ttf-backend")]
    #[test]
    fn ttf_backend_exposes_options_and_status() {
        let opts = TtfBackendOptions {
            max_faces: 2,
            max_face_bytes: 8,
            max_total_face_bytes: 12,
            fallback_policy: TtfFallbackPolicy::MonoOnly,
        };
        let backend = TtfFontBackend::new(opts);
        assert_eq!(backend.options(), opts);
        assert_eq!(backend.status(), "fallback_only");
    }

    #[cfg(feature = "ttf-backend")]
    #[test]
    fn ttf_backend_registration_enforces_limits() {
        let opts = TtfBackendOptions {
            max_faces: 2,
            max_face_bytes: 4,
            max_total_face_bytes: 6,
            fallback_policy: TtfFallbackPolicy::MonoOnly,
        };
        let mut backend = TtfFontBackend::new(opts);
        let face_a = FontFaceRegistration {
            family: "A",
            weight: 400,
            italic: false,
            data: &[1, 2, 3],
        };
        let face_b = FontFaceRegistration {
            family: "B",
            weight: 400,
            italic: false,
            data: &[1, 2, 3],
        };
        let face_c_too_large = FontFaceRegistration {
            family: "C",
            weight: 400,
            italic: false,
            data: &[1, 2, 3, 4, 5],
        };
        let accepted = backend.register_faces(&[face_a, face_b, face_c_too_large]);
        assert_eq!(accepted, 2);
    }

    #[cfg(feature = "ttf-backend")]
    #[test]
    fn ttf_backend_capabilities_enable_ttf_flag() {
        let renderer =
            EgRenderer::with_backend(EgRenderConfig::default(), TtfFontBackend::default());
        assert_eq!(
            renderer.capabilities(),
            BackendCapabilities {
                ttf: true,
                images: false,
                svg: false,
                justification: true,
            }
        );
    }
}
