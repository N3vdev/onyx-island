//! Small, deterministic fact readers shared by corrections and rewrite checks.
//! They deliberately avoid guessing at unrecognised spoken formats.

use super::{norm, Token};

pub(super) fn weekday(word: &str) -> Option<&'static str> {
    Some(match word {
        "monday" | "mon" => "monday",
        "tuesday" | "tue" | "tues" => "tuesday",
        "wednesday" | "wed" => "wednesday",
        "thursday" | "thu" | "thur" | "thurs" => "thursday",
        "friday" | "fri" => "friday",
        "saturday" | "sat" => "saturday",
        "sunday" | "sun" => "sunday",
        _ => return None,
    })
}

pub(super) fn calendar(word: &str) -> Option<&'static str> {
    weekday(word).or_else(|| {
        Some(match word {
            "january" | "jan" => "january",
            "february" | "feb" => "february",
            "march" | "mar" => "march",
            "april" | "apr" => "april",
            "may" => "may",
            "june" | "jun" => "june",
            "july" | "jul" => "july",
            "august" | "aug" => "august",
            "september" | "sep" | "sept" => "september",
            "october" | "oct" => "october",
            "november" | "nov" => "november",
            "december" | "dec" => "december",
            _ => return None,
        })
    })
}

pub(super) fn unit(word: &str) -> Option<&'static str> {
    Some(match word {
        "dollar" | "dollars" | "usd" => "usd",
        "rupee" | "rupees" | "inr" => "inr",
        "euro" | "euros" | "eur" => "eur",
        "pound" | "pounds" | "gbp" => "gbp",
        "percent" | "percentage" => "percent",
        "hour" | "hours" | "hr" | "hrs" => "hour",
        "minute" | "minutes" | "min" | "mins" => "minute",
        "second" | "seconds" | "sec" | "secs" => "second",
        "day" | "days" => "day",
        "week" | "weeks" => "week",
        "month" | "months" => "month",
        "year" | "years" => "year",
        "kilogram" | "kilograms" | "kg" | "kilos" => "kg",
        "gram" | "grams" | "g" => "g",
        "meter" | "meters" | "metre" | "metres" => "m",
        "liter" | "liters" | "litre" | "litres" => "l",
        "copy" | "copies" => "copy",
        "seat" | "seats" => "seat",
        "file" | "files" => "file",
        "mb" | "megabyte" | "megabytes" => "mb",
        "mib" => "mib",
        "gb" | "gigabyte" | "gigabytes" => "gb",
        "gib" => "gib",
        _ => return None,
    })
}

fn small_number(word: &str) -> Option<u32> {
    match word {
        "zero" => Some(0),
        "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        "eleven" => Some(11),
        "twelve" => Some(12),
        "thirteen" => Some(13),
        "fourteen" => Some(14),
        "fifteen" => Some(15),
        "sixteen" => Some(16),
        "seventeen" => Some(17),
        "eighteen" => Some(18),
        "nineteen" => Some(19),
        _ => word.parse().ok(),
    }
}

fn under_hundred(word: &str) -> Option<u32> {
    let tens = |word| match word {
        "twenty" => Some(20),
        "thirty" => Some(30),
        "forty" => Some(40),
        "fifty" => Some(50),
        "sixty" => Some(60),
        "seventy" => Some(70),
        "eighty" => Some(80),
        "ninety" => Some(90),
        _ => None,
    };
    if let Some((left, right)) = word.split_once('-') {
        let one = small_number(right).filter(|n| (1..=9).contains(n))?;
        return Some(tens(left)? + one);
    }
    tens(word).or_else(|| small_number(word).filter(|n| *n < 20))
}

/// Read complete English cardinal phrases without joining separate counts
/// ("four and six") or crossing ASR sentence punctuation.
pub(super) fn spoken_number(tokens: &[Token], start: usize) -> Option<(u64, usize)> {
    fn adjacent(tokens: &[Token], at: usize) -> bool {
        at > 0 && tokens[at - 1].trail.is_empty() && !tokens[at - 1].verbatim
    }
    fn group(tokens: &[Token], at: usize) -> Option<(u64, usize)> {
        let first = tokens.get(at)?;
        if first.verbatim || !first.word.chars().all(|c| c.is_alphabetic() || c == '-') {
            return None;
        }
        let mut value = under_hundred(&norm(first))? as u64;
        let mut end = at + 1;
        if (1..=9).contains(&value)
            && adjacent(tokens, end)
            && tokens
                .get(end)
                .is_some_and(|t| norm(t) == "hundred" && !t.verbatim)
        {
            value *= 100;
            end += 1;
            let mut rest = end;
            if adjacent(tokens, rest) && tokens.get(rest).is_some_and(|t| norm(t) == "and") {
                rest += 1;
            }
            if adjacent(tokens, rest) {
                if let Some((part, next)) = group(tokens, rest).filter(|(n, _)| *n < 100) {
                    value += part;
                    end = next;
                }
            }
        } else if value >= 20 && value % 10 == 0 && adjacent(tokens, end) {
            if let Some(one) = tokens
                .get(end)
                .and_then(|t| small_number(&norm(t)))
                .filter(|n| (1..=9).contains(n))
            {
                value += one as u64;
                end += 1;
            }
        }
        Some((value, end))
    }
    let (mut part, mut end) = group(tokens, start)?;
    let mut total = 0;
    let mut last_scale = u64::MAX;
    while adjacent(tokens, end) {
        let scale = match tokens.get(end).map(norm).as_deref() {
            Some("billion") => 1_000_000_000,
            Some("million") => 1_000_000,
            Some("thousand") => 1_000,
            _ => break,
        };
        if scale >= last_scale || part == 0 {
            break;
        }
        total += part * scale;
        last_scale = scale;
        end += 1;
        part = 0;
        let mut next = end;
        if adjacent(tokens, next) && tokens.get(next).is_some_and(|t| norm(t) == "and") {
            next += 1;
        }
        if !adjacent(tokens, next) {
            break;
        }
        if let Some((value, after)) = group(tokens, next) {
            part = value;
            end = after;
        } else {
            break;
        }
    }
    Some((total + part, end))
}

fn meridiem(word: &str) -> Option<&'static str> {
    match word.replace('.', "").as_str() {
        "am" => Some("am"),
        "pm" => Some("pm"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Quantity {
    pub end: usize,
    pub value: String,
    pub unit: Option<String>,
}

pub(super) fn clock(tokens: &[Token], start: usize) -> Option<Quantity> {
    let first = norm(tokens.get(start)?);
    if matches!(first.as_str(), "noon" | "midnight") {
        return Some(Quantity {
            end: start + 1,
            value: if first == "noon" { "12:00" } else { "00:00" }.into(),
            unit: Some("clock".into()),
        });
    }
    let mut end = start + 1;
    let (body, mut period) = if first.ends_with("am") || first.ends_with("pm") {
        (&first[..first.len() - 2], Some(&first[first.len() - 2..]))
    } else {
        (first.as_str(), None)
    };
    let (hour, mut minute, has_colon) = if let Some((h, m)) = body.split_once(':') {
        (small_number(h)?, m.parse::<u32>().ok()?, true)
    } else {
        (small_number(body)?, 0, false)
    };
    if period.is_none() {
        if let Some(next) = tokens.get(end) {
            let next_key = norm(next);
            if let Some(found) = meridiem(&next_key) {
                period = Some(found);
                end += 1;
            } else if !has_colon && minute == 0 {
                let (m, suffix) = if next_key.ends_with("am") || next_key.ends_with("pm") {
                    (
                        &next_key[..next_key.len() - 2],
                        meridiem(&next_key[next_key.len() - 2..]),
                    )
                } else {
                    (
                        next_key.as_str(),
                        tokens.get(end + 1).and_then(|t| meridiem(&norm(t))),
                    )
                };
                if let (Ok(parsed), Some(found)) = (m.parse::<u32>(), suffix) {
                    minute = parsed;
                    period = Some(found);
                    end += if next_key.ends_with("am") || next_key.ends_with("pm") {
                        1
                    } else {
                        2
                    };
                }
                if period.is_none() {
                    if let Some((spoken_minute, next)) = spoken_number(tokens, end) {
                        if let Some(found) = tokens.get(next).and_then(|t| meridiem(&norm(t))) {
                            if spoken_minute <= 59
                                && tokens[end..next].iter().all(|t| t.trail.is_empty())
                            {
                                minute = spoken_minute as u32;
                                period = Some(found);
                                end = next + 1;
                            }
                        }
                    }
                }
            }
        }
    }
    if minute > 59 || hour > 23 || (period.is_none() && !has_colon) {
        return None;
    }
    let hour = if let Some(period) = period {
        if hour == 0 || hour > 12 {
            return None;
        }
        hour % 12 + if period == "pm" { 12 } else { 0 }
    } else {
        hour
    };
    Some(Quantity {
        end,
        value: format!("{hour:02}:{minute:02}"),
        unit: Some("clock".into()),
    })
}

fn decimal(raw: &str) -> String {
    let mut value = raw.to_owned();
    let integer = raw.split('.').next().unwrap_or(raw);
    let groups: Vec<_> = integer.split(',').collect();
    if groups.len() > 1
        && (1..=3).contains(&groups[0].len())
        && groups
            .iter()
            .skip(1)
            .all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit()))
    {
        value = value.replace(',', "");
    }
    if value.matches('.').count() == 1 && !value.contains([',', '/', '-']) {
        value = value.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    value
}

/// Explicit decimal/version separators; never infer an omitted point.
pub(super) fn spoken_decimal(tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let first = tokens.get(start)?;
    if first.verbatim {
        return None;
    }
    let (mut value, mut end) = if norm(first) == "point" {
        ("0".to_owned(), start)
    } else if let Some((integer, end)) = spoken_number(tokens, start) {
        (integer.to_string(), end)
    } else if first.word.bytes().all(|c| c.is_ascii_digit()) {
        (first.word.clone(), start + 1)
    } else {
        return None;
    };
    let mut points = 0;
    while points < 4 && end - start < 24 {
        let Some(point) = tokens.get(end) else { break };
        if norm(point) != "point"
            || point.verbatim
            || !point.trail.is_empty()
            || (end > start && !tokens[end - 1].trail.is_empty())
        {
            break;
        }
        let mut digits = String::new();
        let mut next = end + 1;
        while next - end <= 12 {
            let Some(token) = tokens.get(next) else { break };
            if token.verbatim {
                break;
            }
            let key = norm(token);
            let digit = if key == "oh" {
                Some(0)
            } else {
                small_number(&key).filter(|n| *n <= 9)
            };
            let Some(digit) = digit else { break };
            digits.push(char::from(b'0' + digit as u8));
            next += 1;
            if !token.trail.is_empty() {
                break;
            }
        }
        if digits.is_empty() {
            break;
        }
        value.push('.');
        value.push_str(&digits);
        points += 1;
        end = next;
    }
    if norm(first) == "point"
        && value.len() == 3
        && tokens.get(end).and_then(|t| unit(&norm(t))).is_none()
    {
        return None; // "Discuss point three in the agenda" is not 0.3.
    }
    (points > 0).then_some((value, end))
}

pub(super) fn quantity(tokens: &[Token], start: usize) -> Option<Quantity> {
    if let Some(clock) = clock(tokens, start) {
        return Some(clock);
    }
    let token = tokens.get(start)?;
    let key = norm(token);
    if unit(&key).is_some_and(|u| matches!(u, "usd" | "eur" | "gbp" | "inr")) {
        if let Some(mut value) = quantity(tokens, start + 1) {
            value.unit = unit(&key).map(str::to_owned);
            return Some(value);
        }
    }
    if let Some((value, mut end)) = spoken_decimal(tokens, start) {
        let identifier = value.matches('.').count() > 1
            || start
                .checked_sub(1)
                .and_then(|at| tokens.get(at))
                .is_some_and(|t| matches!(norm(t).as_str(), "version" | "build" | "pin" | "code"));
        let mut number_unit = if identifier {
            Some("identifier")
        } else {
            tokens.get(end).and_then(|t| unit(&norm(t)))
        };
        if !identifier && number_unit.is_some() {
            end += 1;
        }
        for (symbol, currency) in [
            ('$', "usd"),
            ('\u{20b9}', "inr"),
            ('\u{20ac}', "eur"),
            ('\u{a3}', "gbp"),
            ('%', "percent"),
        ] {
            if token.lead.contains(symbol) || token.trail.contains(symbol) {
                number_unit = Some(currency);
            }
        }
        let value = if identifier { value } else { decimal(&value) };
        let negative = token.lead.contains('-')
            || start
                .checked_sub(1)
                .and_then(|at| tokens.get(at))
                .is_some_and(|t| norm(t) == "minus");
        return Some(Quantity {
            end,
            value: if negative { format!("-{value}") } else { value },
            unit: number_unit.map(str::to_owned),
        });
    }
    if let Some((number, mut end)) = spoken_number(tokens, start) {
        let next_unit = tokens.get(end).and_then(|t| unit(&norm(t)));
        // Preserve the existing distinction between a bare pronoun "one"
        // and a stated quantity, such as "one file" or "one hundred".
        let identifier = start
            .checked_sub(1)
            .and_then(|at| tokens.get(at))
            .is_some_and(|t| matches!(norm(t).as_str(), "version" | "build" | "pin" | "code"));
        if key != "one" || end > start + 1 || next_unit.is_some() || identifier {
            if next_unit.is_some() {
                end += 1;
            }
            let mut number_unit = if identifier {
                Some("identifier".to_owned())
            } else {
                next_unit.map(str::to_owned)
            };
            for (symbol, currency) in [
                ('$', "usd"),
                ('\u{20b9}', "inr"),
                ('\u{20ac}', "eur"),
                ('\u{a3}', "gbp"),
                ('%', "percent"),
            ] {
                if token.lead.contains(symbol) || token.trail.contains(symbol) {
                    number_unit = Some(currency.to_owned());
                }
            }
            let negative = token.lead.contains('-')
                || start
                    .checked_sub(1)
                    .and_then(|at| tokens.get(at))
                    .is_some_and(|t| norm(t) == "minus");
            return Some(Quantity {
                end,
                value: if negative {
                    format!("-{number}")
                } else {
                    number.to_string()
                },
                unit: number_unit,
            });
        }
    }
    let prefix_version =
        key.starts_with('v') && key.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    let raw_number = if prefix_version {
        &key[1..]
    } else {
        key.as_str()
    };
    let attached = raw_number
        .find(char::is_alphabetic)
        .and_then(|at| unit(&raw_number[at..]).map(|u| (at, u)));
    let number = attached.map_or(raw_number, |(at, _)| &raw_number[..at]);
    let next_unit = tokens.get(start + 1).and_then(|t| unit(&norm(t)));
    let mut end = start + 1;
    let mut number_unit = attached.map(|(_, u)| u).or(next_unit).map(str::to_owned);
    if number_unit.is_none() {
        number_unit = start
            .checked_sub(1)
            .and_then(|i| tokens.get(i))
            .and_then(|t| unit(&norm(t)))
            .filter(|u| matches!(*u, "usd" | "eur" | "gbp" | "inr"))
            .map(str::to_owned);
    }
    let value = if token.lead.ends_with('.')
        && token.lead.matches('.').count() == 1
        && number.bytes().all(|b| b.is_ascii_digit())
    {
        decimal(&format!("0.{number}"))
    } else if number.bytes().any(|b| b.is_ascii_digit())
        && number
            .bytes()
            .all(|b| b.is_ascii_digit() || b".,:/-".contains(&b))
    {
        let version = prefix_version
            || number.matches('.').count() > 1
            || start
                .checked_sub(1)
                .and_then(|i| tokens.get(i))
                .is_some_and(|t| matches!(norm(t).as_str(), "version" | "build" | "pin" | "code"));
        if version {
            number_unit = Some("identifier".into());
            number.to_owned()
        } else if number.contains(['/', '-']) {
            number.replace('/', "-")
        } else {
            decimal(number)
        }
    } else if next_unit.is_some() || (number != "one" && small_number(number).is_some()) {
        small_number(number)?.to_string()
    } else {
        return None;
    };
    if next_unit.is_some() {
        end += 1;
    }
    for (symbol, currency) in [
        ('$', "usd"),
        ('₹', "inr"),
        ('€', "eur"),
        ('£', "gbp"),
        ('%', "percent"),
    ] {
        if token.lead.contains(symbol) || token.trail.contains(symbol) {
            number_unit = Some(currency.into());
        }
    }
    let negative = token.lead.contains('-')
        || start
            .checked_sub(1)
            .and_then(|i| tokens.get(i))
            .is_some_and(|t| norm(t) == "minus");
    Some(Quantity {
        end,
        value: if negative { format!("-{value}") } else { value },
        unit: number_unit,
    })
}

pub(super) fn quantities(tokens: &[Token]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        if let Some(value) = quantity(tokens, at) {
            out.push((value.value, value.unit));
            at = value.end;
        } else {
            at += 1;
        }
    }
    out
}
