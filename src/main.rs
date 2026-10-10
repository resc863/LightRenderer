#![windows_subsystem = "windows"]
mod agility;
mod camera;
mod capabilities;
mod detail;
mod renderer;
mod streaming;
mod ultimate;

use anyhow::{Context, Result};
use camera::Camera;
use glam::{Mat4, Vec3};
use light_renderer::{
    physics_demo::{DEMO_BODY_COUNT, DEMO_FLUID_PARTICLES, PhysicsDemo},
    scene,
};
use renderer::{Frame, Renderer};
use std::{cell::RefCell, path::PathBuf, time::Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

#[derive(Default)]
struct Input {
    keys: Vec<bool>,
    actions: Vec<u32>,
    wheel: f32,
    click: bool,
    focus_lost: bool,
}
thread_local! { static INPUT: RefCell<Input> = RefCell::new(Input { keys:vec![false;256], ..Default::default() }); }
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut p = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut p);
                let _ = EndPaint(hwnd, &p);
            }
            LRESULT(0)
        }
        WM_SYSKEYDOWN if wp.0 == 0x73 => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            INPUT.with(|i| {
                let mut i = i.borrow_mut();
                let k = wp.0 as u32;
                if k < 256 {
                    i.keys[k as usize] = true;
                }
                if lp.0 & (1 << 30) == 0 {
                    i.actions.push(k);
                }
            });
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            INPUT.with(|i| {
                if wp.0 < 256 {
                    i.borrow_mut().keys[wp.0] = false;
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            INPUT.with(|i| i.borrow_mut().click = true);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            INPUT.with(|i| i.borrow_mut().wheel += (wp.0 >> 16) as u16 as i16 as f32 / 120.0);
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            INPUT.with(|i| {
                let mut i = i.borrow_mut();
                i.keys.fill(false);
                i.focus_lost = true;
            });
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            unsafe {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT { x: 800, y: 600 };
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

struct Options {
    backend: String,
    vrs: bool,
    streaming: bool,
    error: f32,
    texture_budget: u32,
    frames: Option<u64>,
    capture: Option<PathBuf>,
    report: Option<PathBuf>,
    software: bool,
    debug: bool,
    scripted: bool,
    width: u32,
    height: u32,
    physics: bool,
    fluid: bool,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut o = Self {
            backend: "auto".into(),
            vrs: true,
            streaming: true,
            error: 0.65,
            texture_budget: 16,
            frames: None,
            capture: None,
            report: None,
            software: false,
            debug: cfg!(debug_assertions),
            scripted: false,
            width: 1440,
            height: 900,
            physics: true,
            fluid: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--frames" => {
                    o.frames = Some(args.next().context("--frames needs a count")?.parse()?)
                }
                "--capture" => {
                    o.capture = Some(args.next().context("--capture needs a .bmp path")?.into())
                }
                "--report" => {
                    o.report = Some(args.next().context("--report needs a JSON path")?.into())
                }
                "--software" => o.software = true,
                "--no-physics" => o.physics = false,
                "--fluid" => o.fluid = true,
                "--renderer" => {
                    o.backend = args
                        .next()
                        .context("--renderer needs auto, ultimate or raytracing")?
                }
                "--no-vrs" => o.vrs = false,
                "--no-streaming" => o.streaming = false,
                "--detail-error" => {
                    o.error = args
                        .next()
                        .context("--detail-error needs a pixel value")?
                        .parse()?
                }
                "--texture-budget-mb" => {
                    o.texture_budget = args
                        .next()
                        .context("--texture-budget-mb needs 4 to 256")?
                        .parse()?
                }
                "--debug" => o.debug = true,
                "--scripted-input" => o.scripted = true,
                "--width" => o.width = args.next().context("--width needs a number")?.parse()?,
                "--height" => o.height = args.next().context("--height needs a number")?.parse()?,
                _ => anyhow::bail!("Unknown argument: {a}"),
            }
        }
        anyhow::ensure!(
            o.width >= 800 && o.height >= 500 && o.width <= 3840 && o.height <= 2160,
            "Window size must be 800x500 to 3840x2160"
        );
        anyhow::ensure!(o.frames != Some(0), "--frames must be positive");
        anyhow::ensure!(
            ["auto", "ultimate", "raytracing"].contains(&o.backend.as_str()),
            "Invalid renderer"
        );
        anyhow::ensure!(
            o.error.is_finite() && (0.1..=8.).contains(&o.error),
            "Detail error must be 0.1 to 8 pixels"
        );
        anyhow::ensure!(
            (4..=256).contains(&o.texture_budget),
            "Texture budget must be 4 to 256 MB"
        );
        anyhow::ensure!(
            !o.fluid || o.physics,
            "--fluid requires physics; remove --no-physics"
        );
        Ok(o)
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn mouse_lock(hwnd: HWND, enabled: bool) -> Result<()> {
    unsafe {
        if enabled {
            let mut r = RECT::default();
            GetClientRect(hwnd, &mut r)?;
            let mut a = POINT {
                x: r.left,
                y: r.top,
            };
            let mut b = POINT {
                x: r.right,
                y: r.bottom,
            };
            let _ = ClientToScreen(hwnd, &mut a);
            let _ = ClientToScreen(hwnd, &mut b);
            ClipCursor(Some(&RECT {
                left: a.x,
                top: a.y,
                right: b.x,
                bottom: b.y,
            }))?;
            SetCursorPos((a.x + b.x) / 2, (a.y + b.y) / 2)?;
            ShowCursor(false);
        } else {
            ClipCursor(None)?;
            ShowCursor(true);
        }
        Ok(())
    }
}
fn mouse_delta(hwnd: HWND) -> Result<(f32, f32)> {
    unsafe {
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r)?;
        let mut center = POINT {
            x: r.right / 2,
            y: r.bottom / 2,
        };
        let _ = ClientToScreen(hwnd, &mut center);
        let mut p = POINT::default();
        GetCursorPos(&mut p)?;
        SetCursorPos(center.x, center.y)?;
        Ok(((p.x - center.x) as f32, (p.y - center.y) as f32))
    }
}
fn fullscreen(hwnd: HWND, on: bool, saved: &mut RECT) -> Result<()> {
    unsafe {
        if on {
            GetWindowRect(hwnd, saved)?;
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            anyhow::ensure!(
                GetMonitorInfoW(monitor, &mut info).as_bool(),
                "Cannot get monitor bounds"
            );
            SetWindowLongPtrW(hwnd, GWL_STYLE, WS_POPUP.0 as isize | WS_VISIBLE.0 as isize);
            let r = info.rcMonitor;
            SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_FRAMECHANGED,
            )?;
        } else {
            SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                (WS_OVERLAPPEDWINDOW | WS_VISIBLE).0 as isize,
            );
            SetWindowPos(
                hwnd,
                None,
                saved.left,
                saved.top,
                saved.right - saved.left,
                saved.bottom - saved.top,
                SWP_FRAMECHANGED,
            )?;
        }
        Ok(())
    }
}
struct MouseGuard(bool);
impl Drop for MouseGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                let _ = ClipCursor(None);
                ShowCursor(true);
            }
        }
    }
}

fn hud(
    renderer: &Renderer,
    fps: f32,
    cam: &Camera,
    settings: &[f32; 4],
    paused: bool,
    help: bool,
    physics: Option<&PhysicsDemo>,
) -> (Vec<[f32; 4]>, usize) {
    let width = renderer.width;
    let height = renderer.height;
    let hardware = renderer.hardware;
    let mode = if cam.orbit {
        "ORBIT"
    } else if cam.walking {
        "WALK"
    } else {
        "FLY"
    };
    let backend = if renderer.ultimate.as_ref().is_some_and(|u| u.enabled) {
        "DX12 ULTIMATE"
    } else if hardware {
        "DXR 1.1"
    } else {
        "COMPUTE RT"
    };
    let mut lines = vec![
        (
            "LIGHT / SAKURA GARDEN".to_string(),
            36.,
            25.,
            0.82,
            [0.99, 0.93, 0.87],
        ),
        (
            format!(
                "{backend}  /  {fps:.0} FPS  /  {mode}  /  {}X AA",
                renderer.samples
            ),
            36.,
            60.,
            0.55,
            [0.64, 0.83, 0.88],
        ),
        (
            format!(
                "{} BANDS   |   INK {}   SHADOW {}   MIRROR {}{}",
                settings[3] as u32,
                if settings[0] > 0.5 { "ON" } else { "OFF" },
                if settings[1] > 0.5 { "ON" } else { "OFF" },
                if settings[2] > 0.5 { "ON" } else { "OFF" },
                if paused { "   PAUSED" } else { "" }
            ),
            36.,
            84.,
            0.42,
            [0.77, 0.80, 0.85],
        ),
    ];
    if let Some(u) = &renderer.ultimate
        && u.enabled
    {
        lines.push((
            format!(
                "{} TRI / {:.2}px / VT {:.1}MB / F8 VRS {} / F9 STREAM {} / +/- DETAIL / F7 PATH",
                u.triangles,
                u.detail_error,
                u.streaming.resident_bytes as f32 / 1048576.,
                if u.vrs_enabled { "ON" } else { "OFF" },
                if u.streaming.feedback_enabled {
                    "ON"
                } else {
                    "OFF"
                }
            ),
            36.,
            108.,
            0.39,
            [0.63, 0.89, 0.72],
        ));
    }
    if let Some(p) = physics {
        let s = p.world.stats();
        lines.push((
            format!(
                "PHYSICS 120 HZ / {} ACTIVE / {} SLEEP / {:.2}ms / F4 RESET / F5 KICK",
                s.active_bodies, s.sleeping_bodies, s.last_step_ms
            ),
            36.,
            132.,
            0.42,
            [0.95, 0.72, 0.45],
        ));
        if let Some(fluid) = p.fluid() {
            let s = fluid.stats();
            lines.push((
                format!(
                    "FLUID / {} PARTICLES / {:.2}ms / COMPRESSION {:.1}% / F5 SPLASH",
                    fluid.particles().len(),
                    s.last_step_ms,
                    s.max_compression_after * 100.
                ),
                36.,
                154.,
                0.42,
                [0.40, 0.86, 0.96],
            ));
        }
    }
    if help {
        let y = height as f32 - 110.;
        lines.extend([
            (
                "EXPLORE THE GARDEN".to_string(),
                36.,
                y,
                0.55,
                [0.99, 0.65, 0.67],
            ),
            (
                "WASD Move   Mouse Look   Shift Sprint   Q/E Rise/Fall   G Walk/Fly   Space Jump"
                    .to_string(),
                36.,
                y + 30.,
                0.44,
                [0.87, 0.90, 0.94],
            ),
            (
                "Click Capture   Esc Release   Tab Orbit   Wheel Zoom   F2 Portrait   R Reset"
                    .to_string(),
                36.,
                y + 51.,
                0.44,
                [0.87, 0.90, 0.94],
            ),
            (
                "1 Ink   2 Shadows   3 Mirrors   B Bands   P Pause   H Help   F6 AA   F11 Fullscreen"
                    .to_string(),
                36.,
                y + 72.,
                0.44,
                [0.69, 0.77, 0.85],
            ),
        ]);
    }
    let n = lines.len();
    let mut data = vec![[0.; 4]; n * 2];
    for (i, (s, x, y, scale, color)) in lines.into_iter().enumerate() {
        let max_chars = ((width as f32 - x - 24.) / (20. * scale)).max(0.) as usize;
        let chars: Vec<_> = s.bytes().take(max_chars).collect();
        data[i * 2] = [x, y, scale, chars.len() as f32];
        data[i * 2 + 1] = [data.len() as f32, color[0], color[1], color[2]];
        data.extend(chars.into_iter().map(|c| [c as f32, 0., 0., 0.]));
    }
    (data, n)
}
fn run() -> Result<()> {
    let options = Options::parse()?;
    let mut physics = if options.physics {
        Some(if options.fluid {
            PhysicsDemo::with_fluid()?
        } else {
            PhysicsDemo::new()?
        })
    } else {
        None
    };
    let primitive_count = scene::garden(0.).len()
        + if options.physics { DEMO_BODY_COUNT } else { 0 }
        + if options.fluid {
            DEMO_FLUID_PARTICLES
        } else {
            0
        };
    let hwnd = unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let module = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("LightRendererGarden"),
            ..Default::default()
        };
        anyhow::ensure!(
            RegisterClassW(&class) != 0,
            "Window class registration failed"
        );
        let mut rect = RECT {
            right: options.width as i32,
            bottom: options.height as i32,
            ..Default::default()
        };
        AdjustWindowRectEx(&mut rect, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0))?;
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class.lpszClassName,
            w!("LightRenderer | Sakura Garden"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(module.into()),
            None,
        )?
    };
    let mut renderer = Renderer::new(
        hwnd,
        options.width,
        options.height,
        options.software,
        options.debug,
        options.texture_budget,
        primitive_count,
    )?;
    anyhow::ensure!(
        options.backend != "ultimate" || renderer.ultimate.is_some(),
        "DX12 Ultimate capabilities are unavailable on this device"
    );
    if let Some(u) = &mut renderer.ultimate {
        u.enabled = options.backend != "raytracing";
        u.vrs_enabled = options.vrs;
        u.streaming.feedback_enabled = options.streaming;
        u.detail_error = options.error;
    }
    let mut camera = Camera::default();
    let initial_camera = camera.position;
    let mut mouse = MouseGuard(false);
    let mut settings = [1., 1., 1., 3.];
    let mut paused = false;
    let mut help = true;
    let mut is_fullscreen = false;
    let mut window_rect = RECT::default();
    let mut elapsed = 0f32;
    let mut last = Instant::now();
    let start = last;
    let mut frames = 0u64;
    let mut fps = 60f32;
    let mut capture_request = false;
    let mut resized = false;
    let mut physics_pause_exercised = false;
    let mut physics_kick_exercised = false;
    loop {
        let mut quit = false;
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    quit = true;
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if quit {
            break;
        }
        let now = Instant::now();
        let real_dt = (now - last).as_secs_f32();
        last = now;
        let dt = if options.scripted {
            1. / 60.
        } else {
            real_dt.clamp(0.001, 0.05)
        };
        fps = fps * 0.95 + (1. / real_dt.max(0.001)) * 0.05;
        let (mut keys, actions, wheel, click, lost) = INPUT.with(|i| {
            let mut i = i.borrow_mut();
            let k: [bool; 256] = i.keys.as_slice().try_into().unwrap();
            let result = (
                k,
                std::mem::take(&mut i.actions),
                i.wheel,
                i.click,
                i.focus_lost,
            );
            i.wheel = 0.;
            i.click = false;
            i.focus_lost = false;
            result
        });
        if lost && mouse.0 {
            mouse_lock(hwnd, false)?;
            mouse.0 = false;
        }
        if click && !mouse.0 {
            mouse_lock(hwnd, true)?;
            mouse.0 = true;
        }
        let mut jump = false;
        for action in actions {
            match action {
                27 => {
                    if mouse.0 {
                        mouse_lock(hwnd, false)?;
                        mouse.0 = false;
                    }
                }
                9 => {
                    camera.orbit = !camera.orbit;
                    if camera.orbit {
                        camera.walking = false;
                        camera.update_orbit();
                    }
                }
                32 => jump = true,
                49 => settings[0] = 1. - settings[0],
                50 => settings[1] = 1. - settings[1],
                51 => settings[2] = 1. - settings[2],
                66 => {
                    settings[3] = if settings[3] >= 5. {
                        2.
                    } else {
                        settings[3] + 1.
                    }
                }
                80 => paused = !paused,
                72 => help = !help,
                71 => camera.toggle_walk(),
                82 => camera = Camera::default(),
                113 => camera.preset(true),
                114 => camera.preset(false),
                115 => {
                    if let Some(p) = &mut physics {
                        p.reset()?;
                    }
                }
                116 => {
                    if let Some(p) = &mut physics {
                        p.kick()?;
                        physics_kick_exercised = true;
                    }
                }
                117 => renderer.toggle_antialiasing()?,
                118 => {
                    if let Some(u) = &mut renderer.ultimate {
                        u.enabled = !u.enabled;
                    }
                }
                119 => {
                    if let Some(u) = &mut renderer.ultimate {
                        u.vrs_enabled = !u.vrs_enabled;
                    }
                }
                120 => {
                    if let Some(u) = &mut renderer.ultimate {
                        u.streaming.feedback_enabled = !u.streaming.feedback_enabled;
                    }
                }
                187 => {
                    if let Some(u) = &mut renderer.ultimate {
                        u.detail_error = (u.detail_error / 1.25).max(0.1);
                    }
                }
                189 => {
                    if let Some(u) = &mut renderer.ultimate {
                        u.detail_error = (u.detail_error * 1.25).min(8.);
                    }
                }
                122 => {
                    is_fullscreen = !is_fullscreen;
                    fullscreen(hwnd, is_fullscreen, &mut window_rect)?;
                }
                123 => capture_request = true,
                _ => {}
            }
        }
        if mouse.0 {
            let (dx, dy) = mouse_delta(hwnd)?;
            camera.look(dx, dy);
        }
        camera.zoom(wheel);
        if options.scripted {
            if frames == 12 {
                camera.orbit = false;
            }
            if (12..30).contains(&frames) {
                keys[68] = true;
                camera.look(0.8, 0.0);
            }
            if frames == 30 {
                settings[0] = 0.;
            }
            if frames == 35 {
                settings[0] = 1.;
                paused = true;
            }
            if frames == 40 {
                paused = false;
                camera.preset(true);
            }
            if frames == 45 {
                unsafe {
                    SetWindowPos(hwnd, None, 0, 0, 1056, 720, SWP_NOMOVE | SWP_NOZORDER)?;
                }
            }
            if frames == 50 {
                camera.preset(false);
                if options.fluid {
                    help = false;
                }
            }
            if frames == 52
                && let Some(p) = &mut physics
            {
                p.kick()?;
                physics_kick_exercised = true;
            }
            if options.backend != "raytracing"
                && let Some(u) = &mut renderer.ultimate
            {
                if frames == 60 {
                    u.enabled = false;
                }
                if frames == 65 {
                    u.enabled = true;
                }
                if options.vrs && frames == 56 {
                    u.vrs_enabled = false;
                }
                if options.vrs && frames == 59 {
                    u.vrs_enabled = true;
                }
                if options.streaming && frames == 66 {
                    u.streaming.feedback_enabled = false;
                }
                if options.streaming && frames == 69 {
                    u.streaming.feedback_enabled = true;
                }
                if frames == 70 {
                    u.detail_error = 2.;
                }
                if frames == 72 {
                    u.detail_error = options.error;
                }
            }
        }
        camera.move_keys(&keys, dt, jump);
        let mut rect = RECT::default();
        unsafe {
            GetClientRect(hwnd, &mut rect)?;
        }
        if rect.right == 0 || rect.bottom == 0 || unsafe { IsIconic(hwnd).as_bool() } {
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        }
        if rect.right as u32 != renderer.width || rect.bottom as u32 != renderer.height {
            renderer.resize(rect.right as u32, rect.bottom as u32)?;
            resized = true;
            if mouse.0 {
                mouse_lock(hwnd, false)?;
                mouse.0 = false;
            }
        }
        if !paused {
            elapsed += dt;
        }
        let mut objects = scene::garden(elapsed);
        if let Some(p) = &mut physics {
            let before = p.world.stats().steps;
            p.world
                .advance(if options.scripted { dt } else { real_dt }, paused)?;
            if options.scripted && paused {
                anyhow::ensure!(
                    p.world.stats().steps == before,
                    "Physics advanced while paused"
                );
                physics_pause_exercised = true;
            }
            p.append_objects(&mut objects);
        }
        if let Some(u) = &mut renderer.ultimate
            && u.enabled
        {
            u.prepare(detail::plan(
                &objects,
                &camera,
                renderer.height,
                u.detail_error,
            ))?;
        }
        let (r, u, f) = camera.basis(renderer.width as f32 / renderer.height as f32);
        let vec4 = |v: Vec3, w: f32| [v.x, v.y, v.z, w];
        let (hud, n) = hud(
            &renderer,
            fps,
            &camera,
            &settings,
            paused,
            help,
            physics.as_ref(),
        );
        let frame = Frame {
            camera_time: vec4(camera.position, elapsed),
            right_width: vec4(r, renderer.width as f32),
            up_height: vec4(u, renderer.height as f32),
            forward_count: vec4(f, objects.len() as f32),
            sun_exposure: [-0.5, 0.85, -0.6, 1.05],
            settings,
            view_projection: (Mat4::perspective_lh(
                50f32.to_radians(),
                renderer.width as f32 / renderer.height as f32,
                0.05,
                200.,
            ) * Mat4::look_to_lh(camera.position, camera.forward(), Vec3::Y))
            .to_cols_array_2d(),
            stream_settings: [
                renderer.ultimate.as_ref().is_some_and(|u| u.enabled) as u32 as f32,
                renderer
                    .ultimate
                    .as_ref()
                    .is_some_and(|u| u.streaming.feedback_enabled) as u32 as f32,
                physics.is_some() as u32 as f32,
                options.fluid as u32 as f32,
            ],
            status: [
                n as f32,
                if mouse.0 { -1. } else { paused as u32 as f32 },
                renderer.hardware as u32 as f32,
                help as u32 as f32,
            ],
        };
        renderer.draw(&frame, &objects, &hud)?;
        frames += 1;
        if capture_request {
            renderer.capture(&PathBuf::from("artifacts/screenshot.bmp"))?;
            capture_request = false;
        }
        if frames.is_multiple_of(30) {
            let backend = if let Some(u) = renderer.ultimate.as_ref().filter(|u| u.enabled) {
                format!("DX12 Ultimate / SM {}", u.shader_model)
            } else if renderer.hardware {
                "DXR 1.1".into()
            } else {
                "Compute RT".into()
            };
            let title = wide(&format!(
                "LightRenderer | Sakura Garden | {} | {:.0} FPS | {}",
                backend, fps, renderer.adapter
            ));
            unsafe {
                SetWindowTextW(hwnd, PCWSTR(title.as_ptr()))?;
            }
        }
        if options.frames.is_some_and(|limit| frames >= limit) {
            break;
        }
    }
    if let Some(path) = &options.capture {
        renderer.capture(path)?;
    }
    let diagnostics = renderer.diagnostics();
    let report = serde_json::json!({"adapter":renderer.adapter,"backend":if renderer.ultimate.as_ref().is_some_and(|u|u.enabled){"DX12 Ultimate Mesh Shader + inline DXR"}else if renderer.hardware{"DXR 1.1 inline procedural ray tracing"}else{"DirectX 12 compute software ray tracing"},
        "frames":frames,"seconds":start.elapsed().as_secs_f64(),"resolution":[renderer.width,renderer.height],"primitives":renderer.primitive_count,
        "debug_layer":renderer.debug_layer,"gpu_warnings_and_errors":diagnostics,"scripted_input":options.scripted,"resize_exercised":resized,
        "initial_camera":initial_camera.to_array(),"final_camera":camera.position.to_array(),"animation_seconds":elapsed,"capabilities":renderer.capabilities.json(),"ultimate":renderer.ultimate.as_ref().map(|u|u.json()),
        "physics":physics.as_ref().map(|p|p.report()),"physics_pause_exercised":physics_pause_exercised,"physics_kick_exercised":physics_kick_exercised});
    if let Some(path) = &options.report {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    anyhow::ensure!(
        diagnostics.is_empty(),
        "DirectX debug layer reported: {}",
        diagnostics.join("\n")
    );
    Ok(())
}
fn main() {
    let _ = std::fs::remove_file("artifacts/last-error.txt");
    if let Err(error) = run() {
        let message = format!("{error:#}");
        let _ = std::fs::create_dir_all("artifacts");
        let _ = std::fs::write("artifacts/last-error.txt", &message);
        if !std::env::args().any(|a| a == "--frames") {
            let text = wide(&message);
            unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(text.as_ptr()),
                    w!("LightRenderer error"),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
        std::process::exit(1);
    }
}
