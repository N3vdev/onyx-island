use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use islet_core::Config;
use windows::core::{w, Error, PCWSTR};
use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, FindWindowW,
    GetMessageW, GetWindowLongPtrW, LoadCursorW, LoadIconW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetWindowLongPtrW, TranslateMessage, CREATESTRUCTW,
    CW_USEDEFAULT, GWLP_USERDATA, HICON, IDC_ARROW, IDI_APPLICATION, MSG, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_CREATE, WM_DESTROY,
    WM_LBUTTONUP, WM_NCCREATE, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::config_watch;
use crate::icon;
use crate::island_window;
use crate::settings;

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_CONFIG_CHANGED: u32 = WM_APP + 2;

const TRAY_ICON_ID: u32 = 1;
use crate::context_menu::{
    self, PAUSE_DICTATION as MENU_PAUSE_DICTATION, QUIT as MENU_QUIT, RESTART as MENU_RESTART,
    SETTINGS as MENU_SETTINGS,
};

struct AppState {
    hwnd: HWND,
    icon: HICON,
    nid: NOTIFYICONDATAW,
    tray_added: bool,
    taskbar_created: u32,
    island_hwnd: HWND,
    island_host: Option<Box<island_window::IslandHost>>,
    dictation_paused: bool,
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.remove_tray();
        self.shutdown_island();
        icon::destroy(self.icon);
    }
}

impl AppState {
    fn new(icon: HICON) -> Self {
        Self {
            hwnd: HWND::default(),
            icon,
            nid: NOTIFYICONDATAW::default(),
            tray_added: false,
            taskbar_created: 0,
            island_hwnd: HWND::default(),
            island_host: None,
            dictation_paused: false,
        }
    }

    fn add_tray(&mut self) {
        self.nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        self.nid.hWnd = self.hwnd;
        self.nid.uID = TRAY_ICON_ID;
        self.nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        self.nid.uCallbackMessage = WM_APP_TRAY;
        self.nid.hIcon = self.icon;
        copy_wide(&mut self.nid.szTip, "Islet \u{00b7} idle");

        if unsafe { Shell_NotifyIconW(NIM_ADD, &self.nid) }.as_bool() {
            self.tray_added = true;
            tracing::info!("tray icon installed");
        } else {
            self.tray_added = false;
            tracing::error!("could not install tray icon");
        }
    }

    fn remove_tray(&mut self) {
        if self.tray_added {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid);
            }
            self.tray_added = false;
        }
    }

    fn show_menu(&mut self) {
        match context_menu::show(self.hwnd) {
            Ok(command) if command != 0 => self.handle_command(command),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "could not show context menu"),
        }
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        }
    }

    fn handle_command(&mut self, command: usize) {
        match command {
            MENU_SETTINGS => settings::open(None),
            MENU_PAUSE_DICTATION => {
                self.dictation_paused = !self.dictation_paused;
                context_menu::set_dictation_paused(self.dictation_paused);
                if let Some(host) = &mut self.island_host {
                    host.set_dictation_paused(self.dictation_paused);
                }
                tracing::info!(paused = self.dictation_paused, "dictation pause toggled");
            }
            MENU_RESTART => match crate::restart::launch() {
                Ok(()) => {
                    tracing::info!("restart requested from tray");
                    unsafe {
                        let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "could not restart Islet; keeping current instance running")
                }
            },
            MENU_QUIT => {
                tracing::info!("quit requested from tray");
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            _ => {}
        }
    }

    fn apply_config(&mut self) {
        if let Some(host) = &mut self.island_host {
            host.apply_config();
        }
    }

    fn shutdown_island(&mut self) {
        if let Some(mut host) = self.island_host.take() {
            host.shutdown();
        }
        if !self.island_hwnd.0.is_null() {
            unsafe {
                let _ = DestroyWindow(self.island_hwnd);
            }
            self.island_hwnd = HWND::default();
        }
    }
}

pub fn run(
    config_file: PathBuf,
    shared: Arc<RwLock<Config>>,
    demo: bool,
    demo_state: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let hinstance = unsafe { GetModuleHandleW(None)? };
    let icon = icon::create().or_else(|error| {
        tracing::warn!(%error, "custom tray icon failed; using stock icon");
        unsafe { LoadIconW(None, IDI_APPLICATION) }
    })?;

    let class_name = w!("IsletTrayWindow");
    let window_class = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: HINSTANCE(hinstance.0),
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: Default::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: class_name,
    };
    let atom = unsafe { RegisterClassW(&window_class) };
    if atom == 0 {
        return Err(Box::new(Error::from_thread()));
    }

    let island_class_name = w!("IsletIslandWindow");
    let island_class = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(island_window::window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: HINSTANCE(hinstance.0),
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: Default::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: island_class_name,
    };
    if unsafe { RegisterClassW(&island_class) } == 0 {
        return Err(Box::new(Error::from_thread()));
    }

    let state = Box::new(AppState::new(icon));
    let state_pointer = Box::into_raw(state);

    let hwnd = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("Islet"),
            WINDOW_STYLE(0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            Some(state_pointer.cast()),
        )
    } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            unsafe {
                drop(Box::from_raw(state_pointer));
            }
            return Err(Box::new(error));
        }
    };

    tracing::info!(hwnd = ?hwnd, "tray window created");

    let island_hwnd = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(
                WS_EX_TOPMOST.0
                    | WS_EX_TOOLWINDOW.0
                    | WS_EX_NOACTIVATE.0
                    | WS_EX_NOREDIRECTIONBITMAP.0,
            ),
            island_class_name,
            w!("Islet"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(Box::new(error));
        }
    };

    let mut host =
        match island_window::IslandHost::new(island_hwnd, Arc::clone(&shared), demo, demo_state) {
            Ok(host) => host,
            Err(error) => {
                unsafe {
                    let _ = DestroyWindow(island_hwnd);
                    let _ = DestroyWindow(hwnd);
                }
                return Err(Box::new(error));
            }
        };
    unsafe {
        SetWindowLongPtrW(
            island_hwnd,
            GWLP_USERDATA,
            (&mut *host as *mut island_window::IslandHost) as isize,
        );
    }
    host.show();
    unsafe {
        (*state_pointer).island_hwnd = island_hwnd;
        (*state_pointer).island_host = Some(host);
    }
    tracing::info!(hwnd = ?island_hwnd, demo, "island window created");
    // A restart reconnects the same live preview without polling settings.
    unsafe {
        if let Ok(settings) = FindWindowW(None, w!("Islet Settings")) {
            if let Ok(anchor) =
                FindWindowExW(Some(settings), None, w!("STATIC"), w!("IsletPreviewAnchor"))
            {
                let _ = PostMessageW(
                    Some(island_hwnd),
                    islet_core::handoff::PREVIEW_ATTACH,
                    WPARAM(anchor.0 as usize),
                    LPARAM(0),
                );
            }
        }
    }

    // The watcher lives for the whole message loop. Dropping it disconnects the
    // watcher thread, which then exits naturally.
    let _watcher = config_watch::spawn(config_file, shared, hwnd)?;

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 == -1 {
            let error = unsafe { GetLastError() };
            tracing::error!(?error, "message loop failed");
            break;
        }
        if !result.as_bool() {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    tracing::info!("message loop exited");
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = context_menu::handle_message(message, lparam) {
        return result;
    }
    match message {
        WM_NCCREATE => {
            let create = lparam.0 as *const CREATESTRUCTW;
            if !create.is_null() {
                let state = (*create).lpCreateParams as *mut AppState;
                if !state.is_null() {
                    (*state).hwnd = hwnd;
                    unsafe {
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
                    }
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CREATE => {
            with_state(hwnd, |state| {
                state.taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
                state.add_tray();
            });
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            with_state(hwnd, |state| state.show_menu());
            LRESULT(0)
        }
        WM_APP_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            with_state(hwnd, |state| match event {
                WM_LBUTTONUP => settings::open(None),
                WM_RBUTTONUP => state.show_menu(),
                _ => {}
            });
            LRESULT(0)
        }
        WM_APP_CONFIG_CHANGED => {
            tracing::debug!("UI notified of config reload");
            with_state(hwnd, AppState::apply_config);
            LRESULT(0)
        }
        WM_COMMAND => {
            let command = wparam.0 & 0xFFFF;
            with_state(hwnd, |state| state.handle_command(command));
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            with_state(hwnd, AppState::shutdown_island);
            with_state(hwnd, AppState::remove_tray);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut AppState;
            if !pointer.is_null() {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(pointer));
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => {
            // Explorer restarts broadcast a dynamically registered message.
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const AppState;
            if !pointer.is_null() {
                let state = unsafe { &*pointer };
                if state.taskbar_created != 0 && message == state.taskbar_created {
                    with_state(hwnd, |state| {
                        state.tray_added = false;
                        state.add_tray();
                    });
                    return LRESULT(0);
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

fn with_state(hwnd: HWND, action: impl FnOnce(&mut AppState)) {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut AppState;
    if !pointer.is_null() {
        let state = unsafe { &mut *pointer };
        action(state);
    }
}

fn copy_wide(buffer: &mut [u16], text: &str) {
    let encoded: Vec<u16> = text
        .encode_utf16()
        .take(buffer.len().saturating_sub(1))
        .collect();
    buffer.fill(0);
    buffer[..encoded.len()].copy_from_slice(&encoded);
}
