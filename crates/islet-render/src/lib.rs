//! Direct3D 11 / Direct2D / DirectComposition renderer for the Islet pill.
//!
//! This crate is deliberately unsafe-heavy and Windows-only. The platform-free
//! state lives in `islet-core`; the render thread calls [`Renderer::render`]
//! with monotonic seconds and receives a hit region it can publish to the UI
//! thread.

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};

use islet_core::geometry::{point_in_rounded_rect, Rect};
use islet_core::handoff::Handoff;
use islet_core::island::{
    arrange_at, AgentCard, AgentState, AlertLevel, ContentCard, ContentSnapshot, DictationCard,
    DictationPhase, LayoutTarget, MediaCard, NotificationCard, PillSize, COMPACT,
    DEFAULT_MEDIA_ACCENT,
};
use islet_core::media::{
    expanded, format_clock, ArtImage, BarLevels, BarSmoother, MediaHover, BAR_COUNT,
};
use islet_core::{Ease, Spring, SpringParams, Tween};
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_FILL_MODE_WINDING, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, D2D1CreateDevice, ID2D1Bitmap1, ID2D1BitmapBrush1, ID2D1Brush,
    ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory1, ID2D1Geometry, ID2D1Image,
    ID2D1Layer, ID2D1PathGeometry1, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_BRUSH_PROPERTIES1, D2D1_BITMAP_OPTIONS,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_PROPERTY_TYPE_FLOAT, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_DEMI_BOLD, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIDevice1, IDXGIFactory2, IDXGIOutput, IDXGISurface,
    IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows_numerics::{Matrix3x2, Vector2};

/// Pure black, as on iOS (PLAN Â§4.2).
const FRAME_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};
/// A thin white outline so the black pill stays visible on dark windows.
const HAIRLINE_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.17,
};
/// One line for a glance on the slim docked island.
fn glance_line(glance: &islet_core::island::GlanceCard) -> String {
    match glance.kind {
        islet_core::config::IdleGlance::Stats => format!(
            "CPU {}  \u{b7}  Memory {}",
            islet_core::widgets::format_percent(glance.cpu),
            islet_core::widgets::format_percent(glance.memory)
        ),
        _ => glance.time.clone(),
    }
}

/// The hot-CPU halo.
const HEAT_COLOR: [u8; 3] = [255, 118, 58];
/// Outline thickness in dips (one physical pixel at 100% scaling).
const HAIRLINE_WIDTH: f32 = 1.0;
/// Real visualizer levels older than this fall back to procedural bars.
const BAR_LEVELS_MAX_AGE_MS: u64 = 250;
/// Album-art brushes kept around: both players' tracks plus a transition
/// partner.
const ART_CACHE_SIZE: usize = 4;
/// Margin between the shrunk music pill's bars and its right end.
const SHRUNK_BARS_RIGHT: f32 = 14.0;
/// How far a bar grows above its resting height in the shrunk pill (15 in
/// the full one).
const SHRUNK_BAR_REACH: f32 = 12.0;
/// Black margin between the second player's round art and its bubble's edge,
/// the same at every size. It matches the compact pill's art margin, so the
/// two discs sit side by side at the same size.
const BUBBLE_ART_INSET: f32 = expanded::ART_INSET;

/// How long a hidden island keeps its D3D/D2D device before releasing it.
const GRAPHICS_RELEASE_DELAY: f64 = 15.0;

const TRANSPARENT: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RendererConfig {
    pub hwnd: HWND,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: f32,
    pub logical_width: f32,
    pub logical_height: f32,
    pub anchor_top: bool,
}

// SAFETY: HWND is an opaque window handle. The renderer owns all graphics
// objects on its thread; only the plain handle value crosses the boundary.
unsafe impl Send for RendererConfig {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitRect {
    pub rect: Rect,
    pub corner: f32,
}

impl HitRect {
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        point_in_rounded_rect(x, y, self.rect, self.corner)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HitRegion {
    pub main: Option<HitRect>,
    pub bubble: Option<HitRect>,
}

impl HitRegion {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.main.is_none() && self.bubble.is_none()
    }

    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.main.is_some_and(|rect| rect.contains(x, y))
            || self.bubble.is_some_and(|rect| rect.contains(x, y))
    }
}

#[derive(Debug)]
pub enum RenderError {
    Windows(windows::core::Error),
    Stage(&'static str, windows::core::Error),
    InvalidSize,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Windows(error) => write!(f, "graphics error: {error}"),
            Self::Stage(stage, error) => write!(f, "graphics error during {stage}: {error}"),
            Self::InvalidSize => write!(f, "graphics target has zero size"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Windows(error) | Self::Stage(_, error) => Some(error),
            Self::InvalidSize => None,
        }
    }
}

fn stage<T>(name: &'static str, result: windows::core::Result<T>) -> RenderResult<T> {
    result.map_err(|error| RenderError::Stage(name, error))
}

impl From<windows::core::Error> for RenderError {
    fn from(error: windows::core::Error) -> Self {
        Self::Windows(error)
    }
}

pub type RenderResult<T> = Result<T, RenderError>;

#[cfg(test)]
mod widget_tests;
mod widgets;
mod intro;
mod voice;
mod converter;

pub struct Renderer {
    pull_x: Spring,
    pull_y: Spring,
    widget_hover: Option<islet_core::widgets::WidgetHover>,
    handoff: Option<(Handoff, Sample)>,
    departure_reported: bool,
    hwnd: HWND,
    gfx: Option<Gfx>,
    animator: Animator,
    target: LayoutTarget,
    content: ContentSnapshot,
    previous_content: Option<ContentSnapshot>,
    transition_start: Option<f64>,
    hidden_since: Option<f64>,
    hit_region: Arc<Mutex<HitRegion>>,
    bars: Arc<BarLevels>,
    bar_smoother: BarSmoother,
    /// A layout received while the island was leaving its edge (see
    /// `set_snapshot`).
    deferred: Option<(LayoutTarget, ContentSnapshot)>,
    /// The last small size of the main pill: the music card's expansion grows
    /// from it and collapses back to it.
    media_rest: PillSize,
    /// What the last content-only frame showed, quantized to device pixels.
    /// A vblank whose picture would be identical is skipped entirely.
    presented: Option<u64>,
    media_hover: Option<MediaHover>,
    anchor_top: bool,
    logical_width: f32,
    logical_height: f32,
    pixel_width: u32,
    pixel_height: u32,
    dpi: f32,
    /// When the startup greeting began to show, while it is on screen.
    intro_since: Option<f64>,
}

impl Renderer {
    pub fn new(
        config: RendererConfig,
        hit_region: Arc<Mutex<HitRegion>>,
        bars: Arc<BarLevels>,
        now: f64,
    ) -> RenderResult<Self> {
        if config.pixel_width == 0 || config.pixel_height == 0 {
            return Err(RenderError::InvalidSize);
        }
        let anchor = if config.anchor_top { 0.0 } else { 1.0 };
        let animator = Animator::new(LayoutTarget::HIDDEN, anchor, now);
        Ok(Self {
            pull_x: Spring::new(0.0, 0.0, SpringParams::COLLAPSE, now),
            pull_y: Spring::new(0.0, 0.0, SpringParams::COLLAPSE, now),
            handoff: None,
            widget_hover: None,
            departure_reported: false,
            hwnd: config.hwnd,
            gfx: None,
            animator,
            target: LayoutTarget::HIDDEN,
            content: ContentSnapshot::default(),
            previous_content: None,
            transition_start: None,
            hidden_since: Some(now),
            hit_region,
            bars,
            bar_smoother: BarSmoother::default(),
            deferred: None,
            media_rest: COMPACT,
            presented: None,
            media_hover: None,
            anchor_top: config.anchor_top,
            logical_width: config.logical_width,
            logical_height: config.logical_height,
            pixel_width: config.pixel_width,
            pixel_height: config.pixel_height,
            dpi: config.dpi,
            intro_since: None,
        })
    }

    /// Applies a UI-thread snapshot. Safe to call with the same snapshot.
    ///
    /// While the island is still being drawn into the edge it is leaving, a
    /// new layout waits: the departing droplet keeps its old look, and the new
    /// one wells out of the other edge (applied at the hidden midpoint).
    pub fn set_snapshot(
        &mut self,
        target: LayoutTarget,
        content: ContentSnapshot,
        now: f64,
    ) -> bool {
        // Background media can update while a static widget is in front.
        // Identical visible snapshots need neither another draw nor a wakeup.
        if self.deferred.is_none() && target == self.target && content == self.content {
            return false;
        }
        if target.visible && self.animator.leaving(self.anchor_top, now) {
            self.deferred = Some((target, content));
            return true;
        }
        self.deferred = None;
        self.apply_snapshot(target, content, now, false);
        true
    }

    /// Applies the layout held back during a trip between edges. Inside the
    /// edge nobody sees the switch, so it is instant; if the trip turned back
    /// before getting there, it is the usual morph instead.
    fn apply_deferred(&mut self, now: f64) {
        if self.deferred.is_none() || self.animator.leaving(self.anchor_top, now) {
            return;
        }
        if let Some((target, content)) = self.deferred.take() {
            let hidden = self.animator.edge_fullness(now) < PRESENCE_UNSEEN;
            self.apply_snapshot(target, content, now, hidden);
        }
    }

    fn apply_snapshot(
        &mut self,
        target: LayoutTarget,
        content: ContentSnapshot,
        now: f64,
        instant: bool,
    ) {
        if instant {
            self.animator.jump_to(target, now);
            self.target = target;
            self.content = content;
            self.previous_content = None;
            self.transition_start = None;
        } else {
            self.morph_to(target, content, now);
        }
        if self.target.visible || !self.content.is_empty() {
            self.hidden_since = None;
        } else if self.hidden_since.is_none() {
            self.hidden_since = Some(now);
        }
    }

    /// The usual change while visible: springs to the new size, and a short
    /// cross-fade when the content itself changes.
    fn morph_to(&mut self, target: LayoutTarget, content: ContentSnapshot, now: f64) {
        if target != self.target {
            self.animator.set_target(target, now);
            self.target = target;
        }
        if content != self.content {
            if self.content.updates_in_place(&content) {
                // Timeline ticks, levels, play/pause: no morph.
                self.content = content;
            } else {
                let previous = std::mem::take(&mut self.content);
                if !previous.is_empty() || !content.is_empty() {
                    self.previous_content = Some(previous);
                    self.transition_start = Some(now);
                }
                self.content = content;
            }
        }
    }

    /// A fast tracking spring smooths pointer samples; release softly settles home.
    pub fn set_pull(&mut self, x: f32, y: f32, held: bool, now: f64) {
        for (spring, value) in [(&mut self.pull_x, x), (&mut self.pull_y, y)] {
            let params = if held {
                SpringParams::new(0.09, 1.0)
            } else {
                SpringParams::new(0.32, 0.82)
            };
            spring.set_params(params, now);
            spring.retarget(if held { value as f64 } else { 0.0 }, now);
        }
    }

    pub fn set_anchor(&mut self, anchor_top: bool, now: f64) {
        if self.anchor_top == anchor_top {
            return;
        }
        self.anchor_top = anchor_top;
        self.animator.set_anchor(anchor_top, now);
    }

    pub fn resize(&mut self, pixel_width: u32, pixel_height: u32, dpi: f32) -> RenderResult<()> {
        if pixel_width == 0 || pixel_height == 0 {
            return Err(RenderError::InvalidSize);
        }
        if pixel_width == self.pixel_width
            && pixel_height == self.pixel_height
            && (dpi - self.dpi).abs() < f32::EPSILON
        {
            return Ok(());
        }
        if let Some(gfx) = &mut self.gfx {
            // SAFETY: the renderer owns all graphics state and is only used from
            // the render thread that created it.
            unsafe { gfx.resize(pixel_width, pixel_height, dpi)? };
        }
        self.pixel_width = pixel_width;
        self.pixel_height = pixel_height;
        self.dpi = dpi;
        Ok(())
    }

    pub fn set_logical_size(&mut self, width: f32, height: f32) {
        self.logical_width = width;
        self.logical_height = height;
    }

    /// Hover is a single repaint, including while playback is paused.
    pub fn set_media_hover(&mut self, hover: Option<MediaHover>) {
        self.media_hover = hover;
    }

    pub fn set_widget_hover(&mut self, hover: Option<islet_core::widgets::WidgetHover>) {
        self.widget_hover = hover;
        self.presented = None;
    }

    pub fn depart_surface(&mut self, now: f64, animate: bool) {
        self.handoff = Some((Handoff::new(false, now, animate), self.animator.sample(now)));
        self.departure_reported = false;
        self.media_hover = None;
    }

    pub fn arrive_surface(&mut self, now: f64, animate: bool) {
        let anchor = if self.anchor_top { 0.0 } else { 1.0 };
        self.animator = Animator::new(self.target, anchor, now);
        self.previous_content = None;
        self.transition_start = None;
        self.handoff = Some((Handoff::new(true, now, animate), self.animator.sample(now)));
    }

    /// Called only after the final invisible frame was presented.
    pub fn take_departure_completed(&mut self, now: f64) -> bool {
        let completed = self
            .handoff
            .is_some_and(|(transfer, _)| !transfer.arriving && transfer.settled(now));
        if completed && !self.departure_reported {
            self.departure_reported = true;
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn needs_frame(&self, now: f64) -> bool {
        if let Some((transfer, _)) = self.handoff {
            if !transfer.arriving {
                return !self.departure_reported || !transfer.settled(now);
            }
            if !transfer.settled(now) {
                return true;
            }
        }
        !self.pull_x.is_settled(now) || !self.pull_y.is_settled(now) || !self.animator.is_settled(now)
            || self.transition_progress(now).is_some()
            || self.content.animates()
            || self
                .previous_content
                .as_ref()
                .is_some_and(ContentSnapshot::animates)
    }

    /// True while geometry springs or a content transition are in flight.
    /// These need every vblank; content-only animation can be throttled.
    #[must_use]
    pub fn is_morphing(&self, now: f64) -> bool {
        self.handoff
            .is_some_and(|(transfer, _)| !transfer.settled(now))
            || !self.pull_x.is_settled(now)
            || !self.pull_y.is_settled(now)
            || !self.animator.is_settled(now)
            || self.transition_progress(now).is_some()
    }

    #[must_use]
    pub fn is_settled(&self, now: f64) -> bool {
        !self.needs_frame(now)
    }

    /// Draws and presents a frame. `force` is set when state changed since the
    /// last call; otherwise a content-only frame that would look identical to
    /// the one on screen is skipped. Returns whether a frame was presented.
    pub fn render(&mut self, now: f64, force: bool) -> RenderResult<bool> {
        if self.gfx.is_none()
            && !self.target.visible
            && self.content.is_empty()
            && self.previous_content.is_none()
            && self.animator.is_settled(now)
        {
            self.clear_hit_region();
            return Ok(false);
        }
        if force || self.gfx.is_none() || self.is_morphing(now) {
            self.presented = None;
        }
        let mut now = now;
        if self.gfx.is_none() {
            let started = std::time::Instant::now();
            let config = RendererConfig {
                hwnd: self.hwnd,
                pixel_width: self.pixel_width,
                pixel_height: self.pixel_height,
                dpi: self.dpi,
                logical_width: self.logical_width,
                logical_height: self.logical_height,
                anchor_top: self.anchor_top,
            };
            // SAFETY: the renderer owns all graphics state and is only used from
            // the render thread that created it.
            self.gfx = Some(unsafe { Gfx::new(&config)? });
            // Device creation can take longer than the whole expand
            // animation. Graphics only exist again once the island was
            // hidden and settled, so restart the motion from hidden at the
            // post-creation time instead of skipping its first frames.
            now += started.elapsed().as_secs_f64();
            let anchor = if self.anchor_top { 0.0 } else { 1.0 };
            self.animator = Animator::new(LayoutTarget::HIDDEN, anchor, now);
            self.animator.set_target(self.target, now);
            if self.transition_start.is_some() {
                self.transition_start = Some(now);
            }
        }

        self.apply_deferred(now);
        if self.target.visible && self.target.main.height < 60.0 {
            self.media_rest = self.target.main;
        }
        let mut sample = self.animator.sample(now);
        if let Some((transfer, frozen)) = self.handoff {
            if !transfer.arriving {
                sample = frozen;
            }
            let top = frozen.anchor <= 0.5;
            let fullness = transfer.fullness(now);
            sample.anchor = if top {
                (1.0 - fullness) * 0.5
            } else {
                0.5 + fullness * 0.5
            };
            if transfer.arriving && transfer.settled(now) {
                self.handoff = None;
            }
        }
        let progress = self.transition_progress(now).unwrap_or(1.0);
        let previous = if progress < 1.0 {
            self.previous_content.as_ref()
        } else {
            None
        };

        let base_geometry = self.frame_geometry(&sample);
        let x = self.pull_x.sample(now).0 as f32;
        let y = self.pull_y.sample(now).0 as f32;
        let (geometry, content_geometry) = pull_frames(base_geometry, x, y);

        let levels = self.bar_smoother.sample(
            self.bars
                .load_fresh((now * 1000.0) as u64, BAR_LEVELS_MAX_AGE_MS),
            now,
        );
        // Settled shape, no transition: only bars, seekbar and clock can move.
        // Most vblanks change none of them by a visible step (the expanded
        // seekbar crawls about a pixel a second), so those frames are skipped.
        let key = if self.handoff.is_none() && previous.is_none() && !self.is_morphing(now) {
            self.content_key(now, &geometry, levels)
        } else {
            None
        };
        if key.is_some() && key == self.presented {
            return Ok(false);
        }
        self.presented = key;
        // The greeting's clock starts with its first frame, after the device
        // exists, so a slow start never skips its opening beats.
        let greeting = |content: &ContentSnapshot| matches!(content.primary, Some(ContentCard::Intro));
        let intro_t = if greeting(&self.content) || previous.is_some_and(greeting) {
            (now - *self.intro_since.get_or_insert(now)) as f32
        } else {
            self.intro_since = None;
            0.0
        };
        // SAFETY: the renderer owns all graphics state and is only used from
        // the render thread that created it.
        unsafe {
            let gfx = self.gfx.as_mut().ok_or(RenderError::InvalidSize)?;
            gfx.intro_t = intro_t;
            if self.intro_since.is_none() {
                gfx.intro_sheen = None;
            }
            gfx.media_hover = self.media_hover;
            gfx.media_rest = self.media_rest;
            gfx.main_spring_height = sample.main_height;
            gfx.bar_levels = levels;
            gfx.content_scale = sample.content_scale;
            gfx.widget_hover = self.widget_hover;
            gfx.draw(
                now,
                &geometry,
                &content_geometry,
                self.target,
                &self.content,
                previous,
                progress,
            )?;
            gfx.present()?;
        }
        self.publish_hit(&geometry);

        if progress >= 1.0 {
            self.previous_content = None;
            self.transition_start = None;
        }
        Ok(true)
    }

    /// Hash of everything time-dependent in a settled frame, at half-device-
    /// pixel resolution. `None` when the content has motion this does not
    /// model (agent spinners, dictation waves); those frames always draw.
    fn content_key(
        &self,
        now: f64,
        geometry: &FrameGeometry,
        levels: Option<[f32; BAR_COUNT]>,
    ) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let steps_per_px = f64::from(self.dpi / 96.0) * 2.0;
        let quantize = |value: f64, span: f64| (value * span * steps_per_px).round() as i64;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for value in [
            geometry.main.x,
            geometry.main.y,
            geometry.main.width,
            geometry.main.height,
            geometry.content_alpha,
            geometry.shell_alpha,
        ] {
            value.to_bits().hash(&mut hasher);
        }
        self.media_hover.map(|hover| hover as u8).hash(&mut hasher);
        self.content.cpu_hot.hash(&mut hasher);
        let slots = [
            (Some(geometry.main), self.content.primary.as_ref()),
            (geometry.bubble, self.content.secondary.as_ref()),
        ];
        for (rect, card) in slots {
            let (Some(rect), Some(card)) = (rect, card) else {
                continue;
            };
            match card {
                ContentCard::Notification(_) | ContentCard::Calculator(_) => {}
                ContentCard::Converter(card) => {
                    (&card.title, &card.subtitle, &card.message, card.progress, card.fully_succeeded, card.dragging).hash(&mut hasher);
                    format!("{:?}{:?}{:?}{:?}", card.stage, card.options, card.hover, card.keyboard_focus).hash(&mut hasher);
                    card.preview.as_ref().map(|art| art.id).hash(&mut hasher);
                },
                ContentCard::Stats(stats) => format!("{stats:?}").hash(&mut hasher),
                ContentCard::Glance(glance) => format!("{glance:?}").hash(&mut hasher),
                ContentCard::Agent(_) | ContentCard::Dictation(_) | ContentCard::Intro => return None,
                ContentCard::Media(media) => {
                    // Mirrors `Gfx::draw_card`'s layout choice.
                    if rect.width < 60.0 && rect.height < 60.0 {
                        continue;
                    }
                    if rect.height > 60.0 {
                        if let Some(timeline) = media.timeline.filter(|t| t.duration > 0.0) {
                            let progress = f64::from(
                                media
                                    .scrub
                                    .unwrap_or_else(|| timeline.progress_at(now, media.playing))
                                    .clamp(0.0, 1.0),
                            );
                            let bar = expanded::scrubber(rect.width);
                            quantize(progress, f64::from(bar.width)).hash(&mut hasher);
                            let position = progress * timeline.duration;
                            (position.max(0.0) as u64).hash(&mut hasher);
                            ((timeline.duration - position).max(0.0) as u64).hash(&mut hasher);
                        }
                    } else {
                        // The tallest bars span 15 logical pixels.
                        for index in 0..BAR_COUNT {
                            let level = bar_level(levels, now, index, media.playing);
                            quantize(f64::from(level), 15.0).hash(&mut hasher);
                        }
                    }
                }
            }
        }
        Some(hasher.finish())
    }

    /// Seconds until the device should be released, if the island is hidden
    /// and fully settled. The delay avoids rebuilding the device (100+ ms)
    /// when short alerts come and go.
    #[must_use]
    pub fn graphics_release_in(&self, now: f64) -> Option<f64> {
        let hidden_since = self.hidden_since?;
        (self.gfx.is_some()
            && self.previous_content.is_none()
            && self.transition_progress(now).is_none()
            && self.animator.is_settled(now))
        .then(|| (hidden_since + GRAPHICS_RELEASE_DELAY - now).max(0.0))
    }

    pub fn release_graphics(&mut self) {
        self.gfx = None;
        self.presented = None;
        self.clear_hit_region();
    }

    fn clear_hit_region(&self) {
        if let Ok(mut hit) = self.hit_region.lock() {
            *hit = HitRegion::default();
        }
    }

    fn transition_progress(&self, now: f64) -> Option<f32> {
        let start = self.transition_start?;
        let progress = ((now - start) / 0.34).clamp(0.0, 1.0) as f32;
        (progress < 1.0).then_some(progress)
    }

    fn frame_geometry(&self, sample: &Sample) -> FrameGeometry {
        droplet_geometry(sample, self.logical_width, self.logical_height)
    }

    fn publish_hit(&self, geometry: &FrameGeometry) {
        let scale = self.dpi / 96.0;
        let main = (geometry.main.width >= 0.5 && geometry.main.height >= 0.5).then(|| HitRect {
            rect: Rect::new(
                geometry.main.x * scale,
                geometry.main.y * scale,
                geometry.main.width * scale,
                geometry.main.height * scale,
            ),
            corner: geometry.main_corner * scale,
        });
        let bubble = geometry.bubble.map(|rect| HitRect {
            rect: Rect::new(
                rect.x * scale,
                rect.y * scale,
                rect.width * scale,
                rect.height * scale,
            ),
            corner: geometry.bubble_corner * scale,
        });
        let mut hit = self
            .hit_region
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *hit = HitRegion { main, bubble };
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    content_scale: f32,
    main_width: f32,
    main_height: f32,
    main_corner: f32,
    bubble_width: f32,
    bubble_height: f32,
    bubble_corner: f32,
    gap: f32,
    inset: f32,
    /// 0 = top edge, 1 = bottom edge. Animated when the island docks.
    anchor: f32,
    /// 0 = drawn into the screen edge (hidden), 1 = the resting pill. Above 1
    /// while an appearing pill overshoots, which swells it slightly.
    presence: f32,
}

#[derive(Clone, Copy, Debug)]
enum TransportIcon {
    Previous,
    PlayPause(bool),
    Next,
}

struct Animator {
    content_scale: Spring,
    main_width: Spring,
    main_height: Spring,
    main_corner: Spring,
    bubble_width: Spring,
    bubble_height: Spring,
    bubble_corner: Spring,
    gap: Spring,
    inset: Spring,
    /// Edge phase: 0 at the top, 1 at the bottom (see `droplet_geometry`).
    anchor: Tween,
    /// 0 drawn into the edge (hidden), 1 the resting pill.
    presence: Tween,
}

/// Below this the droplet is invisible, so the pill's size can jump to a new
/// one without anyone seeing it.
const PRESENCE_UNSEEN: f64 = 0.04;
/// Top edge to bottom edge: drawn into one edge and welling out of the other.
/// Each half is as long as a show, so the trip reads as hide-then-show.
const TRAVEL_SECONDS: f64 = 0.84;
/// Welling out of the edge into the resting pill.
const EMERGE_SECONDS: f64 = 0.56;
/// Drawn back into the edge.
const RECEDE_SECONDS: f64 = 0.36;

/// A partial move (an interrupted show, a reversed trip) takes less time, but
/// not proportionally less, so short corrections stay smooth.
fn tween_duration(full: f64, distance: f64) -> f64 {
    full * distance.abs().clamp(0.0, 1.0).sqrt().max(0.35)
}

impl Animator {
    fn new(target: LayoutTarget, anchor: f64, now: f64) -> Self {
        let mut animator = Self {
            content_scale: Spring::new(1.0, 1.0, SpringParams::EXPAND, now),
            main_width: Spring::new(0.0, 0.0, SpringParams::EXPAND, now),
            main_height: Spring::new(0.0, 0.0, SpringParams::EXPAND, now),
            main_corner: Spring::new(0.0, 0.0, SpringParams::EXPAND, now),
            bubble_width: Spring::new(0.0, 0.0, SpringParams::EXPAND, now),
            bubble_height: Spring::new(0.0, 0.0, SpringParams::EXPAND, now),
            bubble_corner: Spring::new(0.0, 0.0, SpringParams::EXPAND, now),
            gap: Spring::new(0.0, 0.0, SpringParams::EXPAND, now),
            // Starts at rest at its target so the first show does not slide.
            inset: Spring::new(
                f64::from(target.inset),
                f64::from(target.inset),
                SpringParams::EXPAND,
                now,
            ),
            // Also starts at rest: a fresh window should already be at the
            // configured edge, not fly there from the other one.
            anchor: Tween::new(anchor, now),
            presence: Tween::new(0.0, now),
        };
        animator.set_target(target, now);
        animator
    }

    fn set_anchor(&mut self, anchor_top: bool, now: f64) {
        let target = if anchor_top { 0.0 } else { 1.0 };
        if (self.anchor.target() - target).abs() < f64::EPSILON {
            return;
        }
        let distance = target - self.anchor.position(now);
        self.anchor.retarget(
            target,
            tween_duration(TRAVEL_SECONDS, distance),
            Ease::TRAVEL,
            now,
        );
    }

    fn set_presence(&mut self, target: f64, now: f64) {
        if (self.presence.target() - target).abs() < f64::EPSILON {
            return;
        }
        let (full, ease) = if target > 0.5 {
            (EMERGE_SECONDS, Ease::EMERGE)
        } else {
            (RECEDE_SECONDS, Ease::RECEDE)
        };
        let distance = target - self.presence.position(now);
        self.presence
            .retarget(target, tween_duration(full, distance), ease, now);
    }

    /// Showing and hiding are one motion on `presence`: the pill keeps its
    /// shape and is drawn into (or wells out of) the screen edge as a
    /// droplet. Size changes while visible are the usual spring morph.
    fn set_target(&mut self, target: LayoutTarget, now: f64) {
        retarget(
            &mut self.content_scale,
            f64::from(target.content_scale),
            SpringParams::EXPAND,
            now,
        );
        if !target.visible {
            // Keep the last shape so the hide is the show played backwards.
            self.set_presence(0.0, now);
            return;
        }
        if self.presence.position(now) < PRESENCE_UNSEEN {
            // Coming back from hidden: start at the new shape, so the only
            // motion is the droplet growing out of the edge.
            self.jump_to(target, now);
        }
        self.set_presence(1.0, now);

        let old_area = self.main_width.target() * self.main_height.target();
        let new_area = f64::from(target.main.width) * f64::from(target.main.height);
        let params = if new_area >= old_area {
            SpringParams::EXPAND
        } else {
            SpringParams::COLLAPSE
        };
        let (width_params, height_params) = (params, params);

        retarget(
            &mut self.main_width,
            f64::from(target.main.width),
            width_params,
            now,
        );
        retarget(
            &mut self.main_height,
            f64::from(target.main.height),
            height_params,
            now,
        );
        retarget(
            &mut self.main_corner,
            f64::from(target.main.corner),
            height_params,
            now,
        );

        let bubble = target.bubble.unwrap_or(PillSize::new(0.0, 0.0, 0.0));
        retarget(
            &mut self.bubble_width,
            f64::from(bubble.width),
            width_params,
            now,
        );
        retarget(
            &mut self.bubble_height,
            f64::from(bubble.height),
            height_params,
            now,
        );
        retarget(
            &mut self.bubble_corner,
            f64::from(bubble.corner),
            height_params,
            now,
        );
        retarget(&mut self.gap, f64::from(target.gap), params, now);
        retarget(
            &mut self.inset,
            f64::from(target.inset),
            SpringParams::COLLAPSE,
            now,
        );
    }

    /// Still on the way into the edge it is leaving, toward the edge
    /// `anchor_top` names.
    fn leaving(&self, anchor_top: bool, now: f64) -> bool {
        if self.anchor.is_settled(now) {
            return false;
        }
        let phase = self.anchor.position(now);
        if anchor_top {
            phase > 0.5
        } else {
            phase < 0.5
        }
    }

    /// How far out of its current edge the travelling island is: 1 resting,
    /// 0 fully inside the edge (the trip's midpoint).
    fn edge_fullness(&self, now: f64) -> f64 {
        let phase = self.anchor.position(now).clamp(0.0, 1.0);
        if phase <= 0.5 {
            1.0 - phase / 0.5
        } else {
            (phase - 0.5) / 0.5
        }
    }

    fn jump_to(&mut self, target: LayoutTarget, now: f64) {
        let bubble = target.bubble.unwrap_or(PillSize::new(0.0, 0.0, 0.0));
        for (spring, value) in [
            (&mut self.content_scale, target.content_scale),
            (&mut self.main_width, target.main.width),
            (&mut self.main_height, target.main.height),
            (&mut self.main_corner, target.main.corner),
            (&mut self.bubble_width, bubble.width),
            (&mut self.bubble_height, bubble.height),
            (&mut self.bubble_corner, bubble.corner),
            (&mut self.gap, target.gap),
            (&mut self.inset, target.inset),
        ] {
            spring.reset(f64::from(value), f64::from(value), now);
        }
    }

    fn sample(&self, now: f64) -> Sample {
        Sample {
            content_scale: self.content_scale.position(now).clamp(0.7, 1.4) as f32,
            main_width: self.main_width.position(now).max(0.0) as f32,
            main_height: self.main_height.position(now).max(0.0) as f32,
            main_corner: self.main_corner.position(now).max(0.0) as f32,
            bubble_width: self.bubble_width.position(now).max(0.0) as f32,
            bubble_height: self.bubble_height.position(now).max(0.0) as f32,
            bubble_corner: self.bubble_corner.position(now).max(0.0) as f32,
            gap: self.gap.position(now).max(0.0) as f32,
            inset: self.inset.position(now).max(0.0) as f32,
            anchor: self.anchor.position(now).clamp(0.0, 1.0) as f32,
            presence: self.presence.position(now).clamp(0.0, 1.0) as f32,
        }
    }

    fn is_settled(&self, now: f64) -> bool {
        self.content_scale.is_settled_with(now, 0.001, 0.01)
            && self.main_width.is_settled(now)
            && self.main_height.is_settled(now)
            && self.main_corner.is_settled(now)
            && self.bubble_width.is_settled(now)
            && self.bubble_height.is_settled(now)
            && self.bubble_corner.is_settled(now)
            && self.gap.is_settled(now)
            && self.inset.is_settled(now)
            && self.presence.is_settled(now)
            && self.anchor.is_settled(now)
    }
}

fn retarget(spring: &mut Spring, target: f64, params: SpringParams, now: f64) {
    spring.set_params(params, now);
    spring.retarget(target, now);
}

unsafe fn set_blur_deviation(effect: &ID2D1Effect, deviation: f32) -> RenderResult<()> {
    let value = deviation.clamp(0.0, 8.0).to_le_bytes();
    effect.SetValue(
        D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
        D2D1_PROPERTY_TYPE_FLOAT,
        &value,
    )?;
    Ok(())
}

/// Bar height 0..1: real loopback levels when fresh, otherwise a gentle
/// procedural wave while playing, and a low rest when paused.
fn bar_level(levels: Option<[f32; BAR_COUNT]>, now: f64, index: usize, playing: bool) -> f32 {
    if !playing {
        return 0.12;
    }
    if let Some(levels) = levels {
        return levels[index % BAR_COUNT].clamp(0.0, 1.0);
    }
    let phase = now * 7.2 + index as f64 * 0.85;
    (phase.sin() * 0.5 + 0.5) as f32 * 0.65 + 0.35
}

/// One part of a card opening into its big look (or closing back), drawn in
/// separate passes so each can fade and blur on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MorphPart {
    /// The small look, fading and blurring out as the pill grows.
    Small,
    /// The big look, fading and sharpening in once there is room for it.
    Big,
    /// The album art, drawn sharp throughout and moving between the two.
    Art,
}

/// A card between its small and big looks.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Morph {
    /// 0 at the small look's height, 1 at the big look's.
    open: f32,
    small: PillSize,
    big: PillSize,
}

/// The small and big looks of a card, and how far open it is at `height`
/// (from the size spring). `None` at either end, where the card is simply
/// drawn in that look, and for cards with only one look.
fn morph_of(
    card: &ContentCard,
    height: f32,
    media_rest: PillSize,
    content_scale: f32,
) -> Option<Morph> {
    // The slot draws in unscaled content coordinates. Compare its spring
    // height in that same space so a smaller expanded card can fully open.
    let height = height / content_scale;
    let (small, big) = match card {
        ContentCard::Converter(_) => (COMPACT, PillSize::new(islet_core::converter::WIDTH, islet_core::converter::HEIGHT, 38.0)),
        ContentCard::Calculator(_) => (COMPACT, islet_core::island::CALCULATOR),
        ContentCard::Stats(_) => (COMPACT, islet_core::island::STATS),
        ContentCard::Media(_) => (media_rest, islet_core::island::MEDIA),
        ContentCard::Notification(_) | ContentCard::Agent(_) => {
            (COMPACT, islet_core::island::NOTIFICATION)
        }
        ContentCard::Dictation(_) | ContentCard::Glance(_) | ContentCard::Intro => return None,
    };
    if big.height <= small.height + 1.0 {
        return None;
    }
    let open = ((height - small.height) / (big.height - small.height)).clamp(0.0, 1.0);
    (open > 0.002 && open < 0.998).then_some(Morph { open, small, big })
}

/// Opacity of the small and big looks: the small one is gone a third of the
/// way open, the big one arrives over the last half. In between only the
/// shape (and a music card's art) moves, so the two never muddle together.
fn morph_fades(open: f32) -> (f32, f32) {
    (
        1.0 - smoothstep(0.0, 0.35, open),
        smoothstep(0.5, 0.95, open),
    )
}

/// Where the compact music pill's art sits (and its corner radius) at a
/// given pill size: a disc filling the left round end, `ART_INSET` from the
/// end, the top and the bottom, like the art in every other pill.
fn compact_art(pill: PillSize) -> (Rect, f32) {
    let inset = expanded::ART_INSET;
    let art = (pill.height - inset * 2.0).max(0.0);
    (Rect::new(inset, inset, art, art), art * 0.5)
}

/// Shell elasticity and content layout are independent: artwork, text and bars
/// translate as one intact group, fading gently as the shell stretches around them.
fn pull_frames(base: FrameGeometry, x: f32, y: f32) -> (FrameGeometry, FrameGeometry) {
    let mut shell = base;
    shell.main = pulled_rect(base.main, x, y);
    let mut content = base;
    content.main.x += shell.main.x + shell.main.width * 0.5
        - (base.main.x + base.main.width * 0.5);
    content.main.y += shell.main.y + shell.main.height * 0.5
        - (base.main.y + base.main.height * 0.5);
    content.content_alpha *= 1.0 - 0.82 * smoothstep(3.0, 90.0, x.hypot(y));
    if let Some(bubble) = &mut shell.bubble {
        bubble.x += x * 0.55;
        bubble.y += y * 0.55;
    }
    content.bubble = shell.bubble;
    (shell, content)
}

/// The far edge follows the hand more than the trailing edge. Rounded magnitude
/// avoids a cusp when crossing the origin; bounded growth keeps compact content usable.
fn pulled_rect(mut rect: Rect, x: f32, y: f32) -> Rect {
    let width = ((x * x + 4.0).sqrt() - 2.0) * 0.65;
    let height = ((y * y + 4.0).sqrt() - 2.0) * 0.40;
    let width = width.min(76.0);
    let height = height.min(44.0);
    rect.x += x * 0.55 - if x < 0.0 { width } else { 0.0 };
    rect.y += y * 0.55 - if y < 0.0 { height } else { 0.0 };
    rect.width += width;
    rect.height += height;
    rect
}

#[cfg(test)]
mod pull_tests {
    use super::*;
    #[test]
    fn pull_never_resizes_content_or_switches_a_compact_card_to_expanded() {
        let base = droplet_geometry(&Sample {
            main_width: 220.0, main_height: 36.0, main_corner: 18.0,
            inset: 8.0, content_scale: 1.0, presence: 1.0, ..Sample::default()
        }, 720.0, 1080.0);
        for (x, y) in [(60.0, 0.0), (-60.0, 0.0), (0.0, 100.0), (0.0, -100.0)] {
            let (shell, content) = pull_frames(base, x, y);
            assert_eq!(content.main.width, base.main.width);
            assert_eq!(content.main.height, base.main.height);
            assert!(content.content_alpha < base.content_alpha);
            assert!(content.content_alpha > 0.0);
            assert!(shell.main.width >= content.main.width);
            assert!(shell.main.height >= content.main.height);
        }
        let (_, rest) = pull_frames(base, 0.0, 0.0);
        assert_eq!(rest.main, base.main);
        assert_eq!(rest.content_alpha, base.content_alpha);
    }
    #[test]
    fn stretching_starts_immediately_and_tracks_the_pulled_axis() {
        let rest = Rect::new(100.0, 100.0, 108.0, 32.0);
        assert_eq!(pulled_rect(rest, 0.0, 0.0), rest);
        assert!(pulled_rect(rest, 1.0, 0.0).width > rest.width);
        let right = pulled_rect(rest, 60.0, 0.0);
        let left = pulled_rect(rest, -60.0, 0.0);
        assert_eq!(right.width, left.width);
        assert_eq!(right.height, rest.height);
        assert!((right.x - rest.x - (rest.x + rest.width - left.x - left.width)).abs() < 0.001);
        let down = pulled_rect(rest, 0.0, 60.0);
        assert!(down.height > rest.height + 20.0);
        assert_eq!(down.width, rest.width);
        assert!(pulled_rect(rest, 1000.0, 1000.0).height <= rest.height + 44.0);
    }
}

#[derive(Clone, Copy, Debug)]
struct FrameGeometry {
    main: Rect,
    main_corner: f32,
    bubble: Option<Rect>,
    bubble_corner: f32,
    /// 0 for the plain squircle, 1 for a leaf whose inner end has drawn to a
    /// point. Both pills share it: the whole group narrows into one shape.
    taper: f32,
    /// Which window edge the leaf's round end sits against (true = the top).
    taper_top: bool,
    /// Shell and content opacity. Both are 1 at rest; a traveling island
    /// dissolves its content first and its shell last.
    shell_alpha: f32,
    content_alpha: f32,
}

/// The island's flight between the two edges.
///
/// The anchor is the phase, not a position: from `0` to `0.5` the pill is
/// drawn at the top edge, being absorbed as the anchor moves away from that
/// edge; from `0.5` to `1` it grows back out of the bottom edge. Exactly at
/// `0.5` both are invisible, so the handoff cannot be seen - and running the
/// anchor backwards plays the whole thing mirrored, which is what makes an
/// interrupted docking reverse smoothly.
///
/// `fullness` is 1 for the resting pill at an edge and 0 for a pill that has
/// completely slipped through it.
fn droplet_geometry(sample: &Sample, window_width: f32, window_height: f32) -> FrameGeometry {
    let anchor = sample.anchor.clamp(0.0, 1.0);
    let (top, travel) = if anchor <= 0.5 {
        (true, 1.0 - anchor / 0.5)
    } else {
        (false, (anchor - 0.5) / 0.5)
    };
    // Appearing and travelling are the same droplet: both scale how far the
    // pill has come out of its edge.
    let fullness = travel * sample.presence.clamp(0.0, 1.0);
    let frame = arrange_at(
        PillSize::new(sample.main_width, sample.main_height, sample.main_corner),
        Some(PillSize::new(
            sample.bubble_width,
            sample.bubble_height,
            sample.bubble_corner,
        )),
        sample.gap,
        sample.inset,
        window_width,
        window_height,
        if top { 0.0 } else { 1.0 },
    );
    if fullness >= 1.0 {
        return FrameGeometry {
            main: frame.main,
            main_corner: frame.main_corner,
            bubble: frame.bubble,
            bubble_corner: frame.bubble_corner,
            taper: 0.0,
            taper_top: top,
            shell_alpha: 1.0,
            content_alpha: 1.0,
        };
    }
    let (height, width, taper) = droplet_shape(fullness);
    // At rest the island sits on its inset. As soon as it starts to gather it
    // presses onto the screen edge, and it only lifts back onto the inset at
    // the very end of the re-form, so the whole flight is flush with the
    // surface it passes through.
    let gap_scale = smoothstep(0.70, 1.0, fullness);
    let main = squeeze(
        frame.main,
        height,
        width,
        top,
        window_width,
        window_height,
        gap_scale,
    );
    let bubble = frame.bubble.map(|bubble| {
        squeeze(
            bubble,
            height,
            width,
            top,
            window_width,
            window_height,
            gap_scale,
        )
    });
    FrameGeometry {
        main,
        main_corner: frame.main_corner,
        bubble,
        bubble_corner: frame.bubble_corner,
        taper,
        taper_top: top,
        shell_alpha: smoothstep(0.05, 0.45, fullness),
        content_alpha: smoothstep(0.50, 0.92, fullness),
    }
}

/// Height, width and leaf taper of the traveling pill, as fractions of its
/// resting size. It first gathers into a leaf (mostly losing width), then is
/// pressed flat into the edge, so it slips through as a droplet rather than a
/// shrunken pill.
fn droplet_shape(fullness: f32) -> (f32, f32, f32) {
    let absorbed = 1.0 - fullness.clamp(0.0, 1.0);
    // The gather spans most of the first half, so the width draws in over
    // many frames instead of snapping into a leaf.
    let gather = smoothstep(0.0, 0.62, absorbed);
    let press = smoothstep(0.30, 1.0, absorbed);
    let width = (1.0 - 0.66 * gather) * (1.0 - 0.46 * press);
    let height = (1.0 - 0.22 * gather) * (1.0 - press);
    (height, width, 0.97 * gather)
}

/// Squeezes a frame toward `window_edge` (the top or bottom of the window), so
/// the pill's inner edge sweeps into that edge while its outer edge stays put.
/// `gap_scale` collapses the resting inset; it is 0 for the whole flight and
/// only returns to 1 as the pill settles back onto its inset.
fn squeeze(
    rect: Rect,
    height: f32,
    width: f32,
    top: bool,
    window_width: f32,
    window_height: f32,
    gap_scale: f32,
) -> Rect {
    let center = window_width * 0.5;
    let gap = if top {
        rect.y
    } else {
        window_height - rect.y - rect.height
    } * gap_scale;
    let y = if top {
        gap
    } else {
        window_height - rect.height * height - gap
    };
    Rect::new(
        center + (rect.x - center) * width,
        y,
        rect.width * width,
        rect.height * height,
    )
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

struct Gfx {
    widget_navigation: bool,
    widget_pages: u8,
    widget_page: u8,
    cpu_hot: bool,
    widget_hover: Option<islet_core::widgets::WidgetHover>,
    content_scale: f32,
    _device: ID3D11Device,
    _device_context: ID3D11DeviceContext,
    factory: ID2D1Factory1,
    _d2d_device: ID2D1Device,
    dc: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    swap_chain: IDXGISwapChain1,
    _bitmap: Option<ID2D1Bitmap1>,
    dwrite: IDWriteFactory,
    formats: TextFormats,
    text_cache: TextCache,
    geometries: GeometryCache,
    content_layers: Option<[ContentLayer; 2]>,
    mask_layer: Option<ID2D1Layer>,
    art: Vec<(u64, ID2D1BitmapBrush1)>,
    bar_levels: Option<[f32; BAR_COUNT]>,
    media_hover: Option<MediaHover>,
    /// The music pill's small size (compact, or shrunk while playing), which
    /// its expansion grows from. Set by the renderer each frame.
    media_rest: PillSize,
    /// The main pill's height from its size spring, before any droplet
    /// squeeze: how far a card has opened from its small look to its big one.
    main_spring_height: f32,
    /// Set while drawing one part of an opening or closing card.
    morph_part: Option<MorphPart>,
    /// The art is drawn on its own while morphing, so the looks skip it.
    skip_art: bool,
    /// Seconds into the startup greeting, set by the renderer each frame.
    intro_t: f32,
    intro_sheen: Option<windows::Win32::Graphics::Direct2D::ID2D1LinearGradientBrush>,
    intro_stroke: Option<ID2D1StrokeStyle>,
    /// The dictation bars' springs, carried from frame to frame.
    voice_bars: voice::VoiceBars,
    pixel_width: u32,
    pixel_height: u32,
    dpi: f32,
}

struct ContentLayer {
    bitmap: ID2D1Bitmap1,
    input: ID2D1Image,
    effect: ID2D1Effect,
    output: ID2D1Image,
}

impl ContentLayer {
    unsafe fn new(
        dc: &ID2D1DeviceContext,
        width: u32,
        height: u32,
        dpi: f32,
    ) -> RenderResult<Self> {
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap: ID2D1Bitmap1 = stage(
            "Create content bitmap",
            dc.CreateBitmap(
                D2D_SIZE_U {
                    width: width.max(1),
                    height: height.max(1),
                },
                None,
                0,
                &properties,
            ),
        )?;
        let effect: ID2D1Effect = stage(
            "Create Gaussian blur effect",
            dc.CreateEffect(&CLSID_D2D1GaussianBlur),
        )?;
        let image: ID2D1Image = stage("cast content bitmap", bitmap.cast())?;
        effect.SetInput(0, &image, true);
        let output: ID2D1Image = stage("Get blur output", effect.GetOutput())?;
        Ok(Self {
            bitmap,
            input: image,
            effect,
            output,
        })
    }
}

impl Gfx {
    unsafe fn new(config: &RendererConfig) -> RenderResult<Self> {
        let mut device = None;
        let mut device_context = None;
        stage(
            "D3D11CreateDevice",
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut device_context),
            ),
        )?;
        let device = device.ok_or(RenderError::InvalidSize)?;
        let device_context = device_context.ok_or(RenderError::InvalidSize)?;
        let dxgi_device: IDXGIDevice = stage("cast IDXGIDevice", device.cast())?;
        let low_latency: IDXGIDevice1 = stage("cast IDXGIDevice1", dxgi_device.cast())?;
        // Avoid buffering several refreshes of old motion on fast displays.
        stage(
            "SetMaximumFrameLatency",
            low_latency.SetMaximumFrameLatency(1),
        )?;
        let adapter: IDXGIAdapter = stage("GetAdapter", dxgi_device.GetAdapter())?;
        let dxgi_factory: IDXGIFactory2 = stage("GetParent IDXGIFactory2", adapter.GetParent())?;

        let d2d_device = stage("D2D1CreateDevice", D2D1CreateDevice(&dxgi_device, None))?;
        let factory = stage("GetFactory", d2d_device.GetFactory())?;
        let factory: ID2D1Factory1 = stage("cast ID2D1Factory1", factory.cast())?;
        let dc = stage(
            "CreateDeviceContext",
            d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE),
        )?;
        let dcomp: IDCompositionDevice = stage(
            "DCompositionCreateDevice",
            DCompositionCreateDevice(&dxgi_device),
        )?;
        let target = stage(
            "CreateTargetForHwnd",
            dcomp.CreateTargetForHwnd(config.hwnd, true),
        )?;
        let visual = stage("CreateVisual", dcomp.CreateVisual())?;

        let swap_chain = create_swap_chain(
            &dxgi_factory,
            &device,
            config.pixel_width,
            config.pixel_height,
        )?;
        stage("visual.SetContent", visual.SetContent(&swap_chain))?;
        stage("target.SetRoot", target.SetRoot(&visual))?;
        stage("dcomp.Commit", dcomp.Commit())?;

        dc.SetDpi(config.dpi, config.dpi);
        dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

        let bitmap = create_target_bitmap(&dc, &swap_chain, config.dpi)?;
        dc.SetTarget(&bitmap);
        let brush = stage(
            "CreateSolidColorBrush",
            dc.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                },
                None,
            ),
        )?;

        let dwrite: IDWriteFactory = stage(
            "DWriteCreateFactory",
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED),
        )?;
        let formats = TextFormats::new(&dwrite)?;

        Ok(Self {
            _device: device,
            content_scale: 1.0,
            widget_navigation: false,
            widget_pages: 0,
            widget_page: 0,
            cpu_hot: false,
            widget_hover: None,
            _device_context: device_context,
            factory,
            _d2d_device: d2d_device,
            dc,
            brush,
            _dcomp: dcomp,
            _target: target,
            _visual: visual,
            swap_chain,
            _bitmap: Some(bitmap),
            dwrite,
            formats,
            text_cache: TextCache::new(),
            geometries: GeometryCache::new(),
            content_layers: None,
            mask_layer: None,
            art: Vec::with_capacity(ART_CACHE_SIZE),
            bar_levels: None,
            media_hover: None,
            media_rest: COMPACT,
            main_spring_height: 0.0,
            morph_part: None,
            skip_art: false,
            intro_t: 0.0,
            intro_sheen: None,
            intro_stroke: None,
            voice_bars: voice::VoiceBars::default(),
            pixel_width: config.pixel_width,
            pixel_height: config.pixel_height,
            dpi: config.dpi,
        })
    }

    unsafe fn resize(&mut self, width: u32, height: u32, dpi: f32) -> RenderResult<()> {
        // ResizeBuffers fails while any reference to a back buffer remains.
        // Clear the D2D target and release our bitmap first.
        self.content_layers = None;
        self.mask_layer = None;
        self.dc.SetTarget(None::<&ID2D1Image>);
        self._bitmap = None;
        self.swap_chain.ResizeBuffers(
            2,
            width.max(1),
            height.max(1),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_SWAP_CHAIN_FLAG(0),
        )?;
        let bitmap = create_target_bitmap(&self.dc, &self.swap_chain, dpi)?;
        self.dc.SetTarget(&bitmap);
        self.dc.SetDpi(dpi, dpi);
        self._bitmap = Some(bitmap);
        self.pixel_width = width;
        self.pixel_height = height;
        self.dpi = dpi;
        Ok(())
    }

    unsafe fn draw(
        &mut self,
        now: f64,
        geometry: &FrameGeometry,
        content_geometry: &FrameGeometry,
        target: LayoutTarget,
        content: &ContentSnapshot,
        previous: Option<&ContentSnapshot>,
        progress: f32,
    ) -> RenderResult<()> {
        let transitioning = previous.is_some() && progress < 1.0;
        self.widget_navigation = content.widget_navigation;
        self.widget_pages = content.widget_pages;
        self.widget_page = content.widget_page;
        self.cpu_hot = content.cpu_hot;
        // A card opening or closing is composed separately (see draw_morph).
        // Not during a content cross-fade or while it is a droplet.
        let morph = if transitioning || geometry.taper > 0.001 {
            None
        } else {
            content.primary.as_ref().and_then(|card| {
                morph_of(
                    card,
                    self.main_spring_height,
                    self.media_rest,
                    self.content_scale,
                )
            })
        };
        self.dc.BeginDraw();
        self.dc.SetTransform(&Matrix3x2::identity());
        self.dc.Clear(Some(&TRANSPARENT));

        if geometry.main.width >= 0.5 && geometry.main.height >= 0.5 {
            let alpha = pill_alpha(geometry.main, target.visible) * geometry.shell_alpha;
            if self.cpu_hot && geometry.taper < 0.001 {
                self.draw_heat_glow(geometry.main, geometry.main_corner, alpha)?;
            }
            self.draw_pill(
                geometry.main,
                geometry.main_corner,
                alpha,
                geometry.taper,
                geometry.taper_top,
            )?;
            if let Some(bubble) = geometry.bubble {
                if bubble.width >= 0.5 && bubble.height >= 0.5 {
                    self.draw_pill(
                        bubble,
                        geometry.bubble_corner,
                        pill_alpha(bubble, target.visible) * geometry.shell_alpha,
                        geometry.taper,
                        geometry.taper_top,
                    )?;
                }
            }

            // The elastic shell never drives content layout. In particular a
            // vertical pull must not cross the compact/expanded height boundary.
            let geometry = content_geometry;
            if morph.is_some() {
                if let (Some(bubble), Some(card)) = (geometry.bubble, &content.secondary) {
                    self.draw_card_in_slot(
                        now,
                        bubble,
                        geometry.bubble_corner,
                        card,
                        content.accent,
                        geometry.content_alpha,
                        1.0,
                        geometry.taper,
                        geometry.taper_top,
                    )?;
                }
            } else if !transitioning {
                self.draw_snapshot(now, geometry, content, geometry.content_alpha, 1.0)?;
            }
        }

        self.dc.EndDraw(None, None)?;

        if transitioning {
            self.draw_transition(now, content_geometry, previous, content, progress)?;
        } else if let (Some(morph), Some(card)) = (morph, content.primary.as_ref()) {
            self.draw_morph(now, content_geometry, card, content.accent, morph)?;
        }
        Ok(())
    }

    /// A card opening into its big look, or closing back. Each look keeps
    /// its own layout at its final size, centred in the growing shape
    /// (revealed, never reflowed), and they take turns, blurring out and
    /// in, while a music card's art travels between them, sharp throughout.
    unsafe fn draw_morph(
        &mut self,
        now: f64,
        geometry: &FrameGeometry,
        card: &ContentCard,
        accent: [u8; 3],
        morph: Morph,
    ) -> RenderResult<()> {
        self.ensure_content_layers()?;
        let main_target = self._bitmap.clone().ok_or(RenderError::InvalidSize)?;
        let layers = self
            .content_layers
            .as_ref()
            .ok_or(RenderError::InvalidSize)?;
        let small = (
            layers[0].bitmap.clone(),
            layers[0].input.clone(),
            layers[0].effect.clone(),
            layers[0].output.clone(),
        );
        let big = (
            layers[1].bitmap.clone(),
            layers[1].input.clone(),
            layers[1].effect.clone(),
            layers[1].output.clone(),
        );
        let (small_alpha, big_alpha) = morph_fades(morph.open);
        let alpha = geometry.content_alpha;
        let mut composed = Vec::with_capacity(2);
        for (part, opacity, (bitmap, input, effect, output)) in [
            (MorphPart::Small, small_alpha, small),
            (MorphPart::Big, big_alpha, big),
        ] {
            if opacity <= 0.004 {
                continue;
            }
            self.dc.SetTarget(&bitmap);
            self.dc.BeginDraw();
            self.dc.SetTransform(&Matrix3x2::identity());
            self.dc.Clear(Some(&TRANSPARENT));
            self.morph_part = Some(part);
            let drawn = self.draw_card_in_slot(
                now,
                geometry.main,
                geometry.main_corner,
                card,
                accent,
                alpha * opacity,
                1.0,
                geometry.taper,
                geometry.taper_top,
            );
            self.morph_part = None;
            self.dc.EndDraw(None, None)?;
            drawn?;
            effect.SetInput(0, &input, true);
            set_blur_deviation(&effect, 7.0 * (1.0 - opacity))?;
            composed.push(output);
        }

        self.dc.SetTarget(&main_target);
        self.dc.BeginDraw();
        self.dc.SetTransform(&Matrix3x2::identity());
        let rect = geometry.main;
        self.dc.PushAxisAlignedClip(
            &D2D_RECT_F {
                left: rect.x,
                top: rect.y,
                right: rect.right(),
                bottom: rect.bottom(),
            },
            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        );
        for output in &composed {
            self.dc.DrawImage(
                output,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
        self.dc.PopAxisAlignedClip();
        let art = if matches!(card, ContentCard::Media(_)) {
            self.morph_part = Some(MorphPart::Art);
            let drawn = self.draw_card_in_slot(
                now,
                geometry.main,
                geometry.main_corner,
                card,
                accent,
                alpha,
                1.0,
                geometry.taper,
                geometry.taper_top,
            );
            self.morph_part = None;
            drawn
        } else {
            Ok(())
        };
        self.dc.EndDraw(None, None)?;
        art
    }

    unsafe fn draw_transition(
        &mut self,
        now: f64,
        geometry: &FrameGeometry,
        previous: Option<&ContentSnapshot>,
        content: &ContentSnapshot,
        progress: f32,
    ) -> RenderResult<()> {
        let Some(previous) = previous else {
            return Ok(());
        };
        if geometry.main.width < 0.5 || geometry.main.height < 0.5 {
            return Ok(());
        }
        self.ensure_content_layers()?;
        let main_target = self._bitmap.clone().ok_or(RenderError::InvalidSize)?;
        let (
            old_bitmap,
            old_input,
            old_effect,
            old_output,
            new_bitmap,
            new_input,
            new_effect,
            new_output,
        ) = {
            let layers = self
                .content_layers
                .as_ref()
                .ok_or(RenderError::InvalidSize)?;
            (
                layers[0].bitmap.clone(),
                layers[0].input.clone(),
                layers[0].effect.clone(),
                layers[0].output.clone(),
                layers[1].bitmap.clone(),
                layers[1].input.clone(),
                layers[1].effect.clone(),
                layers[1].output.clone(),
            )
        };

        let old_ease = ease_out(progress);
        let new_ease = ease_out(((progress - 0.08) / 0.92).clamp(0.0, 1.0));
        let content_alpha = geometry.content_alpha;
        self.render_content_layer(
            &old_bitmap,
            now,
            geometry,
            previous,
            (1.0 - old_ease) * content_alpha,
            1.0 - 0.055 * old_ease,
        )?;
        self.render_content_layer(
            &new_bitmap,
            now,
            geometry,
            content,
            new_ease * content_alpha,
            0.945 + 0.055 * new_ease,
        )?;

        old_effect.SetInput(0, &old_input, true);
        new_effect.SetInput(0, &new_input, true);
        set_blur_deviation(&old_effect, 6.0 * old_ease)?;
        set_blur_deviation(&new_effect, 6.0 * (1.0 - new_ease))?;

        self.dc.SetTarget(&main_target);
        self.dc.BeginDraw();
        self.dc.SetTransform(&Matrix3x2::identity());

        let draw_layer = |dc: &ID2D1DeviceContext,
                          rect: Rect,
                          old_output: &ID2D1Image,
                          new_output: &ID2D1Image|
         -> RenderResult<()> {
            if rect.width < 0.5 || rect.height < 0.5 {
                return Ok(());
            }
            let clip = D2D_RECT_F {
                left: rect.x,
                top: rect.y,
                right: rect.right(),
                bottom: rect.bottom(),
            };
            dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            dc.DrawImage(
                old_output,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            dc.DrawImage(
                new_output,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            dc.PopAxisAlignedClip();
            Ok(())
        };
        draw_layer(&self.dc, geometry.main, &old_output, &new_output)?;
        if let Some(bubble) = geometry.bubble {
            draw_layer(&self.dc, bubble, &old_output, &new_output)?;
        }

        self.dc.EndDraw(None, None)?;
        Ok(())
    }

    unsafe fn ensure_content_layers(&mut self) -> RenderResult<()> {
        if self.content_layers.is_none() {
            let first = ContentLayer::new(&self.dc, self.pixel_width, self.pixel_height, self.dpi)?;
            let second =
                ContentLayer::new(&self.dc, self.pixel_width, self.pixel_height, self.dpi)?;
            self.content_layers = Some([first, second]);
        }
        Ok(())
    }

    unsafe fn render_content_layer(
        &mut self,
        bitmap: &ID2D1Bitmap1,
        now: f64,
        geometry: &FrameGeometry,
        content: &ContentSnapshot,
        alpha: f32,
        scale: f32,
    ) -> RenderResult<()> {
        self.dc.SetTarget(bitmap);
        self.dc.BeginDraw();
        self.dc.SetTransform(&Matrix3x2::identity());
        self.dc.Clear(Some(&TRANSPARENT));
        self.draw_snapshot(now, geometry, content, alpha, scale)?;
        self.dc.EndDraw(None, None)?;
        Ok(())
    }

    unsafe fn draw_pill(
        &mut self,
        rect: Rect,
        corner: f32,
        alpha: f32,
        taper: f32,
        taper_top: bool,
    ) -> RenderResult<()> {
        if alpha <= 0.001 {
            return Ok(());
        }
        let geometry = self.geometries.get(
            &self.factory,
            rect.width,
            rect.height,
            corner.min(rect.width * 0.5).min(rect.height * 0.5),
            taper,
            taper_top,
        )?;
        self.dc
            .SetTransform(&Matrix3x2::translation(rect.x, rect.y));
        self.set_color(D2D1_COLOR_F {
            a: FRAME_COLOR.a * alpha,
            ..FRAME_COLOR
        });
        self.dc
            .FillGeometry(&geometry, &self.brush, None::<&ID2D1Brush>);
        self.set_color(D2D1_COLOR_F {
            a: HAIRLINE_COLOR.a * alpha,
            ..HAIRLINE_COLOR
        });
        self.dc.DrawGeometry(
            &geometry,
            &self.brush,
            HAIRLINE_WIDTH,
            None::<&ID2D1StrokeStyle>,
        );
        self.dc.SetTransform(&Matrix3x2::identity());
        Ok(())
    }

    /// A warm halo round the pill while the CPU is pegged. Static, so a busy
    /// machine is not also asked to animate it.
    unsafe fn draw_heat_glow(&mut self, rect: Rect, corner: f32, alpha: f32) -> RenderResult<()> {
        if alpha <= 0.001 {
            return Ok(());
        }
        let geometry = self.geometries.get(
            &self.factory,
            rect.width,
            rect.height,
            corner.min(rect.width * 0.5).min(rect.height * 0.5),
            0.0,
            false,
        )?;
        self.dc
            .SetTransform(&Matrix3x2::translation(rect.x, rect.y));
        for (width, strength) in [(9.0, 0.07), (5.0, 0.14), (2.5, 0.5)] {
            self.set_color(color(HEAT_COLOR, strength * alpha));
            self.dc.DrawGeometry(
                &geometry,
                &self.brush,
                width,
                None::<&ID2D1StrokeStyle>,
            );
        }
        self.dc.SetTransform(&Matrix3x2::identity());
        Ok(())
    }

    unsafe fn draw_snapshot(
        &mut self,
        now: f64,
        geometry: &FrameGeometry,
        content: &ContentSnapshot,
        alpha: f32,
        scale: f32,
    ) -> RenderResult<()> {
        let accent = content.accent;
        if let Some(card) = &content.primary {
            self.draw_card_in_slot(
                now,
                geometry.main,
                geometry.main_corner,
                card,
                accent,
                alpha,
                scale,
                geometry.taper,
                geometry.taper_top,
            )?;
        }
        if let (Some(bubble), Some(card)) = (geometry.bubble, &content.secondary) {
            self.draw_card_in_slot(
                now,
                bubble,
                geometry.bubble_corner,
                card,
                accent,
                alpha,
                scale,
                geometry.taper,
                geometry.taper_top,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_card_in_slot(
        &mut self,
        now: f64,
        rect: Rect,
        corner: f32,
        card: &ContentCard,
        accent: [u8; 3],
        alpha: f32,
        scale: f32,
        taper: f32,
        taper_top: bool,
    ) -> RenderResult<()> {
        if rect.width < 1.0 || rect.height < 1.0 || alpha <= 0.001 {
            return Ok(());
        }
        let clip = D2D_RECT_F {
            left: rect.x,
            top: rect.y,
            right: rect.right(),
            bottom: rect.bottom(),
        };
        // While the pill is gathered into a droplet, a rectangle clip would let
        // the card peek outside the tapered silhouette. Mask the content with
        // the same outline the shell is drawn with, so text and artwork end
        // exactly at the leaf's edge.
        let mut mask = None;
        if taper > 0.001 {
            let outline: ID2D1Geometry = self
                .geometries
                .get(
                    &self.factory,
                    rect.width,
                    rect.height,
                    corner.min(rect.width * 0.5).min(rect.height * 0.5),
                    taper,
                    taper_top,
                )?
                .cast()?;
            if self.mask_layer.is_none() {
                self.mask_layer = Some(self.dc.CreateLayer(None)?);
            }
            mask = Some(D2D1_LAYER_PARAMETERS1 {
                contentBounds: clip,
                geometricMask: ManuallyDrop::new(Some(outline)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::translation(rect.x, rect.y),
                opacity: 1.0,
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                ..Default::default()
            });
        }
        if let (Some(params), Some(layer)) = (&mask, &self.mask_layer) {
            self.dc.PushLayer(params, layer);
        } else {
            self.dc
                .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        let sizing = self.content_scale;
        let center = Vector2 {
            X: rect.width / sizing * 0.5,
            Y: rect.height / sizing * 0.5,
        };
        let transform = Matrix3x2::scale_around(scale, scale, center)
            * Matrix3x2::scale(sizing, sizing)
            * Matrix3x2::translation(rect.x, rect.y);
        self.dc.SetTransform(&transform);
        let drawn = self.draw_card(
            now,
            rect.width / sizing,
            rect.height / sizing,
            corner / sizing,
            card,
            accent,
            alpha,
        );
        self.dc.SetTransform(&Matrix3x2::identity());
        if let Some(params) = &mut mask {
            self.dc.PopLayer();
            // `PushLayer` wraps the interfaces in `ManuallyDrop`; release the
            // reference we handed it now that the layer has been popped.
            ManuallyDrop::drop(&mut params.geometricMask);
        } else {
            self.dc.PopAxisAlignedClip();
        }
        drawn
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_card(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        corner: f32,
        card: &ContentCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        if let Some(part) = self.morph_part {
            return self.draw_card_part(now, width, height, card, accent, alpha, part);
        }
        if let ContentCard::Intro = card {
            return self.draw_intro(width, height, corner, accent, alpha);
        }
        if width < 60.0 && height < 60.0 && !matches!(card, ContentCard::Dictation(_)) {
            return self.draw_bubble(now, width, height, corner, card, accent, alpha);
        }
        if height < 31.0 && !matches!(card, ContentCard::Dictation(_)) {
            return self.draw_tucked(now, width, height, card, accent, alpha);
        }
        let drawn = match card {
            ContentCard::Converter(card) => self.draw_converter(width, height, card, alpha),
            ContentCard::Calculator(calculator) => {
                self.draw_calculator(width, height, calculator, alpha)
            }
            ContentCard::Stats(stats) => self.draw_stats(width, height, stats, alpha),
            ContentCard::Glance(glance) => self.draw_glance(width, height, glance, alpha),
            ContentCard::Media(media) => {
                if height > 60.0 {
                    self.draw_media_expanded(now, width, height, media, accent, alpha)
                } else {
                    self.draw_media_compact(now, width, height, media, accent, alpha)
                }
            }
            ContentCard::Notification(notification) => {
                self.draw_notification(width, height, notification, accent, alpha)
            }
            ContentCard::Agent(agent) => {
                if height > 60.0 {
                    self.draw_agent_expanded(width, height, agent, accent, alpha)
                } else {
                    self.draw_agent_compact(now, width, height, agent, accent, alpha)
                }
            }
            ContentCard::Dictation(dictation) => {
                self.draw_dictation(now, width, height, dictation, accent, alpha)
            }
            ContentCard::Intro => Ok(()),
        };
        drawn?;
        if height > 60.0
            && self.widget_navigation
            && matches!(
                card,
                ContentCard::Media(_)
                    | ContentCard::Calculator(_)
                    | ContentCard::Stats(_)
                    | ContentCard::Agent(_)
            )
        {
            self.draw_widget_navigation(width, height, alpha)?;
        }
        Ok(())
    }

    /// One part of a morphing card, in the slot's local space (`width` x
    /// `height` is the shape now). Each look is laid out at its own final
    /// size and centred, so the shape reveals it rather than squeezing it.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_card_part(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        card: &ContentCard,
        accent: [u8; 3],
        alpha: f32,
        part: MorphPart,
    ) -> RenderResult<()> {
        let Some(morph) = morph_of(
            card,
            self.main_spring_height,
            self.media_rest,
            self.content_scale,
        ) else {
            return Ok(());
        };
        let origin = |look: PillSize| ((width - look.width) * 0.5, (height - look.height) * 0.5);
        if part == MorphPart::Art {
            let ContentCard::Media(media) = card else {
                return Ok(());
            };
            let (from, from_radius) = compact_art(morph.small);
            let (small_x, small_y) = origin(morph.small);
            let (big_x, big_y) = origin(morph.big);
            let open = morph.open;
            let lerp = |a: f32, b: f32| a + (b - a) * open;
            let to = expanded::ART;
            let art = Rect::new(
                lerp(small_x + from.x, big_x + to.x),
                lerp(small_y + from.y, big_y + to.y),
                lerp(from.width, to.width),
                lerp(from.height, to.height),
            );
            return self.draw_art(
                art,
                lerp(from_radius, expanded::ART_RADIUS),
                media.art.as_ref(),
                accent,
                alpha,
            );
        }
        let (small_alpha, big_alpha) = morph_fades(morph.open);
        let (look, opacity) = if part == MorphPart::Small {
            (morph.small, small_alpha)
        } else {
            (morph.big, big_alpha)
        };
        let (x, y) = origin(look);
        // Arriving and leaving looks sit a touch smaller as they come and go.
        let scale = 0.965 + 0.035 * opacity;
        let mut base = Matrix3x2::identity();
        self.dc.GetTransform(&mut base);
        let local = Matrix3x2::scale_around(
            scale,
            scale,
            Vector2 {
                X: look.width * 0.5,
                Y: look.height * 0.5,
            },
        ) * Matrix3x2::translation(x, y);
        self.dc.SetTransform(&(local * base));
        self.skip_art = true;
        let (w, h) = (look.width, look.height);
        let drawn = match (card, part) {
            (ContentCard::Converter(card), _) => self.draw_converter(w, h, card, alpha),
            (ContentCard::Media(media), MorphPart::Small) => {
                self.draw_media_compact(now, w, h, media, accent, alpha)
            }
            (ContentCard::Media(media), _) => {
                self.draw_media_expanded(now, w, h, media, accent, alpha)
            }
            (ContentCard::Notification(notification), MorphPart::Small) => {
                self.draw_notification_compact(w, h, notification, accent, alpha)
            }
            (ContentCard::Notification(notification), _) => {
                self.draw_notification(w, h, notification, accent, alpha)
            }
            (ContentCard::Agent(agent), MorphPart::Small) => {
                self.draw_agent_compact(now, w, h, agent, accent, alpha)
            }
            (ContentCard::Agent(agent), _) => self.draw_agent_expanded(w, h, agent, accent, alpha),
            (ContentCard::Calculator(calculator), _) => {
                self.draw_calculator(w, h, calculator, alpha)
            }
            (ContentCard::Stats(stats), _) => self.draw_stats(w, h, stats, alpha),
            (ContentCard::Glance(glance), _) => self.draw_glance(w, h, glance, alpha),
            (ContentCard::Dictation(_) | ContentCard::Intro, _) => Ok(()),
        };
        self.skip_art = false;
        self.dc.SetTransform(&base);
        drawn
    }

    unsafe fn draw_media_compact(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        media: &MediaCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        // Once music has played a while the pill narrows to the dictation
        // pill's size. `shrunk` is 0 for the full pill and 1 for that small
        // one, following the width so it tracks the spring: the title and
        // artist fade out first, the art tucks into the round end and the bars
        // settle into the middle of what is left.
        let shrunk = 1.0 - smoothstep(110.0, 200.0, width);
        // The compact pill is a stadium; a disc of half its height keeps the
        // art concentric with the pill's rounded ends.
        if !self.skip_art {
            let (rect, radius) = compact_art(PillSize::new(width, height, height * 0.5));
            self.draw_art(rect, radius, media.art.as_ref(), accent, alpha)?;
        }

        let text_alpha = smoothstep(150.0, 200.0, width);
        if text_alpha > 0.0 {
            let text_w = (width - 104.0).max(10.0);
            self.draw_text(
                &media.title,
                FormatId::CompactTitle,
                40.0,
                3.0,
                text_w,
                15.0,
                color([255, 255, 255], 0.96 * alpha * text_alpha),
            )?;
            self.draw_text(
                &media.artist,
                FormatId::CompactSub,
                40.0,
                19.0,
                text_w,
                13.0,
                color([173, 180, 199], 0.88 * alpha * text_alpha),
            )?;
        }

        // The shrunk pill keeps every bar, the same size and spacing, at the
        // right end with a small margin, leaving clear room after the art.
        let full_start = width - 46.0 - 10.0;
        let small_start = width - SHRUNK_BARS_RIGHT - ((BAR_COUNT - 1) as f32 * 8.0 + 2.8);
        let start = full_start + (small_start - full_start) * shrunk;
        let mid = height * 0.5;
        // Shorter peaks in the shrunk pill, so the bars keep clear of its
        // top and bottom edges.
        let reach = 15.0 + (SHRUNK_BAR_REACH - 15.0) * shrunk;
        for index in 0..BAR_COUNT {
            let level = self.bar_level(now, index, media.playing);
            let bar_h = 3.0 + level * reach;
            self.fill_round_rect(
                start + index as f32 * 8.0,
                mid - bar_h * 0.5,
                2.8,
                bar_h,
                1.4,
                color(accent, if media.playing { 0.92 } else { 0.5 } * alpha),
            )?;
        }
        Ok(())
    }

    /// One line for the slim docked island: what is playing (or what
    /// happened) stays readable while the island keeps out of the way.
    unsafe fn draw_tucked(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        card: &ContentCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        let (text, trailing_bars) = match card {
            ContentCard::Media(media) => {
                let art = (height - 8.0).max(8.0);
                self.draw_art(
                    Rect::new(5.0, (height - art) * 0.5, art, art),
                    5.0,
                    media.art.as_ref(),
                    accent,
                    alpha,
                )?;
                let text = if media.artist.is_empty() {
                    media.title.clone()
                } else {
                    format!("{}  \u{b7}  {}", media.title, media.artist)
                };
                (text, Some(media))
            }
            ContentCard::Notification(notification) => {
                self.fill_circle(13.0, height * 0.5, 3.5, color(accent, alpha))?;
                (
                    format!("{}  \u{b7}  {}", notification.title, notification.source),
                    None,
                )
            }
            ContentCard::Agent(agent) => {
                self.fill_circle(13.0, height * 0.5, 3.5, color(accent, alpha))?;
                (
                    format!("{}  \u{b7}  {}", agent.project, agent.activity),
                    None,
                )
            }
            ContentCard::Glance(glance) => (glance_line(glance), None),
            ContentCard::Converter(_) | ContentCard::Calculator(_) | ContentCard::Stats(_) => return Ok(()),
            ContentCard::Dictation(_) | ContentCard::Intro => return Ok(()),
        };

        let text_x = if trailing_bars.is_some() {
            height + 6.0
        } else {
            24.0
        };
        let bars_w = if trailing_bars.is_some() { 30.0 } else { 0.0 };
        self.draw_text(
            &text,
            FormatId::Tucked,
            text_x,
            0.0,
            (width - text_x - bars_w - 12.0).max(10.0),
            height,
            color([236, 238, 245], 0.95 * alpha),
        )?;

        if let Some(media) = trailing_bars {
            let start = width - bars_w - 6.0;
            let mid = height * 0.5;
            for index in 0..4 {
                let level = self.bar_level(now, index, media.playing);
                let bar_h = 2.5 + level * (height - 12.0);
                self.fill_round_rect(
                    start + index as f32 * 6.5,
                    mid - bar_h * 0.5,
                    2.4,
                    bar_h,
                    1.2,
                    color(accent, if media.playing { 0.92 } else { 0.5 } * alpha),
                )?;
            }
        }
        Ok(())
    }

    fn bar_level(&self, now: f64, index: usize, playing: bool) -> f32 {
        bar_level(self.bar_levels, now, index, playing)
    }

    unsafe fn draw_media_expanded(
        &mut self,
        now: f64,
        width: f32,
        _height: f32,
        media: &MediaCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        if !self.skip_art {
            self.draw_art(
                expanded::ART,
                expanded::ART_RADIUS,
                media.art.as_ref(),
                accent,
                if self.media_hover == Some(MediaHover::Artwork) {
                    alpha * 0.92
                } else {
                    alpha
                },
            )?;
        }

        let text_x = expanded::TEXT_X;
        let text_w = expanded::TEXT_WIDTH;
        self.draw_text(
            &media.title,
            FormatId::MediaTitle,
            text_x,
            expanded::TITLE_Y,
            text_w,
            expanded::TITLE_HEIGHT,
            color(
                if self.media_hover == Some(MediaHover::Title) {
                    [214, 214, 218]
                } else {
                    [255, 255, 255]
                },
                0.98 * alpha,
            ),
        )?;
        self.draw_text(
            &media.artist,
            FormatId::MediaArtist,
            text_x,
            expanded::ARTIST_Y,
            text_w,
            expanded::ARTIST_HEIGHT,
            color(
                [166, 166, 170],
                if self.media_hover == Some(MediaHover::Title) {
                    alpha * 0.9
                } else {
                    alpha
                },
            ),
        )?;
        self.fill_round_rect(
            expanded::PLAYER_WIDTH,
            20.0,
            1.0,
            80.0,
            0.5,
            color([255, 255, 255], 0.085 * alpha),
        )?;
        self.draw_source_chip(&media.source, width, alpha)?;

        if let Some(timeline) = media.timeline.filter(|timeline| timeline.duration > 0.0) {
            let bar = expanded::scrubber(width);
            if self.media_hover == Some(MediaHover::Seek) {
                let hover = bar.inflate(expanded::SCRUBBER_HIT_SLOP);
                self.fill_round_rect(
                    hover.x,
                    hover.y,
                    hover.width,
                    hover.height,
                    7.0,
                    color([255, 255, 255], 0.14 * alpha),
                )?;
            }
            let progress = media
                .scrub
                .unwrap_or_else(|| timeline.progress_at(now, media.playing))
                .clamp(0.0, 1.0);
            let position = f64::from(progress) * timeline.duration;
            self.fill_round_rect(
                bar.x,
                bar.y,
                bar.width,
                bar.height,
                bar.height * 0.5,
                color([255, 255, 255], 0.16 * alpha),
            )?;
            self.fill_round_rect(
                bar.x,
                bar.y,
                (bar.width * progress).max(bar.height),
                bar.height,
                bar.height * 0.5,
                color([255, 255, 255], 0.78 * alpha),
            )?;
            if media.scrub.is_some() || self.media_hover == Some(MediaHover::Seek) {
                self.fill_circle(
                    bar.x + bar.width * progress,
                    bar.y + bar.height * 0.5,
                    5.5,
                    color([255, 255, 255], alpha),
                )?;
            }
            let label = color([145, 145, 150], alpha);
            self.draw_text(
                &format_clock(position),
                FormatId::Tiny,
                bar.x,
                expanded::TIMES_Y,
                60.0,
                expanded::TIMES_HEIGHT,
                label,
            )?;
            self.draw_text(
                &format!("-{}", format_clock(timeline.duration - position)),
                FormatId::TinyRight,
                bar.right() - 60.0,
                expanded::TIMES_Y,
                60.0,
                expanded::TIMES_HEIGHT,
                label,
            )?;
        }

        let [previous, play, next] = expanded::controls(width);
        let hovered_control = match self.media_hover {
            Some(MediaHover::Previous) => Some(previous),
            Some(MediaHover::PlayPause) => Some(play),
            Some(MediaHover::Next) => Some(next),
            _ => None,
        };
        if let Some((x, y)) = hovered_control {
            self.fill_circle(
                x,
                y,
                expanded::CONTROL_HIT_RADIUS,
                color([255, 255, 255], 0.14 * alpha),
            )?;
        }
        self.draw_transport(previous.0, previous.1, TransportIcon::Previous, alpha)?;
        self.draw_transport(
            play.0,
            play.1,
            TransportIcon::PlayPause(media.playing),
            alpha,
        )?;
        self.draw_transport(next.0, next.1, TransportIcon::Next, alpha)?;

        if let Some(volume) = media.volume {
            self.draw_volume(volume, width, alpha)?;
        }
        Ok(())
    }

    /// Source name in a soft pill with an "open" arrow: pressing it brings the
    /// playing app to the front.
    unsafe fn draw_source_chip(
        &mut self,
        source: &str,
        pill_width: f32,
        alpha: f32,
    ) -> RenderResult<()> {
        if source.is_empty() {
            return Ok(());
        }
        let chip = expanded::chip(pill_width, source);
        let fill = color(
            [255, 255, 255],
            if self.media_hover == Some(MediaHover::Source) {
                0.22
            } else {
                0.115
            } * alpha,
        );
        let ink = color([240, 240, 242], alpha);
        self.fill_round_rect(
            chip.x,
            chip.y,
            chip.width,
            chip.height,
            chip.height * 0.5,
            fill,
        )?;
        self.draw_text(
            source,
            FormatId::Chip,
            chip.x + 16.0,
            chip.y + 10.0,
            (chip.width - 44.0).max(10.0),
            16.0,
            ink,
        )?;
        // A small north-east arrow, so the chip reads as "opens the app".
        let ax = chip.right() - 18.0;
        let ay = chip.center().1;
        self.set_color(ink);
        self.dc.DrawLine(
            Vector2 {
                X: ax - 3.0,
                Y: ay + 3.0,
            },
            Vector2 {
                X: ax + 3.0,
                Y: ay - 3.0,
            },
            &self.brush,
            1.3,
            None,
        );
        self.dc.DrawLine(
            Vector2 {
                X: ax + 0.2,
                Y: ay - 3.0,
            },
            Vector2 {
                X: ax + 3.0,
                Y: ay - 3.0,
            },
            &self.brush,
            1.3,
            None,
        );
        self.dc.DrawLine(
            Vector2 {
                X: ax + 3.0,
                Y: ay - 3.0,
            },
            Vector2 {
                X: ax + 3.0,
                Y: ay + 0.2,
            },
            &self.brush,
            1.3,
            None,
        );
        Ok(())
    }

    /// Speaker button and a slim volume slider. At zero the waves turn into a
    /// cross, so mute reads at a glance.
    unsafe fn draw_volume(&mut self, volume: f32, pill_width: f32, alpha: f32) -> RenderResult<()> {
        let area = expanded::volume_area(pill_width);
        self.fill_round_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            18.0,
            color(
                [255, 255, 255],
                if self.media_hover == Some(MediaHover::Volume) {
                    0.22
                } else {
                    0.115
                } * alpha,
            ),
        )?;
        let track = expanded::volume_track(pill_width);
        let (cx, cy) = expanded::volume_icon_center(pill_width);
        let ink = color([210, 210, 214], alpha);
        let volume = volume.clamp(0.0, 1.0);

        self.fill_polygon(
            &[
                Vector2 {
                    X: cx - 7.0,
                    Y: cy - 3.4,
                },
                Vector2 {
                    X: cx - 2.5,
                    Y: cy - 3.4,
                },
                Vector2 {
                    X: cx + 2.5,
                    Y: cy - 7.2,
                },
                Vector2 {
                    X: cx + 2.5,
                    Y: cy + 7.2,
                },
                Vector2 {
                    X: cx - 2.5,
                    Y: cy + 3.4,
                },
                Vector2 {
                    X: cx - 7.0,
                    Y: cy + 3.4,
                },
            ],
            ink,
        )?;
        if volume > 0.005 {
            self.stroke_arc(cx + 2.5, cy, 5.2, -52.0, 52.0, 1.4, ink)?;
            self.stroke_arc(cx + 2.5, cy, 8.8, -46.0, 46.0, 1.4, ink)?;
        } else {
            self.set_color(ink);
            self.dc.DrawLine(
                Vector2 {
                    X: cx + 7.0,
                    Y: cy - 3.0,
                },
                Vector2 {
                    X: cx + 12.0,
                    Y: cy + 3.0,
                },
                &self.brush,
                1.5,
                None,
            );
            self.dc.DrawLine(
                Vector2 {
                    X: cx + 12.0,
                    Y: cy - 3.0,
                },
                Vector2 {
                    X: cx + 7.0,
                    Y: cy + 3.0,
                },
                &self.brush,
                1.5,
                None,
            );
        }

        self.fill_round_rect(
            track.x,
            track.y,
            track.width,
            track.height,
            track.height * 0.5,
            color([255, 255, 255], 0.16 * alpha),
        )?;
        self.fill_round_rect(
            track.x,
            track.y,
            (track.width * volume).max(track.height),
            track.height,
            track.height * 0.5,
            color([255, 255, 255], 0.86 * alpha),
        )?;
        self.fill_circle(
            track.x + track.width * volume,
            track.y + track.height * 0.5,
            4.0,
            color([255, 255, 255], 0.96 * alpha),
        )
    }

    /// Album art clipped to a rounded rect, or a tinted placeholder.
    unsafe fn draw_art(
        &mut self,
        rect: Rect,
        radius: f32,
        art: Option<&ArtImage>,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        if rect.width < 0.5 || rect.height < 0.5 {
            return Ok(());
        }
        // The same outline as the pills (squircle corners, true half-circles
        // when fully round), so the art inset in a card runs parallel to the
        // card's own edge and the gap stays even around the corners.
        let shape =
            self.geometries
                .get(&self.factory, rect.width, rect.height, radius, 0.0, false)?;
        let mut base = Matrix3x2::identity();
        self.dc.GetTransform(&mut base);
        self.dc
            .SetTransform(&(Matrix3x2::translation(rect.x, rect.y) * base));
        if let Some(art) = art {
            let brush = self.art_brush(art);
            if let Ok(brush) = &brush {
                brush.SetOpacity(alpha.clamp(0.0, 1.0));
                brush.SetTransform(&Matrix3x2::scale(
                    rect.width / art.width.max(1) as f32,
                    rect.height / art.height.max(1) as f32,
                ));
                self.dc.FillGeometry(&shape, brush, None::<&ID2D1Brush>);
            }
            self.dc.SetTransform(&base);
            return brush.map(|_| ());
        }
        self.set_color(color(accent, 0.55 * alpha));
        self.dc
            .FillGeometry(&shape, &self.brush, None::<&ID2D1Brush>);
        self.dc.SetTransform(&base);
        let (cx, cy) = rect.center();
        let disc = rect.width.min(rect.height);
        self.fill_circle(cx, cy, disc * 0.2, color([255, 255, 255], 0.22 * alpha))?;
        self.fill_circle(cx, cy, disc * 0.07, color([255, 255, 255], 0.8 * alpha))
    }

    unsafe fn art_brush(&mut self, art: &ArtImage) -> RenderResult<ID2D1BitmapBrush1> {
        if let Some((_, brush)) = self.art.iter().find(|(id, _)| *id == art.id) {
            return Ok(brush.clone());
        }
        let expected = (art.width as usize) * (art.height as usize) * 4;
        if art.width == 0 || art.height == 0 || art.pixels.len() < expected {
            return Err(RenderError::InvalidSize);
        }
        let properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap = stage(
            "Create album art bitmap",
            self.dc.CreateBitmap(
                D2D_SIZE_U {
                    width: art.width,
                    height: art.height,
                },
                Some(art.pixels.as_ptr().cast()),
                art.width * 4,
                &properties,
            ),
        )?;
        let brush = stage(
            "Create album art brush",
            self.dc.CreateBitmapBrush(
                &bitmap,
                Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                }),
                None,
            ),
        )?;
        if self.art.len() >= ART_CACHE_SIZE {
            self.art.remove(0);
        }
        self.art.push((art.id, brush.clone()));
        Ok(brush)
    }

    unsafe fn draw_transport(
        &mut self,
        x: f32,
        y: f32,
        icon: TransportIcon,
        alpha: f32,
    ) -> RenderResult<()> {
        let ink = color([240, 240, 242], alpha);
        match icon {
            TransportIcon::Previous | TransportIcon::Next => {
                let direction = if matches!(icon, TransportIcon::Previous) {
                    -1.0
                } else {
                    1.0
                };
                self.fill_polygon(
                    &[
                        Vector2 {
                            X: x - direction * 4.0,
                            Y: y - 4.5,
                        },
                        Vector2 {
                            X: x + direction * 3.0,
                            Y: y,
                        },
                        Vector2 {
                            X: x - direction * 4.0,
                            Y: y + 4.5,
                        },
                    ],
                    ink,
                )?;
                self.fill_round_rect(x + direction * 4.5 - 0.8, y - 4.5, 1.6, 9.0, 0.6, ink)?;
            }
            TransportIcon::PlayPause(true) => {
                self.fill_round_rect(x - 4.0, y - 5.0, 2.7, 10.0, 0.7, ink)?;
                self.fill_round_rect(x + 1.3, y - 5.0, 2.7, 10.0, 0.7, ink)?;
            }
            TransportIcon::PlayPause(false) => {
                self.fill_polygon(
                    &[
                        Vector2 {
                            X: x - 3.5,
                            Y: y - 5.5,
                        },
                        Vector2 { X: x + 5.0, Y: y },
                        Vector2 {
                            X: x - 3.5,
                            Y: y + 5.5,
                        },
                    ],
                    ink,
                )?;
            }
        }
        Ok(())
    }
    unsafe fn draw_notification(
        &mut self,
        width: f32,
        height: f32,
        notification: &NotificationCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        if height < 60.0 {
            return self.draw_notification_compact(width, height, notification, accent, alpha);
        }
        let icon = 36.0;
        let icon_y = (height - icon) * 0.5;
        self.draw_notice_icon(Rect::new(16.0, icon_y, icon, icon), 12.0, notification, accent, alpha)?;

        let text_x = 64.0;
        let text_w = (width - text_x - 24.0).max(40.0);
        // Without a source line the title and body sit centred.
        let lift = if notification.source.is_empty() { 7.0 } else { 0.0 };
        self.draw_text(
            &notification.source.to_uppercase(),
            FormatId::Tiny,
            text_x,
            12.0,
            text_w,
            14.0,
            color([156, 164, 184], 0.9 * alpha),
        )?;
        self.draw_text(
            &notification.title,
            FormatId::Title,
            text_x,
            26.0 - lift,
            text_w,
            22.0,
            color([255, 255, 255], 0.98 * alpha),
        )?;
        self.draw_text(
            &notification.body,
            FormatId::Sub,
            text_x,
            48.0 - lift,
            text_w,
            17.0,
            color([177, 185, 204], 0.92 * alpha),
        )?;

        self.draw_alert_dot(width - 19.0, 19.0, notification.level, accent, alpha)
    }

    /// The square beside a notice: the app's own icon when there is one,
    /// the Onyx mark for Onyx's own messages, else the sender's initial.
    unsafe fn draw_notice_icon(
        &mut self,
        rect: Rect,
        radius: f32,
        notification: &NotificationCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        if let Some(icon) = &notification.icon {
            return self.draw_art(rect, radius * 0.6, Some(icon), accent, alpha);
        }
        if notification.source.is_empty() {
            // A tiny version of the app icon: a black pill with a silver
            // rim and the lavender dot at its right end.
            let pill_h = rect.height * 0.42;
            let pill = Rect::new(rect.x + rect.width * 0.08, rect.y + (rect.height - pill_h) * 0.5, rect.width * 0.84, pill_h);
            self.fill_round_rect(rect.x, rect.y, rect.width, rect.height, radius, color([255, 255, 255], 0.07 * alpha))?;
            let rounded = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: pill.x, top: pill.y, right: pill.right(), bottom: pill.bottom() },
                radiusX: pill_h * 0.5,
                radiusY: pill_h * 0.5,
            };
            self.set_color(color([0, 0, 0], alpha));
            self.dc.FillRoundedRectangle(&rounded, &self.brush);
            self.set_color(color([214, 218, 230], 0.85 * alpha));
            self.dc.DrawRoundedRectangle(&rounded, &self.brush, (rect.height / 24.0).max(1.0), None::<&ID2D1StrokeStyle>);
            let dot = pill_h * 0.2;
            return self.fill_circle(pill.right() - pill_h * 0.5, pill.y + pill_h * 0.5, dot, color(islet_core::island::INTRO_ACCENT, alpha));
        }
        self.fill_round_rect(rect.x, rect.y, rect.width, rect.height, radius, color(accent, 0.18 * alpha))?;
        let initial = notification.source.chars().next().unwrap_or('I').to_uppercase().next().unwrap_or('I');
        self.draw_text(&initial.to_string(), FormatId::Bubble, rect.x, rect.y, rect.width, rect.height, color(accent, 0.98 * alpha))
    }

    unsafe fn draw_notification_compact(
        &mut self,
        width: f32,
        height: f32,
        notification: &NotificationCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        let icon = 24.0;
        let icon_y = (height - icon) * 0.5;
        self.draw_notice_icon(Rect::new(8.0, icon_y, icon, icon), 8.0, notification, accent, alpha)?;

        let text_x = 40.0;
        let text_w = (width - text_x - 28.0).max(30.0);
        // The second line names the sender; Onyx's own messages show their
        // detail there instead, or centre a lone title.
        let second = if notification.source.is_empty() { &notification.body } else { &notification.source };
        self.draw_text(
            &notification.title,
            FormatId::CompactTitle,
            text_x,
            if second.is_empty() { (height - 16.0) * 0.5 } else { 2.0 },
            text_w,
            16.0,
            color([255, 255, 255], 0.96 * alpha),
        )?;
        self.draw_text(
            second,
            FormatId::CompactSub,
            text_x,
            19.0,
            text_w,
            13.0,
            color([173, 180, 199], 0.88 * alpha),
        )?;

        self.draw_alert_dot(
            width - 13.0,
            height * 0.5,
            notification.level,
            accent,
            alpha,
        )
    }

    unsafe fn draw_alert_dot(
        &mut self,
        x: f32,
        y: f32,
        level: AlertLevel,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        match level {
            AlertLevel::NeedsInput | AlertLevel::Error => {
                self.fill_circle(x, y, 3.2, color(accent, alpha))?;
            }
            AlertLevel::Success => {
                self.fill_circle(x, y, 3.2, color(accent, 0.9 * alpha))?;
            }
            AlertLevel::Info => {}
        }
        Ok(())
    }

    unsafe fn draw_agent_compact(
        &mut self,
        now: f64,
        width: f32,
        _height: f32,
        agent: &AgentCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        self.fill_circle(17.0, 18.0, 5.0, color(accent, alpha))?;
        self.draw_text(
            &agent.project,
            FormatId::CompactTitle,
            29.0,
            3.0,
            (width - 84.0).max(20.0),
            15.0,
            color([255, 255, 255], 0.96 * alpha),
        )?;
        self.draw_text(
            &agent.activity,
            FormatId::CompactSub,
            29.0,
            19.0,
            (width - 84.0).max(20.0),
            13.0,
            color([173, 180, 199], 0.88 * alpha),
        )?;
        let phase = (now * 6.0).sin() as f32;
        self.fill_circle(
            width - 18.0,
            18.0 + phase * 2.0,
            2.4,
            color(accent, 0.9 * alpha),
        )?;
        Ok(())
    }

    unsafe fn draw_agent_expanded(
        &mut self,
        width: f32,
        height: f32,
        agent: &AgentCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        self.draw_notification(
            width,
            height,
            &NotificationCard {
                source: agent.project.clone(),
                title: agent.activity.clone(),
                body: match agent.state {
                    AgentState::Running => "Working in the background".to_owned(),
                    AgentState::NeedsInput => "Waiting for your approval".to_owned(),
                    AgentState::Done => "Finished".to_owned(),
                },
                level: match agent.state {
                    AgentState::Running => AlertLevel::Info,
                    AgentState::NeedsInput => AlertLevel::NeedsInput,
                    AgentState::Done => AlertLevel::Success,
                },
                icon: None,
            },
            accent,
            alpha,
        )
    }

    /// The dictation pill: six bars and nothing else, so it stays small. Bars
    /// follow the voice while recording, breathe while the model warms up,
    /// and sweep while transcribing. Once the text is in, the pill simply goes.
    /// A faint dot on the left marks a locked-on (hands-free) recording.
    unsafe fn draw_dictation(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        dictation: &DictationCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        use voice::BARS;
        const PITCH: f32 = 9.0;
        const BAR_WIDTH: f32 = 3.0;

        let mid = height * 0.5;

        let span = (BARS - 1) as f32 * PITCH + BAR_WIDTH;
        let left = (width - span) * 0.5;
        let max_h = (height - 12.0).max(6.0);
        // Loading shows the live mic too: the audio is already being kept,
        // and the model catches up on it once it is ready.
        let targets = match dictation.phase {
            DictationPhase::Transcribing => {
                let mut wave = [0.0; BARS];
                for (index, value) in wave.iter_mut().enumerate() {
                    let phase = (now * 6.0 - index as f64 * 0.8).sin() * 0.5 + 0.5;
                    *value = 0.12 + 0.34 * phase as f32;
                }
                wave
            }
            _ => voice::targets(&dictation.bars, now),
        };
        let heights = self.voice_bars.step(now, targets);
        for (index, level) in heights.iter().enumerate() {
            let bar_alpha = match dictation.phase {
                DictationPhase::Transcribing => 0.9,
                _ => 0.7 + 0.25 * (1.0 - (index as f32 - 2.5).abs() / 2.5),
            };
            let bar_h = 3.0 + level.max(0.0) * (max_h - 3.0);
            self.fill_round_rect(
                left + index as f32 * PITCH,
                mid - bar_h * 0.5,
                BAR_WIDTH,
                bar_h,
                BAR_WIDTH * 0.5,
                color(accent, bar_alpha * alpha),
            )?;
        }

        if dictation.hands_free && dictation.phase == DictationPhase::Recording {
            let pulse = 0.65 + 0.35 * (now * 4.0).sin() as f32;
            self.fill_circle(
                (left * 0.5).max(5.0),
                mid,
                2.2,
                color(accent, 0.9 * pulse * alpha),
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_bubble(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        corner: f32,
        card: &ContentCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        let center_x = width * 0.5;
        let center_y = height * 0.5;
        match card {
            ContentCard::Agent(agent) => {
                let initial = agent
                    .project
                    .chars()
                    .next()
                    .unwrap_or('A')
                    .to_uppercase()
                    .next()
                    .unwrap_or('A');
                self.fill_round_rect(
                    4.0,
                    4.0,
                    width - 8.0,
                    height - 8.0,
                    (corner - 3.0).max(4.0),
                    color(accent, 0.16 * alpha),
                )?;
                self.draw_text(
                    &initial.to_string(),
                    FormatId::Bubble,
                    0.0,
                    0.0,
                    width,
                    height,
                    color(accent, 0.98 * alpha),
                )?;
                let angle = now * 3.8;
                self.fill_circle(
                    center_x + (angle.cos() as f32) * (width * 0.36),
                    center_y + (angle.sin() as f32) * (height * 0.36),
                    2.2,
                    color([255, 255, 255], 0.82 * alpha),
                )?;
            }
            ContentCard::Notification(notification) => {
                let mark = match notification.level {
                    AlertLevel::NeedsInput => "!",
                    AlertLevel::Error => "!",
                    AlertLevel::Success => "âœ“",
                    AlertLevel::Info => "i",
                };
                self.fill_round_rect(
                    4.0,
                    4.0,
                    width - 8.0,
                    height - 8.0,
                    (corner - 3.0).max(4.0),
                    color(accent, 0.18 * alpha),
                )?;
                self.draw_text(
                    mark,
                    FormatId::Bubble,
                    0.0,
                    0.0,
                    width,
                    height,
                    color(accent, 0.98 * alpha),
                )?;
            }
            ContentCard::Media(media) => {
                let half = width.min(height) * 0.5;
                self.draw_media_bubble(center_x, center_y, half, media, alpha)?;
            }
            ContentCard::Dictation(_) => {
                self.fill_circle(center_x, center_y, 5.0, color(accent, alpha))?;
            }
            ContentCard::Converter(_) | ContentCard::Calculator(_) | ContentCard::Stats(_) | ContentCard::Glance(_) | ContentCard::Intro => {
                return Ok(())
            }
        }
        Ok(())
    }

    /// The second player: its album art as a disc, with the same thin black
    /// margin all the way round. A paused player dims under a pause mark.
    unsafe fn draw_media_bubble(
        &mut self,
        cx: f32,
        cy: f32,
        half: f32,
        media: &MediaCard,
        alpha: f32,
    ) -> RenderResult<()> {
        let accent = media.accent.unwrap_or(DEFAULT_MEDIA_ACCENT);
        let r = (half - BUBBLE_ART_INSET).max(2.0);
        self.draw_art(
            Rect::new(cx - r, cy - r, r * 2.0, r * 2.0),
            r,
            media.art.as_ref(),
            accent,
            if media.playing { alpha } else { alpha * 0.45 },
        )?;
        if !media.playing {
            let bar_h = (r * 0.62).round();
            let bar_w = (r * 0.17).max(2.0);
            let gap = bar_w * 1.1;
            for x in [cx - gap * 0.5 - bar_w, cx + gap * 0.5] {
                self.fill_round_rect(
                    x,
                    cy - bar_h * 0.5,
                    bar_w,
                    bar_h,
                    bar_w * 0.5,
                    color([255, 255, 255], 0.95 * alpha),
                )?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_text(
        &mut self,
        text: &str,
        format: FormatId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: D2D1_COLOR_F,
    ) -> RenderResult<()> {
        if text.is_empty() || width < 1.0 || height < 1.0 || color.a <= 0.001 {
            return Ok(());
        }
        let format_ref = self.formats.get(format);
        let layout =
            self.text_cache
                .layout(&self.dwrite, format_ref, format, text, width, height)?;
        self.set_color(color);
        self.dc.DrawTextLayout(
            Vector2 { X: x, Y: y },
            &layout,
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
        );
        Ok(())
    }

    unsafe fn set_color(&self, color: D2D1_COLOR_F) {
        self.brush.SetColor(&color);
    }

    unsafe fn fill_round_rect(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        color: D2D1_COLOR_F,
    ) -> RenderResult<()> {
        if width <= 0.0 || height <= 0.0 || color.a <= 0.001 {
            return Ok(());
        }
        let radius = radius.max(0.0).min(width * 0.5).min(height * 0.5);
        let rounded = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + width,
                bottom: y + height,
            },
            radiusX: radius,
            radiusY: radius,
        };
        self.set_color(color);
        self.dc.FillRoundedRectangle(&rounded, &self.brush);
        Ok(())
    }

    unsafe fn fill_circle(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        color: D2D1_COLOR_F,
    ) -> RenderResult<()> {
        if radius <= 0.0 || color.a <= 0.001 {
            return Ok(());
        }
        let ellipse = D2D1_ELLIPSE {
            point: Vector2 { X: x, Y: y },
            radiusX: radius,
            radiusY: radius,
        };
        self.set_color(color);
        self.dc.FillEllipse(&ellipse, &self.brush);
        Ok(())
    }

    /// Fills a closed polygon. Used for glyphs that are clearer as outlines
    /// than as rectangles (the speaker cone).
    unsafe fn fill_polygon(&mut self, points: &[Vector2], color: D2D1_COLOR_F) -> RenderResult<()> {
        if points.len() < 3 || color.a <= 0.001 {
            return Ok(());
        }
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.SetFillMode(D2D1_FILL_MODE_WINDING);
        sink.BeginFigure(points[0], D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLines(&points[1..]);
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        self.set_color(color);
        self.dc.FillGeometry(&geometry, &self.brush, None);
        Ok(())
    }

    /// Draws a circular arc as short segments, for the speaker's sound waves.
    #[allow(clippy::too_many_arguments)]
    unsafe fn stroke_arc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        from_deg: f32,
        to_deg: f32,
        width: f32,
        color: D2D1_COLOR_F,
    ) -> RenderResult<()> {
        if radius <= 0.0 || color.a <= 0.001 {
            return Ok(());
        }
        self.set_color(color);
        const STEPS: usize = 8;
        let mut previous = None;
        for step in 0..=STEPS {
            let t = step as f32 / STEPS as f32;
            let angle = (from_deg + (to_deg - from_deg) * t).to_radians();
            let point = Vector2 {
                X: cx + radius * angle.cos(),
                Y: cy + radius * angle.sin(),
            };
            if let Some(previous) = previous {
                self.dc.DrawLine(previous, point, &self.brush, width, None);
            }
            previous = Some(point);
        }
        Ok(())
    }

    unsafe fn present(&mut self) -> RenderResult<()> {
        self.swap_chain
            .Present(0, DXGI_PRESENT(0))
            .ok()
            .map_err(RenderError::from)
    }
}

unsafe fn create_swap_chain(
    factory: &IDXGIFactory2,
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> RenderResult<IDXGISwapChain1> {
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: width.max(1),
        Height: height.max(1),
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        Flags: 0,
    };
    stage(
        "CreateSwapChainForComposition",
        factory.CreateSwapChainForComposition(device, &desc, None::<&IDXGIOutput>),
    )
}

unsafe fn create_target_bitmap(
    dc: &ID2D1DeviceContext,
    swap_chain: &IDXGISwapChain1,
    dpi: f32,
) -> RenderResult<ID2D1Bitmap1> {
    let surface: IDXGISurface = stage("swap chain GetBuffer", swap_chain.GetBuffer(0))?;
    let properties = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: D2D1_BITMAP_OPTIONS(
            D2D1_BITMAP_OPTIONS_TARGET.0 | D2D1_BITMAP_OPTIONS_CANNOT_DRAW.0,
        ),
        colorContext: std::mem::ManuallyDrop::new(None),
    };
    stage(
        "CreateBitmapFromDxgiSurface",
        dc.CreateBitmapFromDxgiSurface(&surface, Some(&properties)),
    )
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum FormatId {
    StatsValue,
    CalculatorSmallResult,
    CalculatorKey,
    CalculatorResult,
    CompactTitle,
    CompactSub,
    Title,
    Sub,
    MediaTitle,
    MediaArtist,
    Tiny,
    TinyRight,
    Chip,
    Tucked,
    Bubble,
}

struct TextFormats {
    stats_value: IDWriteTextFormat,
    calculator_small_result: IDWriteTextFormat,
    calculator_key: IDWriteTextFormat,
    calculator_result: IDWriteTextFormat,
    compact_title: IDWriteTextFormat,
    compact_sub: IDWriteTextFormat,
    title: IDWriteTextFormat,
    sub: IDWriteTextFormat,
    media_title: IDWriteTextFormat,
    media_artist: IDWriteTextFormat,
    tiny: IDWriteTextFormat,
    tiny_right: IDWriteTextFormat,
    chip: IDWriteTextFormat,
    tucked: IDWriteTextFormat,
    bubble: IDWriteTextFormat,
}

impl TextFormats {
    unsafe fn new(factory: &IDWriteFactory) -> RenderResult<Self> {
        Ok(Self {
            stats_value: create_format(
                factory,
                30.0,
                DWRITE_FONT_WEIGHT_DEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            calculator_small_result: create_format(
                factory,
                20.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?,
            calculator_key: create_format(
                factory,
                12.0,
                DWRITE_FONT_WEIGHT_MEDIUM,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?,
            calculator_result: create_format(
                factory,
                26.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?,
            compact_title: create_format(
                factory,
                13.0,
                DWRITE_FONT_WEIGHT_DEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            compact_sub: create_format(
                factory,
                10.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            title: create_format(
                factory,
                16.0,
                DWRITE_FONT_WEIGHT_DEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            sub: create_format(
                factory,
                11.5,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            media_title: create_format(
                factory,
                13.0,
                DWRITE_FONT_WEIGHT_DEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            media_artist: create_format(
                factory,
                12.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            tiny: create_format(
                factory,
                9.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            tiny_right: create_format(
                factory,
                9.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            chip: create_format(
                factory,
                11.0,
                DWRITE_FONT_WEIGHT_MEDIUM,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
            )?,
            tucked: create_format(
                factory,
                12.0,
                DWRITE_FONT_WEIGHT_DEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?,
            bubble: create_format(
                factory,
                14.0,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?,
        })
    }

    fn get(&self, id: FormatId) -> &IDWriteTextFormat {
        match id {
            FormatId::StatsValue => &self.stats_value,
            FormatId::CalculatorSmallResult => &self.calculator_small_result,
            FormatId::CalculatorKey => &self.calculator_key,
            FormatId::CalculatorResult => &self.calculator_result,
            FormatId::CompactTitle => &self.compact_title,
            FormatId::CompactSub => &self.compact_sub,
            FormatId::Title => &self.title,
            FormatId::Sub => &self.sub,
            FormatId::MediaTitle => &self.media_title,
            FormatId::MediaArtist => &self.media_artist,
            FormatId::Tiny => &self.tiny,
            FormatId::TinyRight => &self.tiny_right,
            FormatId::Chip => &self.chip,
            FormatId::Tucked => &self.tucked,
            FormatId::Bubble => &self.bubble,
        }
    }
}

unsafe fn create_text_format(
    factory: &IDWriteFactory,
    family: &str,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> windows::core::Result<IDWriteTextFormat> {
    let family: Vec<u16> = family.encode_utf16().chain([0]).collect();
    factory.CreateTextFormat(
        PCWSTR(family.as_ptr()),
        None::<&IDWriteFontCollection>,
        weight,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        size,
        w!("en-us"),
    )
}

unsafe fn create_format(
    factory: &IDWriteFactory,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    alignment: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
    paragraph: windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT,
) -> RenderResult<IDWriteTextFormat> {
    // A Helvetica-like grotesque with a full weight range, so title and
    // caption weights stay distinct. Inter is installed here; keep Segoe UI
    // on machines that lack it.
    let format = stage(
        "CreateTextFormat",
        create_text_format(factory, "Inter", size, weight)
            .or_else(|_| create_text_format(factory, "Segoe UI", size, weight)),
    )?;
    stage("SetTextAlignment", format.SetTextAlignment(alignment))?;
    stage(
        "SetParagraphAlignment",
        format.SetParagraphAlignment(paragraph),
    )?;
    stage(
        "SetWordWrapping",
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP),
    )?;
    // Long titles end in "..." instead of being cut mid-glyph.
    let ellipsis = stage(
        "CreateEllipsisTrimmingSign",
        factory.CreateEllipsisTrimmingSign(&format),
    )?;
    stage(
        "SetTrimming",
        format.SetTrimming(
            &DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            },
            &ellipsis,
        ),
    )?;
    Ok(format)
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    format: u8,
    width: u32,
    height: u32,
}

struct TextCache {
    entries: HashMap<TextKey, IDWriteTextLayout>,
    order: VecDeque<TextKey>,
}

impl TextCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    unsafe fn layout(
        &mut self,
        factory: &IDWriteFactory,
        format: &IDWriteTextFormat,
        format_id: FormatId,
        text: &str,
        width: f32,
        height: f32,
    ) -> RenderResult<IDWriteTextLayout> {
        let key = TextKey {
            text: text.to_owned(),
            format: format_id as u8,
            width: (width.ceil() as u32).max(1),
            height: (height.ceil() as u32).max(1),
        };
        if let Some(layout) = self.entries.get(&key) {
            return Ok(layout.clone());
        }

        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout =
            factory.CreateTextLayout(&wide, format, key.width as f32, key.height as f32)?;
        if self.entries.len() >= 96 {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, layout.clone());
        Ok(layout)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GeometryKey {
    width: u32,
    height: u32,
    corner: u32,
    taper: u32,
    top: bool,
}

struct GeometryCache {
    entries: HashMap<GeometryKey, ID2D1PathGeometry1>,
    order: VecDeque<GeometryKey>,
}

impl GeometryCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn get(
        &mut self,
        factory: &ID2D1Factory1,
        width: f32,
        height: f32,
        corner: f32,
        taper: f32,
        taper_top: bool,
    ) -> RenderResult<ID2D1PathGeometry1> {
        let qw = ((width * 4.0).round() as u32).max(1);
        let qh = ((height * 4.0).round() as u32).max(1);
        let qr = ((corner.max(0.0).min(width * 0.5).min(height * 0.5) * 4.0).round()) as u32;
        let qt = (taper.clamp(0.0, 1.0) * 512.0).round() as u32;
        let key = GeometryKey {
            width: qw,
            height: qh,
            corner: qr,
            taper: qt,
            top: taper_top && qt > 0,
        };
        if let Some(geometry) = self.entries.get(&key) {
            return Ok(geometry.clone());
        }
        let geometry = create_squircle(
            factory,
            qw as f32 / 4.0,
            qh as f32 / 4.0,
            qr as f32 / 4.0,
            qt as f32 / 512.0,
            key.top,
        )?;
        // The travel animates through dozens of distinct sizes, so keep enough
        // entries that a dock does not evict every resting pill.
        if self.entries.len() >= 128 {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key);
        self.entries.insert(key, geometry.clone());
        Ok(geometry)
    }
}

/// Bezier handle length for a corner, as a fraction of its radius. A card's
/// corners use a slightly softened (squircle) curve; once the corners meet
/// in a capsule, the ends become true half-circles, so a small pill reads as
/// a proper capsule instead of a rounded box. Blended so a resize never jumps.
fn corner_control(radius: f32, short_side: f32) -> f32 {
    /// Handle length that makes a cubic Bezier follow a quarter circle.
    const CIRCLE: f32 = 0.552_284_8;
    const SQUIRCLE: f32 = 0.48;
    let roundness = if short_side > 0.0 {
        (radius / (short_side * 0.5)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    SQUIRCLE + (CIRCLE - SQUIRCLE) * smoothstep(0.85, 1.0, roundness)
}

/// The island's outline: a squircle that can also draw itself together into a
/// leaf. With `taper` at 0 it is the plain rounded pill. As `taper` approaches
/// 1 every point is pulled toward the centre line in proportion to how far it
/// is from the `top` edge, so the end against that edge stays full and round
/// while the far end narrows into a droplet's tail.
unsafe fn create_squircle(
    factory: &ID2D1Factory1,
    width: f32,
    height: f32,
    corner: f32,
    taper: f32,
    top: bool,
) -> RenderResult<ID2D1PathGeometry1> {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let radius = corner.max(0.0).min(width * 0.5).min(height * 0.5);
    let control = radius * corner_control(radius, width.min(height));
    let warp = |point: Vector2| {
        if taper <= 0.0 {
            return point;
        }
        let reach = if top {
            point.Y / height
        } else {
            (height - point.Y) / height
        }
        .clamp(0.0, 1.0);
        // A gentle power curve: the sides leave the round end tangentially and
        // keep bending all the way to the tail, so the silhouette is a plump
        // droplet instead of a shield with a keel.
        let scale = 1.0 - taper * reach.powf(1.15);
        Vector2 {
            X: width * 0.5 + (point.X - width * 0.5) * scale,
            Y: point.Y,
        }
    };
    let point = |x: f32, y: f32| warp(Vector2 { X: x, Y: y });
    // The sides must be sampled: warping a straight line's endpoints alone
    // would leave a trapezoid instead of a droplet's curve.
    let side = |x: f32, from_y: f32, to_y: f32| {
        let mut points = Vec::with_capacity(12);
        for step in 1..=12 {
            let t = step as f32 / 12.0;
            points.push(point(x, from_y + (to_y - from_y) * t));
        }
        points
    };
    let geometry = factory.CreatePathGeometry()?;
    let sink = geometry.Open()?;
    sink.SetFillMode(D2D1_FILL_MODE_WINDING);
    sink.BeginFigure(point(radius, 0.0), D2D1_FIGURE_BEGIN_FILLED);
    sink.AddLines(&[point(width - radius, 0.0)]);
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: point(width - radius + control, 0.0),
        point2: point(width, radius - control),
        point3: point(width, radius),
    }]);
    sink.AddLines(&side(width, radius, height - radius));
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: point(width, height - radius + control),
        point2: point(width - radius + control, height),
        point3: point(width - radius, height),
    }]);
    sink.AddLines(&[point(radius, height)]);
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: point(radius - control, height),
        point2: point(0.0, height - radius + control),
        point3: point(0.0, height - radius),
    }]);
    sink.AddLines(&side(0.0, height - radius, radius));
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: point(0.0, radius - control),
        point2: point(radius - control, 0.0),
        point3: point(radius, 0.0),
    }]);
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close()?;
    Ok(geometry)
}

fn color(rgb: [u8; 3], alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(rgb[0]) / 255.0,
        g: f32::from(rgb[1]) / 255.0,
        b: f32::from(rgb[2]) / 255.0,
        a: alpha.clamp(0.0, 1.0),
    }
}

/// Pill opacity. Visible targets are opaque; while closing to hidden the
/// droplet melts away as its short side drops below 28 dp.
fn pill_alpha(rect: Rect, target_visible: bool) -> f32 {
    if target_visible {
        return 1.0;
    }
    let t = (rect.width.min(rect.height) / 28.0).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease_out(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    1.0 - (1.0 - value).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_region_respects_rounded_corners() {
        let region = HitRegion {
            main: Some(HitRect {
                rect: Rect::new(0.0, 0.0, 100.0, 40.0),
                corner: 18.0,
            }),
            bubble: None,
        };
        assert!(region.contains(50.0, 20.0));
        assert!(!region.contains(0.5, 0.5));
        assert!(!region.contains(110.0, 20.0));
    }

    #[test]
    fn empty_hit_region_contains_nothing() {
        assert!(!HitRegion::default().contains(0.0, 0.0));
    }

    #[test]
    fn closing_pill_fades_as_it_shrinks() {
        let big = Rect::new(0.0, 0.0, 220.0, 36.0);
        let small = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(pill_alpha(small, true), 1.0);
        assert_eq!(pill_alpha(big, false), 1.0);
        let fading = pill_alpha(small, false);
        assert!(fading > 0.0 && fading < 0.5, "{fading}");
        assert_eq!(pill_alpha(Rect::new(0.0, 0.0, 0.0, 30.0), false), 0.0);
    }

    #[test]
    fn easing_is_clamped() {
        assert_eq!(ease_out(-1.0), 0.0);
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(2.0), 1.0);
    }

    #[test]
    fn anchor_spring_carries_the_pill_between_edges_and_settles() {
        // Created hidden, like the renderer does: everything starts at rest,
        // docked at the top edge.
        let mut animator = Animator::new(LayoutTarget::HIDDEN, 0.0, 0.0);
        assert!(animator.is_settled(0.0), "starts settled");
        assert_eq!(animator.sample(0.0).anchor, 0.0, "starts at the top edge");

        // Show the pill and let the shape finish before testing the travel.
        animator.set_target(LayoutTarget::single(islet_core::island::COMPACT), 0.0);
        assert!(animator.is_settled(2.0));

        // Dock at the bottom.
        animator.set_anchor(false, 2.0);
        let early = animator.sample(2.10).anchor;
        let middle = animator.sample(2.30).anchor;
        let late = animator.sample(2.60).anchor;
        assert!(
            early > 0.0 && early < middle && middle < late,
            "the phase should move steadily: {early} {middle} {late}"
        );
        assert!(
            !animator.is_settled(2.30),
            "mid-travel must not report settled"
        );
        assert!(animator.is_settled(3.2), "travel settles in about a second");
        assert!(
            (animator.sample(3.2).anchor - 1.0).abs() < 0.012,
            "the phase should arrive at the bottom edge"
        );

        // And back to the top, preserving the motion.
        animator.set_anchor(true, 3.2);
        assert!(!animator.is_settled(3.25));
        assert!(animator.is_settled(4.4));
        assert!((animator.sample(4.4).anchor).abs() < 0.012);
    }

    #[test]
    fn the_expanded_cover_follows_the_card_corner_curve() {
        use islet_core::island::MEDIA;
        // Same corner curve family for the card and the cover inset in it:
        // both are cards (not capsules), so both use the squircle handle, and
        // the cover's radius is the card's minus the inset.
        let card = corner_control(MEDIA.corner, MEDIA.height);
        let cover = corner_control(expanded::ART_RADIUS, expanded::ART.height);
        assert!((card - cover).abs() < 1e-6, "{card} vs {cover}");
        assert_eq!(MEDIA.corner - expanded::ART_RADIUS, expanded::ART_INSET);
    }

    #[test]
    fn capsules_have_round_ends_and_cards_keep_soft_corners() {
        // A fully round pill (radius = half its height): true half-circles.
        assert!((corner_control(16.0, 32.0) - 0.552_284_8).abs() < 1e-6);
        // The expanded card's corners keep the squircle.
        assert!((corner_control(38.0, 120.0) - 0.48).abs() < 1e-6);
        // In between it blends, without a jump.
        let mut previous = corner_control(0.0, 32.0);
        for step in 1..=100 {
            let next = corner_control(16.0 * step as f32 / 100.0, 32.0);
            assert!(next >= previous - 1e-6 && next - previous < 0.01);
            previous = next;
        }
    }

    #[test]
    fn opening_hands_over_between_looks_without_overlap() {
        use islet_core::island::{MEDIA, MEDIA_CONDENSED};
        let card = ContentCard::Media(MediaCard {
            title: "Song".to_owned(),
            artist: "Artist".to_owned(),
            source: "Spotify".to_owned(),
            playing: true,
            timeline: None,
            art: None,
            accent: None,
            scrub: None,
            volume: None,
        });
        // At either end the card is simply drawn in that look.
        assert!(morph_of(&card, COMPACT.height, COMPACT, 1.0).is_none());
        assert!(morph_of(&card, MEDIA.height, COMPACT, 1.0).is_none());
        // Overshoot past either end is still that end.
        assert!(morph_of(&card, MEDIA.height + 4.0, COMPACT, 1.0).is_none());
        // In between it grows from whichever small size it rested at.
        let halfway = (MEDIA_CONDENSED.height + MEDIA.height) * 0.5;
        let morph = morph_of(&card, halfway, MEDIA_CONDENSED, 1.0).unwrap();
        assert_eq!(morph.small, MEDIA_CONDENSED);
        assert_eq!(morph.big, MEDIA);
        assert!((morph.open - 0.5).abs() < 1e-6);

        // Custom sizing must finish the shared-element animation at every
        // supported expanded size, especially 80% (96 physical dp, 120 local).
        for scale in [0.8, 1.0, 1.25] {
            assert!(morph_of(&card, MEDIA.height * scale, MEDIA_CONDENSED, scale).is_none());
            let mid = morph_of(&card, halfway * scale, MEDIA_CONDENSED, scale).unwrap();
            assert!((mid.open - 0.5).abs() < 1e-6);
        }

        // The looks take turns: never both on screen at once, small first out.
        for step in 0..=100 {
            let (small, big) = morph_fades(step as f32 / 100.0);
            assert!(small.min(big) < 1e-6, "both visible at {step}");
        }
        assert_eq!(morph_fades(0.0), (1.0, 0.0));
        assert_eq!(morph_fades(1.0), (0.0, 1.0));

        // The art is centred in the pill's round end: the same margin from
        // the end as from the top and bottom, at both sizes.
        let (rest, radius) = compact_art(COMPACT);
        assert_eq!((rest.x, rest.y, rest.width), (4.0, 4.0, 28.0));
        assert_eq!(radius, 14.0);
        assert_eq!(rest.x + rest.width * 0.5, COMPACT.height * 0.5);
        let (shrunk, _) = compact_art(MEDIA_CONDENSED);
        assert_eq!((shrunk.x, shrunk.y, shrunk.width), (4.0, 4.0, 24.0));
        // The expanded cover keeps the same gap, its corners following the
        // card's.
        assert_eq!((expanded::ART.x, expanded::ART.y), (4.0, 4.0));
        assert_eq!(expanded::ART_RADIUS, MEDIA.corner - 4.0);
    }

    #[test]
    fn a_new_look_waits_for_the_hidden_midpoint_of_a_trip() {
        use islet_core::island::{DictationCard, COMPACT, DICTATION};
        let config = RendererConfig {
            hwnd: HWND(std::ptr::null_mut()),
            pixel_width: 520,
            pixel_height: 1040,
            dpi: 96.0,
            logical_width: 520.0,
            logical_height: 1040.0,
            anchor_top: true,
        };
        let mut renderer = Renderer::new(
            config,
            Arc::new(Mutex::new(HitRegion::default())),
            Arc::new(BarLevels::default()),
            0.0,
        )
        .unwrap();
        let music = ContentSnapshot {
            widget_navigation: false,
            primary: None,
            secondary: None,
            accent: [1, 2, 3],
            ..Default::default()
        };
        renderer.set_snapshot(LayoutTarget::single(COMPACT), music, 0.0);
        assert!(renderer.animator.is_settled(2.0));

        // Dictation starts: travel to the bottom, with a new look.
        let dictation = ContentSnapshot {
            widget_navigation: false,
            primary: Some(ContentCard::Dictation(DictationCard::default())),
            secondary: None,
            accent: [255, 255, 255],
            ..Default::default()
        };
        renderer.set_anchor(false, 2.0);
        renderer.set_snapshot(LayoutTarget::single(DICTATION), dictation.clone(), 2.0);
        // The departing droplet keeps the music pill's shape.
        renderer.apply_deferred(2.2);
        assert_eq!(renderer.target.main, COMPACT);
        assert_eq!(renderer.animator.sample(2.2).main_width, COMPACT.width);

        // Frame by frame, as the render loop does: past the midpoint it comes
        // out of the bottom as the dictation pill, already at its size.
        for frame in 0..=54 {
            renderer.apply_deferred(2.2 + f64::from(frame) / 180.0);
        }
        assert_eq!(renderer.target.main, DICTATION);
        assert_eq!(renderer.content, dictation);
        assert!(renderer.previous_content.is_none());
        assert_eq!(renderer.animator.sample(2.5).main_width, DICTATION.width);
    }

    fn shown_animator(top: bool) -> Animator {
        let mut animator = Animator::new(LayoutTarget::HIDDEN, if top { 0.0 } else { 1.0 }, 0.0);
        animator.set_target(LayoutTarget::single(islet_core::island::COMPACT), 0.0);
        animator
    }

    fn geometry_of(animator: &Animator, now: f64) -> FrameGeometry {
        droplet_geometry(&animator.sample(now), 440.0, 1080.0)
    }

    #[test]
    fn appearing_wells_out_of_its_edge_as_a_droplet() {
        let compact = islet_core::island::COMPACT;
        for top in [true, false] {
            let animator = shown_animator(top);
            // It is already the pill's width class from the first frame: the
            // motion is the droplet, not a zero-size pill growing.
            assert_eq!(animator.sample(0.0).main_width, compact.width);
            let first = geometry_of(&animator, 0.0);
            assert_eq!(first.shell_alpha, 0.0, "starts invisible");

            let early = geometry_of(&animator, 0.08);
            assert!(early.taper > 0.5, "starts as a leaf: {early:?}");
            assert_eq!(early.taper_top, top);
            // Flush with its own edge while it comes through.
            if top {
                assert!(early.main.y.abs() < 1e-3, "{early:?}");
            } else {
                assert!((early.main.bottom() - 1080.0).abs() < 1e-3, "{early:?}");
            }

            // It grows steadily into its size and lands without a wobble past it.
            let mut previous = 0.0_f32;
            for step in 0..90 {
                let width = geometry_of(&animator, f64::from(step) / 180.0).main.width;
                assert!(width >= previous - 1e-3, "never shrinks back: {step}");
                assert!(width <= compact.width + 1e-3, "no overshoot: {width}");
                previous = width;
            }
            assert!(animator.is_settled(1.4));
            let rest = geometry_of(&animator, 1.4);
            assert!((rest.main.width - compact.width).abs() < 0.5);
            assert_eq!(rest.taper, 0.0);
        }
    }

    #[test]
    fn hiding_keeps_the_shape_and_draws_it_back_into_the_edge() {
        let compact = islet_core::island::COMPACT;
        for top in [true, false] {
            let mut animator = shown_animator(top);
            animator.set_target(LayoutTarget::HIDDEN, 2.0);
            // The size springs do not collapse; only the droplet does.
            assert_eq!(animator.sample(2.3).main_width, compact.width);
            let mut previous_height = f32::MAX;
            for step in 0..40 {
                let geometry = geometry_of(&animator, 2.0 + f64::from(step) * 0.02);
                assert!(
                    geometry.main.height <= previous_height + 1e-3,
                    "only ever drawn in at step {step}"
                );
                assert_eq!(geometry.taper_top, top);
                previous_height = geometry.main.height;
            }
            assert_eq!(geometry_of(&animator, 2.6).shell_alpha, 0.0, "gone");
            assert!(animator.is_settled(3.2));
        }
    }

    #[test]
    fn an_interrupted_hide_reverses_without_a_jump() {
        let mut animator = shown_animator(true);
        animator.set_target(LayoutTarget::HIDDEN, 2.0);
        let before = geometry_of(&animator, 2.12);
        animator.set_target(LayoutTarget::single(islet_core::island::COMPACT), 2.12);
        let after = geometry_of(&animator, 2.12);
        assert!((before.main.width - after.main.width).abs() < 1e-3);
        assert!((before.main.height - after.main.height).abs() < 1e-3);
        assert!(animator.is_settled(3.6));
        assert_eq!(geometry_of(&animator, 3.6).taper, 0.0);
    }

    #[test]
    fn a_resize_while_visible_is_still_a_morph() {
        let mut animator = shown_animator(true);
        animator.set_target(LayoutTarget::single(islet_core::island::DICTATION), 2.0);
        // Halfway through, it is between the two sizes and fully present.
        let mid = geometry_of(&animator, 2.12);
        assert!(mid.main.width < islet_core::island::COMPACT.width);
        assert!(mid.main.width > islet_core::island::DICTATION.width);
        assert_eq!(mid.shell_alpha, 1.0);
    }

    fn sample_at(anchor: f32) -> Sample {
        Sample {
            content_scale: 1.0,
            main_width: 220.0,
            main_height: 36.0,
            main_corner: 18.0,
            bubble_width: 0.0,
            bubble_height: 0.0,
            bubble_corner: 0.0,
            gap: 0.0,
            inset: 8.0,
            anchor,
            presence: 1.0,
        }
    }

    fn geometry_at(anchor: f32) -> FrameGeometry {
        droplet_geometry(&sample_at(anchor), 440.0, 1080.0)
    }

    #[test]
    fn the_pill_is_untouched_at_both_edges() {
        let top = geometry_at(0.0);
        assert_eq!(top.main, Rect::new(110.0, 8.0, 220.0, 36.0));
        assert_eq!(top.main_corner, 18.0);
        assert_eq!(top.taper, 0.0);
        assert_eq!(top.shell_alpha, 1.0);
        assert_eq!(top.content_alpha, 1.0);

        let bottom = geometry_at(1.0);
        assert_eq!(bottom.main, Rect::new(110.0, 1036.0, 220.0, 36.0));
        assert_eq!(bottom.main_corner, 18.0);
        assert_eq!(bottom.taper, 0.0);
        assert_eq!(bottom.shell_alpha, 1.0);
        assert_eq!(bottom.content_alpha, 1.0);
    }

    #[test]
    fn half_way_the_island_has_slipped_through_an_edge_invisibly() {
        let half = geometry_at(0.5);
        assert!(half.main.height < 0.5, "{:?}", half.main);
        assert_eq!(half.shell_alpha, 0.0);
        assert_eq!(half.content_alpha, 0.0);
    }

    #[test]
    fn the_absorb_sweeps_into_the_edge_and_never_overshoots_the_screen() {
        let mut previous = f32::MAX;
        for step in 0..=50 {
            let geometry = geometry_at(step as f32 / 100.0);
            // The inner edge only ever moves up toward the top of the window.
            assert!(
                geometry.main.bottom() <= previous + 1e-3,
                "inner edge moved back down at step {step}: {previous} -> {}",
                geometry.main.bottom()
            );
            assert!(geometry.main.y >= -1e-3, "left the window at step {step}");
            assert!(
                geometry.main.width <= 220.0 + 1e-3 && geometry.main.height <= 36.0 + 1e-3,
                "the pill should only shrink while leaving"
            );
            previous = geometry.main.bottom();
        }
    }

    #[test]
    fn the_emerge_grows_out_of_the_bottom_edge() {
        let mut previous = f32::MAX;
        for step in 51..=100 {
            let geometry = geometry_at(step as f32 / 100.0);
            // Growing back up out of the bottom edge: both edges move up.
            assert!(
                geometry.main.y <= previous + 1e-3,
                "top edge moved back down at step {step}: {previous} -> {}",
                geometry.main.y
            );
            assert!(
                geometry.main.bottom() <= 1080.0 + 1e-3,
                "left the window at step {step}"
            );
            previous = geometry.main.y;
        }
    }

    #[test]
    fn both_halves_are_the_same_shape_mirrored() {
        // The exact handoff is on the top half of the curve; compare from one
        // step inside the emerge, where both sides are visible.
        for step in 1..=40 {
            let fullness = step as f32 / 40.0;
            let leaving = geometry_at(0.5 * (1.0 - fullness));
            let arriving = geometry_at(0.5 + 0.5 * fullness);
            assert!((leaving.main.width - arriving.main.width).abs() < 1e-3);
            assert!((leaving.main.height - arriving.main.height).abs() < 1e-3);
            assert!(
                (leaving.main.y - (1080.0 - arriving.main.bottom())).abs() < 1e-3,
                "positions should mirror at fullness {fullness}"
            );
            // The taper is mirrored between the two edges: the same leaf, the
            // round end against a different screen edge.
            assert!((leaving.taper - arriving.taper).abs() < 1e-6);
            assert_ne!(leaving.taper_top, arriving.taper_top);
            assert!((leaving.shell_alpha - arriving.shell_alpha).abs() < 1e-6);
            assert!((leaving.content_alpha - arriving.content_alpha).abs() < 1e-6);
        }
    }

    #[test]
    fn the_travelling_shape_tapers_to_a_leaf_tip() {
        // About halfway into the top edge: it has gathered into a leaf whose
        // tail points away from the edge, before the shell starts to fade.
        let leaf = geometry_at(0.27);
        assert!(leaf.taper_top);
        assert!(leaf.taper > 0.85, "expected a full leaf: {leaf:?}");
        assert!(
            leaf.main.width < 220.0 * 0.45,
            "should have gathered: {leaf:?}"
        );
        assert!(leaf.main.height > 12.0, "should still be a leaf: {leaf:?}");
        // The content is already dissolving while the shell is solid.
        assert!(leaf.content_alpha < 0.5, "{leaf:?}");
        assert_eq!(leaf.shell_alpha, 1.0);
    }

    #[test]
    fn the_pressed_leaf_is_flush_with_the_edge() {
        // While it is being pressed through, the island sits on the screen
        // edge with no gap, as if pulled through a surface.
        let leaving = geometry_at(0.38);
        assert!(leaving.taper_top);
        assert_eq!(leaving.main.y, 0.0, "expected a flush leaf: {leaving:?}");
        assert!(leaving.main.height > 2.0, "{leaving:?}");

        // The arriving half mirrors it against the bottom edge.
        let arriving = geometry_at(0.62);
        assert!(!arriving.taper_top);
        let bottom = arriving.main.y + arriving.main.height;
        assert!((bottom - 1080.0).abs() < 1e-3, "{arriving:?}");
    }
}
