//! OLE file drops, registered only on the native island. Never returns MOVE.
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::{cell::Cell, path::PathBuf};
use windows::core::{implement, Ref, Result};
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, POINTL, WPARAM},
    System::{
        Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL},
        Ole::{
            IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
            CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
        },
        SystemServices::MODIFIERKEYS_FLAGS,
    },
    UI::{
        Shell::{DragQueryFileW, HDROP},
        WindowsAndMessaging::{PostMessageW, WM_APP},
    },
};

pub const MESSAGE: u32 = WM_APP + 60;
#[derive(Debug)]
pub enum Event {
    Enter(Vec<PathBuf>),
    Over(POINT),
    Leave,
    Drop(Vec<PathBuf>, POINT),
}

pub struct Registration {
    hwnd: HWND,
    _target: IDropTarget,
    enabled: Arc<AtomicBool>,
    pub events: Receiver<Event>,
}
impl Registration {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let (tx, events) = unbounded();
        let enabled = Arc::new(AtomicBool::new(true));
        let target: IDropTarget = Target {
            hwnd: hwnd.0 as isize,
            tx,
            accepted: Cell::new(false),
            enabled: Arc::clone(&enabled),
        }
        .into();
        unsafe {
            RegisterDragDrop(hwnd, &target)?;
        }
        Ok(Self {
            hwnd,
            _target: target,
            enabled,
            events,
        })
    }
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
        }
    }
}

#[implement(IDropTarget)]
struct Target {
    hwnd: isize,
    tx: Sender<Event>,
    accepted: Cell<bool>,
    enabled: Arc<AtomicBool>,
}
impl Target {
    fn send(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.hwnd as *mut _)),
                    MESSAGE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
    unsafe fn effect(&self, effect: *mut DROPEFFECT) {
        if !effect.is_null() {
            *effect = if self.accepted.get()
                && self.enabled.load(Ordering::Relaxed)
                && (*effect & DROPEFFECT_COPY) != DROPEFFECT_NONE
            {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
    }
}
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = if self.enabled.load(Ordering::Relaxed) {
            data.as_ref().map(read_files).unwrap_or_default()
        } else {
            Vec::new()
        };
        self.accepted.set(!files.is_empty());
        tracing::info!(file_count=files.len(),enabled=self.enabled.load(Ordering::Relaxed),"File drag entered island");
        if self.accepted.get() {
            self.send(Event::Enter(files));
            self.send(Event::Over(POINT { x: pt.x, y: pt.y }));
        }
        unsafe {
            self.effect(effect);
        }
        Ok(())
    }
    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        if self.accepted.get() {
            self.send(Event::Over(POINT { x: pt.x, y: pt.y }));
        }
        unsafe {
            self.effect(effect);
        }
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        self.accepted.set(false);
        self.send(Event::Leave);
        Ok(())
    }
    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = data.as_ref().map(read_files).unwrap_or_default();
        self.accepted.set(!files.is_empty());
        unsafe {
            self.effect(effect);
        }
        if self.accepted.get() && !effect.is_null() && unsafe { *effect } == DROPEFFECT_COPY {
            self.send(Event::Drop(files, POINT { x: pt.x, y: pt.y }));
        }
        self.accepted.set(false);
        Ok(())
    }
}

fn read_files(data: &IDataObject) -> Vec<PathBuf> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = (unsafe { data.GetData(&format) }) else {
        return Vec::new();
    };
    let files = unsafe {
        if medium.tymed != TYMED_HGLOBAL.0 as u32 {
            ReleaseStgMedium(&mut medium);
            return Vec::new();
        }
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        if count > 64 {
            ReleaseStgMedium(&mut medium);
            return Vec::new();
        }
        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None) as usize;
            if length == 0 || length > 32767 {
                continue;
            }
            let mut buffer = vec![0u16; length + 1];
            DragQueryFileW(drop, index, Some(&mut buffer));
            use std::os::windows::ffi::OsStringExt;
            paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                &buffer[..length],
            )));
        }
        paths
    };
    unsafe {
        ReleaseStgMedium(&mut medium);
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_drop_never_requests_move_and_rejects_disabled_or_move_only_sources() {
        use windows::Win32::System::Ole::DROPEFFECT_MOVE;
        let (tx, _) = unbounded();
        let target = Target {
            hwnd: 0,
            tx,
            accepted: Cell::new(true),
            enabled: Arc::new(AtomicBool::new(true)),
        };
        let mut effect = DROPEFFECT_MOVE;
        unsafe { target.effect(&mut effect) };
        assert_eq!(effect, DROPEFFECT_NONE);
        let mut effect = DROPEFFECT_MOVE | DROPEFFECT_COPY;
        unsafe { target.effect(&mut effect) };
        assert_eq!(effect, DROPEFFECT_COPY);
        target.enabled.store(false, Ordering::Relaxed);
        unsafe { target.effect(&mut effect) };
        assert_eq!(effect, DROPEFFECT_NONE);
    }
}
