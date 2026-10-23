//! Dictation hotkeys: combo parsing and the press/release state machine.
//!
//! The keyboard hook must decide *synchronously* whether to swallow a key, so
//! [`HotkeyFsm::handle`] runs inside the hook callback. It therefore never
//! allocates, locks, or logs: all state is fixed-size and the result is a
//! small `Copy` value. Keys are Windows virtual-key codes, which are plain
//! numbers, so this module stays platform-free and fully testable.

use std::fmt;

pub const VK_BACK: u16 = 0x08;
pub const VK_TAB: u16 = 0x09;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_SPACE: u16 = 0x20;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;

/// A press shorter than this is an accidental tap, not a dictation.
pub const MIN_HOLD_SECONDS: f64 = 0.25;
/// Two taps within this window start hands-free dictation.
pub const DOUBLE_TAP_SECONDS: f64 = 0.4;

/// A set of modifier keys (either side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const CTRL: Self = Self(1);
    pub const SHIFT: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const WIN: Self = Self(8);
    pub const NONE: Self = Self(0);

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Which modifier a virtual-key code is, if any.
#[must_use]
pub const fn modifier_of(vk: u16) -> Option<Modifiers> {
    match vk {
        VK_LCONTROL | VK_RCONTROL | VK_CONTROL => Some(Modifiers::CTRL),
        VK_LSHIFT | VK_RSHIFT | VK_SHIFT => Some(Modifiers::SHIFT),
        VK_LMENU | VK_RMENU | VK_MENU => Some(Modifiers::ALT),
        VK_LWIN | VK_RWIN => Some(Modifiers::WIN),
        _ => None,
    }
}

/// A hotkey: modifiers, plus at most one ordinary key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub modifiers: Modifiers,
    pub key: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    Empty,
    UnknownKey(String),
    /// More than one non-modifier key.
    TooManyKeys,
    /// A bare key (other than F1-F24) would fire while typing.
    NeedsModifier,
    /// A modifiers-only combo needs at least two, or it fires constantly.
    NeedsTwoModifiers,
    /// Esc cancels a dictation, so it cannot be a hotkey.
    ReservedKey,
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "press at least one key"),
            Self::UnknownKey(name) => write!(f, "`{name}` is not a key Islet knows"),
            Self::TooManyKeys => write!(f, "use one key plus modifiers (Ctrl, Alt, Shift, Win)"),
            Self::NeedsModifier => write!(f, "add a modifier such as Ctrl or Alt"),
            Self::NeedsTwoModifiers => write!(f, "use two modifiers, or add a key"),
            Self::ReservedKey => write!(f, "Esc is reserved for cancelling a dictation"),
        }
    }
}

impl std::error::Error for HotkeyError {}

/// Key names, in the form shown in settings and stored in `config.toml`.
const NAMED_KEYS: &[(&str, u16)] = &[
    ("Space", VK_SPACE),
    ("Enter", VK_RETURN),
    ("Tab", VK_TAB),
    ("Backspace", VK_BACK),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("`", 0xC0),
    ("-", 0xBD),
    ("=", 0xBB),
    (",", 0xBC),
    (".", 0xBE),
    ("/", 0xBF),
    (";", 0xBA),
    ("'", 0xDE),
    ("[", 0xDB),
    ("]", 0xDD),
    ("\\", 0xDC),
];

fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => Some(char::from(vk as u8).to_string()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x6F)),
        _ => NAMED_KEYS
            .iter()
            .find(|(_, code)| *code == vk)
            .map(|(name, _)| (*name).to_owned()),
    }
}

fn key_code(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if bytes.len() == 1 && (bytes[0].is_ascii_uppercase() || bytes[0].is_ascii_digit()) {
        return Some(u16::from(bytes[0]));
    }
    if let Some(number) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        if (1..=24).contains(&number) {
            return Some(0x6F + number);
        }
    }
    let wanted = match upper.as_str() {
        "ESC" | "ESCAPE" => return Some(VK_ESCAPE),
        "RETURN" => "Enter",
        "BACKTICK" | "GRAVE" => "`",
        "PGUP" => "PageUp",
        "PGDN" => "PageDown",
        "DEL" => "Delete",
        "INS" => "Insert",
        other => other,
    };
    NAMED_KEYS
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(wanted))
        .map(|(_, code)| *code)
}

/// The settings capture widget turns a pressed key into the config name.
#[must_use]
pub fn name_for_vk(vk: u16) -> Option<String> {
    key_name(vk)
}

impl Combo {
    /// Parses `"Ctrl+Win"`, `"Ctrl+Alt+D"`, `"F9"`, case-insensitively.
    pub fn parse(text: &str) -> Result<Self, HotkeyError> {
        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        let mut any = false;
        for part in text.split('+') {
            let part = part.trim();
            if part.is_empty() {
                // "Ctrl++" means Ctrl and the plus key; a plus is not a
                // supported key, so treat stray separators as empty input.
                continue;
            }
            any = true;
            let modifier = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Some(Modifiers::CTRL),
                "shift" => Some(Modifiers::SHIFT),
                "alt" | "option" => Some(Modifiers::ALT),
                "win" | "windows" | "super" | "meta" | "cmd" => Some(Modifiers::WIN),
                _ => None,
            };
            if let Some(modifier) = modifier {
                modifiers = modifiers.union(modifier);
                continue;
            }
            let code = key_code(part).ok_or_else(|| HotkeyError::UnknownKey(part.to_owned()))?;
            if code == VK_ESCAPE {
                return Err(HotkeyError::ReservedKey);
            }
            if key.replace(code).is_some() {
                return Err(HotkeyError::TooManyKeys);
            }
        }
        if !any {
            return Err(HotkeyError::Empty);
        }
        match key {
            None if modifiers.count() < 2 => return Err(HotkeyError::NeedsTwoModifiers),
            Some(code) if modifiers.is_empty() && !(0x70..=0x87).contains(&code) => {
                return Err(HotkeyError::NeedsModifier)
            }
            _ => {}
        }
        Ok(Self { modifiers, key })
    }
}

impl fmt::Display for Combo {
    /// The canonical form, which `parse` accepts back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        let mut part = |f: &mut fmt::Formatter<'_>, text: &str| -> fmt::Result {
            if !first {
                write!(f, "+")?;
            }
            first = false;
            write!(f, "{text}")
        };
        for (modifier, name) in [
            (Modifiers::CTRL, "Ctrl"),
            (Modifiers::ALT, "Alt"),
            (Modifiers::SHIFT, "Shift"),
            (Modifiers::WIN, "Win"),
        ] {
            if self.modifiers.contains(modifier) {
                part(f, name)?;
            }
        }
        if let Some(name) = self.key.and_then(key_name) {
            part(f, &name)?;
        }
        Ok(())
    }
}

/// What the dictation controller should do. Delivered in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Begin recording.
    Start,
    /// A hold or double-tap turned into hands-free recording.
    EnterHandsFree,
    /// Finish: transcribe and insert.
    Stop,
    /// Throw the recording away.
    Discard,
    /// Send a harmless key so Windows does not open the Start menu when the
    /// Win key is released.
    SuppressStartMenu,
}

/// The result of one key event. `Copy` and allocation-free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Do not pass this key on to applications.
    pub swallow: bool,
    actions: [Option<Action>; 3],
}

impl Outcome {
    fn push(&mut self, action: Action) {
        if let Some(slot) = self.actions.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(action);
        }
    }

    pub fn actions(&self) -> impl Iterator<Item = Action> + '_ {
        self.actions.iter().flatten().copied()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.swallow && self.actions[0].is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    /// The push-to-talk combo is held and recording.
    Ptt {
        since: f64,
        double_tap: bool,
    },
    HandsFree,
    /// A dictation ended or was interrupted; ignore keys until all are up.
    Blocked,
}

/// What the dictation hotkeys are doing right now, for the island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyState {
    Idle,
    Recording { hands_free: bool },
}

const MAX_KEYS: usize = 8;

/// Tracks held keys and turns presses into dictation actions.
#[derive(Clone, Debug)]
pub struct HotkeyFsm {
    push_to_talk: Option<Combo>,
    hands_free: Option<Combo>,
    enabled: bool,
    /// One bit per modifier virtual key (LShift..RWin), so left and right
    /// are tracked separately and released independently.
    modifier_keys: u8,
    keys: [u16; MAX_KEYS],
    key_count: usize,
    /// Keys whose key-up must also be swallowed.
    swallowed: [u16; 4],
    phase: Phase,
    last_tap_release: f64,
}

fn modifier_bit(vk: u16) -> Option<u8> {
    Some(match vk {
        VK_LSHIFT | VK_SHIFT => 1,
        VK_RSHIFT => 2,
        VK_LCONTROL | VK_CONTROL => 4,
        VK_RCONTROL => 8,
        VK_LMENU | VK_MENU => 16,
        VK_RMENU => 32,
        VK_LWIN => 64,
        VK_RWIN => 128,
        _ => return None,
    })
}

impl HotkeyFsm {
    #[must_use]
    pub fn new(push_to_talk: Option<Combo>, hands_free: Option<Combo>) -> Self {
        Self {
            push_to_talk,
            hands_free,
            enabled: true,
            modifier_keys: 0,
            keys: [0; MAX_KEYS],
            key_count: 0,
            swallowed: [0; 4],
            phase: Phase::Idle,
            last_tap_release: f64::NEG_INFINITY,
        }
    }

    /// Builds from config strings. An unparsable combo disables that hotkey.
    #[must_use]
    pub fn from_strings(push_to_talk: &str, hands_free: &str) -> Self {
        Self::new(
            Combo::parse(push_to_talk).ok(),
            Combo::parse(hands_free).ok(),
        )
    }

    pub fn set_combos(&mut self, push_to_talk: Option<Combo>, hands_free: Option<Combo>) {
        self.push_to_talk = push_to_talk;
        self.hands_free = hands_free;
    }

    /// Pausing dictation (tray menu) makes every key pass through.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    #[must_use]
    pub fn state(&self) -> HotkeyState {
        match self.phase {
            Phase::Ptt { .. } => HotkeyState::Recording { hands_free: false },
            Phase::HandsFree => HotkeyState::Recording { hands_free: true },
            Phase::Idle | Phase::Blocked => HotkeyState::Idle,
        }
    }

    /// The controller ended a recording on its own (a model error, say).
    pub fn recording_ended(&mut self) {
        if matches!(self.phase, Phase::Ptt { .. } | Phase::HandsFree) {
            self.finish();
        }
    }

    fn modifiers_down(&self) -> Modifiers {
        let bits = self.modifier_keys;
        let mut mods = Modifiers::NONE;
        if bits & 0b0000_0011 != 0 {
            mods = mods.union(Modifiers::SHIFT);
        }
        if bits & 0b0000_1100 != 0 {
            mods = mods.union(Modifiers::CTRL);
        }
        if bits & 0b0011_0000 != 0 {
            mods = mods.union(Modifiers::ALT);
        }
        if bits & 0b1100_0000 != 0 {
            mods = mods.union(Modifiers::WIN);
        }
        mods
    }

    fn nothing_held(&self) -> bool {
        self.modifier_keys == 0 && self.key_count == 0
    }

    fn only_key_held(&self, vk: u16) -> bool {
        self.key_count == 1 && self.keys[0] == vk
    }

    fn press(&mut self, vk: u16) {
        if let Some(bit) = modifier_bit(vk) {
            self.modifier_keys |= bit;
        } else if !self.keys[..self.key_count].contains(&vk) && self.key_count < MAX_KEYS {
            self.keys[self.key_count] = vk;
            self.key_count += 1;
        }
    }

    fn release(&mut self, vk: u16) {
        if let Some(bit) = modifier_bit(vk) {
            self.modifier_keys &= !bit;
        } else if let Some(index) = self.keys[..self.key_count].iter().position(|&k| k == vk) {
            self.keys.copy_within(index + 1..self.key_count, index);
            self.key_count -= 1;
        }
    }

    fn is_held(&self, vk: u16) -> bool {
        match modifier_bit(vk) {
            Some(bit) => self.modifier_keys & bit != 0,
            None => self.keys[..self.key_count].contains(&vk),
        }
    }

    fn swallow_release_of(&mut self, vk: u16) {
        if !self.swallowed.contains(&vk) {
            if let Some(slot) = self.swallowed.iter_mut().find(|slot| **slot == 0) {
                *slot = vk;
            }
        }
    }

    /// Ends the current dictation; stays blocked while keys are still down
    /// so a half-released combo cannot immediately re-trigger.
    fn finish(&mut self) {
        self.phase = if self.nothing_held() {
            Phase::Idle
        } else {
            Phase::Blocked
        };
    }

    fn hands_free_pressed(&self, vk: u16) -> bool {
        self.hands_free.is_some_and(|combo| {
            combo.key == Some(vk) && self.modifiers_down() == combo.modifiers && self.key_count == 1
        })
    }

    fn push_to_talk_triggered(&self, vk: u16) -> bool {
        let Some(combo) = self.push_to_talk else {
            return false;
        };
        if self.modifiers_down() != combo.modifiers {
            return false;
        }
        match combo.key {
            // Modifiers-only: the press that completes the set triggers it,
            // and only if no ordinary key is down (typing Ctrl+Win+... first
            // must not start a dictation).
            None => modifier_of(vk).is_some() && self.key_count == 0,
            Some(key) => vk == key && self.only_key_held(vk),
        }
    }

    /// Feeds one key event. `now` is monotonic seconds.
    pub fn handle(&mut self, vk: u16, down: bool, now: f64) -> Outcome {
        if down {
            self.key_down(vk, now)
        } else {
            self.key_up(vk, now)
        }
    }

    fn key_down(&mut self, vk: u16, now: f64) -> Outcome {
        let mut out = Outcome::default();
        let repeat = self.is_held(vk);
        self.press(vk);
        if !self.enabled {
            return out;
        }
        if repeat {
            // Auto-repeat of a held key: only keep swallowing what we took.
            out.swallow = self.swallowed.contains(&vk);
            return out;
        }

        let recording = matches!(self.phase, Phase::Ptt { .. } | Phase::HandsFree);
        if vk == VK_ESCAPE && recording {
            out.push(Action::Discard);
            out.swallow = true;
            self.swallow_release_of(vk);
            self.finish();
            return out;
        }

        match self.phase {
            Phase::Idle => {
                if self.hands_free_pressed(vk) {
                    self.begin_hands_free(vk, &mut out);
                } else if self.push_to_talk_triggered(vk) {
                    let double_tap = now - self.last_tap_release <= DOUBLE_TAP_SECONDS;
                    self.phase = Phase::Ptt {
                        since: now,
                        double_tap,
                    };
                    out.push(Action::Start);
                    if self
                        .push_to_talk
                        .is_some_and(|c| c.modifiers.contains(Modifiers::WIN))
                    {
                        out.push(Action::SuppressStartMenu);
                    }
                    if self.push_to_talk.is_some_and(|c| c.key == Some(vk)) {
                        out.swallow = true;
                        self.swallow_release_of(vk);
                    }
                }
            }
            Phase::Ptt { .. } => {
                if self.hands_free_pressed(vk) {
                    self.phase = Phase::HandsFree;
                    out.push(Action::EnterHandsFree);
                    out.swallow = true;
                    self.swallow_release_of(vk);
                } else if self.push_to_talk.is_some_and(|c| c.key == Some(vk)) {
                    // The push-to-talk key itself (repeat or re-press).
                    out.swallow = true;
                } else {
                    // Another key or modifier joined: the user wants some
                    // other shortcut, not a dictation.
                    out.push(Action::Discard);
                    self.finish();
                }
            }
            Phase::HandsFree => {
                if self.hands_free_pressed(vk) {
                    out.push(Action::Stop);
                    out.swallow = true;
                    self.swallow_release_of(vk);
                    self.finish();
                }
            }
            Phase::Blocked => {}
        }
        out
    }

    fn begin_hands_free(&mut self, vk: u16, out: &mut Outcome) {
        self.phase = Phase::HandsFree;
        out.push(Action::Start);
        out.push(Action::EnterHandsFree);
        out.swallow = true;
        self.swallow_release_of(vk);
        if self
            .hands_free
            .is_some_and(|c| c.modifiers.contains(Modifiers::WIN))
        {
            out.push(Action::SuppressStartMenu);
        }
    }

    fn key_up(&mut self, vk: u16, now: f64) -> Outcome {
        let mut out = Outcome::default();
        self.release(vk);
        if let Some(slot) = self.swallowed.iter_mut().find(|slot| **slot == vk) {
            *slot = 0;
            out.swallow = true;
        }
        if !self.enabled {
            return out;
        }

        match self.phase {
            Phase::Ptt { since, double_tap } => {
                let combo_still_held = self.push_to_talk.is_some_and(|combo| {
                    self.modifiers_down() == combo.modifiers
                        && combo.key.is_none_or(|key| self.is_held(key))
                });
                if combo_still_held {
                    return out;
                }
                let held_for = now - since;
                if held_for >= MIN_HOLD_SECONDS {
                    out.push(Action::Stop);
                    self.last_tap_release = f64::NEG_INFINITY;
                    self.finish();
                } else if double_tap {
                    self.phase = Phase::HandsFree;
                    out.push(Action::EnterHandsFree);
                    self.last_tap_release = f64::NEG_INFINITY;
                } else {
                    out.push(Action::Discard);
                    self.last_tap_release = now;
                    self.finish();
                }
            }
            Phase::Blocked => {
                if self.nothing_held() {
                    self.phase = Phase::Idle;
                }
            }
            Phase::Idle | Phase::HandsFree => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: u16 = VK_LCONTROL;
    const WIN: u16 = VK_LWIN;

    fn fsm() -> HotkeyFsm {
        HotkeyFsm::from_strings("Ctrl+Win", "Ctrl+Win+Space")
    }

    /// Presses a list of keys in order at `t`, returning every outcome.
    fn press(fsm: &mut HotkeyFsm, keys: &[u16], t: f64) -> Vec<Outcome> {
        keys.iter().map(|&vk| fsm.handle(vk, true, t)).collect()
    }

    fn release(fsm: &mut HotkeyFsm, keys: &[u16], t: f64) -> Vec<Outcome> {
        keys.iter().map(|&vk| fsm.handle(vk, false, t)).collect()
    }

    fn all(outcomes: &[Outcome]) -> Vec<Action> {
        outcomes.iter().flat_map(|o| o.actions()).collect()
    }

    #[test]
    fn parses_and_formats_combos() {
        let combo = Combo::parse("ctrl + WIN").unwrap();
        assert_eq!(combo.modifiers, Modifiers::CTRL.union(Modifiers::WIN));
        assert_eq!(combo.key, None);
        assert_eq!(combo.to_string(), "Ctrl+Win");
        assert_eq!(
            Combo::parse("Win+Ctrl+Space").unwrap().to_string(),
            "Ctrl+Win+Space"
        );
        assert_eq!(
            Combo::parse("alt+shift+f9").unwrap().to_string(),
            "Alt+Shift+F9"
        );
        assert_eq!(Combo::parse("F9").unwrap().key, Some(0x78));
        assert_eq!(Combo::parse("Ctrl+`").unwrap().key, Some(0xC0));
        // Canonical output parses back to itself.
        for text in [
            "Ctrl+Win",
            "Ctrl+Alt+D",
            "Win+Shift+F12",
            "Ctrl+Space",
            "Ctrl+PageUp",
        ] {
            let combo = Combo::parse(text).unwrap();
            assert_eq!(Combo::parse(&combo.to_string()).unwrap(), combo, "{text}");
        }
    }

    #[test]
    fn rejects_bad_combos() {
        assert_eq!(Combo::parse(""), Err(HotkeyError::Empty));
        assert_eq!(Combo::parse("  + "), Err(HotkeyError::Empty));
        assert_eq!(Combo::parse("Ctrl"), Err(HotkeyError::NeedsTwoModifiers));
        assert_eq!(Combo::parse("A"), Err(HotkeyError::NeedsModifier));
        assert_eq!(Combo::parse("Ctrl+A+B"), Err(HotkeyError::TooManyKeys));
        assert_eq!(Combo::parse("Ctrl+Esc"), Err(HotkeyError::ReservedKey));
        assert!(matches!(
            Combo::parse("Ctrl+Banana"),
            Err(HotkeyError::UnknownKey(_))
        ));
    }

    #[test]
    fn key_names_round_trip() {
        for vk in [
            0x41u16, 0x5A, 0x30, 0x39, 0x70, 0x87, VK_SPACE, 0x25, 0xC0, 0xDC,
        ] {
            let name = name_for_vk(vk).unwrap();
            assert_eq!(key_code(&name), Some(vk), "{name}");
        }
        assert_eq!(name_for_vk(0xE8), None);
    }

    #[test]
    fn holding_the_combo_records_until_release() {
        let mut fsm = fsm();
        let down = press(&mut fsm, &[CTRL, WIN], 1.0);
        assert_eq!(all(&down), [Action::Start, Action::SuppressStartMenu]);
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: false });
        // Modifiers are never swallowed.
        assert!(down.iter().all(|o| !o.swallow));

        let up = release(&mut fsm, &[WIN, CTRL], 2.0);
        assert_eq!(all(&up), [Action::Stop]);
        assert_eq!(fsm.state(), HotkeyState::Idle);
    }

    #[test]
    fn order_and_side_of_modifiers_do_not_matter() {
        let mut fsm = fsm();
        let down = press(&mut fsm, &[VK_RWIN, VK_RCONTROL], 0.0);
        assert_eq!(all(&down), [Action::Start, Action::SuppressStartMenu]);
        assert_eq!(
            all(&release(&mut fsm, &[VK_RCONTROL, VK_RWIN], 1.0)),
            [Action::Stop]
        );
    }

    #[test]
    fn a_short_tap_is_discarded() {
        let mut fsm = fsm();
        press(&mut fsm, &[CTRL, WIN], 1.0);
        let up = release(&mut fsm, &[WIN, CTRL], 1.1);
        assert_eq!(all(&up), [Action::Discard]);
        assert_eq!(fsm.state(), HotkeyState::Idle);
    }

    #[test]
    fn double_tap_enters_hands_free_and_the_combo_stops_it() {
        let mut fsm = fsm();
        press(&mut fsm, &[CTRL, WIN], 1.0);
        release(&mut fsm, &[WIN, CTRL], 1.1);

        // Second tap within the window.
        let second = press(&mut fsm, &[CTRL, WIN], 1.3);
        assert_eq!(all(&second), [Action::Start, Action::SuppressStartMenu]);
        let up = release(&mut fsm, &[WIN, CTRL], 1.4);
        assert_eq!(all(&up), [Action::EnterHandsFree]);
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: true });

        // Ctrl+Win+Space stops it.
        press(&mut fsm, &[CTRL, WIN], 5.0);
        let stop = press(&mut fsm, &[VK_SPACE], 5.1);
        assert_eq!(all(&stop), [Action::Stop]);
        assert!(stop[0].swallow);
        assert_eq!(fsm.state(), HotkeyState::Idle);
    }

    #[test]
    fn taps_too_far_apart_are_not_a_double_tap() {
        let mut fsm = fsm();
        press(&mut fsm, &[CTRL, WIN], 1.0);
        release(&mut fsm, &[WIN, CTRL], 1.1);
        press(&mut fsm, &[CTRL, WIN], 3.0);
        let up = release(&mut fsm, &[WIN, CTRL], 3.1);
        assert_eq!(all(&up), [Action::Discard]);
    }

    #[test]
    fn space_while_holding_switches_to_hands_free() {
        let mut fsm = fsm();
        press(&mut fsm, &[CTRL, WIN], 1.0);
        let space = press(&mut fsm, &[VK_SPACE], 1.5);
        assert_eq!(all(&space), [Action::EnterHandsFree]);
        assert!(space[0].swallow);
        // Space's key-up is swallowed too, and releasing the modifiers does
        // not stop a hands-free dictation.
        assert!(release(&mut fsm, &[VK_SPACE], 1.6)[0].swallow);
        assert!(all(&release(&mut fsm, &[WIN, CTRL], 1.7)).is_empty());
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: true });

        // Toggle off.
        press(&mut fsm, &[CTRL, WIN], 9.0);
        assert_eq!(all(&press(&mut fsm, &[VK_SPACE], 9.1)), [Action::Stop]);
    }

    #[test]
    fn another_key_during_the_hold_cancels_and_passes_through() {
        let mut fsm = fsm();
        press(&mut fsm, &[CTRL, WIN], 1.0);
        let left = press(&mut fsm, &[0x25], 1.2); // Ctrl+Win+Left
        assert_eq!(all(&left), [Action::Discard]);
        assert!(!left[0].swallow, "the other shortcut must still work");
        assert_eq!(fsm.state(), HotkeyState::Idle);
        // Releasing everything does not start or stop anything.
        assert!(all(&release(&mut fsm, &[0x25, WIN, CTRL], 1.5)).is_empty());
        // And the combo works again afterwards.
        assert_eq!(
            all(&press(&mut fsm, &[CTRL, WIN], 3.0)),
            [Action::Start, Action::SuppressStartMenu]
        );
    }

    #[test]
    fn typing_before_the_combo_does_not_trigger() {
        let mut fsm = fsm();
        press(&mut fsm, &[0x41], 1.0); // 'A' held
        assert!(all(&press(&mut fsm, &[CTRL, WIN], 1.1)).is_empty());
        assert_eq!(fsm.state(), HotkeyState::Idle);
    }

    #[test]
    fn extra_modifier_does_not_trigger() {
        let mut fsm = fsm();
        assert!(all(&press(&mut fsm, &[VK_LSHIFT, CTRL, WIN], 1.0)).is_empty());
    }

    #[test]
    fn escape_discards_and_is_swallowed_only_while_recording() {
        let mut fsm = fsm();
        // Idle: Esc passes through untouched.
        assert!(fsm.handle(VK_ESCAPE, true, 0.5).is_empty());
        fsm.handle(VK_ESCAPE, false, 0.6);

        press(&mut fsm, &[CTRL, WIN], 1.0);
        let esc = fsm.handle(VK_ESCAPE, true, 1.5);
        assert_eq!(esc.actions().collect::<Vec<_>>(), [Action::Discard]);
        assert!(esc.swallow);
        assert!(fsm.handle(VK_ESCAPE, false, 1.6).swallow);
        assert_eq!(fsm.state(), HotkeyState::Idle);
        // Releasing the held combo afterwards does nothing.
        assert!(all(&release(&mut fsm, &[WIN, CTRL], 2.0)).is_empty());
    }

    #[test]
    fn releasing_one_modifier_ends_the_hold() {
        let mut fsm = fsm();
        press(&mut fsm, &[CTRL, WIN], 1.0);
        assert_eq!(all(&release(&mut fsm, &[CTRL], 2.0)), [Action::Stop]);
        // Pressing Ctrl again while Win is still down must not restart.
        assert!(all(&press(&mut fsm, &[CTRL], 2.1)).is_empty());
        release(&mut fsm, &[CTRL, WIN], 2.5);
        assert_eq!(
            all(&press(&mut fsm, &[CTRL, WIN], 3.0)),
            [Action::Start, Action::SuppressStartMenu]
        );
    }

    #[test]
    fn key_combo_triggers_and_swallows_its_key() {
        let mut fsm = HotkeyFsm::from_strings("Ctrl+Alt+D", "Ctrl+Alt+H");
        press(&mut fsm, &[CTRL, VK_LMENU], 1.0);
        let d = press(&mut fsm, &[0x44], 1.1);
        assert_eq!(all(&d), [Action::Start]);
        assert!(d[0].swallow);
        // Auto-repeat of D stays swallowed and does nothing else.
        let repeat = fsm.handle(0x44, true, 1.3);
        assert!(repeat.swallow && repeat.actions().next().is_none());
        let up = fsm.handle(0x44, false, 1.8);
        assert_eq!(up.actions().collect::<Vec<_>>(), [Action::Stop]);
        assert!(up.swallow);
    }

    #[test]
    fn hands_free_can_start_on_its_own() {
        let mut fsm = HotkeyFsm::from_strings("Ctrl+Win", "Ctrl+Alt+H");
        press(&mut fsm, &[CTRL, VK_LMENU], 1.0);
        let h = press(&mut fsm, &[0x48], 1.1);
        assert_eq!(all(&h), [Action::Start, Action::EnterHandsFree]);
        release(&mut fsm, &[0x48, VK_LMENU, CTRL], 1.2);
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: true });
        press(&mut fsm, &[CTRL, VK_LMENU], 4.0);
        assert_eq!(all(&press(&mut fsm, &[0x48], 4.1)), [Action::Stop]);
    }

    #[test]
    fn disabled_fsm_passes_everything_through() {
        let mut fsm = fsm();
        fsm.set_enabled(false);
        assert!(all(&press(&mut fsm, &[CTRL, WIN, VK_SPACE], 1.0)).is_empty());
        fsm.set_enabled(true);
        release(&mut fsm, &[VK_SPACE, WIN, CTRL], 2.0);
        assert_eq!(
            all(&press(&mut fsm, &[CTRL, WIN], 3.0)),
            [Action::Start, Action::SuppressStartMenu]
        );
    }

    #[test]
    fn controller_can_end_a_recording() {
        let mut fsm = fsm();
        press(&mut fsm, &[CTRL, WIN], 1.0);
        fsm.recording_ended();
        assert_eq!(fsm.state(), HotkeyState::Idle);
        assert!(all(&release(&mut fsm, &[WIN, CTRL], 2.0)).is_empty());
    }

    #[test]
    fn too_many_held_keys_do_not_panic() {
        let mut fsm = fsm();
        for vk in 0x41u16..0x51 {
            fsm.handle(vk, true, 1.0);
        }
        for vk in 0x41u16..0x51 {
            fsm.handle(vk, false, 1.1);
        }
        assert_eq!(fsm.state(), HotkeyState::Idle);
    }
}
