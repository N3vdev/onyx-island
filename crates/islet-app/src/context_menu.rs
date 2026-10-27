//! A dark native popup: Windows retains keyboard navigation and dismissal.
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_SELECTED, ODT_MENU};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const SETTINGS: usize = 5;
pub const QUIT: usize = 4;
pub const RESTART: usize = 6;
pub const PAUSE_DICTATION: usize = 7;

/// Whether dictation is paused, so its menu row reads "Resume dictation".
static DICTATION_PAUSED: AtomicBool = AtomicBool::new(false);

pub fn set_dictation_paused(paused: bool) {
    DICTATION_PAUSED.store(paused, Ordering::Relaxed);
}

/// The text of a row, which can depend on state.
fn row_label(id: usize, label: Option<&'static str>) -> Option<&'static str> {
    match (id, label) {
        (PAUSE_DICTATION, Some(_)) if DICTATION_PAUSED.load(Ordering::Relaxed) => {
            Some("Resume dictation")
        }
        _ => label,
    }
}

const ENTRIES: &[(usize, Option<&str>)] = &[
    (SETTINGS, Some("Settings...")),
    (PAUSE_DICTATION, Some("Pause dictation")),
    (101, None),
    (RESTART, Some("Restart app")),
    (QUIT, Some("Quit Onyx")),
];

struct Popup {
    menu: HMENU,
    background: HBRUSH,
    hover: HBRUSH,
    line: HBRUSH,
    font: HFONT,
    scale: f32,
}

/// Scoped to this thread and the lifetime of an open menu; no idle hooks.
struct PopupCorners(HWINEVENTHOOK);

impl PopupCorners {
    fn install() -> Self {
        unsafe {
            Self(SetWinEventHook(
                EVENT_SYSTEM_MENUPOPUPSTART,
                EVENT_SYSTEM_MENUPOPUPSTART,
                None,
                Some(round_popup),
                GetCurrentProcessId(),
                GetCurrentThreadId(),
                WINEVENT_OUTOFCONTEXT,
            ))
        }
    }
}

impl Drop for PopupCorners {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.0);
            }
        }
    }
}

unsafe extern "system" fn round_popup(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let mut class = [0_u16; 16];
    let len = GetClassNameW(hwnd, &mut class) as usize;
    if &class[..len] != w!("#32768").as_wide() {
        return;
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return;
    }
    let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
    let diameter = (20.0 * scale).round() as i32;
    // Windows paints its stock light edge outside the owner-drawn rows.
    // Clip just that edge while keeping native menu layout and hit testing.
    let inset = (2.0 * scale).ceil() as i32;
    let region = CreateRoundRectRgn(
        inset,
        inset,
        rect.right - rect.left - inset,
        rect.bottom - rect.top - inset,
        diameter,
        diameter,
    );
    // Windows owns the region after a successful transfer.
    if !region.is_invalid() && SetWindowRgn(hwnd, Some(region), true) == 0 {
        let _ = DeleteObject(region.into());
    }
}

impl Drop for Popup {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.menu);
            for brush in [self.background, self.hover, self.line] {
                let _ = DeleteObject(brush.into());
            }
            let _ = DeleteObject(self.font.into());
        }
    }
}

pub fn show(owner: HWND) -> Result<usize> {
    unsafe {
        let mut point = POINT::default();
        GetCursorPos(&mut point)?;
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.0;
        // Box keeps itemData stable through the synchronous native menu loop.
        let popup = Box::new(Popup {
            menu: CreatePopupMenu()?,
            background: CreateSolidBrush(COLORREF(0x00150f12)),
            hover: CreateSolidBrush(COLORREF(0x00372a31)),
            line: CreateSolidBrush(COLORREF(0x003a2b34)),
            font: CreateFontW(
                -(13.0 * scale).round() as i32,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Segoe UI"),
            ),
            scale,
        });
        let info = MENUINFO {
            cbSize: std::mem::size_of::<MENUINFO>() as u32,
            fMask: MIM_BACKGROUND | MIM_STYLE,
            hbrBack: popup.background,
            dwStyle: MNS_NOCHECK,
            ..Default::default()
        };
        SetMenuInfo(popup.menu, &info)?;
        for (position, (id, label)) in ENTRIES.iter().enumerate() {
            let label = row_label(*id, *label);
            let wide: Vec<u16> = label.unwrap_or("").encode_utf16().chain(Some(0)).collect();
            AppendMenuW(
                popup.menu,
                if label.is_some() {
                    MF_STRING
                } else {
                    MF_SEPARATOR
                },
                *id,
                PCWSTR(wide.as_ptr()),
            )?;
            // Retain the native caption for accessibility; change only painting.
            let item = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA,
                fType: MFT_OWNERDRAW
                    | if label.is_none() {
                        MFT_SEPARATOR
                    } else {
                        MENU_ITEM_TYPE(0)
                    },
                dwItemData: (&*popup as *const Popup) as usize,
                ..Default::default()
            };
            SetMenuItemInfoW(popup.menu, position as u32, true, &item)?;
        }
        let _ = SetForegroundWindow(owner);
        let _corners = PopupCorners::install();
        Ok(TrackPopupMenu(
            popup.menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            point.x,
            point.y,
            Some(0),
            owner,
            None,
        )
        .0 as usize)
    }
}

pub fn handle_message(message: u32, lparam: LPARAM) -> Option<LRESULT> {
    // These structures and itemData are supplied synchronously by Windows for
    // our popup and cannot outlive the boxed Popup in show().
    unsafe {
        match message {
            WM_MEASUREITEM if lparam.0 != 0 => {
                let item = &mut *(lparam.0 as *mut MEASUREITEMSTRUCT);
                if item.CtlType != ODT_MENU || item.itemData == 0 {
                    return None;
                }
                let style = &*(item.itemData as *const Popup);
                let separator = !ENTRIES
                    .iter()
                    .any(|(id, label)| *id == item.itemID as usize && label.is_some());
                item.itemWidth = (164.0 * style.scale).round() as u32;
                item.itemHeight = (if separator { 7.0 } else { 27.0 } * style.scale).round() as u32;
                Some(LRESULT(1))
            }
            WM_DRAWITEM if lparam.0 != 0 => {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if item.CtlType != ODT_MENU || item.itemData == 0 {
                    return None;
                }
                let style = &*(item.itemData as *const Popup);
                let label = ENTRIES
                    .iter()
                    .find(|(id, _)| *id == item.itemID as usize)
                    .and_then(|(id, label)| row_label(*id, *label));
                let dc = SaveDC(item.hDC);
                FillRect(item.hDC, &item.rcItem, style.background);
                if label.is_some() && item.itemState.0 & ODS_SELECTED.0 != 0 {
                    let inset = (3.0 * style.scale).round() as i32;
                    let diameter = (12.0 * style.scale).round() as i32;
                    SelectObject(item.hDC, style.hover.into());
                    SelectObject(item.hDC, GetStockObject(NULL_PEN));
                    let _ = RoundRect(
                        item.hDC,
                        item.rcItem.left + inset,
                        item.rcItem.top + 1,
                        item.rcItem.right - inset,
                        item.rcItem.bottom - 1,
                        diameter,
                        diameter,
                    );
                }
                let mut rect = item.rcItem;
                rect.left += (10.0 * style.scale).round() as i32;
                rect.right -= (10.0 * style.scale).round() as i32;
                if let Some(label) = label {
                    SelectObject(item.hDC, style.font.into());
                    SetBkMode(item.hDC, TRANSPARENT);
                    SetTextColor(item.hDC, COLORREF(0x00f2edec));
                    let mut text: Vec<u16> = label.encode_utf16().collect();
                    DrawTextW(
                        item.hDC,
                        &mut text,
                        &mut rect,
                        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                    );
                } else {
                    rect.top = (rect.top + rect.bottom) / 2;
                    rect.bottom = rect.top + 1;
                    FillRect(item.hDC, &rect, style.line);
                }
                let _ = RestoreDC(item.hDC, dc);
                Some(LRESULT(1))
            }
            _ => None,
        }
    }
}
