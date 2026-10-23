//! Installs the dictation hotkey hook and prints every action it produces,
//! for a fixed number of seconds. Drive it by pressing Ctrl+Win yourself, or
//! with synthetic key events from a script.
//!
//!   cargo run -p islet-input --example hook_probe -- 10

use std::time::{Duration, Instant};

use islet_input::{suppress_start_menu, HookEvent, KeyboardHook};

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let hook = KeyboardHook::start("Ctrl+Win", "Ctrl+Win+Space").expect("install hook");
    println!("hook installed for {seconds} s");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) {
        if let Ok(HookEvent::Action(action)) = hook.events().recv_timeout(Duration::from_millis(50))
        {
            println!("{:>7.3}s  {action:?}", start.elapsed().as_secs_f64());
            if action == islet_core::hotkey::Action::SuppressStartMenu {
                suppress_start_menu();
            }
        }
    }
}
