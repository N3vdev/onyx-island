//! Event-driven widgets: no timers, workers, or platform dependencies.
use crate::Rect;

pub const CALCULATOR_WIDTH: f32 = 496.0;
pub const CALCULATOR_HEIGHT: f32 = 168.0;
const MAX_INPUT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalculatorKey {
    Digit(u8),
    Decimal,
    Add,
    Subtract,
    Multiply,
    Divide,
    Equals,
    Clear,
    Backspace,
    Negate,
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetHover {
    Key(CalculatorKey),
    /// A button on the lens toolbar.
    Lens(crate::lens::LensItem),
    /// A task, or a button, on the tasks page.
    Task(crate::tasks::TaskItem),
    /// A button or a line on the lyrics view.
    Lyrics(crate::lyrics::LyricsItem),
    /// The field, a result or the close button on Spotify search.
    Search(crate::spotify::SearchItem),
}

#[derive(Default)]
pub struct ScrollGate {
    remainder: i32,
    last_switch: Option<f64>,
    last_input: Option<f64>,
}
impl ScrollGate {
    /// Feeds one wheel event (`delta` as Windows reports it: positive is
    /// away from you). Returns the step to take once a full notch has
    /// built up: `Some(true)` for the next page (scrolling down),
    /// `Some(false)` for the previous one (scrolling up).
    pub fn push(&mut self, delta: i32, now: f64) -> Option<bool> {
        if self.last_input.is_none_or(|last| now - last > 0.3)
            || self.remainder.signum() != delta.signum()
        {
            self.remainder = 0;
        }
        self.last_input = Some(now);
        if self.last_switch.is_some_and(|last| now - last < 0.18) {
            self.remainder = 0;
            return None;
        }
        self.remainder = self.remainder.saturating_add(delta);
        if self.remainder.unsigned_abs() < 120 {
            return None;
        }
        let forward = self.remainder < 0;
        self.remainder = 0;
        self.last_switch = Some(now);
        Some(forward)
    }
}

impl CalculatorKey {
    /// Text produced by the active keyboard layout, including the numpad.
    /// Navigation/shortcut keys are handled by the native window instead.
    pub fn from_character(character: char) -> Option<Self> {
        Some(match character {
            '0'..='9' => Self::Digit(character as u8 - b'0'),
            '.' | ',' => Self::Decimal,
            '+' => Self::Add,
            '-' | '\u{2212}' => Self::Subtract,
            '*' | '\u{00d7}' => Self::Multiply,
            '/' | '\u{00f7}' => Self::Divide,
            '=' | '\r' | '\n' => Self::Equals,
            '\u{8}' => Self::Backspace,
            '%' => Self::Percent,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Digit(0) => "0",
            Self::Digit(1) => "1",
            Self::Digit(2) => "2",
            Self::Digit(3) => "3",
            Self::Digit(4) => "4",
            Self::Digit(5) => "5",
            Self::Digit(6) => "6",
            Self::Digit(7) => "7",
            Self::Digit(8) => "8",
            Self::Digit(_) => "9",
            Self::Decimal => ".",
            Self::Add => "+",
            Self::Subtract => "−",
            Self::Multiply => "×",
            Self::Divide => "÷",
            Self::Equals => "=",
            Self::Clear => "AC",
            Self::Backspace => "Delete",
            Self::Negate => "+/−",
            Self::Percent => "%",
        }
    }
}

pub const KEYS: [CalculatorKey; 20] = {
    use CalculatorKey::*;
    [
        Clear,
        Backspace,
        Percent,
        Divide,
        Digit(7),
        Digit(8),
        Digit(9),
        Multiply,
        Digit(4),
        Digit(5),
        Digit(6),
        Subtract,
        Digit(1),
        Digit(2),
        Digit(3),
        Add,
        Negate,
        Digit(0),
        Decimal,
        Equals,
    ]
};

pub fn key_rect(index: usize) -> Rect {
    Rect::new(
        246.0 + (index % 4) as f32 * 58.0,
        12.0 + (index / 4) as f32 * 30.0,
        52.0,
        24.0,
    )
}

pub fn key_at(x: f32, y: f32) -> Option<CalculatorKey> {
    KEYS.iter()
        .copied()
        .enumerate()
        .find_map(|(i, key)| key_rect(i).contains(x, y).then_some(key))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Calculator {
    input: String,
    previous: String,
    result: Option<f64>,
    error: Option<&'static str>,
}

impl Calculator {
    pub fn expression(&self) -> &str {
        &self.previous
    }
    pub fn display(&self) -> &str {
        self.error.unwrap_or(if self.input.is_empty() {
            "0"
        } else {
            &self.input
        })
    }
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
    pub fn is_result(&self) -> bool {
        self.result.is_some()
    }

    pub fn press(&mut self, key: CalculatorKey) {
        use CalculatorKey::*;
        if key == Clear {
            *self = Self::default();
            return;
        }
        if self.error.take().is_some() {
            *self = Self::default();
        }
        let operator = match key {
            Add => Some('+'),
            Subtract => Some('-'),
            Multiply => Some('*'),
            Divide => Some('/'),
            _ => None,
        };
        if let Some(op) = operator {
            self.result = None;
            if self.input.is_empty() {
                self.input.push('0');
            }
            if self.input.ends_with(['+', '-', '*', '/']) {
                self.input.pop();
            }
            if self.input.len() < MAX_INPUT {
                self.input.push(op);
            }
            return;
        }
        if matches!(key, Digit(_) | Decimal) && self.result.take().is_some() {
            self.input.clear();
            self.previous.clear();
        }
        match key {
            Digit(n) if n <= 9 && self.input.len() < MAX_INPUT => {
                if self.input == "0" {
                    self.input.clear();
                }
                self.input.push(char::from(b'0' + n));
            }
            Decimal if self.input.len() + 2 <= MAX_INPUT => {
                let start = operand_start(&self.input);
                if !self.input[start..].contains('.') {
                    if start == self.input.len() || &self.input[start..] == "-" {
                        self.input.push('0');
                    }
                    self.input.push('.');
                }
            }
            Backspace => {
                self.input.pop();
                self.result = None;
                self.previous.clear();
            }
            Negate => {
                let start = operand_start(&self.input);
                if self.input[start..].starts_with('-') {
                    self.input.remove(start);
                } else if self.input.len() < MAX_INPUT {
                    self.input.insert(start, '-');
                }
                self.result = None;
            }
            Percent => {
                let start = operand_start(&self.input);
                if let Ok(value) = self.input[start..].parse::<f64>() {
                    self.input.replace_range(start.., &number(value / 100.0));
                    self.result = None;
                }
            }
            Equals => {
                let expression = if self.input.is_empty() {
                    "0"
                } else {
                    &self.input
                };
                match evaluate(expression) {
                    Ok(value) => {
                        self.previous = format!("{expression} =");
                        self.input = number(value);
                        self.result = Some(value);
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.result = None;
                    }
                }
            }
            _ => {}
        }
    }
}

fn operand_start(input: &str) -> usize {
    let mut start = 0;
    for (i, ch) in input.char_indices() {
        if matches!(ch, '+' | '-') && i > 0 && matches!(input.as_bytes()[i - 1], b'e' | b'E') {
            continue;
        }
        if matches!(ch, '+' | '*' | '/')
            || (ch == '-' && i > 0 && !matches!(input.as_bytes()[i - 1], b'+' | b'-' | b'*' | b'/'))
        {
            start = i + 1;
        }
    }
    start
}

fn number(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if value.abs() >= 1e12 || value.abs() < 1e-8 {
        return format!("{value:.8e}");
    }
    format!("{value:.10}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// Bounded basic arithmetic with normal precedence. Never evaluates code.
fn evaluate(input: &str) -> Result<f64, &'static str> {
    if input.len() > MAX_INPUT {
        return Err("Expression too long");
    }
    let bytes = input.as_bytes();
    let mut pos = 0;
    fn operand(bytes: &[u8], pos: &mut usize) -> Result<f64, &'static str> {
        let start = *pos;
        if bytes.get(*pos).is_some_and(|c| matches!(c, b'+' | b'-')) {
            *pos += 1;
        }
        while bytes.get(*pos).is_some_and(|c| {
            c.is_ascii_digit()
                || matches!(c, b'.' | b'e' | b'E')
                || (matches!(c, b'+' | b'-')
                    && *pos > start
                    && matches!(bytes[*pos - 1], b'e' | b'E'))
        }) {
            *pos += 1;
        }
        std::str::from_utf8(&bytes[start..*pos])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or("Check expression")
    }
    let mut total = 0.0;
    let mut term = operand(bytes, &mut pos)?;
    while let Some(&op) = bytes.get(pos) {
        pos += 1;
        let rhs = operand(bytes, &mut pos)?;
        match op {
            b'*' => term *= rhs,
            b'/' if rhs == 0.0 => return Err("Can't divide by zero"),
            b'/' => term /= rhs,
            b'+' => {
                total += term;
                term = rhs;
            }
            b'-' => {
                total += term;
                term = -rhs;
            }
            _ => return Err("Check expression"),
        }
    }
    let value = total + term;
    if value.is_finite() {
        Ok(value)
    } else {
        Err("Result out of range")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_calculation_uses_layout_characters_and_editing() {
        let mut calc = Calculator::default();
        for character in "12,5+7.5\r".chars() {
            calc.press(CalculatorKey::from_character(character).unwrap());
        }
        assert_eq!(calc.display(), "20");
        for character in "*30\u{8}2=".chars() {
            calc.press(CalculatorKey::from_character(character).unwrap());
        }
        assert_eq!(calc.display(), "640");
        for character in "letters\t\u{3}\u{1b}".chars() {
            assert_eq!(CalculatorKey::from_character(character), None);
        }
        for character in "10%+1\n".chars() {
            calc.press(CalculatorKey::from_character(character).unwrap());
        }
        assert_eq!(calc.display(), "1.1");
    }

    #[test]
    fn precision_wheel_accumulates_and_bursts_do_not_flicker() {
        let mut gate = ScrollGate::default();
        // Positive deltas are the wheel turned up: back a page.
        for (time, expected) in [(0.0, None), (0.01, None), (0.02, None), (0.03, Some(false))] {
            assert_eq!(gate.push(30, time), expected);
        }
        assert_eq!(gate.push(120, 0.05), None);
        // Down: the next page.
        assert_eq!(gate.push(-120, 0.3), Some(true));
        assert_eq!(gate.push(60, 1.0), None);
        assert_eq!(gate.push(-60, 1.01), None);
        assert_eq!(gate.push(-60, 1.02), Some(true));
    }

    #[test]
    fn the_wheel_direction_picks_the_page_direction() {
        let mut gate = ScrollGate::default();
        assert_eq!(gate.push(120, 0.0), Some(false));
        assert_eq!(gate.push(120, 0.5), Some(false));
        assert_eq!(gate.push(-120, 1.0), Some(true));
    }
    #[test]
    fn arithmetic_precedence_errors_and_recovery() {
        assert_eq!(evaluate("2+3*4-10/2"), Ok(9.0));
        assert_eq!(evaluate("3*-2"), Ok(-6.0));
        assert!(evaluate("1/0").is_err());
        assert!(evaluate("1+").is_err());
        assert!(evaluate("9e999").is_err());
        let mut calc = Calculator::default();
        for key in [
            CalculatorKey::Digit(1),
            CalculatorKey::Divide,
            CalculatorKey::Digit(0),
            CalculatorKey::Equals,
        ] {
            calc.press(key);
        }
        assert!(calc.is_error());
        calc.press(CalculatorKey::Digit(7));
        assert_eq!(calc.display(), "7");
    }
    #[test]
    fn result_chaining_decimal_percent_and_bounded_input() {
        use CalculatorKey::*;
        let mut calc = Calculator::default();
        for key in [Digit(1), Digit(0), Digit(0), Percent, Add, Digit(2), Equals] {
            calc.press(key);
        }
        assert_eq!(calc.display(), "3");
        for key in [Multiply, Digit(2), Equals] {
            calc.press(key);
        }
        assert_eq!(calc.display(), "6");
        for key in [Decimal, Digit(5), Decimal] {
            calc.press(key);
        }
        assert_eq!(calc.display(), "0.5");
        for _ in 0..100 {
            calc.press(Digit(9));
        }
        assert!(calc.input.len() <= MAX_INPUT);
        for (i, key) in KEYS.iter().enumerate() {
            let r = key_rect(i);
            assert_eq!(key_at(r.x + 1.0, r.y + 1.0), Some(*key));
        }
    }
}

pub const STATS_WIDTH: f32 = 496.0;
/// The same height as the calculator, so paging between them keeps the
/// island's size.
pub const STATS_HEIGHT: f32 = CALCULATOR_HEIGHT;
/// CPU samples kept for the sparkline (one a second).
pub const STATS_HISTORY: usize = 40;
/// Sustained load above this lights the glow round the pill.
pub const CPU_HOT: f32 = 0.9;

/// One reading of the machine, already in display units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemStats {
    /// 0..1 across all cores.
    pub cpu: f32,
    /// 0..1, busiest 3D engine; `None` where Windows reports no GPU counters.
    pub gpu: Option<f32>,
    pub ram_used: u64,
    pub ram_total: u64,
    /// Bytes per second over all physical adapters.
    pub down_bps: f64,
    pub up_bps: f64,
    /// Hottest ACPI thermal zone, in Celsius; `None` where there is none.
    pub temp_c: Option<f32>,
    /// Recent CPU load, oldest first, 0..1.
    pub cpu_history: Vec<f32>,
}

impl SystemStats {
    #[must_use]
    pub fn ram_fraction(&self) -> f32 {
        if self.ram_total == 0 {
            0.0
        } else {
            (self.ram_used as f64 / self.ram_total as f64) as f32
        }
    }
}

/// "0 B/s", "840 KB/s", "12.4 MB/s".
#[must_use]
pub fn format_rate(bytes_per_second: f64) -> String {
    let rate = bytes_per_second.max(0.0);
    if rate >= 1e9 {
        format!("{:.1} GB/s", rate / 1e9)
    } else if rate >= 1e7 {
        format!("{:.0} MB/s", rate / 1e6)
    } else if rate >= 1e6 {
        format!("{:.1} MB/s", rate / 1e6)
    } else if rate >= 1e3 {
        format!("{:.0} KB/s", rate / 1e3)
    } else {
        format!("{rate:.0} B/s")
    }
}

/// "11.2 / 31.9 GB".
#[must_use]
pub fn format_memory(used: u64, total: u64) -> String {
    let gb = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
    format!("{:.1} / {:.0} GB", gb(used), gb(total))
}

/// "42%".
#[must_use]
pub fn format_percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction.clamp(0.0, 1.0) * 100.0).round())
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn rates_and_sizes_read_naturally() {
        assert_eq!(format_rate(0.0), "0 B/s");
        assert_eq!(format_rate(840_000.0), "840 KB/s");
        assert_eq!(format_rate(1_240_000.0), "1.2 MB/s");
        assert_eq!(format_rate(52_000_000.0), "52 MB/s");
        assert_eq!(format_memory(12 << 30, 32 << 30), "12.0 / 32 GB");
        assert_eq!(format_percent(0.424), "42%");
        assert_eq!(format_percent(1.7), "100%");
        let stats = SystemStats {
            ram_used: 1,
            ram_total: 4,
            ..Default::default()
        };
        assert!((stats.ram_fraction() - 0.25).abs() < 1e-6);
    }
}
