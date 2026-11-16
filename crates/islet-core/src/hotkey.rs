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

/// Which side of the keyboard a modifier must be pressed on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    /// Either the left or the right key.
    #[default]
    Any,
    Left,
    Right,
}

/// The four modifier families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Ctrl,
    Shift,
    Alt,
    Win,
}

impl Group {
    const ALL: [Self; 4] = [Self::Ctrl, Self::Alt, Self::Shift, Self::Win];

    /// The bit for this family's left key in the held-modifier bitmap
    /// (the right key is the next bit up).
    const fn left_bit(self) -> u8 {
        match self {
            Self::Shift => 1,
            Self::Ctrl => 4,
            Self::Alt => 16,
            Self::Win => 64,
        }
    }

    const fn right_bit(self) -> u8 {
        self.left_bit() << 1
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Ctrl => "Ctrl",
            Self::Shift => "Shift",
            Self::Alt => "Alt",
            Self::Win => "Win",
        }
    }
}

/// The modifier keys a hotkey needs, each optionally tied to one side.
/// A family that is not required must not be held (matching is exact).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    ctrl: Option<Side>,
    shift: Option<Side>,
    alt: Option<Side>,
    win: Option<Side>,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: None,
        shift: None,
        alt: None,
        win: None,
    };
    /// Either-side requirements.
    pub const CTRL: Self = Self {
        ctrl: Some(Side::Any),
        ..Self::NONE
    };
    pub const SHIFT: Self = Self {
        shift: Some(Side::Any),
        ..Self::NONE
    };
    pub const ALT: Self = Self {
        alt: Some(Side::Any),
        ..Self::NONE
    };
    pub const WIN: Self = Self {
        win: Some(Side::Any),
        ..Self::NONE
    };

    /// The requirement for one family.
    #[must_use]
    pub const fn group(self, group: Group) -> Option<Side> {
        match group {
            Group::Ctrl => self.ctrl,
            Group::Shift => self.shift,
            Group::Alt => self.alt,
            Group::Win => self.win,
        }
    }

    #[must_use]
    pub const fn has(self, group: Group) -> bool {
        self.group(group).is_some()
    }

    #[must_use]
    pub const fn with(mut self, group: Group, side: Side) -> Self {
        match group {
            Group::Ctrl => self.ctrl = Some(side),
            Group::Shift => self.shift = Some(side),
            Group::Alt => self.alt = Some(side),
            Group::Win => self.win = Some(side),
        }
        self
    }

    /// Both requirements at once. A family needed on two different sides
    /// becomes "either side".
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let merge = |a: Option<Side>, b: Option<Side>| match (a, b) {
            (None, side) | (side, None) => side,
            (Some(a), Some(b)) if a == b => Some(a),
            _ => Some(Side::Any),
        };
        Self {
            ctrl: merge(self.ctrl, other.ctrl),
            shift: merge(self.shift, other.shift),
            alt: merge(self.alt, other.alt),
            win: merge(self.win, other.win),
        }
    }

    /// How many families are required.
    #[must_use]
    pub fn count(self) -> u32 {
        Group::ALL.into_iter().filter(|g| self.has(*g)).count() as u32
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.count() == 0
    }

    /// The only required family and its side, when exactly one is required.
    #[must_use]
    pub fn single(self) -> Option<(Group, Side)> {
        let mut found = Group::ALL
            .into_iter()
            .filter_map(|group| self.group(group).map(|side| (group, side)));
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }

    /// Whether the held modifier keys (`down`, one bit per physical key, see
    /// [`modifier_bit`]) are exactly what this requires.
    #[must_use]
    pub fn matches(self, down: u8) -> bool {
        Group::ALL.into_iter().all(|group| {
            let (left, right) = (group.left_bit(), group.right_bit());
            let held = down & (left | right);
            match self.group(group) {
                None => held == 0,
                Some(Side::Any) => held != 0,
                Some(Side::Left) => held == left,
                Some(Side::Right) => held == right,
            }
        })
    }

    /// Exactly the physical keys in `down`: a family with only its left key
    /// down needs the left key, only the right needs the right, both either.
    #[must_use]
    pub fn from_physical(down: u8) -> Self {
        let mut modifiers = Self::NONE;
        for group in Group::ALL {
            let (left, right) = (down & group.left_bit() != 0, down & group.right_bit() != 0);
            let side = match (left, right) {
                (false, false) => continue,
                (true, false) => Side::Left,
                (false, true) => Side::Right,
                (true, true) => Side::Any,
            };
            modifiers = modifiers.with(group, side);
        }
        modifiers
    }

    /// What a user means by the keys they pressed while setting a hotkey.
    /// A lone modifier key keeps its side ("Right Ctrl" is not "Left Ctrl").
    /// In a combination, left-side keys mean "either side" so that the usual
    /// Ctrl+Win works with whichever hand is free; right-side keys stay
    /// right-only because pressing one is a deliberate choice.
    #[must_use]
    pub fn from_capture(down: u8, has_key: bool) -> Self {
        let exact = Self::from_physical(down);
        if exact.count() == 1 && !has_key {
            return exact;
        }
        let mut modifiers = Self::NONE;
        for group in Group::ALL {
            if let Some(side) = exact.group(group) {
                modifiers =
                    modifiers.with(group, if side == Side::Left { Side::Any } else { side });
            }
        }
        modifiers
    }
}

/// Which modifier key a virtual-key code is, and on which side.
#[must_use]
pub const fn modifier_of(vk: u16) -> Option<(Group, Side)> {
    match vk {
        VK_LCONTROL | VK_CONTROL => Some((Group::Ctrl, Side::Left)),
        VK_RCONTROL => Some((Group::Ctrl, Side::Right)),
        VK_LSHIFT | VK_SHIFT => Some((Group::Shift, Side::Left)),
        VK_RSHIFT => Some((Group::Shift, Side::Right)),
        VK_LMENU | VK_MENU => Some((Group::Alt, Side::Left)),
        VK_RMENU => Some((Group::Alt, Side::Right)),
        VK_LWIN => Some((Group::Win, Side::Left)),
        VK_RWIN => Some((Group::Win, Side::Right)),
        _ => None,
    }
}

/// The bit a modifier virtual key occupies in the held-modifier bitmap
/// (`LShift`=1, `RShift`=2, `LCtrl`=4, `RCtrl`=8, `LAlt`=16, `RAlt`=32,
/// `LWin`=64, `RWin`=128).
#[must_use]
pub const fn modifier_bit(vk: u16) -> Option<u8> {
    match modifier_of(vk) {
        Some((group, Side::Right)) => Some(group.right_bit()),
        Some((group, _)) => Some(group.left_bit()),
        None => None,
    }
}

/// A hotkey: modifiers, plus at most one ordinary key. A hotkey can be a
/// single key: one side-specific modifier ("Right Ctrl") or one non-typing
/// key ("F9", "CapsLock").
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
    /// A key that is used for typing would fire while typing.
    NeedsModifier,
    /// A lone modifier must say which side ("Right Ctrl"), or it would fire
    /// on every shortcut.
    NeedsSide,
    /// Esc cancels a dictation, so it cannot be a hotkey.
    ReservedKey,
    /// A system-wide shortcut needs an ordinary key, not modifiers alone.
    NeedsKey,
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "press at least one key"),
            Self::UnknownKey(name) => write!(f, "`{name}` is not a key Onyx knows"),
            Self::TooManyKeys => write!(f, "use one key plus modifiers (Ctrl, Alt, Shift, Win)"),
            Self::NeedsModifier => write!(
                f,
                "add a modifier such as Ctrl or Alt (single keys like F9 or Right Ctrl are fine)"
            ),
            Self::NeedsSide => write!(f, "choose the left or right key, or add a second key"),
            Self::ReservedKey => write!(f, "Esc is reserved for cancelling a dictation"),
            Self::NeedsKey => write!(f, "add a key such as S or Space; modifiers alone can't be a shortcut"),
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
    ("CapsLock", 0x14),
    ("Pause", 0x13),
    ("ScrollLock", 0x91),
    ("PrintScreen", 0x2C),
    ("Menu", 0x5D),
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

/// Keys that are safe as a hotkey on their own: they are not used for
/// typing, so taking them over costs little (F1-F24, CapsLock, Pause,
/// ScrollLock, PrintScreen, Insert, Menu).
#[must_use]
pub const fn is_standalone_key(vk: u16) -> bool {
    matches!(vk, 0x70..=0x87 | 0x14 | 0x13 | 0x91 | 0x2C | 0x2D | 0x5D)
}

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
        "CAPS" | "CAPSLOCK" => "CapsLock",
        "SCROLL" | "SCROLLLOCK" => "ScrollLock",
        "BREAK" => "Pause",
        "PRTSC" | "PRINTSCRN" | "PRINTSCREEN" => "PrintScreen",
        "APPS" | "APPLICATION" | "CONTEXTMENU" => "Menu",
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

/// "Ctrl", "Right Ctrl", "LAlt", "right-win", "AltGr": a modifier key name.
fn modifier_token(token: &str) -> Option<(Group, Side)> {
    let normalized: String = token
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .collect::<String>()
        .to_ascii_lowercase();
    let (side, base) = if let Some(rest) = normalized.strip_prefix("left") {
        (Side::Left, rest)
    } else if let Some(rest) = normalized.strip_prefix("right") {
        (Side::Right, rest)
    } else if let Some(rest) = normalized.strip_prefix('l').filter(|r| is_modifier_base(r)) {
        (Side::Left, rest)
    } else if let Some(rest) = normalized.strip_prefix('r').filter(|r| is_modifier_base(r)) {
        (Side::Right, rest)
    } else {
        (Side::Any, normalized.as_str())
    };
    let group = match base {
        "ctrl" | "control" | "ctl" => Group::Ctrl,
        "shift" => Group::Shift,
        "alt" | "option" => Group::Alt,
        "win" | "windows" | "super" | "meta" | "cmd" => Group::Win,
        "altgr" if side == Side::Any => return Some((Group::Alt, Side::Right)),
        _ => return None,
    };
    Some((group, side))
}

fn is_modifier_base(text: &str) -> bool {
    matches!(
        text,
        "ctrl"
            | "control"
            | "ctl"
            | "shift"
            | "alt"
            | "option"
            | "win"
            | "windows"
            | "super"
            | "meta"
            | "cmd"
    )
}

impl Combo {
    /// Parses `"Ctrl+Win"`, `"Right Ctrl"`, `"Right Alt+D"`, `"F9"`,
    /// case-insensitively. An empty string is an error (no hotkey).
    pub fn parse(text: &str) -> Result<Self, HotkeyError> {
        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        let mut any = false;
        for part in text.split('+') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            any = true;
            if let Some((group, side)) = modifier_token(part) {
                modifiers = modifiers.union(Modifiers::NONE.with(group, side));
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
            None => {
                // One modifier alone is allowed, but only for a named side.
                if let Some((_, Side::Any)) = modifiers.single() {
                    return Err(HotkeyError::NeedsSide);
                }
            }
            Some(code) => {
                if modifiers.is_empty() && !is_standalone_key(code) {
                    return Err(HotkeyError::NeedsModifier);
                }
            }
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
        for group in Group::ALL {
            if let Some(side) = self.modifiers.group(group) {
                let prefix = match side {
                    Side::Any => "",
                    Side::Left => "Left ",
                    Side::Right => "Right ",
                };
                part(f, &format!("{prefix}{}", group.name()))?;
            }
        }
        if let Some(name) = self.key.and_then(key_name) {
            part(f, &name)?;
        }
        Ok(())
    }
}

/// `MOD_ALT`, `MOD_CONTROL`, `MOD_SHIFT` and `MOD_WIN` for `RegisterHotKey`.
const SYSTEM_MODIFIERS: [(Group, u32); 4] = [
    (Group::Alt, 0x1),
    (Group::Ctrl, 0x2),
    (Group::Shift, 0x4),
    (Group::Win, 0x8),
];

/// A combo as a system-wide shortcut for `RegisterHotKey`: its `MOD_*`
/// flags and virtual key. Windows does not tell left from right there, so
/// a side ("Right Ctrl+S") is accepted as either side.
pub fn system_shortcut(text: &str) -> Result<(u32, u16), HotkeyError> {
    let combo = Combo::parse(text)?;
    let key = combo.key.ok_or(HotkeyError::NeedsKey)?;
    let modifiers = SYSTEM_MODIFIERS
        .iter()
        .filter(|(group, _)| combo.modifiers.group(*group).is_some())
        .fold(0, |flags, (_, flag)| flags | flag);
    Ok((modifiers, key))
}

/// What the dictation controller should do. Delivered in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Begin recording.
    Start,
    /// Begin recording a spoken command for the assistant (the command
    /// hotkey), instead of text to type.
    StartCommand,
    /// A hold turned into hands-free recording (or the hands-free hotkey
    /// started one).
    EnterHandsFree,
    /// Finish: transcribe and insert.
    Stop,
    /// Throw the recording away without a word: an accidental tap, or the
    /// press turned out to be part of another shortcut.
    Discard,
    /// The user pressed Esc to throw the recording away. Same effect as
    /// `Discard`, but worth telling them about.
    Cancel,
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
        /// Started by the command hotkey rather than push-to-talk.
        command: bool,
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
    /// Hold to speak a command to the assistant.
    command: Option<Combo>,
    enabled: bool,
    /// One bit per modifier virtual key (LShift..RWin), so left and right
    /// are tracked separately and released independently.
    modifier_keys: u8,
    keys: [u16; MAX_KEYS],
    key_count: usize,
    /// Keys whose key-up must also be swallowed.
    swallowed: [u16; 4],
    phase: Phase,
}

impl HotkeyFsm {
    #[must_use]
    pub fn new(push_to_talk: Option<Combo>, hands_free: Option<Combo>) -> Self {
        Self {
            push_to_talk,
            hands_free,
            command: None,
            enabled: true,
            modifier_keys: 0,
            keys: [0; MAX_KEYS],
            key_count: 0,
            swallowed: [0; 4],
            phase: Phase::Idle,
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

    /// The assistant's hold-to-speak combo; `None` turns it off.
    pub fn set_command(&mut self, command: Option<Combo>) {
        self.command = command;
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
            if !combo.modifiers.matches(self.modifier_keys) {
                return false;
            }
            match combo.key {
                Some(key) => vk == key && self.key_count == 1,
                // A modifier-only toggle: the press that completes it.
                None => modifier_of(vk).is_some() && self.key_count == 0,
            }
        })
    }

    fn push_to_talk_triggered(&self, vk: u16) -> bool {
        self.hold_triggered(self.push_to_talk, vk)
    }

    /// The combo for the hold in progress.
    fn active_hold(&self, command: bool) -> Option<Combo> {
        if command {
            self.command
        } else {
            self.push_to_talk
        }
    }

    fn hold_triggered(&self, combo: Option<Combo>, vk: u16) -> bool {
        let Some(combo) = combo else {
            return false;
        };
        if !combo.modifiers.matches(self.modifier_keys) {
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
            out.push(Action::Cancel);
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
                    self.phase = Phase::Ptt { since: now, command: false };
                    out.push(Action::Start);
                    if self
                        .push_to_talk
                        .is_some_and(|c| c.modifiers.has(Group::Win))
                    {
                        out.push(Action::SuppressStartMenu);
                    }
                    if self.push_to_talk.is_some_and(|c| c.key == Some(vk)) {
                        out.swallow = true;
                        self.swallow_release_of(vk);
                    }
                } else if self.hold_triggered(self.command, vk) {
                    self.phase = Phase::Ptt { since: now, command: true };
                    out.push(Action::StartCommand);
                    // A lone Alt or Win released would open the app's menu
                    // bar or the Start menu.
                    if self
                        .command
                        .is_some_and(|c| c.modifiers.has(Group::Win) || c.modifiers.has(Group::Alt))
                    {
                        out.push(Action::SuppressStartMenu);
                    }
                    if self.command.is_some_and(|c| c.key == Some(vk)) {
                        out.swallow = true;
                        self.swallow_release_of(vk);
                    }
                }
            }
            Phase::Ptt { command, .. } => {
                if !command && self.hands_free_pressed(vk) {
                    self.phase = Phase::HandsFree;
                    out.push(Action::EnterHandsFree);
                    out.swallow = true;
                    self.swallow_release_of(vk);
                } else if self.active_hold(command).is_some_and(|c| c.key == Some(vk)) {
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
        if self.hands_free.is_some_and(|c| c.modifiers.has(Group::Win)) {
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
            Phase::Ptt { since, command } => {
                let combo_still_held = self.active_hold(command).is_some_and(|combo| {
                    combo.modifiers.matches(self.modifier_keys)
                        && combo.key.is_none_or(|key| self.is_held(key))
                });
                if combo_still_held {
                    return out;
                }
                // Hold to speak: a real hold ends with a Stop; a quick tap is
                // an accident and is dropped silently.
                let held_for = now - since;
                out.push(if held_for >= MIN_HOLD_SECONDS {
                    Action::Stop
                } else {
                    Action::Discard
                });
                self.finish();
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
    fn the_command_hotkey_records_a_command_and_keeps_alt_quiet() {
        let mut fsm = HotkeyFsm::from_strings("Right Ctrl", "");
        fsm.set_command(Combo::parse("Right Alt").ok());
        let down = press(&mut fsm, &[VK_RMENU], 1.0);
        assert_eq!(all(&down), [Action::StartCommand, Action::SuppressStartMenu]);
        let up = release(&mut fsm, &[VK_RMENU], 2.0);
        assert_eq!(all(&up), [Action::Stop]);
        // Push-to-talk still starts ordinary dictation.
        let down = press(&mut fsm, &[VK_RCONTROL], 3.0);
        assert_eq!(all(&down), [Action::Start]);
        release(&mut fsm, &[VK_RCONTROL], 4.0);
        // A quick tap, or Right Alt used with another key, is not a command.
        press(&mut fsm, &[VK_RMENU], 5.0);
        assert_eq!(all(&release(&mut fsm, &[VK_RMENU], 5.1)), [Action::Discard]);
        press(&mut fsm, &[VK_RMENU], 6.0);
        assert_eq!(all(&press(&mut fsm, &[0x45], 6.3)), [Action::Discard]);
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
        assert_eq!(Combo::parse("Ctrl"), Err(HotkeyError::NeedsSide));
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
    fn push_to_talk_is_hold_only_so_taps_never_lock_it_on() {
        let mut fsm = fsm();
        // Two quick taps in a row: each is dropped, neither starts hands-free.
        for t in [1.0, 1.3] {
            press(&mut fsm, &[CTRL, WIN], t);
            let up = release(&mut fsm, &[WIN, CTRL], t + 0.08);
            assert_eq!(all(&up), [Action::Discard]);
            assert_eq!(fsm.state(), HotkeyState::Idle);
        }
    }

    #[test]
    fn the_hands_free_hotkey_toggles_on_its_own() {
        let mut fsm = HotkeyFsm::from_strings("Ctrl+Win", "Ctrl+Alt+H");
        press(&mut fsm, &[CTRL, VK_LMENU], 1.0);
        let on = press(&mut fsm, &[0x48], 1.1);
        assert_eq!(all(&on), [Action::Start, Action::EnterHandsFree]);
        release(&mut fsm, &[0x48, VK_LMENU, CTRL], 1.2);
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: true });
        press(&mut fsm, &[CTRL, VK_LMENU], 5.0);
        assert_eq!(all(&press(&mut fsm, &[0x48], 5.1)), [Action::Stop]);
        assert_eq!(fsm.state(), HotkeyState::Idle);
    }

    #[test]
    fn with_hands_free_off_space_does_nothing_special() {
        // An empty hands-free hotkey is how the toggle being off reaches the
        // state machine.
        let mut fsm = HotkeyFsm::from_strings("Ctrl+Win", "");
        press(&mut fsm, &[CTRL, WIN], 1.0);
        let space = press(&mut fsm, &[VK_SPACE], 1.3);
        // Space joins the hold, so it cancels the push-to-talk and passes through.
        assert_eq!(all(&space), [Action::Discard]);
        assert!(!space[0].swallow);
        assert_eq!(fsm.state(), HotkeyState::Idle);
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
        assert_eq!(esc.actions().collect::<Vec<_>>(), [Action::Cancel]);
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

    // ----- sides and single keys ------------------------------------------

    const RCTRL: u16 = VK_RCONTROL;
    const RALT: u16 = VK_RMENU;

    #[test]
    fn parses_sided_modifiers_and_aliases() {
        for text in [
            "Right Ctrl",
            "right ctrl",
            "RCtrl",
            "right-control",
            "RightCtrl",
            "r_ctrl",
        ] {
            let combo = Combo::parse(text).unwrap();
            assert_eq!(
                combo.modifiers.single(),
                Some((Group::Ctrl, Side::Right)),
                "{text}"
            );
            assert_eq!(combo.key, None);
            assert_eq!(combo.to_string(), "Right Ctrl");
        }
        assert_eq!(Combo::parse("Left Alt").unwrap().to_string(), "Left Alt");
        assert_eq!(Combo::parse("lshift").unwrap().to_string(), "Left Shift");
        assert_eq!(Combo::parse("AltGr").unwrap().to_string(), "Right Alt");
        assert_eq!(Combo::parse("Right Win").unwrap().to_string(), "Right Win");
        assert_eq!(
            Combo::parse("Right Alt+D").unwrap().to_string(),
            "Right Alt+D"
        );
        assert_eq!(
            Combo::parse("Right Ctrl+Left Win").unwrap().to_string(),
            "Right Ctrl+Left Win"
        );
        // The arrow keys are still keys, not sides.
        assert_eq!(Combo::parse("Ctrl+Right").unwrap().key, Some(0x27));
        assert_eq!(Combo::parse("Ctrl+Left").unwrap().key, Some(0x25));
    }

    #[test]
    fn sided_combos_round_trip() {
        for text in [
            "Right Ctrl",
            "Left Ctrl",
            "Right Alt",
            "Right Ctrl+Win",
            "Ctrl+Right Alt+Space",
            "Left Shift+Right Win+F9",
            "F9",
            "CapsLock",
            "Pause",
            "ScrollLock",
            "PrintScreen",
            "Menu",
            "Insert",
        ] {
            let combo = Combo::parse(text).unwrap();
            assert_eq!(Combo::parse(&combo.to_string()).unwrap(), combo, "{text}");
        }
    }

    #[test]
    fn single_key_rules() {
        // A lone modifier needs a named side, or it would fire on every shortcut.
        assert_eq!(Combo::parse("Ctrl"), Err(HotkeyError::NeedsSide));
        assert_eq!(Combo::parse("Alt"), Err(HotkeyError::NeedsSide));
        assert!(Combo::parse("Right Ctrl").is_ok());
        // Keys used for typing and editing cannot stand alone...
        for text in [
            "A",
            "Space",
            "Enter",
            "Tab",
            "Backspace",
            "Delete",
            "Home",
            "Left",
            "5",
        ] {
            assert_eq!(
                Combo::parse(text),
                Err(HotkeyError::NeedsModifier),
                "{text}"
            );
        }
        // ...but keys nobody types with can.
        for text in [
            "F1",
            "F24",
            "CapsLock",
            "Pause",
            "ScrollLock",
            "PrintScreen",
            "Menu",
            "Insert",
        ] {
            assert!(Combo::parse(text).is_ok(), "{text}");
        }
        assert_eq!(Combo::parse("Esc"), Err(HotkeyError::ReservedKey));
        // Clearing a hotkey is an empty string.
        assert_eq!(Combo::parse(""), Err(HotkeyError::Empty));
    }

    #[test]
    fn matching_is_exact_per_side() {
        let right_ctrl = Combo::parse("Right Ctrl").unwrap().modifiers;
        let any_ctrl = Combo::parse("Ctrl+Win").unwrap().modifiers;
        let bit = |vk| modifier_bit(vk).unwrap();
        assert!(right_ctrl.matches(bit(VK_RCONTROL)));
        assert!(!right_ctrl.matches(bit(VK_LCONTROL)));
        assert!(!right_ctrl.matches(bit(VK_RCONTROL) | bit(VK_LCONTROL)));
        assert!(!right_ctrl.matches(bit(VK_RCONTROL) | bit(VK_LSHIFT)));
        assert!(any_ctrl.matches(bit(VK_LCONTROL) | bit(VK_RWIN)));
        assert!(any_ctrl.matches(bit(VK_RCONTROL) | bit(VK_LWIN)));
        assert!(!any_ctrl.matches(bit(VK_LCONTROL)));
    }

    #[test]
    fn capture_keeps_the_side_of_a_lone_modifier() {
        let bit = |vk| modifier_bit(vk).unwrap();
        let single = |vk| Modifiers::from_capture(bit(vk), false);
        assert_eq!(
            single(VK_RCONTROL).single(),
            Some((Group::Ctrl, Side::Right))
        );
        assert_eq!(
            single(VK_LCONTROL).single(),
            Some((Group::Ctrl, Side::Left))
        );
        assert_eq!(single(VK_RMENU).single(), Some((Group::Alt, Side::Right)));
        // In a combination, left means "either side"; right stays right.
        let ctrl_win = Modifiers::from_capture(bit(VK_LCONTROL) | bit(VK_LWIN), false);
        assert_eq!(ctrl_win, Modifiers::CTRL.union(Modifiers::WIN));
        let right_alt_d = Modifiers::from_capture(bit(VK_RMENU), true);
        assert_eq!(right_alt_d.single(), Some((Group::Alt, Side::Right)));
        let left_ctrl_d = Modifiers::from_capture(bit(VK_LCONTROL), true);
        assert_eq!(left_ctrl_d, Modifiers::CTRL);
        let mixed = Modifiers::from_capture(bit(VK_RCONTROL) | bit(VK_LSHIFT), false);
        assert_eq!(
            mixed,
            Modifiers::NONE
                .with(Group::Ctrl, Side::Right)
                .with(Group::Shift, Side::Any)
        );
        // Both sides of one family pressed: either.
        let both = Modifiers::from_capture(bit(VK_LCONTROL) | bit(VK_RCONTROL), false);
        assert_eq!(both, Modifiers::CTRL);
    }

    fn right_ctrl_fsm() -> HotkeyFsm {
        HotkeyFsm::from_strings("Right Ctrl", "Right Ctrl+Space")
    }

    #[test]
    fn right_ctrl_alone_is_push_to_talk() {
        let mut fsm = right_ctrl_fsm();
        let down = press(&mut fsm, &[RCTRL], 1.0);
        assert_eq!(all(&down), [Action::Start]);
        assert!(!down[0].swallow, "modifier keys always pass through");
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: false });
        // No Start-menu suppression: Win is not involved.
        assert_eq!(all(&release(&mut fsm, &[RCTRL], 2.0)), [Action::Stop]);
    }

    #[test]
    fn the_left_key_does_not_trigger_a_right_only_hotkey() {
        let mut fsm = right_ctrl_fsm();
        assert!(all(&press(&mut fsm, &[VK_LCONTROL], 1.0)).is_empty());
        assert_eq!(fsm.state(), HotkeyState::Idle);
        // Left held first, then right: still not exactly Right Ctrl.
        assert!(all(&press(&mut fsm, &[RCTRL], 1.1)).is_empty());
    }

    #[test]
    fn right_ctrl_in_a_shortcut_cancels_and_lets_the_shortcut_through() {
        let mut fsm = right_ctrl_fsm();
        press(&mut fsm, &[RCTRL], 1.0);
        let c = press(&mut fsm, &[0x43], 1.1); // Right Ctrl+C
        assert_eq!(all(&c), [Action::Discard]);
        assert!(!c[0].swallow, "Ctrl+C must still copy");
        assert_eq!(fsm.state(), HotkeyState::Idle);
        assert!(all(&release(&mut fsm, &[0x43, RCTRL], 1.2)).is_empty());
        // And it works again afterwards.
        assert_eq!(all(&press(&mut fsm, &[RCTRL], 3.0)), [Action::Start]);
    }

    #[test]
    fn right_ctrl_hold_then_space_reaches_hands_free() {
        let mut fsm = right_ctrl_fsm();
        press(&mut fsm, &[RCTRL], 1.0);
        assert_eq!(
            all(&press(&mut fsm, &[VK_SPACE], 1.5)),
            [Action::EnterHandsFree]
        );
        release(&mut fsm, &[VK_SPACE, RCTRL], 1.6);
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: true });
        // Right Ctrl+Space toggles it off.
        press(&mut fsm, &[RCTRL], 5.0);
        assert_eq!(all(&press(&mut fsm, &[VK_SPACE], 5.1)), [Action::Stop]);
    }

    #[test]
    fn right_alt_alone_works_and_left_alt_does_not() {
        let mut fsm = HotkeyFsm::from_strings("Right Alt", "");
        assert!(all(&press(&mut fsm, &[VK_LMENU], 1.0)).is_empty());
        release(&mut fsm, &[VK_LMENU], 1.1);
        assert_eq!(all(&press(&mut fsm, &[RALT], 2.0)), [Action::Start]);
        assert_eq!(all(&release(&mut fsm, &[RALT], 3.0)), [Action::Stop]);
    }

    #[test]
    fn a_single_function_key_is_swallowed_and_works() {
        let mut fsm = HotkeyFsm::from_strings("F9", "");
        let down = press(&mut fsm, &[0x78], 1.0);
        assert_eq!(all(&down), [Action::Start]);
        assert!(down[0].swallow);
        // Auto-repeat stays swallowed and starts nothing new.
        let repeat = fsm.handle(0x78, true, 1.3);
        assert!(repeat.swallow && repeat.actions().next().is_none());
        let up = fsm.handle(0x78, false, 1.8);
        assert_eq!(up.actions().collect::<Vec<_>>(), [Action::Stop]);
        assert!(up.swallow);
        // With a modifier held, F9 is a different shortcut and is left alone.
        let mut fsm = HotkeyFsm::from_strings("F9", "");
        press(&mut fsm, &[CTRL], 5.0);
        assert!(all(&press(&mut fsm, &[0x78], 5.1)).is_empty());
    }

    #[test]
    fn caps_lock_can_be_the_hotkey() {
        let mut fsm = HotkeyFsm::from_strings("CapsLock", "");
        let down = press(&mut fsm, &[0x14], 1.0);
        assert_eq!(all(&down), [Action::Start]);
        assert!(down[0].swallow, "so it does not toggle caps");
        assert_eq!(all(&release(&mut fsm, &[0x14], 2.0)), [Action::Stop]);
    }

    #[test]
    fn a_modifier_only_hands_free_toggle() {
        // F9 for push to talk, Right Alt alone to toggle hands-free.
        let mut fsm = HotkeyFsm::from_strings("F9", "Right Alt");
        let start = press(&mut fsm, &[RALT], 1.0);
        assert_eq!(all(&start), [Action::Start, Action::EnterHandsFree]);
        release(&mut fsm, &[RALT], 1.1);
        assert_eq!(fsm.state(), HotkeyState::Recording { hands_free: true });
        assert_eq!(all(&press(&mut fsm, &[RALT], 4.0)), [Action::Stop]);
        release(&mut fsm, &[RALT], 4.1);
        assert_eq!(fsm.state(), HotkeyState::Idle);
    }

    #[test]
    fn removed_hotkeys_do_nothing() {
        let mut fsm = HotkeyFsm::from_strings("", "");
        assert!(all(&press(&mut fsm, &[CTRL, WIN, VK_SPACE, 0x78], 1.0)).is_empty());
        assert_eq!(fsm.state(), HotkeyState::Idle);
        // Only hands-free set: push to talk stays disabled.
        let mut fsm = HotkeyFsm::from_strings("", "Ctrl+Alt+H");
        press(&mut fsm, &[CTRL, VK_LMENU], 1.0);
        assert_eq!(
            all(&press(&mut fsm, &[0x48], 1.1)),
            [Action::Start, Action::EnterHandsFree]
        );
    }

    #[test]
    fn ctrl_win_still_works_with_either_hand() {
        let mut fsm = fsm();
        let down = press(&mut fsm, &[VK_RCONTROL, VK_RWIN], 1.0);
        assert_eq!(all(&down), [Action::Start, Action::SuppressStartMenu]);
        assert_eq!(
            all(&release(&mut fsm, &[VK_RWIN, VK_RCONTROL], 2.0)),
            [Action::Stop]
        );
    }

    #[test]
    fn system_shortcuts_need_a_key_and_map_modifiers() {
        assert_eq!(system_shortcut("Ctrl+Shift+Space"), Ok((0x2 | 0x4, VK_SPACE)));
        assert_eq!(system_shortcut("Right Alt+S"), Ok((0x1, u16::from(b'S'))));
        assert_eq!(system_shortcut("Win+Shift+F9"), Ok((0x8 | 0x4, 0x78)));
        assert_eq!(system_shortcut("F9"), Ok((0, 0x78)));
        assert_eq!(system_shortcut("Ctrl+Win"), Err(HotkeyError::NeedsKey));
        assert_eq!(system_shortcut("B"), Err(HotkeyError::NeedsModifier));
        assert_eq!(system_shortcut(""), Err(HotkeyError::Empty));
    }
}
