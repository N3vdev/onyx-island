//! The to-do list, its reminders and the focus timer, on the UI thread.
//!
//! The list is `tasks.toml` beside the config. Onyx's settings edit it too
//! and post [`changed_message`] to the island after writing, so neither
//! side polls. Task text stays on this PC and is never logged.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use islet_core::config::WidgetConfig;
use islet_core::tasks::{self, FocusPhase, FocusView, TaskItem, TaskLine, TaskList, TasksCard};
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;

/// Posted to the island by Onyx's settings after it changed the list.
pub fn changed_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: registers a message name.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Onyx.Tasks.Changed")) })
}

/// Longest task that can be typed into the island.
const DRAFT_LIMIT: usize = 200;

/// Something the timer brought about, for the island to show.
pub enum Happening {
    /// A reminder is due: its task id and text.
    Reminder(u64, String),
    FocusEnded,
    BreakEnded,
}

struct Run {
    phase: FocusPhase,
    ends: Instant,
    length: Duration,
}

pub struct Tasks {
    path: PathBuf,
    list: TaskList,
    pub draft: String,
    focus: Option<Run>,
    pub enabled: bool,
    focus_minutes: u32,
    break_minutes: u32,
    pub sound: bool,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

impl Tasks {
    pub fn new(path: PathBuf) -> Self {
        let list = TaskList::load(&path);
        Self {
            path,
            list,
            draft: String::new(),
            focus: None,
            enabled: true,
            focus_minutes: 25,
            break_minutes: 5,
            sound: true,
        }
    }

    pub fn configure(&mut self, config: &WidgetConfig) {
        self.enabled = config.tasks;
        self.focus_minutes = config.focus_minutes.clamp(1, 180);
        self.break_minutes = config.break_minutes.clamp(1, 60);
        self.sound = config.task_sound;
        if !self.enabled {
            self.focus = None;
        }
    }

    /// The settings changed the file.
    pub fn reload(&mut self) {
        self.list = TaskList::load(&self.path);
    }

    fn save(&self) {
        if let Err(error) = self.list.save(&self.path) {
            tracing::warn!(%error, "could not save the to-do list");
        }
    }

    /// Adds what was typed. False when it was empty.
    pub fn add_draft(&mut self) -> bool {
        let draft = std::mem::take(&mut self.draft);
        let added = self.list.add(&draft, now(), crate::clock::utc_offset()).is_some();
        if added {
            self.save();
        }
        added
    }

    /// Adds what was typed, and says what was added: the task and, with a
    /// reminder, when ("at 5:00 PM", "in 20 min").
    pub fn add_draft_described(&mut self) -> Option<(String, Option<String>)> {
        let draft = std::mem::take(&mut self.draft);
        let (now, offset) = (now(), crate::clock::utc_offset());
        let id = self.list.add(&draft, now, offset)?;
        self.save();
        let task = self.list.get(id)?;
        let when = task.remind_at.map(|at| {
            let when = tasks::when(at, now, offset, crate::clock::twelve_hour());
            if when.starts_with(|c: char| c.is_ascii_digit()) {
                format!("at {when}")
            } else {
                let mut chars = when.chars();
                chars.next().map_or_else(String::new, |first| first.to_lowercase().chain(chars).collect())
            }
        });
        Some((task.text.clone(), when))
    }

    /// A key typed while the tasks page has the keyboard. True if it
    /// changed anything.
    pub fn type_char(&mut self, c: char) -> bool {
        match c {
            '\r' | '\n' => self.add_draft(),
            '\u{8}' => self.draft.pop().is_some(),
            // Ctrl+Backspace: the last word.
            '\u{7f}' => {
                let trimmed = self.draft.trim_end().len();
                let cut = self.draft[..trimmed].rfind(' ').map_or(0, |space| space + 1);
                let changed = cut != self.draft.len();
                self.draft.truncate(cut);
                changed
            }
            '\u{1b}' => !std::mem::take(&mut self.draft).is_empty(),
            c if c.is_control() => false,
            c if self.draft.chars().count() < DRAFT_LIMIT => {
                self.draft.push(c);
                true
            }
            _ => false,
        }
    }

    pub fn paste(&mut self, text: &str) -> bool {
        let before = self.draft.len();
        for c in text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }) {
            if !c.is_control() && self.draft.chars().count() < DRAFT_LIMIT {
                self.draft.push(c);
            }
        }
        self.draft.len() != before
    }

    /// A click on the tasks page.
    pub fn click(&mut self, item: TaskItem) {
        match item {
            TaskItem::Check(id) => {
                if self.list.toggle(id) {
                    self.save();
                }
            }
            TaskItem::Remove(id) => {
                if self.list.remove(id) {
                    self.save();
                }
            }
            TaskItem::ClearDone => {
                if self.list.clear_done() {
                    self.save();
                }
            }
            TaskItem::Focus => {
                if self.focus.is_some() {
                    self.focus = None;
                } else {
                    self.start(FocusPhase::Focus);
                }
            }
            TaskItem::Input => {}
        }
    }

    pub fn complete(&mut self, id: u64) {
        if self.list.set_done(id) {
            self.save();
        }
    }

    pub fn start(&mut self, phase: FocusPhase) {
        let minutes = match phase {
            FocusPhase::Focus => self.focus_minutes,
            FocusPhase::Break => self.break_minutes,
        };
        let length = Duration::from_secs(u64::from(minutes) * 60);
        self.focus = Some(Run { phase, ends: Instant::now() + length, length });
    }

    /// Due reminders (marked shown) and a finished focus session or break.
    pub fn tick(&mut self) -> Vec<Happening> {
        let mut happened = Vec::new();
        if !self.enabled {
            return happened;
        }
        let due = self.list.due(now());
        for id in &due {
            self.list.mark_reminded(*id);
            if let Some(task) = self.list.get(*id) {
                happened.push(Happening::Reminder(*id, task.text.clone()));
            }
        }
        if !due.is_empty() {
            self.save();
        }
        if let Some(run) = self.focus.as_ref().filter(|run| Instant::now() >= run.ends) {
            happened.push(match run.phase {
                FocusPhase::Focus => Happening::FocusEnded,
                FocusPhase::Break => Happening::BreakEnded,
            });
            self.focus = None;
        }
        happened
    }

    /// How long until the timer should next look: every second while the
    /// focus timer counts, else at the next reminder (at most a minute, so
    /// sleep and clock changes are caught).
    pub fn next_wake(&self) -> Option<Duration> {
        if !self.enabled {
            return None;
        }
        if let Some(run) = &self.focus {
            let left = run.ends.saturating_duration_since(Instant::now());
            // On the second boundary, so the countdown steps evenly.
            let into = left.subsec_millis();
            return Some(Duration::from_millis(u64::from(if into == 0 { 1000 } else { into }) + 5));
        }
        self.list.next_reminder().map(|at| {
            let wait = (at - now()).clamp(0, 60) as u64;
            Duration::from_millis(wait * 1000 + 50)
        })
    }

    pub fn card(&self, typing: bool) -> TasksCard {
        let (now, offset) = (now(), crate::clock::utc_offset());
        let twelve = crate::clock::twelve_hour();
        let open = self.list.open();
        TasksCard {
            lines: open
                .iter()
                .take(tasks::LINES)
                .map(|task| TaskLine {
                    id: task.id,
                    text: task.text.clone(),
                    when: task.remind_at.map(|at| tasks::when(at, now, offset, twelve)),
                    overdue: task.remind_at.is_some_and(|at| at <= now),
                })
                .collect(),
            open: open.len(),
            done: self.list.done_count(),
            draft: self.draft.clone(),
            typing,
            focus: self.focus.as_ref().map(|run| {
                let left = run.ends.saturating_duration_since(Instant::now());
                let length = run.length.as_secs_f32().max(1.0);
                FocusView {
                    phase: run.phase,
                    // Rounded up: it reads 25:00 at the start, 0:01 at the end.
                    left: left.as_secs() as u32 + u32::from(left.subsec_nanos() > 0),
                    progress: 1.0 - left.as_secs_f32() / length,
                }
            }),
            focus_minutes: self.focus_minutes,
        }
    }

    pub fn break_minutes(&self) -> u32 {
        self.break_minutes
    }

    pub fn focus_minutes(&self) -> u32 {
        self.focus_minutes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> Tasks {
        let path = std::env::temp_dir().join(format!("onyx-app-tasks-{}-{:?}.toml", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_file(&path);
        Tasks::new(path)
    }

    #[test]
    fn typing_builds_a_task_and_enter_adds_it() {
        let mut tasks = scratch();
        for c in "buy milk extra".chars() {
            tasks.type_char(c);
        }
        assert!(tasks.type_char('\u{7f}'));
        assert_eq!(tasks.draft, "buy milk ");
        tasks.type_char('\u{8}');
        assert!(tasks.type_char('\r'));
        assert!(tasks.draft.is_empty());
        let card = tasks.card(false);
        assert_eq!(card.open, 1);
        assert_eq!(card.lines[0].text, "Buy milk");
        // Enter on nothing adds nothing.
        assert!(!tasks.type_char('\r'));
        tasks.click(TaskItem::Check(card.lines[0].id));
        assert_eq!(tasks.card(false).done, 1);
        tasks.click(TaskItem::ClearDone);
        assert_eq!(tasks.card(false).done, 0);
        let _ = std::fs::remove_file(&tasks.path);
    }

    #[test]
    fn the_focus_timer_counts_down_and_stops() {
        let mut tasks = scratch();
        tasks.configure(&WidgetConfig { focus_minutes: 25, ..WidgetConfig::default() });
        tasks.click(TaskItem::Focus);
        let focus = tasks.card(false).focus.unwrap();
        assert_eq!(focus.phase, FocusPhase::Focus);
        assert!((24 * 60..=25 * 60).contains(&focus.left));
        assert!(tasks.next_wake().unwrap() <= Duration::from_millis(1010));
        tasks.click(TaskItem::Focus);
        assert!(tasks.card(false).focus.is_none());
    }

    #[test]
    fn a_due_reminder_is_reported_once() {
        let mut tasks = scratch();
        tasks.draft = "stretch in 1 min".into();
        tasks.add_draft();
        let id = tasks.card(false).lines[0].id;
        tasks.list.snooze(id, now() - 1);
        let happened = tasks.tick();
        assert!(matches!(happened.as_slice(), [Happening::Reminder(due, text)] if *due == id && text == "Stretch"));
        assert!(tasks.tick().is_empty());
        let _ = std::fs::remove_file(&tasks.path);
    }
}
