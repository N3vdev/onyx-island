//! Direct3D 11 / Direct2D / DirectComposition renderer for the Islet pill.
//!
//! This crate is deliberately unsafe-heavy and Windows-only. The platform-free
//! state lives in `islet-core`; the render thread calls [`Renderer::render`]
//! with monotonic seconds and receives a hit region it can publish to the UI
//! thread.

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use islet_core::geometry::{point_in_rounded_rect, Rect};
use islet_core::island::{
    arrange, AgentCard, AgentState, AlertLevel, ContentCard, ContentSnapshot, DictationCard,
    LayoutTarget, MediaCard, NotificationCard, PillSize,
};
use islet_core::media::{expanded, format_clock, ArtImage, BarLevels, BAR_COUNT};
use islet_core::{Spring, SpringParams};
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F,
    D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_FILL_MODE_WINDING, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1GaussianBlur, D2D1CreateDevice, ID2D1Bitmap1, ID2D1BitmapBrush1, ID2D1Brush,
    ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory1, ID2D1Image, ID2D1PathGeometry1,
    ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_BITMAP_BRUSH_PROPERTIES1, D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_INTERPOLATION_MODE_LINEAR,
    D2D1_PROPERTY_TYPE_FLOAT, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
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
    DWRITE_FONT_WEIGHT_DEMI_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGIOutput, IDXGISurface, IDXGISwapChain1,
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows_numerics::{Matrix3x2, Vector2};

/// Pure black, as on iOS (PLAN §4.2).
const FRAME_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};
const HAIRLINE_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.075,
};
/// Real visualizer levels older than this fall back to procedural bars.
const BAR_LEVELS_MAX_AGE_MS: u64 = 250;
/// Album-art brushes kept around (current track plus a transition partner).
const ART_CACHE_SIZE: usize = 3;
const DEFAULT_ACCENT_TEXT: [u8; 3] = [146, 153, 172];

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

pub struct Renderer {
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
    anchor_top: bool,
    logical_width: f32,
    logical_height: f32,
    pixel_width: u32,
    pixel_height: u32,
    dpi: f32,
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
        let animator = Animator::new(LayoutTarget::HIDDEN, now);
        Ok(Self {
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
            anchor_top: config.anchor_top,
            logical_width: config.logical_width,
            logical_height: config.logical_height,
            pixel_width: config.pixel_width,
            pixel_height: config.pixel_height,
            dpi: config.dpi,
        })
    }

    /// Applies a UI-thread snapshot. Safe to call with the same snapshot.
    pub fn set_snapshot(&mut self, target: LayoutTarget, content: ContentSnapshot, now: f64) {
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
        if self.target.visible || !self.content.is_empty() {
            self.hidden_since = None;
        } else if self.hidden_since.is_none() {
            self.hidden_since = Some(now);
        }
    }

    pub fn set_anchor(&mut self, anchor_top: bool) {
        self.anchor_top = anchor_top;
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

    #[must_use]
    pub fn needs_frame(&self, now: f64) -> bool {
        !self.animator.is_settled(now)
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
        !self.animator.is_settled(now) || self.transition_progress(now).is_some()
    }

    #[must_use]
    pub fn is_settled(&self, now: f64) -> bool {
        !self.needs_frame(now)
    }

    pub fn render(&mut self, now: f64) -> RenderResult<()> {
        if self.gfx.is_none()
            && !self.target.visible
            && self.content.is_empty()
            && self.previous_content.is_none()
            && self.animator.is_settled(now)
        {
            self.clear_hit_region();
            return Ok(());
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
            self.animator = Animator::new(LayoutTarget::HIDDEN, now);
            self.animator.set_target(self.target, now);
            if self.transition_start.is_some() {
                self.transition_start = Some(now);
            }
        }

        let sample = self.animator.sample(now);
        let progress = self.transition_progress(now).unwrap_or(1.0);
        let previous = if progress < 1.0 {
            self.previous_content.as_ref()
        } else {
            None
        };

        let geometry = self.frame_geometry(&sample);
        // SAFETY: the renderer owns all graphics state and is only used from
        // the render thread that created it.
        unsafe {
            let gfx = self.gfx.as_mut().ok_or(RenderError::InvalidSize)?;
            gfx.bar_levels = self
                .bars
                .load_fresh((now * 1000.0) as u64, BAR_LEVELS_MAX_AGE_MS);
            gfx.draw(
                now,
                &geometry,
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
        Ok(())
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
        let frame = arrange(
            PillSize::new(sample.main_width, sample.main_height, sample.main_corner),
            Some(PillSize::new(
                sample.bubble_width,
                sample.bubble_height,
                sample.bubble_corner,
            )),
            sample.gap,
            sample.inset,
            self.logical_width,
            self.logical_height,
            self.anchor_top,
        );
        FrameGeometry {
            main: frame.main,
            main_corner: frame.main_corner,
            bubble: frame.bubble,
            bubble_corner: frame.bubble_corner,
        }
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
    main_width: f32,
    main_height: f32,
    main_corner: f32,
    bubble_width: f32,
    bubble_height: f32,
    bubble_corner: f32,
    gap: f32,
    inset: f32,
}

#[derive(Clone, Copy, Debug)]
enum TransportIcon {
    Previous,
    PlayPause(bool),
    Next,
}

struct Animator {
    main_width: Spring,
    main_height: Spring,
    main_corner: Spring,
    bubble_width: Spring,
    bubble_height: Spring,
    bubble_corner: Spring,
    gap: Spring,
    inset: Spring,
}

impl Animator {
    fn new(target: LayoutTarget, now: f64) -> Self {
        let mut animator = Self {
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
        };
        animator.set_target(target, now);
        animator
    }

    fn set_target(&mut self, target: LayoutTarget, now: f64) {
        let old_area = self.main_width.target() * self.main_height.target();
        let new_area = f64::from(target.main.width) * f64::from(target.main.height);
        let params = if new_area >= old_area {
            SpringParams::EXPAND
        } else {
            SpringParams::COLLAPSE
        };
        // Closing to hidden is its own fluid motion (see DISMISS_*).
        let (width_params, height_params) = if target.visible {
            (params, params)
        } else {
            (SpringParams::DISMISS_WIDTH, SpringParams::DISMISS_HEIGHT)
        };

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

    fn sample(&self, now: f64) -> Sample {
        Sample {
            main_width: self.main_width.position(now).max(0.0) as f32,
            main_height: self.main_height.position(now).max(0.0) as f32,
            main_corner: self.main_corner.position(now).max(0.0) as f32,
            bubble_width: self.bubble_width.position(now).max(0.0) as f32,
            bubble_height: self.bubble_height.position(now).max(0.0) as f32,
            bubble_corner: self.bubble_corner.position(now).max(0.0) as f32,
            gap: self.gap.position(now).max(0.0) as f32,
            inset: self.inset.position(now).max(0.0) as f32,
        }
    }

    fn is_settled(&self, now: f64) -> bool {
        self.main_width.is_settled(now)
            && self.main_height.is_settled(now)
            && self.main_corner.is_settled(now)
            && self.bubble_width.is_settled(now)
            && self.bubble_height.is_settled(now)
            && self.bubble_corner.is_settled(now)
            && self.gap.is_settled(now)
            && self.inset.is_settled(now)
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

#[derive(Clone, Copy, Debug)]
struct FrameGeometry {
    main: Rect,
    main_corner: f32,
    bubble: Option<Rect>,
    bubble_corner: f32,
}

struct Gfx {
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
    art: Vec<(u64, ID2D1BitmapBrush1)>,
    bar_levels: Option<[f32; BAR_COUNT]>,
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
            art: Vec::with_capacity(ART_CACHE_SIZE),
            bar_levels: None,
            pixel_width: config.pixel_width,
            pixel_height: config.pixel_height,
            dpi: config.dpi,
        })
    }

    unsafe fn resize(&mut self, width: u32, height: u32, dpi: f32) -> RenderResult<()> {
        // ResizeBuffers fails while any reference to a back buffer remains.
        // Clear the D2D target and release our bitmap first.
        self.content_layers = None;
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
        target: LayoutTarget,
        content: &ContentSnapshot,
        previous: Option<&ContentSnapshot>,
        progress: f32,
    ) -> RenderResult<()> {
        let transitioning = previous.is_some() && progress < 1.0;
        self.dc.BeginDraw();
        self.dc.SetTransform(&Matrix3x2::identity());
        self.dc.Clear(Some(&TRANSPARENT));

        if geometry.main.width >= 0.5 && geometry.main.height >= 0.5 {
            self.draw_pill(
                geometry.main,
                geometry.main_corner,
                pill_alpha(geometry.main, target.visible),
            )?;
            if let Some(bubble) = geometry.bubble {
                if bubble.width >= 0.5 && bubble.height >= 0.5 {
                    self.draw_pill(
                        bubble,
                        geometry.bubble_corner,
                        pill_alpha(bubble, target.visible),
                    )?;
                }
            }

            if !transitioning {
                self.draw_snapshot(now, geometry, content, 1.0, 1.0)?;
            }
        }

        self.dc.EndDraw(None, None)?;

        if transitioning {
            self.draw_transition(now, geometry, previous, content, progress)?;
        }
        Ok(())
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
        self.render_content_layer(
            &old_bitmap,
            now,
            geometry,
            previous,
            1.0 - old_ease,
            1.0 - 0.055 * old_ease,
        )?;
        self.render_content_layer(
            &new_bitmap,
            now,
            geometry,
            content,
            new_ease,
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

    unsafe fn draw_pill(&mut self, rect: Rect, corner: f32, alpha: f32) -> RenderResult<()> {
        if alpha <= 0.001 {
            return Ok(());
        }
        let geometry = self.geometries.get(
            &self.factory,
            rect.width,
            rect.height,
            corner.min(rect.width * 0.5).min(rect.height * 0.5),
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
        self.dc
            .DrawGeometry(&geometry, &self.brush, 0.75, None::<&ID2D1StrokeStyle>);
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
        self.dc
            .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let center = Vector2 {
            X: rect.width * 0.5,
            Y: rect.height * 0.5,
        };
        let transform =
            Matrix3x2::scale_around(scale, scale, center) * Matrix3x2::translation(rect.x, rect.y);
        self.dc.SetTransform(&transform);
        self.draw_card(now, rect.width, rect.height, corner, card, accent, alpha)?;
        self.dc.SetTransform(&Matrix3x2::identity());
        self.dc.PopAxisAlignedClip();
        Ok(())
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
        if width < 60.0 && height < 60.0 {
            return self.draw_bubble(now, width, height, corner, card, accent, alpha);
        }
        if height < 31.0 && !matches!(card, ContentCard::Dictation(_)) {
            return self.draw_tucked(now, width, height, card, accent, alpha);
        }
        match card {
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
        }
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
        let art = 24.0;
        let art_y = (height - art) * 0.5;
        self.draw_art(
            Rect::new(8.0, art_y, art, art),
            6.0,
            media.art.as_ref(),
            accent,
            alpha,
        )?;

        let text_w = (width - 104.0).max(10.0);
        self.draw_text(
            &media.title,
            FormatId::CompactTitle,
            40.0,
            3.0,
            text_w,
            15.0,
            color([255, 255, 255], 0.96 * alpha),
        )?;
        self.draw_text(
            &media.artist,
            FormatId::CompactSub,
            40.0,
            19.0,
            text_w,
            13.0,
            color([173, 180, 199], 0.88 * alpha),
        )?;

        let start = width - 46.0 - 10.0;
        let mid = height * 0.5;
        for index in 0..BAR_COUNT {
            let level = self.bar_level(now, index, media.playing);
            let bar_h = 3.0 + level * 15.0;
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
            ContentCard::Dictation(_) => return Ok(()),
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

    /// Bar height 0..1: real loopback levels when fresh, otherwise a gentle
    /// procedural wave while playing, and a low rest when paused.
    fn bar_level(&self, now: f64, index: usize, playing: bool) -> f32 {
        if !playing {
            return 0.12;
        }
        if let Some(levels) = self.bar_levels {
            return levels[index % BAR_COUNT].clamp(0.0, 1.0);
        }
        let phase = now * 7.2 + index as f64 * 0.85;
        (phase.sin() * 0.5 + 0.5) as f32 * 0.65 + 0.35
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
        self.draw_art(
            expanded::ART,
            expanded::ART_RADIUS,
            media.art.as_ref(),
            accent,
            alpha,
        )?;

        let text_x = expanded::TEXT_X;
        let text_w = (width - text_x - 20.0).max(40.0);
        self.draw_text(
            &media.title,
            FormatId::Title,
            text_x,
            20.0,
            text_w,
            24.0,
            color([255, 255, 255], 0.98 * alpha),
        )?;
        self.draw_text(
            &media.artist,
            FormatId::Sub,
            text_x,
            45.0,
            text_w,
            18.0,
            color([177, 185, 204], 0.92 * alpha),
        )?;
        self.draw_text(
            &media.source.to_uppercase(),
            FormatId::Tiny,
            text_x,
            66.0,
            text_w,
            14.0,
            color(
                if media.accent.is_some() {
                    accent
                } else {
                    DEFAULT_ACCENT_TEXT
                },
                0.95 * alpha,
            ),
        )?;

        if let Some(timeline) = media.timeline.filter(|timeline| timeline.duration > 0.0) {
            let bar = expanded::scrubber(width);
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
                color(accent, 0.98 * alpha),
            )?;
            if media.scrub.is_some() {
                self.fill_circle(
                    bar.x + bar.width * progress,
                    bar.y + bar.height * 0.5,
                    6.0,
                    color([255, 255, 255], alpha),
                )?;
            }
            let label = color([146, 153, 172], 0.9 * alpha);
            self.draw_text(
                &format_clock(position),
                FormatId::Tiny,
                bar.x,
                bar.bottom() + 5.0,
                60.0,
                12.0,
                label,
            )?;
            self.draw_text(
                &format!("-{}", format_clock(timeline.duration - position)),
                FormatId::TinyRight,
                bar.right() - 60.0,
                bar.bottom() + 5.0,
                60.0,
                12.0,
                label,
            )?;
        }

        let [previous, play, next] = expanded::controls(width);
        self.draw_transport(
            previous.0,
            previous.1,
            TransportIcon::Previous,
            accent,
            alpha,
        )?;
        self.draw_transport(
            play.0,
            play.1,
            TransportIcon::PlayPause(media.playing),
            accent,
            alpha,
        )?;
        self.draw_transport(next.0, next.1, TransportIcon::Next, accent, alpha)?;
        Ok(())
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
        let rounded = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: rect.x,
                top: rect.y,
                right: rect.right(),
                bottom: rect.bottom(),
            },
            radiusX: radius,
            radiusY: radius,
        };
        if let Some(art) = art {
            let brush = self.art_brush(art)?;
            brush.SetOpacity(alpha.clamp(0.0, 1.0));
            brush.SetTransform(
                &(Matrix3x2::scale(
                    rect.width / art.width.max(1) as f32,
                    rect.height / art.height.max(1) as f32,
                ) * Matrix3x2::translation(rect.x, rect.y)),
            );
            self.dc.FillRoundedRectangle(&rounded, &brush);
            return Ok(());
        }
        self.fill_round_rect(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            radius,
            color(accent, 0.55 * alpha),
        )?;
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
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        let _ = accent;
        match icon {
            TransportIcon::Previous => {
                self.set_color(color([220, 225, 238], 0.72 * alpha));
                self.dc.DrawLine(
                    Vector2 {
                        X: x + 5.0,
                        Y: y - 7.0,
                    },
                    Vector2 { X: x - 2.0, Y: y },
                    &self.brush,
                    2.0,
                    None,
                );
                self.dc.DrawLine(
                    Vector2 { X: x - 2.0, Y: y },
                    Vector2 {
                        X: x + 5.0,
                        Y: y + 7.0,
                    },
                    &self.brush,
                    2.0,
                    None,
                );
                self.fill_round_rect(
                    x + 7.0,
                    y - 7.0,
                    2.0,
                    14.0,
                    1.0,
                    color([220, 225, 238], 0.72 * alpha),
                )?;
            }
            TransportIcon::Next => {
                self.set_color(color([220, 225, 238], 0.72 * alpha));
                self.dc.DrawLine(
                    Vector2 {
                        X: x - 5.0,
                        Y: y - 7.0,
                    },
                    Vector2 { X: x + 2.0, Y: y },
                    &self.brush,
                    2.0,
                    None,
                );
                self.dc.DrawLine(
                    Vector2 { X: x + 2.0, Y: y },
                    Vector2 {
                        X: x - 5.0,
                        Y: y + 7.0,
                    },
                    &self.brush,
                    2.0,
                    None,
                );
                self.fill_round_rect(
                    x - 9.0,
                    y - 7.0,
                    2.0,
                    14.0,
                    1.0,
                    color([220, 225, 238], 0.72 * alpha),
                )?;
            }
            TransportIcon::PlayPause(playing) => {
                self.fill_circle(x, y, 14.0, color([255, 255, 255], 0.12 * alpha))?;
                if playing {
                    self.fill_round_rect(
                        x - 4.0,
                        y - 6.0,
                        3.0,
                        12.0,
                        1.5,
                        color([255, 255, 255], 0.9 * alpha),
                    )?;
                    self.fill_round_rect(
                        x + 1.0,
                        y - 6.0,
                        3.0,
                        12.0,
                        1.5,
                        color([255, 255, 255], 0.9 * alpha),
                    )?;
                } else {
                    self.set_color(color([255, 255, 255], 0.9 * alpha));
                    self.dc.DrawLine(
                        Vector2 {
                            X: x - 4.0,
                            Y: y - 6.0,
                        },
                        Vector2 { X: x + 6.0, Y: y },
                        &self.brush,
                        2.0,
                        None,
                    );
                    self.dc.DrawLine(
                        Vector2 { X: x + 6.0, Y: y },
                        Vector2 {
                            X: x - 4.0,
                            Y: y + 6.0,
                        },
                        &self.brush,
                        2.0,
                        None,
                    );
                    self.dc.DrawLine(
                        Vector2 {
                            X: x - 4.0,
                            Y: y + 6.0,
                        },
                        Vector2 {
                            X: x - 4.0,
                            Y: y - 6.0,
                        },
                        &self.brush,
                        2.0,
                        None,
                    );
                }
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
        self.fill_round_rect(16.0, icon_y, icon, icon, 12.0, color(accent, 0.18 * alpha))?;
        let initial = notification
            .source
            .chars()
            .next()
            .unwrap_or('I')
            .to_uppercase()
            .next()
            .unwrap_or('I');
        self.draw_text(
            &initial.to_string(),
            FormatId::Bubble,
            16.0,
            icon_y,
            icon,
            icon,
            color(accent, 0.98 * alpha),
        )?;

        let text_x = 64.0;
        let text_w = (width - text_x - 24.0).max(40.0);
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
            26.0,
            text_w,
            22.0,
            color([255, 255, 255], 0.98 * alpha),
        )?;
        self.draw_text(
            &notification.body,
            FormatId::Sub,
            text_x,
            48.0,
            text_w,
            17.0,
            color([177, 185, 204], 0.92 * alpha),
        )?;

        self.draw_alert_dot(width - 19.0, 19.0, notification.level, accent, alpha)
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
        self.fill_round_rect(8.0, icon_y, icon, icon, 8.0, color(accent, 0.18 * alpha))?;
        let initial = notification
            .source
            .chars()
            .next()
            .unwrap_or('I')
            .to_uppercase()
            .next()
            .unwrap_or('I');
        self.draw_text(
            &initial.to_string(),
            FormatId::Bubble,
            8.0,
            icon_y,
            icon,
            icon,
            color(accent, 0.98 * alpha),
        )?;

        let text_x = 40.0;
        let text_w = (width - text_x - 28.0).max(30.0);
        self.draw_text(
            &notification.title,
            FormatId::CompactTitle,
            text_x,
            2.0,
            text_w,
            16.0,
            color([255, 255, 255], 0.96 * alpha),
        )?;
        self.draw_text(
            &notification.source,
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
            },
            accent,
            alpha,
        )
    }

    unsafe fn draw_dictation(
        &mut self,
        now: f64,
        width: f32,
        height: f32,
        dictation: &DictationCard,
        accent: [u8; 3],
        alpha: f32,
    ) -> RenderResult<()> {
        self.fill_circle(17.0, height * 0.5, 4.5, color(accent, alpha))?;
        let bars = 14;
        let start = 31.0;
        let spacing = 9.0;
        let mid = height * 0.5;
        for index in 0..bars {
            let phase = now * 9.0 + f64::from(index) * 0.7;
            let wave = ((phase.sin() * 0.5 + 0.5) as f32) * 0.75 + 0.25;
            let bar_h = 4.0 + wave * (height - 15.0) * dictation.level.clamp(0.0, 1.0);
            self.fill_round_rect(
                start + index as f32 * spacing,
                mid - bar_h * 0.5,
                3.0,
                bar_h,
                1.5,
                color(accent, 0.92 * alpha),
            )?;
        }
        let timer = format_elapsed(dictation.elapsed_ms);
        self.draw_text(
            &timer,
            FormatId::Timer,
            width - 62.0,
            8.0,
            48.0,
            height - 16.0,
            color([255, 255, 255], 0.9 * alpha),
        )?;
        let _ = dictation.phase;
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
                    AlertLevel::Success => "✓",
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
            ContentCard::Media(_) | ContentCard::Dictation(_) => {
                self.fill_circle(center_x, center_y, 5.0, color(accent, alpha))?;
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
    CompactTitle,
    CompactSub,
    Title,
    Sub,
    Tiny,
    TinyRight,
    Tucked,
    Bubble,
    Timer,
}

struct TextFormats {
    compact_title: IDWriteTextFormat,
    compact_sub: IDWriteTextFormat,
    title: IDWriteTextFormat,
    sub: IDWriteTextFormat,
    tiny: IDWriteTextFormat,
    tiny_right: IDWriteTextFormat,
    tucked: IDWriteTextFormat,
    bubble: IDWriteTextFormat,
    timer: IDWriteTextFormat,
}

impl TextFormats {
    unsafe fn new(factory: &IDWriteFactory) -> RenderResult<Self> {
        Ok(Self {
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
            timer: create_format(
                factory,
                12.0,
                DWRITE_FONT_WEIGHT_DEMI_BOLD,
                DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            )?,
        })
    }

    fn get(&self, id: FormatId) -> &IDWriteTextFormat {
        match id {
            FormatId::CompactTitle => &self.compact_title,
            FormatId::CompactSub => &self.compact_sub,
            FormatId::Title => &self.title,
            FormatId::Sub => &self.sub,
            FormatId::Tiny => &self.tiny,
            FormatId::TinyRight => &self.tiny_right,
            FormatId::Tucked => &self.tucked,
            FormatId::Bubble => &self.bubble,
            FormatId::Timer => &self.timer,
        }
    }
}

unsafe fn create_format(
    factory: &IDWriteFactory,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    alignment: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
    paragraph: windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT,
) -> RenderResult<IDWriteTextFormat> {
    let family: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
    let format = stage(
        "CreateTextFormat",
        factory.CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None::<&IDWriteFontCollection>,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        ),
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

struct GeometryCache {
    entries: HashMap<(u32, u32, u32), ID2D1PathGeometry1>,
    order: VecDeque<(u32, u32, u32)>,
}

impl GeometryCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    unsafe fn get(
        &mut self,
        factory: &ID2D1Factory1,
        width: f32,
        height: f32,
        corner: f32,
    ) -> RenderResult<ID2D1PathGeometry1> {
        let qw = ((width * 4.0).round() as u32).max(1);
        let qh = ((height * 4.0).round() as u32).max(1);
        let qr = (corner * 4.0).round() as u32;
        let key = (qw, qh, qr);
        if let Some(geometry) = self.entries.get(&key) {
            return Ok(geometry.clone());
        }
        let geometry = create_squircle(factory, qw as f32 / 4.0, qh as f32 / 4.0, qr as f32 / 4.0)?;
        if self.entries.len() >= 64 {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key);
        self.entries.insert(key, geometry.clone());
        Ok(geometry)
    }
}

unsafe fn create_squircle(
    factory: &ID2D1Factory1,
    width: f32,
    height: f32,
    corner: f32,
) -> RenderResult<ID2D1PathGeometry1> {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let radius = corner.max(0.0).min(width * 0.5).min(height * 0.5);
    let control = radius * 0.48;
    let geometry = factory.CreatePathGeometry()?;
    let sink = geometry.Open()?;
    sink.SetFillMode(D2D1_FILL_MODE_WINDING);
    sink.BeginFigure(Vector2 { X: radius, Y: 0.0 }, D2D1_FIGURE_BEGIN_FILLED);
    sink.AddLines(&[Vector2 {
        X: width - radius,
        Y: 0.0,
    }]);
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: Vector2 {
            X: width - radius + control,
            Y: 0.0,
        },
        point2: Vector2 {
            X: width,
            Y: radius - control,
        },
        point3: Vector2 {
            X: width,
            Y: radius,
        },
    }]);
    sink.AddLines(&[Vector2 {
        X: width,
        Y: height - radius,
    }]);
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: Vector2 {
            X: width,
            Y: height - radius + control,
        },
        point2: Vector2 {
            X: width - radius + control,
            Y: height,
        },
        point3: Vector2 {
            X: width - radius,
            Y: height,
        },
    }]);
    sink.AddLines(&[Vector2 {
        X: radius,
        Y: height,
    }]);
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: Vector2 {
            X: radius - control,
            Y: height,
        },
        point2: Vector2 {
            X: 0.0,
            Y: height - radius + control,
        },
        point3: Vector2 {
            X: 0.0,
            Y: height - radius,
        },
    }]);
    sink.AddLines(&[Vector2 { X: 0.0, Y: radius }]);
    sink.AddBeziers(&[D2D1_BEZIER_SEGMENT {
        point1: Vector2 {
            X: 0.0,
            Y: radius - control,
        },
        point2: Vector2 {
            X: radius - control,
            Y: 0.0,
        },
        point3: Vector2 { X: radius, Y: 0.0 },
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

fn format_elapsed(milliseconds: u32) -> String {
    let seconds = milliseconds / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
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
    fn formatting_helpers_are_stable() {
        assert_eq!(format_elapsed(7_400), "0:07");
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
}
