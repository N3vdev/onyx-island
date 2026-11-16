//! The to-do list: tasks with optional reminders, and the focus timer.
//!
//! Tasks are kept in `tasks.toml` beside the config (Onyx and its settings
//! both edit it). They stay on this PC and are never logged.
//!
//! A reminder is written into the task itself: "call Sam at 5pm", "stretch
//! in 20 min", "pay rent tomorrow 9am". [`parse`] takes the time out and
//! keeps the rest as the task. Times are Unix seconds; the local clock is
//! that plus the UTC offset the caller passes, so everything here is plain
//! arithmetic and testable.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

const DAY: i64 = 86_400;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub text: String,
    #[serde(default)]
    pub done: bool,
    /// When to remind, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remind_at: Option<i64>,
    /// The reminder has been shown.
    #[serde(default)]
    pub reminded: bool,
    #[serde(default)]
    pub created: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskList {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

impl TaskList {
    /// The list in `path`; empty when there is none yet or it is unreadable.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Written to a temporary file, then renamed over the list, so a
    /// reader never sees half of it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)
    }

    /// Adds `input` (a reminder in it is taken out, see [`parse`]). `None`
    /// when there is nothing to add.
    pub fn add(&mut self, input: &str, now: i64, offset: i64) -> Option<u64> {
        let parsed = parse(input, now, offset);
        if parsed.text.is_empty() {
            return None;
        }
        self.next_id = self.next_id.max(self.tasks.iter().map(|task| task.id).max().unwrap_or(0)) + 1;
        let id = self.next_id;
        self.tasks.push(Task {
            id,
            text: parsed.text,
            done: false,
            remind_at: parsed.remind_at,
            reminded: false,
            created: now,
        });
        Some(id)
    }

    pub fn get(&self, id: u64) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    /// Ticks a task off, or back on.
    pub fn toggle(&mut self, id: u64) -> bool {
        self.get_mut(id).map(|task| task.done = !task.done).is_some()
    }

    pub fn set_done(&mut self, id: u64) -> bool {
        self.get_mut(id).map(|task| task.done = true).is_some()
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.tasks.len();
        self.tasks.retain(|task| task.id != id);
        self.tasks.len() != before
    }

    pub fn clear_done(&mut self) -> bool {
        let before = self.tasks.len();
        self.tasks.retain(|task| !task.done);
        self.tasks.len() != before
    }

    /// Reminds again at `at`.
    pub fn snooze(&mut self, id: u64, at: i64) -> bool {
        self.get_mut(id)
            .map(|task| {
                task.remind_at = Some(at);
                task.reminded = false;
            })
            .is_some()
    }

    /// What is left, soonest reminder first, then oldest first.
    #[must_use]
    pub fn open(&self) -> Vec<&Task> {
        let mut open: Vec<&Task> = self.tasks.iter().filter(|task| !task.done).collect();
        open.sort_by_key(|task| (task.remind_at.unwrap_or(i64::MAX), task.created, task.id));
        open
    }

    #[must_use]
    pub fn done_count(&self) -> usize {
        self.tasks.iter().filter(|task| task.done).count()
    }

    /// Reminders due by `now` and not shown yet, earliest first.
    #[must_use]
    pub fn due(&self, now: i64) -> Vec<u64> {
        let mut due: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|task| !task.done && !task.reminded && task.remind_at.is_some_and(|at| at <= now))
            .collect();
        due.sort_by_key(|task| task.remind_at);
        due.into_iter().map(|task| task.id).collect()
    }

    pub fn mark_reminded(&mut self, id: u64) {
        if let Some(task) = self.get_mut(id) {
            task.reminded = true;
        }
    }

    /// The next reminder still to come.
    #[must_use]
    pub fn next_reminder(&self) -> Option<i64> {
        self.tasks
            .iter()
            .filter(|task| !task.done && !task.reminded)
            .filter_map(|task| task.remind_at)
            .min()
    }
}

/// A task as typed, with its reminder taken out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub text: String,
    pub remind_at: Option<i64>,
}

/// Takes a reminder out of a task as typed:
/// - "in 20 min", "in 2 hours", "in an hour", "in half an hour"
/// - "at 5pm", "at 17:30", "at 9" (the next 9 o'clock to come)
/// - "tomorrow", "tomorrow at 9am", "tomorrow 9am" (tomorrow alone is 9 AM)
/// - "tonight" (8 PM)
///
/// "Remind me to" in front is dropped too.
#[must_use]
pub fn parse(input: &str, now: i64, offset: i64) -> Parsed {
    let mut words: Vec<String> = input.split_whitespace().map(str::to_owned).collect();
    let lower = |word: &str| word.trim_matches(|c: char| !c.is_alphanumeric() && c != ':').to_lowercase();

    for prefix in [&["remind", "me", "to"][..], &["remember", "to"][..], &["remind", "me"][..]] {
        if words.len() > prefix.len() && words.iter().zip(prefix).all(|(word, want)| lower(word) == *want) {
            words.drain(..prefix.len());
            break;
        }
    }

    let local_now = now + offset;
    let today = local_now.div_euclid(DAY) * DAY;
    let mut remind_at = None;
    let mut taken = vec![false; words.len()];

    // "in N unit", "in an hour", "in half an hour"
    for i in 0..words.len() {
        if lower(&words[i]) != "in" || taken[i] {
            continue;
        }
        let rest: Vec<String> = words[i + 1..].iter().take(4).map(|word| lower(word)).collect();
        let found = match rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["half", "an", "hour", ..] => Some((30 * 60, 3)),
            ["an", "hour", ..] | ["a", "hour", ..] => Some((60 * 60, 2)),
            [count, unit, ..] => count.parse::<f64>().ok().zip(unit_seconds(unit)).map(|(n, unit)| ((n * unit as f64) as i64, 2)),
            [single, ..] => split_amount(single).map(|seconds| (seconds, 1)),
            _ => None,
        };
        if let Some((seconds, used)) = found.filter(|(seconds, _)| *seconds > 0) {
            remind_at = Some(now + seconds);
            taken[i..=i + used].iter_mut().for_each(|flag| *flag = true);
            break;
        }
    }

    // "tomorrow" / "today" / "tonight", then "at" and a time, or a bare time
    // after a day word.
    if remind_at.is_none() {
        let mut day = None;
        let mut time = None;
        for i in 0..words.len() {
            let word = lower(&words[i]);
            match word.as_str() {
                "tomorrow" => {
                    day = Some(today + DAY);
                    taken[i] = true;
                }
                "today" => {
                    day = Some(today);
                    taken[i] = true;
                }
                "tonight" => {
                    day = Some(today);
                    taken[i] = true;
                    time.get_or_insert((20 * 3600, true));
                }
                "at" | "by" => {
                    if let Some((seconds, explicit, used)) = time_at(&words, i + 1) {
                        time = Some((seconds, explicit));
                        taken[i..=i + used].iter_mut().for_each(|flag| *flag = true);
                    }
                }
                _ if day.is_some() && time.is_none() && i > 0 && taken[i - 1] => {
                    if let Some((seconds, explicit, used)) = time_at(&words, i) {
                        time = Some((seconds, explicit));
                        taken[i..i + used].iter_mut().for_each(|flag| *flag = true);
                    }
                }
                _ => {}
            }
        }
        remind_at = match (day, time) {
            (None, None) => None,
            (Some(day), None) => Some(day + 9 * 3600 - offset),
            (day, Some((seconds, explicit))) => {
                let base = day.unwrap_or(today);
                let mut at = base + seconds;
                if !explicit && seconds < 12 * 3600 && at + 12 * 3600 > local_now && at <= local_now {
                    // "at 5" past 5 AM: 5 PM.
                    at += 12 * 3600;
                }
                if day.is_none() && at <= local_now {
                    at += DAY;
                }
                Some(at - offset)
            }
        };
    }

    let text: Vec<&str> = words
        .iter()
        .zip(&taken)
        .filter(|(_, taken)| !**taken)
        .map(|(word, _)| word.as_str())
        .collect();
    let text = text
        .join(" ")
        .trim_matches(|c: char| c.is_whitespace() || c == ',' || c == '-')
        .to_owned();
    Parsed { text: capitalise(&text), remind_at }
}

/// Seconds since 1970 of a calendar date and time, as if it were UTC
/// (Howard Hinnant's days-from-civil). The difference of the local and UTC
/// clocks read this way is the UTC offset.
#[must_use]
pub fn civil_seconds(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    days * DAY + hour * 3600 + minute * 60 + second
}

fn unit_seconds(unit: &str) -> Option<i64> {
    match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => Some(1),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(60),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(3600),
        "d" | "day" | "days" => Some(DAY),
        _ => None,
    }
}

/// "20m", "2h", "90min".
fn split_amount(word: &str) -> Option<i64> {
    let digits = word.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (count, unit) = word.split_at(digits);
    Some((count.parse::<f64>().ok()? * unit_seconds(unit)? as f64) as i64)
}

/// A time of day at `words[start..]`: (seconds into the day, whether AM or
/// PM was said, words used). "5pm", "5 pm", "17:30", "9".
fn time_at(words: &[String], start: usize) -> Option<(i64, bool, usize)> {
    let word = words.get(start)?.to_lowercase();
    let word = word.trim_matches(|c: char| !c.is_alphanumeric() && c != ':');
    let (clock, mut suffix) = match word.find(|c: char| c.is_alphabetic()) {
        Some(index) => (&word[..index], word[index..].to_owned()),
        None => (word, String::new()),
    };
    let mut used = 1;
    if suffix.is_empty() {
        if let Some(next) = words.get(start + 1) {
            let next = next.to_lowercase();
            let next = next.trim_matches(|c: char| !c.is_alphabetic());
            if matches!(next, "am" | "pm" | "a.m" | "p.m") {
                suffix = next.to_owned();
                used = 2;
            }
        }
    }
    let (hour, minute) = match clock.split_once(':') {
        Some((hour, minute)) => (hour.parse::<i64>().ok()?, minute.parse::<i64>().ok()?),
        None => (clock.parse::<i64>().ok()?, 0),
    };
    if !(0..60).contains(&minute) {
        return None;
    }
    let pm = suffix.starts_with('p');
    let am = suffix.starts_with('a');
    if !suffix.is_empty() && !pm && !am {
        return None;
    }
    let hour = match (am, pm) {
        (true, _) if hour == 12 => 0,
        (_, true) if hour < 12 => hour + 12,
        _ => hour,
    };
    if !(0..24).contains(&hour) {
        return None;
    }
    Some((hour * 3600 + minute * 60, am || pm || hour >= 13 || clock.contains(':') && hour == 0, used))
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "5:00 PM", or "17:00".
#[must_use]
pub fn clock_time(at: i64, offset: i64, twelve_hour: bool) -> String {
    let seconds = (at + offset).rem_euclid(DAY);
    let (hour, minute) = (seconds / 3600, seconds % 3600 / 60);
    if twelve_hour {
        let suffix = if hour < 12 { "AM" } else { "PM" };
        let hour = match hour % 12 {
            0 => 12,
            hour => hour,
        };
        format!("{hour}:{minute:02} {suffix}")
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

/// When a reminder is, as short as reads well: "in 12 min", "5:00 PM",
/// "Tomorrow 9:00 AM", "Mon 9:00 AM", "Overdue".
#[must_use]
pub fn when(at: i64, now: i64, offset: i64, twelve_hour: bool) -> String {
    if at <= now {
        return "Overdue".to_owned();
    }
    let left = at - now;
    if left < 3600 {
        return format!("in {} min", (left + 59) / 60);
    }
    let day = |time: i64| (time + offset).div_euclid(DAY);
    let time = clock_time(at, offset, twelve_hour);
    match day(at) - day(now) {
        0 => time,
        1 => format!("Tomorrow {time}"),
        days if days < 7 => {
            // 1 January 1970 was a Thursday.
            let name = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][day(at).rem_euclid(7) as usize];
            format!("{name} {time}")
        }
        days => format!("in {days} days"),
    }
}

/// "18:42" for a countdown.
#[must_use]
pub fn countdown(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A focus session or the break after one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusPhase {
    Focus,
    Break,
}

/// The focus timer, as the island shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusView {
    pub phase: FocusPhase,
    pub left: u32,
    /// How much of it has passed, 0..=1.
    pub progress: f32,
}

/// One task as the island shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskLine {
    pub id: u64,
    pub text: String,
    /// "5:00 PM", "in 12 min".
    pub when: Option<String>,
    pub overdue: bool,
}

/// The tasks page of the opened island, and the focus pill.
#[derive(Clone, Debug, PartialEq)]
pub struct TasksCard {
    /// The first few open tasks.
    pub lines: Vec<TaskLine>,
    /// Open tasks in all.
    pub open: usize,
    pub done: usize,
    /// What is being typed into the island.
    pub draft: String,
    /// The island has the keyboard: typing adds a task.
    pub typing: bool,
    pub focus: Option<FocusView>,
    pub focus_minutes: u32,
}

/// Something on the tasks page that can be clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskItem {
    Check(u64),
    Remove(u64),
    /// Start or stop the focus timer.
    Focus,
    /// The line where a task is typed.
    Input,
    ClearDone,
}

/// The tasks page: wide enough for a task and its time on one line.
pub const WIDTH: f32 = 440.0;
pub const HEIGHT: f32 = 268.0;
/// The focus pill: a ring and the time left.
pub const FOCUS_PILL_WIDTH: f32 = 96.0;
/// Task lines shown at once.
pub const LINES: usize = 4;

pub const PAD: f32 = 24.0;
pub const HEADER_Y: f32 = 20.0;
pub const LIST_Y: f32 = 62.0;
pub const LINE_HEIGHT: f32 = 34.0;
pub const INPUT_HEIGHT: f32 = 38.0;

impl TasksCard {
    /// The focus button, top right.
    #[must_use]
    pub fn focus_button(&self) -> Rect {
        Rect::new(WIDTH - PAD - 124.0, HEADER_Y - 4.0, 124.0, 30.0)
    }

    /// "Clear done" beside the count, when there are done tasks.
    #[must_use]
    pub fn clear_button(&self) -> Option<Rect> {
        (self.done > 0).then(|| Rect::new(WIDTH - PAD - 124.0 - 8.0 - 84.0, HEADER_Y - 4.0, 84.0, 30.0))
    }

    #[must_use]
    pub fn line_rect(index: usize) -> Rect {
        Rect::new(PAD - 8.0, LIST_Y + index as f32 * LINE_HEIGHT, WIDTH - 2.0 * PAD + 16.0, LINE_HEIGHT)
    }

    /// The remove button at the end of a line (shown while it is hovered).
    #[must_use]
    pub fn remove_rect(index: usize) -> Rect {
        let line = Self::line_rect(index);
        Rect::new(line.right() - 30.0, line.y + 4.0, 26.0, line.height - 8.0)
    }

    #[must_use]
    pub fn input_rect() -> Rect {
        Rect::new(PAD - 8.0, HEIGHT - PAD + 2.0 - INPUT_HEIGHT, WIDTH - 2.0 * PAD + 16.0, INPUT_HEIGHT)
    }

    /// What is under (`x`, `y`), in the page's own coordinates.
    #[must_use]
    pub fn item_at(&self, x: f32, y: f32) -> Option<TaskItem> {
        let inside = |rect: Rect| x >= rect.x && x <= rect.right() && y >= rect.y && y <= rect.bottom();
        if inside(self.focus_button()) {
            return Some(TaskItem::Focus);
        }
        if self.clear_button().is_some_and(inside) {
            return Some(TaskItem::ClearDone);
        }
        if inside(Self::input_rect()) {
            return Some(TaskItem::Input);
        }
        for (index, line) in self.lines.iter().enumerate().take(LINES) {
            if inside(Self::remove_rect(index)) {
                return Some(TaskItem::Remove(line.id));
            }
            if inside(Self::line_rect(index)) {
                return Some(TaskItem::Check(line.id));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday 11 November 2026, 14:30 local, at UTC+5:30.
    const OFFSET: i64 = 5 * 3600 + 1800;
    const NOW: i64 = 20_768 * DAY - OFFSET + 14 * 3600 + 1800;

    fn local(at: i64) -> (i64, i64) {
        let seconds = (at + OFFSET).rem_euclid(DAY);
        ((at + OFFSET).div_euclid(DAY) - (NOW + OFFSET).div_euclid(DAY), seconds / 60)
    }

    #[test]
    fn the_clock_in_the_tests_is_what_it_says() {
        assert_eq!(local(NOW), (0, 14 * 60 + 30));
        assert_eq!(when(NOW + DAY * 5, NOW, OFFSET, true), "Mon 2:30 PM");
    }

    #[test]
    fn a_task_without_a_time_is_kept_as_typed() {
        let parsed = parse("buy milk", NOW, OFFSET);
        assert_eq!(parsed, Parsed { text: "Buy milk".into(), remind_at: None });
        // An "at" that is no time stays in the task.
        assert_eq!(parse("look at the report", NOW, OFFSET).text, "Look at the report");
        assert_eq!(parse("meet in person", NOW, OFFSET).text, "Meet in person");
    }

    #[test]
    fn relative_times_count_from_now() {
        let parsed = parse("stretch in 20 min", NOW, OFFSET);
        assert_eq!(parsed, Parsed { text: "Stretch".into(), remind_at: Some(NOW + 1200) });
        assert_eq!(parse("in 2 hours call back", NOW, OFFSET).remind_at, Some(NOW + 7200));
        assert_eq!(parse("tea in 5m", NOW, OFFSET).remind_at, Some(NOW + 300));
        assert_eq!(parse("check oven in half an hour", NOW, OFFSET).remind_at, Some(NOW + 1800));
        assert_eq!(parse("check oven in an hour", NOW, OFFSET).text, "Check oven");
    }

    #[test]
    fn clock_times_are_the_next_one_to_come() {
        let at = |input| local(parse(input, NOW, OFFSET).remind_at.unwrap());
        assert_eq!(at("call Sam at 5pm"), (0, 17 * 60));
        assert_eq!(at("call Sam at 5 pm"), (0, 17 * 60));
        assert_eq!(at("standup at 17:30"), (0, 17 * 60 + 30));
        // 9 has passed this morning and 9 PM is to come.
        assert_eq!(at("call at 9"), (0, 21 * 60));
        // 9 AM said: tomorrow's.
        assert_eq!(at("call at 9am"), (1, 9 * 60));
        assert_eq!(at("lunch at 12pm"), (1, 12 * 60));
        assert_eq!(parse("call Sam at 5pm", NOW, OFFSET).text, "Call Sam");
    }

    #[test]
    fn day_words_set_the_day() {
        let at = |input| local(parse(input, NOW, OFFSET).remind_at.unwrap());
        assert_eq!(at("pay rent tomorrow"), (1, 9 * 60));
        assert_eq!(at("pay rent tomorrow 8am"), (1, 8 * 60));
        assert_eq!(at("pay rent tomorrow at 6:15 pm"), (1, 18 * 60 + 15));
        assert_eq!(at("call mum tonight"), (0, 20 * 60));
        assert_eq!(parse("pay rent tomorrow 8am", NOW, OFFSET).text, "Pay rent");
        assert_eq!(parse("remind me to water the plants tomorrow", NOW, OFFSET).text, "Water the plants");
    }

    #[test]
    fn reminders_come_due_once_and_can_be_snoozed() {
        let mut list = TaskList::default();
        let a = list.add("first in 10 min", NOW, OFFSET).unwrap();
        let b = list.add("plain", NOW, OFFSET).unwrap();
        assert!(list.add("   ", NOW, OFFSET).is_none());
        assert_eq!(list.next_reminder(), Some(NOW + 600));
        assert!(list.due(NOW).is_empty());
        assert_eq!(list.due(NOW + 600), vec![a]);
        list.mark_reminded(a);
        assert!(list.due(NOW + 700).is_empty());
        list.snooze(a, NOW + 1200);
        assert_eq!(list.due(NOW + 1200), vec![a]);
        // The task with a reminder comes first.
        assert_eq!(list.open().iter().map(|task| task.id).collect::<Vec<_>>(), vec![a, b]);
        list.toggle(b);
        assert_eq!(list.done_count(), 1);
        assert!(list.clear_done());
        assert_eq!(list.tasks.len(), 1);
        assert!(list.remove(a));
        // Ids are never reused.
        assert!(list.add("again", NOW, OFFSET).unwrap() > b);
    }

    #[test]
    fn the_list_saves_and_loads() {
        let path = std::env::temp_dir().join(format!("onyx-tasks-test-{}.toml", std::process::id()));
        let mut list = TaskList::default();
        list.add("Buy milk", NOW, OFFSET);
        list.add("Call Sam at 5pm", NOW, OFFSET);
        list.save(&path).unwrap();
        assert_eq!(TaskList::load(&path), list);
        let _ = std::fs::remove_file(&path);
        assert_eq!(TaskList::load(&path), TaskList::default());
    }

    #[test]
    fn calendar_times_count_from_1970() {
        assert_eq!(civil_seconds(1970, 1, 1, 0, 0, 0), 0);
        assert_eq!(civil_seconds(2000, 3, 1, 0, 0, 0), 951_868_800);
        assert_eq!(civil_seconds(2026, 11, 13, 12, 0, 0), 1_794_571_200);
        assert_eq!(civil_seconds(2026, 11, 11, 0, 0, 0), 20_768 * DAY);
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(when(NOW + 12 * 60, NOW, OFFSET, true), "in 12 min");
        assert_eq!(when(NOW + 3 * 3600, NOW, OFFSET, true), "5:30 PM");
        assert_eq!(when(NOW + 3 * 3600, NOW, OFFSET, false), "17:30");
        assert_eq!(when(NOW + 20 * 3600, NOW, OFFSET, true), "Tomorrow 10:30 AM");
        assert_eq!(when(NOW - 1, NOW, OFFSET, true), "Overdue");
        assert_eq!(countdown(25 * 60), "25:00");
        assert_eq!(countdown(61), "1:01");
    }

    #[test]
    fn clicks_find_their_item() {
        let card = TasksCard {
            lines: vec![
                TaskLine { id: 7, text: "A".into(), when: None, overdue: false },
                TaskLine { id: 9, text: "B".into(), when: None, overdue: false },
            ],
            open: 2,
            done: 1,
            draft: String::new(),
            typing: false,
            focus: None,
            focus_minutes: 25,
        };
        let center = |rect: Rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let (x, y) = center(card.focus_button());
        assert_eq!(card.item_at(x, y), Some(TaskItem::Focus));
        let (x, y) = center(card.clear_button().unwrap());
        assert_eq!(card.item_at(x, y), Some(TaskItem::ClearDone));
        let (x, y) = center(TasksCard::line_rect(1));
        assert_eq!(card.item_at(x, y), Some(TaskItem::Check(9)));
        let (x, y) = center(TasksCard::remove_rect(0));
        assert_eq!(card.item_at(x, y), Some(TaskItem::Remove(7)));
        let (x, y) = center(TasksCard::input_rect());
        assert_eq!(card.item_at(x, y), Some(TaskItem::Input));
        // An empty line is nothing.
        let (x, y) = center(TasksCard::line_rect(3));
        assert_eq!(card.item_at(x, y), None);
        // Nothing overlaps: the input sits below the last line.
        assert!(TasksCard::line_rect(LINES - 1).bottom() <= TasksCard::input_rect().y);
    }
}
