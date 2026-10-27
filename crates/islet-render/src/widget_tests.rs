use super::*;
use islet_core::island::{IslandState, CALCULATOR};
use islet_core::widgets::{Calculator, CalculatorKey};

fn calculator_renderer(hwnd: HWND, scale: f32) -> Renderer {
    let mut renderer = Renderer::new(
        RendererConfig {
            hwnd,
            pixel_width: 720,
            pixel_height: 248,
            dpi: 96.0,
            logical_width: 720.0,
            logical_height: 248.0,
            anchor_top: true,
        },
        Arc::new(Mutex::new(HitRegion::default())),
        Arc::new(BarLevels::default()),
        0.0,
    )
    .unwrap();
    let mut calc = Calculator::default();
    for key in [
        CalculatorKey::Digit(2),
        CalculatorKey::Add,
        CalculatorKey::Digit(3),
        CalculatorKey::Multiply,
        CalculatorKey::Digit(4),
        CalculatorKey::Equals,
    ] {
        calc.press(key);
    }
    let state = IslandState {
        calculator: Some(calc),
        expanded: true,
        ..Default::default()
    };
    let target = LayoutTarget::single(CALCULATOR).customized(scale, 0.0, 248.0);
    renderer.set_snapshot(target, state.content(), 0.0);
    renderer.animator.jump_to(target, 0.0);
    renderer
}

#[test]
fn calculator_parks_after_transition_at_every_size() {
    for scale in [0.8, 1.0, 1.25] {
        let mut renderer = calculator_renderer(HWND(std::ptr::null_mut()), scale);
        // Rendering normally retires previous_content at the transition's end.
        renderer.previous_content = None;
        assert!(!renderer.needs_frame(5.0));
        assert!(!renderer.content.animates());
        assert!(!renderer.set_snapshot(renderer.target, renderer.content.clone(), 6.0));
        assert!(!renderer.needs_frame(6.0));
    }
}

/// Opt-in native evidence capture; this code is absent from shipping binaries.
#[test]
#[ignore = "requires Windows GPU; writes native render evidence"]
fn capture_widget_previews() {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP,
    };
    let directory = std::path::PathBuf::from(
        std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").expect("set capture directory"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("Islet Widget Capture"),
            WS_POPUP,
            0,
            0,
            720,
            248,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        for (name, scale) in [
            ("calculator-80", 0.8),
            ("calculator-100", 1.0),
            ("calculator-125", 1.25),
        ] {
            let mut renderer = calculator_renderer(hwnd, scale);
            renderer.render(5.0, true).unwrap();
            renderer.render(8.0, true).unwrap();
            renderer.render(8.1, true).unwrap();
            save_buffer(
                renderer.gfx.as_ref().unwrap(),
                &directory.join(format!("{name}.tga")),
            )
            .unwrap();
        }
        // Also capture navigation in the current media layout.
        let mut renderer = calculator_renderer(hwnd, 1.0);
        let mut state = IslandState {
            calculator: Some(Calculator::default()),
            expanded: true,
            ..Default::default()
        };
        state.media = Some(MediaCard {
            title: "Midnight City".into(),
            artist: "M83".into(),
            source: "Spotify".into(),
            playing: false,
            timeline: None,
            art: None,
            accent: None,
            scrub: None,
            volume: Some(0.65),
        });
        let target = state.layout();
        renderer.set_snapshot(target, state.content(), 0.0);
        renderer.animator.jump_to(target, 0.0);
        renderer.render(5.0, true).unwrap();
        renderer.render(8.0, true).unwrap();
        renderer.render(8.1, true).unwrap();
        save_buffer(
            renderer.gfx.as_ref().unwrap(),
            &directory.join("widget-navigation.tga"),
        )
        .unwrap();
        DestroyWindow(hwnd).unwrap();
    }
}

unsafe fn save_buffer(gfx: &Gfx, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    let source: ID3D11Texture2D = gfx.swap_chain.GetBuffer(0)?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    source.GetDesc(&mut desc);
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    let mut staging = None;
    gfx._device
        .CreateTexture2D(&desc, None, Some(&mut staging))?;
    let staging = staging.ok_or(RenderError::InvalidSize)?;
    gfx._device_context.CopyResource(&staging, &source);
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    gfx._device_context
        .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    let result = (|| -> std::io::Result<()> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut header = [0u8; 18];
        header[2] = 2;
        header[12..14].copy_from_slice(&(desc.Width as u16).to_le_bytes());
        header[14..16].copy_from_slice(&(desc.Height as u16).to_le_bytes());
        header[16] = 32;
        header[17] = 0x28;
        file.write_all(&header)?;
        for row in 0..desc.Height as usize {
            file.write_all(std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                desc.Width as usize * 4,
            ))?;
        }
        file.flush()
    })();
    gfx._device_context.Unmap(&staging, 0);
    result?;
    Ok(())
}
