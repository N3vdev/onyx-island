use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use islet_core::Config;
use windows::core::{w, Error, PCWSTR};
use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetWindowLongPtrW, LoadCursorW, LoadIconW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetWindowLongPtrW, TrackPopupMenu, TranslateMessage, CREATESTRUCTW, CW_USEDEFAULT,
    GWLP_USERDATA, HICON, IDC_ARROW, IDI_APPLICATION, MF_SEPARATOR, MF_STRING, MSG, SW_SHOWNORMAL,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CONTEXTMENU, WM_CREATE, WM_DESTROY, WM_LBUTTONUP, WM_NCCREATE, WM_NCDESTROY,
    WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};

use crate::config_watch;
use crate::icon;

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_CONFIG_CHANGED: u32 = WM_APP + 2;

const TRAY_ICON_ID: u32 = 1;
const MENU_OPEN_CONFIG: usize = 1;
const MENU_OPEN_LOGS: usize = 2;
const MENU_RELOAD_CONFIG: usize = 3;
const MENU_QUIT: usize = 4;

struct AppState {
    hwnd: HWND,
    icon: HICON,
    nid: NOTIFYICONDATAW,
    tray_added: bool,
    config_file: PathBuf,
    log_dir: PathBuf,
    shared: Arc<RwLock<Config>>,
    taskbar_created: u32,
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.remove_tray();
        icon::destroy(self.icon);
    }
}

impl AppState {
    fn new(
        icon: HICON,
        config_file: PathBuf,
        log_dir: PathBuf,
        shared: Arc<RwLock<Config>>,
    ) -> Self {
        Self {
            hwnd: HWND::default(),
            icon,
            nid: NOTIFYICONDATAW::default(),
            tray_added: false,
            config_file,
            log_dir,
            shared,
            taskbar_created: 0,
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

    fn show_menu(&self) {
        let menu = match unsafe { CreatePopupMenu() } {
            Ok(menu) => menu,
            Err(error) => {
                tracing::warn!(%error, "could not create tray menu");
                return;
            }
        };

        let _ = unsafe { AppendMenuW(menu, MF_STRING, MENU_OPEN_CONFIG, w!("Open config")) };
        let _ = unsafe { AppendMenuW(menu, MF_STRING, MENU_OPEN_LOGS, w!("Open logs")) };
        let _ = unsafe { AppendMenuW(menu, MF_STRING, MENU_RELOAD_CONFIG, w!("Reload config")) };
        let _ = unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) };
        let _ = unsafe { AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit Islet")) };

        let mut point = windows::Win32::Foundation::POINT::default();
        let _ = unsafe { GetCursorPos(&mut point) };
        let _ = unsafe { SetForegroundWindow(self.hwnd) };

        let command = unsafe {
            TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                point.x,
                point.y,
                Some(0),
                self.hwnd,
                None,
            )
        };
        let _ = unsafe { DestroyMenu(menu) };
        let _ = unsafe { PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };

        if command.0 != 0 {
            self.handle_command(command.0 as usize);
        }
    }

    fn handle_command(&self, command: usize) {
        match command {
            MENU_OPEN_CONFIG => open_path(&self.config_file),
            MENU_OPEN_LOGS => open_path(&self.log_dir),
            MENU_RELOAD_CONFIG => self.reload_config(),
            MENU_QUIT => {
                tracing::info!("quit requested from tray");
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            _ => {}
        }
    }

    fn reload_config(&self) {
        match Config::load(&self.config_file) {
            Ok(config) => {
                match self.shared.write() {
                    Ok(mut current) => *current = config,
                    Err(poisoned) => *poisoned.into_inner() = config,
                }
                tracing::info!(path = %self.config_file.display(), "config reloaded from tray");
            }
            Err(error) => {
                tracing::warn!(%error, "config reload failed; keeping previous config");
            }
        }
    }
}

pub fn run(
    config_file: PathBuf,
    log_dir: PathBuf,
    shared: Arc<RwLock<Config>>,
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

    let state = Box::new(AppState::new(
        icon,
        config_file.clone(),
        log_dir.clone(),
        Arc::clone(&shared),
    ));
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
                WM_LBUTTONUP => {
                    // Until the settings process exists, left click opens the
                    // config file: the only interactive surface in P0.
                    open_path(&state.config_file);
                }
                WM_RBUTTONUP => state.show_menu(),
                _ => {}
            });
            LRESULT(0)
        }
        WM_APP_CONFIG_CHANGED => {
            tracing::debug!("UI notified of config reload");
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

fn open_path(path: &Path) {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        tracing::warn!(path = %path.display(), "could not open path");
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
