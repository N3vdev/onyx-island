//! Short-lived OrbitDrop provider bridge. No converter process exists at idle.
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, WM_APP},
};

pub const MESSAGE: u32 = WM_APP + 61;
pub struct Update {
    pub generation: u64,
    pub value: Value,
}
pub struct Worker {
    child: Arc<Mutex<Option<Child>>>,
    canceled: Arc<AtomicBool>,
}
impl Worker {
    pub fn cancel(&self) {
        if self.canceled.swap(true, Ordering::Relaxed) {
            return;
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(stdin) = guard.as_mut().and_then(|child| child.stdin.as_mut()) {
                let _ = stdin.write_all(b"cancel\n");
                let _ = stdin.flush();
            }
        }
        let child = Arc::clone(&self.child);
        let _ = std::thread::Builder::new()
            .name("onyx-convert-cancel".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if let Ok(mut guard) = child.lock() {
                    if let Some(child) = guard.as_mut() {
                        let _ = child.kill();
                    }
                }
            });
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Never wait for a process while holding this mutex or the UI thread.
        let child = self.child.lock().ok().and_then(|mut guard| guard.take());
        if let Some(mut child) = child {
            let _ = child.kill();
        }
    }
}

pub fn channel() -> (Sender<Update>, Receiver<Update>) {
    unbounded()
}
pub fn start(
    hwnd: HWND,
    tx: Sender<Update>,
    generation: u64,
    files: &[PathBuf],
    operation: Option<&str>,
) -> std::io::Result<Worker> {
    let helper = program();
    let mut child = Command::new(helper)
        .creation_flags(0x0800_0000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    islet_asr::tie_to_app(&child);
    let request = json!({"command":if operation.is_some(){"convert"}else{"catalog"},"files":files,"operation":operation});
    if let Some(stdin) = child.stdin.as_mut() {
        writeln!(stdin, "{request}")?;
        stdin.flush()?;
    }
    let output = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("Converter output unavailable"))?;
    let shared = Arc::new(Mutex::new(Some(child)));
    let wait = Arc::clone(&shared);
    let canceled = Arc::new(AtomicBool::new(false));
    let reader_canceled = Arc::clone(&canceled);
    let worker = Worker {
        child: shared,
        canceled,
    };
    let hwnd = hwnd.0 as isize;
    std::thread::Builder::new().name("onyx-convert".into()).spawn(move||{
        let notify=|value| {
            if tx.send(Update{generation,value}).is_ok() { unsafe{ let _=PostMessageW(Some(HWND(hwnd as *mut _)),MESSAGE,WPARAM(0),LPARAM(0)); } }
        };
        let mut terminal=false;
        let (mut succeeded,mut failed)=(0u64,0u64);
        let mut previous_percent=None;
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            let Ok(value)=serde_json::from_str::<Value>(&line) else {continue};
            if value["type"]=="outcome" {if value["succeeded"].as_bool().unwrap_or(false){succeeded+=1}else{failed+=1}}
            if value["type"]=="progress" {
                let percent=(value["fraction"].as_f64().unwrap_or(0.0).clamp(0.0,1.0)*100.0).round() as u8;
                if previous_percent==Some(percent){continue;}
                previous_percent=Some(percent);
            }
            terminal|=matches!(value["type"].as_str(),Some("catalog"|"complete"|"error"));
            notify(value);
        }
        let child=wait.lock().ok().and_then(|mut guard|guard.take());
        if let Some(mut child)=child {drop(child.stdin.take());let _=child.wait();}
        if !terminal {
            if reader_canceled.load(Ordering::Relaxed) {notify(json!({"type":"complete","succeeded":succeeded,"failed":failed,"canceled":true}));}
            else {notify(json!({"type":"error","message":"The converter stopped. Try another file or format."}));}
        }
    })?;
    Ok(worker)
}

pub fn program() -> PathBuf {
    let bundled = std::env::current_exe()
        .unwrap_or_default()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("converter/Onyx.Converter.exe");
    if cfg!(debug_assertions) && !bundled.is_file() {
        return Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/release/converter/Onyx.Converter.exe");
    }
    bundled
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires tools/build-converter.ps1; exercises the real helper and open cancellation pipe"]
    fn completed_helpers_never_block_the_ui_waiting_on_the_child_mutex() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("OrbitDrop-main/src/OrbitDrop.UI/Assets/orbitdrop-32.png");
        let temporary = std::env::temp_dir().join(format!(
            "onyx-converter-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&temporary).unwrap();
        let input = temporary.join("fixture.png");
        std::fs::copy(fixture, &input).unwrap();
        for _ in 0..3 {
            let (tx, rx) = channel();
            let worker = start(
                HWND::default(),
                tx,
                1,
                std::slice::from_ref(&input),
                Some("convert.image.jpg"),
            )
            .unwrap();
            loop {
                let update = rx
                    .recv_timeout(std::time::Duration::from_secs(8))
                    .expect("helper must finish with stdin open");
                assert_ne!(update.value["type"], "error");
                if update.value["type"] == "complete" {
                    assert_eq!(update.value["succeeded"], 1);
                    break;
                }
            }
            let before = std::time::Instant::now();
            drop(worker);
            assert!(
                before.elapsed() < std::time::Duration::from_millis(500),
                "the UI must never wait on an unfinished helper"
            );
        }
        assert!(input.is_file());
    }
}
