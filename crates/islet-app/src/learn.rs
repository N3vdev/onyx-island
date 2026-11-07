//! Learning from your corrections (opt-in: `dictation.learn_corrections`).
//!
//! After a dictation is typed, a short-lived thread reads the same text box
//! twice (about 12 and 40 seconds later) through UI Automation. If you
//! changed a few words of what Onyx typed, the dictated passage and your
//! version are appended to `corrections.jsonl` in Onyx's local data folder.
//! Nothing else in the box is kept, password fields and other windows are
//! never read, nothing is logged, and nothing leaves this PC.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationValuePattern, UIA_TextPatternId,
    UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// When to look at the box again after typing.
const CHECKS: [Duration; 2] = [Duration::from_secs(12), Duration::from_secs(28)];
/// Most text read from a box (a long document only needs its recent part).
const MAX_READ: i32 = 60_000;

/// What one dictation produced, kept until its checks are done.
pub struct Dictated {
    /// What the speech model heard.
    pub heard: String,
    /// After Onyx's rules (what the cleanup model saw).
    pub rule: String,
    /// What was typed into the box.
    pub typed: String,
}

pub fn corrections_file() -> PathBuf {
    // Tests write to their own file, never the user's.
    if let Some(path) = std::env::var_os("ONYX_CORRECTIONS_FILE") {
        return PathBuf::from(path);
    }
    crate::paths::Paths::discover().data_dir.join("corrections.jsonl")
}

/// Starts watching the window that is in front for edits to `dictated`.
pub fn watch(dictated: Dictated) {
    // SAFETY: a plain query.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return;
    }
    let handle = window.0 as isize;
    std::thread::spawn(move || {
        let window = HWND(handle as *mut _);
        let mut found = None;
        for wait in CHECKS {
            std::thread::sleep(wait);
            // Only while you are still in the same window.
            // SAFETY: a plain query.
            if unsafe { GetForegroundWindow() } != window {
                continue;
            }
            if let Some(field) = focused_text() {
                if let Some(fixed) = islet_core::learn::corrected(&dictated.typed, &field) {
                    found = Some(fixed);
                }
            }
        }
        if let Some(corrected) = found {
            save(&dictated, &corrected);
        }
    });
}

fn save(dictated: &Dictated, corrected: &str) {
    let swaps: Vec<[String; 2]> = islet_core::learn::word_swaps(&dictated.typed, corrected)
        .into_iter()
        .map(|(heard, meant)| [heard, meant])
        .collect();
    let record = serde_json::json!({
        "time": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        "heard": dictated.heard,
        "rule": dictated.rule,
        "typed": dictated.typed,
        "corrected": corrected,
        "swaps": swaps,
    });
    let path = corrections_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{record}");
    }
    tracing::info!(swaps = swaps.len(), "saved a dictation correction");
}

/// The text of the focused text box, unless it is a password field.
fn focused_text() -> Option<String> {
    // SAFETY: this function owns its COM apartment until CoUninitialize;
    // every UI Automation object is dropped before then.
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return None;
        }
        let text = read_focused();
        CoUninitialize();
        text
    }
}

unsafe fn read_focused() -> Option<String> {
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
    let element = automation.GetFocusedElement().ok()?;
    if element.CurrentIsPassword().is_ok_and(|password| password.as_bool()) {
        return None;
    }
    if let Ok(value) = element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
        if let Ok(text) = value.CurrentValue() {
            let text = text.to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    let pattern = element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).ok()?;
    let range = pattern.DocumentRange().ok()?;
    range.GetText(MAX_READ).ok().map(|text| text.to_string())
}

#[cfg(test)]
mod live_tests {
    //! Drives the real watcher against windows this test creates itself (a
    //! small form with a text box), never the user's own apps. Run by hand,
    //! without touching the keyboard for about two minutes:
    //! `cargo test -p islet-app live_ -- --ignored --nocapture --test-threads=1`
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;
    use windows::core::HSTRING;
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};

    const VK_CONTROL: u16 = 0x11;
    const VK_A: u16 = 0x41;
    const TYPED: &str = "Can you send the contract to Daniel before 5 PM? Tanks.";
    const FIXED: &str = "Can you send the contract to Daniel before 5 PM? Thanks.";

    /// A form with one text box, run by its own PowerShell process.
    struct TestWindow {
        title: String,
        process: Child,
    }

    impl TestWindow {
        fn open(title: &str) -> Self {
            let script = format!(
                "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object Windows.Forms.Form; $f.Text = '{title}'; \
                 $f.Width = 700; $f.Height = 300; $t = New-Object Windows.Forms.RichTextBox; \
                 $t.Dock = 'Fill'; $f.Controls.Add($t); $f.Add_Shown({{ $f.Activate(); $t.Focus() }}); [void]$f.ShowDialog()"
            );
            let process = Command::new("powershell.exe")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("could not start the test window");
            let window = Self { title: title.to_owned(), process };
            let started = Instant::now();
            while !window.in_front() {
                window.bring_front();
                assert!(started.elapsed() < Duration::from_secs(20), "test window did not come to the front");
                std::thread::sleep(Duration::from_millis(300));
            }
            std::thread::sleep(Duration::from_millis(500));
            window
        }

        fn handle(&self) -> Option<HWND> {
            // SAFETY: a plain lookup by title.
            unsafe { FindWindowW(None, &HSTRING::from(self.title.as_str())).ok() }
        }

        fn in_front(&self) -> bool {
            // SAFETY: a plain query.
            self.handle().is_some_and(|window| unsafe { GetForegroundWindow() } == window)
        }

        /// The way the assistant does it: an empty mouse input first, for
        /// the foreground lock.
        fn bring_front(&self) {
            let Some(window) = self.handle() else { return };
            // SAFETY: plain input and window calls.
            unsafe {
                let nudge = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_MOVE, dwExtraInfo: islet_input::EXTRA_INFO, ..Default::default() },
                    },
                };
                SendInput(&[nudge], std::mem::size_of::<INPUT>() as i32);
                let _ = SetForegroundWindow(window);
            }
        }

        /// Types into this window only; refuses if it isn't in front.
        fn type_text(&self, text: &str) {
            assert!(self.in_front(), "refusing to type: the test window is not in front");
            islet_input::insert_text_with_clipboard(text, islet_input::Method::Clipboard, false).expect("typing failed");
            std::thread::sleep(Duration::from_millis(600));
        }

        fn select_all(&self) {
            assert!(self.in_front(), "refusing to select: the test window is not in front");
            islet_input::press_chord(&[VK_CONTROL, VK_A]);
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            let _ = self.process.kill();
        }
    }

    fn records(path: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    #[test]
    #[ignore]
    fn live_correction_is_learned() {
        let file = std::env::temp_dir().join(format!("onyx-corrections-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&file);
        std::env::set_var("ONYX_CORRECTIONS_FILE", &file);

        // 1. Reading the box: what was typed is what UI Automation sees.
        let window = TestWindow::open("Onyx correction test");
        window.type_text(TYPED);
        let seen = focused_text().expect("could not read the test box");
        println!("read back from the box: {:?}", seen.trim());
        assert_eq!(seen.trim(), TYPED);

        // 2. The real watcher, with one word fixed meanwhile.
        watch(Dictated { heard: "Can you set the contract to Daniel before 5pm? Tanks.".into(), rule: TYPED.into(), typed: TYPED.into() });
        std::thread::sleep(Duration::from_secs(3));
        window.select_all();
        window.type_text(FIXED);
        println!("fixed one word; waiting for both checks (about 40 s)...");
        std::thread::sleep(Duration::from_secs(44));
        let saved = records(&file);
        assert_eq!(saved.len(), 1, "expected exactly one correction");
        assert_eq!(saved[0]["typed"], TYPED);
        assert_eq!(saved[0]["corrected"], FIXED);
        assert_eq!(saved[0]["swaps"], serde_json::json!([["tanks", "thanks"]]));
        println!("saved: {} (swap {})", saved[0]["corrected"], saved[0]["swaps"]);

        // 3. An unchanged dictation saves nothing.
        let unchanged = "Please book a table for four people at seven tonight.";
        window.select_all();
        window.type_text(unchanged);
        watch(Dictated { heard: String::new(), rule: String::new(), typed: unchanged.into() });
        std::thread::sleep(Duration::from_secs(44));
        assert_eq!(records(&file).len(), 1, "an unchanged dictation was saved");
        println!("unchanged dictation: nothing saved (correct)");

        // 4. Another window in front: the box isn't read.
        let typed = "Remind me to call Maya tomorrow about the budget.";
        window.select_all();
        window.type_text(typed);
        watch(Dictated { heard: String::new(), rule: String::new(), typed: typed.into() });
        window.select_all();
        window.type_text("Remind me to call Mia tomorrow about the budget.");
        let other = TestWindow::open("Onyx other window");
        std::thread::sleep(Duration::from_secs(44));
        assert!(other.in_front(), "the other window lost the front during the check");
        assert_eq!(records(&file).len(), 1, "a correction was read while another window was in front");
        println!("other window in front: nothing read (correct)");
        drop(other);
        drop(window);
        let _ = std::fs::remove_file(&file);
    }
}
