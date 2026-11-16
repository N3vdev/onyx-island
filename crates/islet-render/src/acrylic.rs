//! The glass look's frost: what is behind the pill, softened.
//!
//! The pill is drawn in the DirectComposition swap chain on top (`Gfx`),
//! with a translucent dark tint as its fill. Under it, in the window's
//! bottom composition layer, a sprite shows what is behind the window as
//! DWM blurs it (the host backdrop, which Windows' acrylic uses). Its shape
//! is a second swap chain holding only the pill's silhouette, drawn with
//! every frame of the pill, so the frost follows each animation exactly.
//! The silhouette is at half resolution (a quarter of the memory): the
//! frost is soft anyway, and the pill's sharp edge is drawn above it.
//!
//! The blur itself costs this process nothing: DWM keeps the blurred
//! backdrop for every acrylic surface on screen.
//!
//! Windows.UI.Composition commits on a dispatcher queue thread. This keeps
//! one of its own, which only builds the tree and tears it down; frames are
//! plain presents of the silhouette swap chain from the render thread.

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use windows::core::{h, Interface, GUID, HSTRING, PCWSTR};
use windows_core::implement;
use windows::Foundation::{IPropertyValue, PropertyValue};
use windows::Graphics::Effects::{
    IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectSource_Impl, IGraphicsEffect_Impl,
};
use windows::System::{DispatcherQueueController, DispatcherQueueHandler};
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{CompositionEffectSourceParameter, CompositionStretch, Compositor};
use windows::Win32::Foundation::{E_BOUNDS, E_FAIL, E_INVALIDARG, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_GRADIENT_STOP,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap1, ID2D1Brush, ID2D1DeviceContext, ID2D1LinearGradientBrush, ID2D1PathGeometry1,
    ID2D1RenderTarget, ID2D1StrokeStyle, CLSID_D2D1ColorMatrix, CLSID_D2D1GammaTransfer,
    D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIFactory2, IDXGISwapChain1, DXGI_PRESENT, DXGI_SWAP_CHAIN_FLAG};
use windows::Win32::System::WinRT::Composition::{ICompositorDesktopInterop, ICompositorInterop};
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    IGraphicsEffectD2D1Interop, IGraphicsEffectD2D1Interop_Impl, GRAPHICS_EFFECT_PROPERTY_MAPPING,
};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_STA, DQTYPE_THREAD_DEDICATED,
};
use windows_numerics::Vector2;

use crate::{create_swap_chain, create_target_bitmap, RenderError, RenderResult};

/// The silhouette's resolution, as a fraction of the window's.
pub(crate) const SILHOUETTE_SCALE: f32 = 0.5;

fn scaled(pixels: u32) -> u32 {
    ((pixels as f32 * SILHOUETTE_SCALE).ceil() as u32).max(1)
}

/// Moves COM objects to and from the composition thread, which is the only
/// one that uses them.
struct Carried<T>(T);
// SAFETY: see above; the render thread only holds them.
unsafe impl<T> Send for Carried<T> {}

pub(crate) struct Acrylic {
    hwnd: HWND,
    controller: DispatcherQueueController,
    tree: Option<Carried<(Compositor, DesktopWindowTarget)>>,
    swap_chain: IDXGISwapChain1,
    pub(crate) bitmap: Option<ID2D1Bitmap1>,
}

impl Acrylic {
    pub(crate) unsafe fn new(
        hwnd: HWND,
        factory: &IDXGIFactory2,
        device: &ID3D11Device,
        dc: &ID2D1DeviceContext,
        width: u32,
        height: u32,
        dpi: f32,
    ) -> RenderResult<Self> {
        let on: i32 = 1;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_HOSTBACKDROPBRUSH,
            (&on as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        )?;
        let swap_chain = create_swap_chain(factory, device, scaled(width), scaled(height))?;
        let bitmap = create_target_bitmap(dc, &swap_chain, dpi * SILHOUETTE_SCALE)?;
        let controller = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_DEDICATED,
            apartmentType: DQTAT_COM_STA,
        })?;
        let (tx, rx) = mpsc::channel();
        let chain = Carried(swap_chain.clone());
        let window = hwnd.0 as isize;
        let handler = DispatcherQueueHandler::new(move || {
            let built = build(HWND(window as *mut _), &chain.0).map(Carried);
            let _ = tx.send(built);
            Ok(())
        });
        let mut acrylic = Self {
            hwnd,
            controller,
            tree: None,
            swap_chain,
            bitmap: Some(bitmap),
        };
        let failed = || RenderError::Stage("glass backdrop", E_FAIL.into());
        if !acrylic.controller.DispatcherQueue()?.TryEnqueue(&handler)? {
            return Err(failed());
        }
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(tree)) => acrylic.tree = Some(tree),
            Ok(Err(error)) => return Err(RenderError::Stage("glass backdrop", error)),
            Err(_) => return Err(failed()),
        }
        Ok(acrylic)
    }

    pub(crate) unsafe fn resize(
        &mut self,
        dc: &ID2D1DeviceContext,
        width: u32,
        height: u32,
        dpi: f32,
    ) -> RenderResult<()> {
        self.bitmap = None;
        self.swap_chain.ResizeBuffers(
            2,
            scaled(width),
            scaled(height),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_SWAP_CHAIN_FLAG(0),
        )?;
        self.bitmap = Some(create_target_bitmap(dc, &self.swap_chain, dpi * SILHOUETTE_SCALE)?);
        Ok(())
    }

    pub(crate) unsafe fn present(&self) -> RenderResult<()> {
        self.swap_chain
            .Present(0, DXGI_PRESENT(0))
            .ok()
            .map_err(RenderError::from)
    }
}

impl Drop for Acrylic {
    fn drop(&mut self) {
        // The tree goes on its own thread, before that thread stops.
        if let (Some(tree), Ok(queue)) = (self.tree.take(), self.controller.DispatcherQueue()) {
            let (tx, rx) = mpsc::channel();
            let tree = Mutex::new(Some(tree));
            let handler = DispatcherQueueHandler::new(move || {
                if let Ok(mut tree) = tree.lock() {
                    drop(tree.take());
                }
                let _ = tx.send(());
                Ok(())
            });
            if queue.TryEnqueue(&handler).unwrap_or(false) {
                let _ = rx.recv_timeout(Duration::from_millis(500));
            }
        }
        let _ = self.controller.ShutdownQueueAsync();
        let off: i32 = 0;
        // SAFETY: a plain attribute on our own window.
        unsafe {
            let _ = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_HOSTBACKDROPBRUSH,
                (&off as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}

/// On the composition thread: the blurred backdrop, cut to the silhouette.
fn build(
    hwnd: HWND,
    silhouette: &IDXGISwapChain1,
) -> windows::core::Result<(Compositor, DesktopWindowTarget)> {
    let compositor = Compositor::new()?;
    let desktop: ICompositorDesktopInterop = compositor.cast()?;
    // Not topmost: under the swap chain's DirectComposition target.
    let target = unsafe { desktop.CreateDesktopWindowTarget(hwnd, false)? };
    let interop: ICompositorInterop = compositor.cast()?;
    let surface = unsafe { interop.CreateCompositionSurfaceForSwapChain(silhouette)? };
    let shape = compositor.CreateSurfaceBrushWithSurface(&surface)?;
    shape.SetStretch(CompositionStretch::Fill)?;

    // A mask brush takes no backdrop brush directly, only through an effect.
    let frost = compositor.CreateEffectFactory(&backdrop_effect()?)?.CreateBrush()?;
    frost.SetSourceParameter(h!("backdrop"), &compositor.CreateHostBackdropBrush()?)?;
    let cut = compositor.CreateMaskBrush()?;
    cut.SetSource(&frost)?;
    cut.SetMask(&shape)?;

    let sprite = compositor.CreateSpriteVisual()?;
    sprite.SetBrush(&cut)?;
    sprite.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
    target.SetRoot(&sprite)?;
    Ok((compositor, target))
}

/// The backdrop, more saturated: blurring averages colours toward grey,
/// and under a dark tint that reads as mud. Boosted, a green page stays a
/// deep green, as Apple's materials do it.
fn backdrop_effect() -> windows::core::Result<IGraphicsEffect> {
    let backdrop: IGraphicsEffectSource =
        CompositionEffectSourceParameter::Create(h!("backdrop"))?.cast()?;
    let matrix: IPropertyValue = PropertyValue::CreateSingleArray(&saturation_matrix(SATURATION))?.cast()?;
    let alpha_mode: IPropertyValue =
        PropertyValue::CreateUInt32(D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED.0 as u32)?.cast()?;
    let clamp: IPropertyValue = PropertyValue::CreateBoolean(true)?.cast()?;
    let saturated: IGraphicsEffect = Effect {
        id: CLSID_D2D1ColorMatrix,
        properties: vec![matrix, alpha_mode, clamp],
        sources: vec![backdrop],
    }
    .into();
    tamed(saturated.cast()?)
}

/// Bright backdrops held down, dark ones left nearly as they are. The tint
/// alone darkens everything by the same share, so a white page comes
/// through it mid-grey and the pill goes flat and pale; Windows' own
/// acrylic stays dark over white. This curve (`brightness_curve`) dims
/// white to under half and barely touches the shadows, so the pill stays
/// dark on a light page and see-through on a dark one, colours and all.
fn tamed(source: IGraphicsEffectSource) -> windows::core::Result<IGraphicsEffect> {
    let float = |value: f32| -> windows::core::Result<IPropertyValue> { PropertyValue::CreateSingle(value)?.cast() };
    let flag = |value: bool| -> windows::core::Result<IPropertyValue> { PropertyValue::CreateBoolean(value)?.cast() };
    // Per channel, red, green, blue then alpha: amplitude, exponent,
    // offset, disable. Then clamp the output.
    let mut properties = Vec::with_capacity(17);
    for _ in 0..3 {
        properties.extend([float(CURVE_AMPLITUDE)?, float(CURVE_EXPONENT)?, float(0.0)?, flag(false)?]);
    }
    properties.extend([float(1.0)?, float(1.0)?, float(0.0)?, flag(true)?]);
    properties.push(flag(true)?);
    Ok(Effect {
        id: CLSID_D2D1GammaTransfer,
        properties,
        sources: vec![source],
    }
    .into())
}

/// The brightness curve's shape: `amplitude * value ^ exponent`.
const CURVE_AMPLITUDE: f32 = 0.45;
const CURVE_EXPONENT: f32 = 0.6;

#[cfg(test)]
fn brightness_curve(value: f32) -> f32 {
    (CURVE_AMPLITUDE * value.powf(CURVE_EXPONENT)).clamp(0.0, 1.0)
}

/// How strongly the frost's colours are boosted (1 leaves them).
const SATURATION: f32 = 1.7;

/// Direct2D's 5x4 colour matrix (rows: input red, green, blue, alpha,
/// offset; columns: output red, green, blue, alpha) for a saturation
/// change that keeps each colour's brightness.
fn saturation_matrix(amount: f32) -> [f32; 20] {
    let weights = [0.2126, 0.7152, 0.0722];
    let mut matrix = [0.0; 20];
    for input in 0..3 {
        for output in 0..3 {
            let keep = if input == output { amount } else { 0.0 };
            matrix[input * 4 + output] = weights[input] * (1.0 - amount) + keep;
        }
    }
    matrix[15] = 1.0;
    matrix
}

impl crate::Gfx {
    /// Light on the glass: a sheen down from the top, and a rim brighter
    /// along the top edge, as light catches a pane. In the pill's own
    /// coordinates.
    pub(crate) unsafe fn draw_glass_light(
        &mut self,
        geometry: &ID2D1PathGeometry1,
        height: f32,
        alpha: f32,
    ) -> RenderResult<()> {
        let sheen = gradient(&self.dc, &mut self.glass_sheen, &[(0.0, 0.11), (0.5, 0.025), (1.0, 0.0)])?;
        let rim = gradient(&self.dc, &mut self.glass_rim, &[(0.0, 0.42), (0.45, 0.13), (1.0, 0.07)])?;
        for brush in [&sheen, &rim] {
            brush.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 });
            brush.SetEndPoint(Vector2 { X: 0.0, Y: height });
            brush.SetOpacity(alpha);
        }
        self.dc.FillGeometry(geometry, &sheen, None::<&ID2D1Brush>);
        self.dc.DrawGeometry(geometry, &rim, crate::HAIRLINE_WIDTH, None::<&ID2D1StrokeStyle>);
        Ok(())
    }
}

/// A vertical white gradient with these (position, alpha) stops, made once.
unsafe fn gradient(
    dc: &ID2D1DeviceContext,
    cache: &mut Option<ID2D1LinearGradientBrush>,
    stops: &[(f32, f32)],
) -> RenderResult<ID2D1LinearGradientBrush> {
    if let Some(brush) = cache {
        return Ok(brush.clone());
    }
    let stops: Vec<D2D1_GRADIENT_STOP> = stops
        .iter()
        .map(|&(position, a)| D2D1_GRADIENT_STOP {
            position,
            color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a },
        })
        .collect();
    // The plain render-target overload (the device context's own takes
    // colour spaces).
    let collection = ID2D1RenderTarget::CreateGradientStopCollection(
        dc,
        &stops,
        D2D1_GAMMA_2_2,
        D2D1_EXTEND_MODE_CLAMP,
    )?;
    let brush = dc.CreateLinearGradientBrush(
        &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: Vector2 { X: 0.0, Y: 0.0 },
            endPoint: Vector2 { X: 0.0, Y: 1.0 },
        },
        None,
        &collection,
    )?;
    *cache = Some(brush.clone());
    Ok(brush)
}

/// A Direct2D effect described to Windows.UI.Composition: what Win2D's
/// effect classes do.
#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct Effect {
    id: GUID,
    properties: Vec<IPropertyValue>,
    sources: Vec<IGraphicsEffectSource>,
}

impl IGraphicsEffect_Impl for Effect_Impl {
    fn Name(&self) -> windows::core::Result<HSTRING> {
        Ok(HSTRING::new())
    }

    fn SetName(&self, _name: &HSTRING) -> windows::core::Result<()> {
        Ok(())
    }
}

impl IGraphicsEffectSource_Impl for Effect_Impl {}

impl IGraphicsEffectD2D1Interop_Impl for Effect_Impl {
    fn GetEffectId(&self) -> windows::core::Result<GUID> {
        Ok(self.id)
    }

    fn GetNamedPropertyMapping(
        &self,
        _name: &PCWSTR,
        _index: *mut u32,
        _mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> windows::core::Result<()> {
        // Nothing is animated, so nothing is looked up by name.
        Err(E_INVALIDARG.into())
    }

    fn GetPropertyCount(&self) -> windows::core::Result<u32> {
        Ok(self.properties.len() as u32)
    }

    fn GetProperty(&self, index: u32) -> windows::core::Result<IPropertyValue> {
        self.properties
            .get(index as usize)
            .cloned()
            .ok_or_else(|| E_BOUNDS.into())
    }

    fn GetSource(&self, index: u32) -> windows::core::Result<IGraphicsEffectSource> {
        self.sources
            .get(index as usize)
            .cloned()
            .ok_or_else(|| E_BOUNDS.into())
    }

    fn GetSourceCount(&self) -> windows::core::Result<u32> {
        Ok(self.sources.len() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saturation_matrix_keeps_grey_grey_and_boosts_colour() {
        let m = saturation_matrix(SATURATION);
        let apply = |rgb: [f32; 3]| {
            let mut out = [0.0; 3];
            for (o, value) in out.iter_mut().enumerate() {
                *value = (0..3).map(|i| rgb[i] * m[i * 4 + o]).sum::<f32>();
            }
            out
        };
        let grey = apply([0.4, 0.4, 0.4]);
        assert!(grey.iter().all(|c| (c - 0.4).abs() < 1e-5));
        let green = apply([0.3, 0.6, 0.3]);
        assert!(green[1] - green[0] > 0.3 * SATURATION - 1e-5);
        assert_eq!(saturation_matrix(1.0)[0], 1.0);
        assert_eq!(m[15], 1.0);
    }

    #[test]
    fn the_curve_holds_white_down_and_leaves_the_shadows() {
        // White comes through at under half.
        assert!(brightness_curve(1.0) < 0.5);
        // Dark stays dark: barely lifted.
        assert!((brightness_curve(0.05) - 0.05).abs() < 0.03);
        assert!(brightness_curve(0.0) == 0.0);
        // Still in order, so the picture behind still reads.
        assert!(brightness_curve(0.2) < brightness_curve(0.5) && brightness_curve(0.5) < brightness_curve(1.0));
    }
}
