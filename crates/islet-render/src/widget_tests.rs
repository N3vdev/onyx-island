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
        // The system stats page, to compare with the calculator's size. One
        // renderer per window at a time.
        drop(renderer);
        let mut renderer = calculator_renderer(hwnd, 1.0);
        let stats = islet_core::widgets::SystemStats {
            cpu: 0.42,
            gpu: Some(0.18),
            ram_used: 9_800_000_000,
            ram_total: 16_000_000_000,
            down_bps: 2_400_000.0,
            up_bps: 310_000.0,
            temp_c: Some(54.0),
            cpu_history: (0..40).map(|i| 0.3 + 0.2 * ((i as f32) * 0.4).sin()).collect(),
            ..Default::default()
        };
        let state = IslandState {
            stats: Some(stats),
            widget: Some(islet_core::island::WidgetKind::Stats),
            expanded: true,
            ..Default::default()
        };
        let target = state.layout();
        renderer.set_snapshot(target, state.content(), 0.0);
        renderer.animator.jump_to(target, 0.0);
        renderer.render(5.0, true).unwrap();
        renderer.render(8.0, true).unwrap();
        renderer.render(8.1, true).unwrap();
        save_buffer(renderer.gfx.as_ref().unwrap(), &directory.join("stats-100.tga")).unwrap();
        DestroyWindow(hwnd).unwrap();
    }
}

#[test]
#[ignore = "requires Windows GPU; writes authored converter render evidence"]
fn capture_converter_previews() {
    use islet_core::converter::{ConversionOption,ConversionStage,ConverterCard};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW,DestroyWindow,WINDOW_EX_STYLE,WS_POPUP};
    let directory=std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").expect("set capture directory"));
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd=CreateWindowExW(WINDOW_EX_STYLE(0),w!("STATIC"),w!("Onyx converter capture"),WS_POPUP,0,0,720,320,None,None,None,None).unwrap();
        for scale in [0.8,1.0,1.25] {
            for stage in [ConversionStage::Loading,ConversionStage::Ready,ConversionStage::Running,ConversionStage::Complete,ConversionStage::Error] {
                let mut card=ConverterCard::new("Authored sample.pdf".into(),"PDF · Choose a conversion format".into());
                card.stage=stage;card.progress=54;card.dragging=false;card.fully_succeeded=stage==ConversionStage::Complete;
                card.message=match stage {
                    ConversionStage::Loading=>"Finding available formats…",
                    ConversionStage::Ready=>"",
                    ConversionStage::Running=>"Converting to PNG · Original files stay unchanged",
                    ConversionStage::Complete=>"1 result saved beside the original",
                    ConversionStage::Error=>"Windows couldn’t open this PDF. Try another file.",
                }.into();
                if stage == ConversionStage::Complete && std::env::var_os("ISLET_CAPTURE_PARTIAL").is_some() {
                    card.fully_succeeded = false;
                    card.message = "Canceled · 1 completed · Original files unchanged".into();
                }
                card.options=if stage == ConversionStage::Ready { ["PNG","JPG","WEBP","BMP","TIFF","GIF","JFIF","PDF"].as_slice() } else { ["PNG","JPG"].as_slice() }.iter().map(|label|ConversionOption{id:label.to_string(),label:label.to_string()}).collect();
                if stage == ConversionStage::Ready {
                    if let Some(path) = std::env::var_os("ISLET_CONVERTER_CAPTURE_PREVIEW") {
                        let pixels = std::fs::read(path).unwrap();
                        assert_eq!(pixels.len(), 256 * 256 * 4);
                        card.preview = Some(ArtImage { id: 99, width: 256, height: 256, pixels: pixels.into() });
                    }
                }
                let state=IslandState{converter:Some(card),expanded:true,..Default::default()};
                let target=state.layout().customized(scale,0.0,320.0);
                let content=state.content();
                let mut renderer=Renderer::new(RendererConfig{hwnd,pixel_width:720,pixel_height:320,dpi:96.0,logical_width:720.0,logical_height:320.0,anchor_top:true},Arc::new(Mutex::new(HitRegion::default())),Arc::new(BarLevels::default()),0.0).unwrap();
                renderer.set_snapshot(target,content.clone(),0.0);
                renderer.render(5.0,true).unwrap();
                renderer.animator.jump_to(target,8.0);
                let sample=renderer.animator.sample(8.0);
                let geometry=renderer.frame_geometry(&sample);
                let gfx=renderer.gfx.as_mut().unwrap();
                gfx.main_spring_height=sample.main_height;gfx.content_scale=scale;
                gfx.draw(8.0,&geometry,&geometry,target,&content,None,1.0).unwrap();
                save_buffer(gfx,&directory.join(format!("converter-{stage:?}-{}.tga",(scale*100.0)as u32))).unwrap();
            }
        }
        DestroyWindow(hwnd).unwrap();
    }
}

unsafe fn save_buffer(gfx: &Gfx, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    // Read the actual D2D target, not whichever swap-chain buffer is next
    // after a flip. EndDraw has submitted D2D; flush before D3D readback.
    gfx._device_context.Flush();
    let source: ID3D11Texture2D = gfx._bitmap.as_ref().ok_or(RenderError::InvalidSize)?.GetSurface()?.cast()?;
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

#[test]
#[ignore = "requires Windows GPU; writes authored pull render evidence"]
fn capture_pull_previews() {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW,DestroyWindow,WINDOW_EX_STYLE,WS_POPUP};
    let directory = std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0),w!("STATIC"),w!("Pull capture"),WS_POPUP,0,0,720,248,None,None,None,None).unwrap();
        let mut renderer = calculator_renderer(hwnd, 1.0);
        let state = IslandState { media: Some(MediaCard {
            title: "Sample track".into(), artist: "Sample artist".into(), source: "".into(),
            playing: false, timeline: None, art: None, accent: None, scrub: None, volume: None,
        }), ..Default::default() };
        let target = state.layout(); let content = state.content();
        renderer.set_snapshot(target, content.clone(), 0.0);
        renderer.render(5.0, true).unwrap();
        renderer.animator.jump_to(target, 8.0);
        let sample = renderer.animator.sample(8.0);
        let base = renderer.frame_geometry(&sample);
        let gfx = renderer.gfx.as_mut().unwrap();
        gfx.main_spring_height = sample.main_height;
        for (name, x, y) in [("rest",0.0,0.0),("horizontal",60.0,0.0),("vertical",0.0,70.0)] {
            let (shell, content_geometry) = pull_frames(base,x,y);
            gfx.draw(8.0,&shell,&content_geometry,target,&content,None,1.0).unwrap();
            save_buffer(gfx,&directory.join(format!("pull-{name}.tga"))).unwrap();
        }
        DestroyWindow(hwnd).unwrap();
    }
}

#[test]
#[ignore = "requires Windows GPU; writes eye render evidence"]
fn capture_eye_previews() {
    use islet_core::config::IdleGlance;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW,DestroyWindow,WINDOW_EX_STYLE,WS_POPUP};
    let directory = std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0),w!("STATIC"),w!("Eye capture"),WS_POPUP,0,0,720,248,None,None,None,None).unwrap();
        let mut renderer = calculator_renderer(hwnd, 1.0);
        let state = IslandState { compact_widget: Some(IdleGlance::Eyes), ..Default::default() };
        let target = state.layout(); let content = state.content();
        renderer.set_snapshot(target, content.clone(), 0.0);
        renderer.render(5.0, true).unwrap();
        renderer.animator.jump_to(target, 8.0);
        let sample = renderer.animator.sample(8.0);
        let base = renderer.frame_geometry(&sample);
        let gfx = renderer.gfx.as_mut().unwrap();
        gfx.main_spring_height = sample.main_height;
        let hop = |t| Some((eyes::Reaction::Hop, t));
        for (name, x, y, open, reaction) in [
            ("rest", 0.0, 0.0, 1.0, None), ("shut", 0.0, 0.0, 0.0, None),
            ("look-right", 30.0, 0.0, 1.0, None), ("squeeze", 100.0, 0.0, 1.0, None),
            ("lift-half", 0.0, -22.0, 1.0, None), ("lift", 0.0, -50.0, 1.0, None),
            ("strain", 0.0, 4.0, 1.0, None), ("hop", 0.0, 0.0, 1.0, hop(0.15)),
            ("pop", 0.0, 0.0, 1.0, Some((eyes::Reaction::Pop, 0.1))),
        ] {
            let (shell, mut content_geometry) = pull_frames(base, x, f32::abs(y));
            content_geometry.content_alpha = base.content_alpha;
            // The capture island sits at the top: lifting is pulling down.
            let lift = -y;
            let pose = eyes::EyePose::new(x, y, lift, open, 0.01);
            gfx.eyes = match reaction { Some((r, t)) => pose.react(r, t), None => pose };
            gfx.draw(8.0,&shell,&content_geometry,target,&content,None,1.0).unwrap();
            save_buffer(gfx,&directory.join(format!("eyes-{name}.tga"))).unwrap();
        }
        DestroyWindow(hwnd).unwrap();
    }
}

#[test]
#[ignore = "requires Windows GPU; writes eye morph render evidence"]
fn capture_eye_morphs() {
    use islet_core::config::IdleGlance;
    use islet_core::island::DictationCard;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW,DestroyWindow,WINDOW_EX_STYLE,WS_POPUP};
    let directory = std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0),w!("STATIC"),w!("Eye morph capture"),WS_POPUP,0,0,720,248,None,None,None,None).unwrap();
        let mut renderer = calculator_renderer(hwnd, 1.0);
        let state = IslandState { compact_widget: Some(IdleGlance::Eyes), ..Default::default() };
        let target = state.layout(); let content = state.content();
        renderer.set_snapshot(target, content.clone(), 0.0);
        renderer.render(5.0, true).unwrap();
        renderer.animator.jump_to(target, 8.0);
        let sample = renderer.animator.sample(8.0);
        let base = renderer.frame_geometry(&sample);
        let gfx = renderer.gfx.as_mut().unwrap();
        gfx.main_spring_height = sample.main_height;
        // A sideways pull growing, and a lift growing (no trembling: t = 0).
        let mut frames: Vec<(String, f32, f32)> = [40.0, 56.0, 64.0, 72.0, 80.0, 88.0, 100.0]
            .iter().map(|x| (format!("squeeze-{x}"), *x, 0.0)).collect();
        frames.extend([6.0, 14.0, 20.0, 24.0, 28.0, 34.0, 44.0].iter().map(|y| (format!("lift-{y}"), 0.0, *y)));
        for (name, x, lift) in frames {
            let (shell, mut content_geometry) = pull_frames(base, x, lift);
            content_geometry.content_alpha = base.content_alpha;
            gfx.eyes = eyes::EyePose::new(x, -lift, lift, 1.0, 0.0);
            gfx.draw(8.0,&shell,&content_geometry,target,&content,None,1.0).unwrap();
            save_buffer(gfx,&directory.join(format!("morph-{name}.tga"))).unwrap();
        }
        // A right wink closing, and a left one shut.
        for wink in [0.25f32, 0.5, 0.75, 1.0, -1.0] {
            gfx.eyes = eyes::EyePose::REST.wink(wink);
            gfx.draw(8.0,&base,&base,target,&content,None,1.0).unwrap();
            save_buffer(gfx,&directory.join(format!("wink-{}.tga", (wink * 100.0) as i32))).unwrap();
        }
        // Eyes into dictation bars, partway, with a voice on the mic.
        gfx.eyes = eyes::EyePose::REST;
        let dictation = IslandState {
            dictation: Some(DictationCard { bars: [200; islet_core::island::DICTATION_BARS], ..Default::default() }),
            ..Default::default()
        };
        let speaking = dictation.content();
        let mut now = 20.0;
        for step in 0..60 { now += 1.0 / 120.0; gfx.voice_bars.step(now, [0.7; voice::BARS]); let _ = step; }
        for amount in [0.0f32, 0.2, 0.4, 0.6, 0.8, 1.0] {
            // The real path: a content transition from the eyes to dictation.
            gfx.draw(now,&base,&base,target,&speaking,Some(&content),amount).unwrap();
            save_buffer(gfx,&directory.join(format!("bars-{}.tga", (amount * 100.0) as u32))).unwrap();
        }
        DestroyWindow(hwnd).unwrap();
    }
}

/// Opt-in native evidence: the AI dictation offer and its download.
#[test]
#[ignore = "requires Windows GPU; writes notice render evidence"]
fn capture_notice_previews() {
    use islet_core::island::{AlertLevel, NoticeAction, NotificationCard};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};
    let directory = std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    let card = |title: &str, body: &str, level, action, progress| NotificationCard {
        source: String::new(),
        title: title.to_owned(),
        body: body.to_owned(),
        level,
        icon: None,
        action,
        progress,
    };
    let offer = Some(NoticeAction::DownloadAiCleanup);
    let cards = [
        ("offer", card("Use AI dictation", "Improves accuracy \u{b7} 290 MB", AlertLevel::Info, offer, None)),
        ("progress", card("Getting AI dictation", "117 of 290 MB \u{b7} 40%", AlertLevel::Info, None, Some(0.4))),
        ("failed", card("Download failed", "download failed: connection reset", AlertLevel::Error, offer, None)),
        ("ready", card("AI dictation is ready", "Your next dictation is cleaned up on this PC.", AlertLevel::Success, None, None)),
    ];
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Islet Notice Capture"), WS_POPUP, 0, 0, 720, 248, None, None, None, None).unwrap();
        for (name, notice) in cards {
            for expanded in [false, true] {
                let mut renderer = calculator_renderer(hwnd, 1.0);
                let state = IslandState { done: Some(notice.clone()), expanded, ..Default::default() };
                let target = state.layout();
                renderer.set_snapshot(target, state.content(), 0.0);
                renderer.animator.jump_to(target, 0.0);
                renderer.render(5.0, true).unwrap();
                renderer.render(8.0, true).unwrap();
                renderer.render(8.1, true).unwrap();
                let size = if expanded { "open" } else { "pill" };
                save_buffer(renderer.gfx.as_ref().unwrap(), &directory.join(format!("notice-{name}-{size}.tga"))).unwrap();
            }
        }
        DestroyWindow(hwnd).unwrap();
    }
}

/// Opt-in native evidence: the eyes as the night light comes on.
#[test]
#[ignore = "requires Windows GPU; writes night eye render evidence"]
fn capture_night_eyes() {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};
    let directory = std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Islet Night Capture"), WS_POPUP, 0, 0, 720, 248, None, None, None, None).unwrap();
        use islet_core::island::NightGreeting;
        let times: [(NightGreeting, &str, &[f64]); 2] = [
            (NightGreeting::Evening, "evening", &[1.2, 1.4, 2.0, 2.4, 3.0, 3.6, 5.0]),
            (NightGreeting::Morning, "morning", &[1.6, 2.0, 2.6, 3.0, 3.3, 3.6, 4.1]),
        ];
        for (greeting, name, frames) in times {
            let mut renderer = calculator_renderer(hwnd, 1.0);
            let state = IslandState { night_greeting: Some(greeting), ..Default::default() };
            let target = state.layout();
            renderer.set_snapshot(target, state.content(), 10.0);
            renderer.animator.jump_to(target, 10.0);
            renderer.previous_content = None;
            // The reaction starts on the first frame the eyes are shown.
            renderer.render(10.0, true).unwrap();
            for t in frames {
                renderer.render(10.0 + t, true).unwrap();
                save_buffer(renderer.gfx.as_ref().unwrap(), &directory.join(format!("{name}-eyes-{:02}.tga", (t * 10.0) as u32))).unwrap();
            }
            drop(renderer);
        }
        DestroyWindow(hwnd).unwrap();
    }
}

/// Opt-in native evidence: resting eyes with the night light on and off.
#[test]
#[ignore = "requires Windows GPU; writes night eye render evidence"]
fn capture_resting_night_eyes() {
    use islet_core::config::IdleGlance;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};
    let directory = std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Islet Resting Eyes"), WS_POPUP, 0, 0, 720, 248, None, None, None, None).unwrap();
        for (name, on) in [("day", false), ("night", true)] {
            let mut renderer = calculator_renderer(hwnd, 1.0);
            let mut state = IslandState { glance: Some(IdleGlance::Eyes), ..Default::default() };
            state.glance_info.night_light = on;
            let target = state.layout();
            renderer.set_snapshot(target, state.content(), 10.0);
            renderer.animator.jump_to(target, 10.0);
            renderer.previous_content = None;
            for step in 0..=40 {
                renderer.render(10.0 + f64::from(step) * 0.1, true).unwrap();
            }
            save_buffer(renderer.gfx.as_ref().unwrap(), &directory.join(format!("resting-{name}.tga"))).unwrap();
            drop(renderer);
        }
        DestroyWindow(hwnd).unwrap();
    }
}

/// Opt-in native evidence: the eyes pill attached to the left screen side
/// at the top, and to the right side at the bottom.
#[test]
#[ignore = "requires Windows GPU; writes corner eye render evidence"]
fn capture_corner_eyes() {
    use islet_core::config::{IdleGlance, IslandCorner};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP};
    let directory = std::path::PathBuf::from(std::env::var_os("ISLET_WIDGET_CAPTURE_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    unsafe {
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Islet Corner Eyes"), WS_POPUP, 0, 0, 720, 248, None, None, None, None).unwrap();
        for (name, corner, top) in [("left-top", IslandCorner::Left, true), ("right-bottom", IslandCorner::Right, false)] {
            let mut renderer = calculator_renderer(hwnd, 1.0);
            renderer.set_anchor(top, 0.0);
            renderer.animator = Animator::new(LayoutTarget::HIDDEN, if top { 0.0 } else { 1.0 }, 10.0);
            let state = IslandState { glance: Some(IdleGlance::Eyes), corner, ..Default::default() };
            let target = state.layout();
            renderer.set_snapshot(target, state.content(), 10.0);
            renderer.animator.jump_to(target, 10.0);
            renderer.previous_content = None;
            for step in 0..=40 {
                renderer.render(10.0 + f64::from(step) * 0.1, true).unwrap();
            }
            save_buffer(renderer.gfx.as_ref().unwrap(), &directory.join(format!("corner-{name}.tga"))).unwrap();
            // Brought back to the middle: a happy hop once grown in.
            let middle = IslandState { glance: Some(IdleGlance::Eyes), ..Default::default() };
            let target = middle.layout();
            renderer.set_snapshot(target, middle.content(), 15.0);
            renderer.animator = Animator::new(target, if top { 0.0 } else { 1.0 }, 15.0);
            renderer.animator.jump_to(target, 15.0);
            renderer.previous_content = None;
            for step in 0..=12 {
                renderer.render(15.0 + (CHEER_DELAY + 0.2) * f64::from(step) / 12.0, true).unwrap();
            }
            save_buffer(renderer.gfx.as_ref().unwrap(), &directory.join(format!("back-{name}.tga"))).unwrap();
            drop(renderer);
        }
        DestroyWindow(hwnd).unwrap();
    }
}
