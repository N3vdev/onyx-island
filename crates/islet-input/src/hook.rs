//! The global keyboard hook behind the dictation hotkeys.
//!
//! A dedicated thread installs `WH_KEYBOARD_LL` and pumps messages. The hook
//! callback runs the allocation-free [`HotkeyFsm`] directly, because it must
//! decide *right now* whether to swallow a key. Resulting [`Action`]s leave
//! through a bounded channel (`try_send`: no allocation, never blocks). The
//! callback never logs and never touches the clipboard or the network.
//!
//! Key events are used only to drive the state machine. They are not stored,
//! logged, or sent anywhere.

#![allow(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::io;
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use islet_core::hotkey::{Action, Combo, HotkeyFsm};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG,
    PM_NOREMOVE, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
};

use crate::EXTRA_INFO;

const WM_COMMANDS: u32 = WM_APP + 1;
/// Actions buffered between the hook and the controller.
const EVENT_CAPACITY: usize = 64;
const LLKHF_INJECTED: u32 = 0x10;

/// What the hook reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    Action(Action),
}

enum Command {
    SetCombos(Option<Combo>, Option<Combo>),
    SetEnabled(bool),
    RecordingEnded,
}

/// Handle to the hook thread. Dropping it removes the hook.
pub struct KeyboardHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
    events: Receiver<HookEvent>,
    commands: Sender<Command>,
}

struct HookState {
    fsm: HotkeyFsm,
    events: Sender<HookEvent>,
    started: Instant,
}

thread_local! {
    // The callback runs on the thread that installed the hook, so a
    // thread-local needs no lock.
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

impl KeyboardHook {
    /// Installs the hook with the given combos (config strings such as
    /// `"Ctrl+Win"`; an unparsable one disables that hotkey).
    pub fn start(push_to_talk: &str, hands_free: &str) -> io::Result<Self> {
        let (events_tx, events) = bounded(EVENT_CAPACITY);
        let (commands, commands_rx) = unbounded();
        let (ready_tx, ready_rx) = bounded::<io::Result<u32>>(1);
        let fsm = HotkeyFsm::from_strings(push_to_talk, hands_free);

        let thread = std::thread::Builder::new()
            .name("islet-keyboard-hook".to_owned())
            .spawn(move || run(fsm, events_tx, &commands_rx, &ready_tx))?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| io::Error::other("keyboard hook thread ended early"))??;
        Ok(Self {
            thread_id,
            thread: Some(thread),
            events,
            commands,
        })
    }

    #[must_use]
    pub fn events(&self) -> &Receiver<HookEvent> {
        &self.events
    }

    pub fn set_combos(&self, push_to_talk: &str, hands_free: &str) {
        self.send(Command::SetCombos(
            Combo::parse(push_to_talk).ok(),
            Combo::parse(hands_free).ok(),
        ));
    }

    /// Pausing dictation makes every key pass straight through.
    pub fn set_enabled(&self, enabled: bool) {
        self.send(Command::SetEnabled(enabled));
    }

    /// The controller ended a recording itself (for example the model failed
    /// to load), so the hotkey state machine should stop treating it as live.
    pub fn recording_ended(&self) {
        self.send(Command::RecordingEnded);
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_ok() {
            // SAFETY: posts a plain thread message; a dead thread just fails.
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_COMMANDS, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        // SAFETY: as above.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    fsm: HotkeyFsm,
    events: Sender<HookEvent>,
    commands: &Receiver<Command>,
    ready: &Sender<io::Result<u32>>,
) {
    // SAFETY: standard hook-thread setup; the hook is removed before return.
    unsafe {
        // Make sure this thread has a message queue before anyone posts to it.
        let mut message = MSG::default();
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);

        STATE.with(|state| {
            *state.borrow_mut() = Some(HookState {
                fsm,
                events,
                started: Instant::now(),
            });
        });

        let module = GetModuleHandleW(None).ok();
        let hook = match SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(hook_proc),
            module.map(|module| windows::Win32::Foundation::HINSTANCE(module.0)),
            0,
        ) {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready.send(Err(io::Error::other(error.to_string())));
                return;
            }
        };
        let _ = ready.send(Ok(GetCurrentThreadId()));

        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if message.message == WM_COMMANDS {
                while let Ok(command) = commands.try_recv() {
                    apply(command);
                }
            } else {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let _ = UnhookWindowsHookEx(hook);
        STATE.with(|state| *state.borrow_mut() = None);
    }
}

fn apply(command: Command) {
    STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            match command {
                Command::SetCombos(push_to_talk, hands_free) => {
                    state.fsm.set_combos(push_to_talk, hands_free);
                }
                Command::SetEnabled(enabled) => state.fsm.set_enabled(enabled),
                Command::RecordingEnded => state.fsm.recording_ended(),
            }
        }
    });
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // HC_ACTION == 0; anything else must be passed on untouched.
    if code == 0 {
        let key = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // Our own injected keys (paste, Start-menu suppression) never count.
        let ours = key.flags.0 & LLKHF_INJECTED != 0 && key.dwExtraInfo == EXTRA_INFO;
        if !ours {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let swallow = STATE.with(|state| {
                let Ok(mut guard) = state.try_borrow_mut() else {
                    return false;
                };
                let Some(state) = guard.as_mut() else {
                    return false;
                };
                let now = state.started.elapsed().as_secs_f64();
                let outcome = state.fsm.handle(key.vkCode as u16, down, now);
                for action in outcome.actions() {
                    // A full queue means the controller stalled; dropping an
                    // action beats blocking the keyboard.
                    let _ = state.events.try_send(HookEvent::Action(action));
                }
                outcome.swallow
            });
            if swallow {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(Some(HHOOK::default()), code, wparam, lparam)
}
