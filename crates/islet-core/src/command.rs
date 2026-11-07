//! The voice assistant's commands: turns what was said ("open Spotify",
//! "next song", "type see you at five") into a [`Command`].
//!
//! Plain phrase matching, no language model: instant, offline and
//! predictable. Anything it doesn't recognise returns `None`, and the
//! caller says so rather than guessing.

/// Something the assistant can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open an app or website, or bring it to the front if it is open.
    Open(String),
    /// Close an app's windows (`None`: the window in front).
    Close(Option<String>),
    /// Minimize an app's windows (`None`: the window in front).
    Minimize(Option<String>),
    /// Maximize the window in front.
    Maximize,
    ShowDesktop,
    Media(MediaVerb),
    /// Find and play something, in a given app if one was named.
    Play { query: String, app: Option<String> },
    Volume(VolumeChange),
    /// Type this text into the window in front.
    Type(String),
    /// Search the web, or a site such as YouTube.
    Search { query: String, site: Option<String> },
    Screenshot,
    Lock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaVerb {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeChange {
    Up,
    Down,
    /// 0..=100.
    Set(u8),
    Mute,
    Unmute,
}

/// Words that open a command but carry no meaning of their own.
const LEADING_FILLER: &[&str] = &[
    "hey onyx", "ok onyx", "okay onyx", "onyx", "hey", "okay", "ok", "so", "um", "uh",
    "please", "can you", "could you", "would you", "will you", "i want to", "i'd like to",
    "i want you to", "i need to", "go ahead and", "just", "now",
];
const TRAILING_FILLER: &[&str] = &[
    "please", "for me", "now", "thanks", "thank you", "right now", "a bit", "a little",
    "a little bit", "a lot",
];

/// Words before a name that aren't part of it.
const NAME_PREFIX: &[&str] = &["the", "my", "a", "an", "up"];
const NAME_SUFFIX: &[&str] = &["app", "application", "program", "window", "windows", "browser"];

/// "Play ..." that means "resume", not a search.
const RESUME_WORDS: &[&str] = &[
    "", "music", "the music", "my music", "some music", "a song", "something", "songs",
    "the song", "it", "again", "media", "audio",
];

/// Words a command starts with, for telling "open Notepad and type hi"
/// (two steps) from "play rock and roll" (one).
const VERBS: &[&str] = &[
    "open", "launch", "start", "run", "switch", "go", "bring", "focus", "close", "quit",
    "exit", "minimize", "minimise", "maximize", "maximise", "hide", "play", "pause", "resume",
    "skip", "next", "previous", "type", "search", "google", "mute", "unmute", "set", "turn",
    "volume", "take", "lock",
];

/// Parses a transcript that may hold several steps ("open Notepad and type
/// hello", "pause the music, then open Spotify"). `None` unless every step
/// is understood, so a half-understood request never runs.
#[must_use]
pub fn parse_all(spoken: &str) -> Option<Vec<Command>> {
    let steps = split_steps(spoken);
    if steps.len() > 1 {
        if let Some(commands) = steps.iter().map(|step| parse(step)).collect::<Option<Vec<_>>>() {
            return Some(commands);
        }
    }
    parse(spoken).map(|command| vec![command])
}

/// Splits at "and" / "then" / a sentence end when a command verb follows.
/// Everything after "type" is the text to type, so it is never split.
fn split_steps(spoken: &str) -> Vec<String> {
    let bare = |word: &str| -> String {
        word.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
    };
    let tokens: Vec<&str> = spoken.split_whitespace().collect();
    let mut steps = Vec::new();
    let mut start = 0;
    let mut index = 1;
    while index < tokens.len() {
        if bare(tokens[start]) == "type" {
            break;
        }
        let word = bare(tokens[index]);
        let joiner = word == "and" || word == "then";
        let sentence_end = tokens[index - 1].ends_with(['.', ',', ';', '!', '?']);
        let mut next = index + usize::from(joiner);
        if joiner && next < tokens.len() && bare(tokens[next]) == "then" {
            next += 1;
        }
        if (joiner || sentence_end) && next < tokens.len() && VERBS.contains(&bare(tokens[next]).as_str()) {
            steps.push(tokens[start..index].join(" "));
            start = next;
            index = next + 1;
            continue;
        }
        index += 1;
    }
    steps.push(tokens[start..].join(" "));
    steps.retain(|step| !step.trim().is_empty());
    steps
}

/// Parses a transcript. `None` when it isn't a command this knows.
#[must_use]
pub fn parse(spoken: &str) -> Option<Command> {
    // `type` keeps the words as spoken, so it is found in the original.
    if let Some(text) = typed_text(spoken) {
        return Some(Command::Type(text));
    }
    let said = strip_filler(&normalize(spoken));
    let s = said.as_str();
    if s.is_empty() {
        return None;
    }

    if let Some(command) = media(s) {
        return Some(command);
    }
    if let Some(change) = volume(s) {
        return Some(Command::Volume(change));
    }
    if let Some(command) = window(s) {
        return Some(command);
    }
    if matches!(s, "take a screenshot" | "screenshot" | "take screenshot" | "capture the screen" | "snip") {
        return Some(Command::Screenshot);
    }
    if matches!(
        s,
        "lock" | "lock the computer" | "lock my computer" | "lock the pc" | "lock my pc"
            | "lock the screen" | "lock screen" | "lock my laptop"
    ) {
        return Some(Command::Lock);
    }
    if let Some(command) = search(s).or_else(|| play(s)) {
        return Some(match command {
            Command::Search { query, site } => Command::Search { query: tidy_query(&query), site },
            Command::Play { query, app } => Command::Play { query: tidy_query(&query), app },
            other => other,
        });
    }
    for verb in ["open up", "open", "launch", "start", "run", "switch to", "go to", "bring up", "show me", "show", "focus"] {
        if let Some(rest) = after(s, verb) {
            let name = clean_name(rest);
            if !name.is_empty() {
                return Some(Command::Open(name));
            }
        }
    }
    None
}

/// "type hello there" → "Hello there", keeping the spoken punctuation.
fn typed_text(spoken: &str) -> Option<String> {
    let trimmed = spoken.trim_start();
    let mut words = trimmed.split_whitespace();
    let first = words.next()?;
    let first_clean: String = first.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    if first_clean != "type" {
        return None;
    }
    let rest = trimmed[first.len()..].trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | ',' | '-'));
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let mut chars = rest.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// Lower case, single spaces, punctuation gone except a dot inside a word
/// (a web address) and a percent sign.
fn normalize(text: &str) -> String {
    let lowered = text.to_lowercase().replace('’', "'");
    let chars: Vec<char> = lowered.chars().collect();
    let mut out = String::with_capacity(chars.len());
    for (index, &c) in chars.iter().enumerate() {
        let inner_dot = c == '.'
            && index > 0
            && chars.get(index + 1).is_some_and(|next| next.is_alphanumeric())
            && chars[index - 1].is_alphanumeric();
        if c.is_alphanumeric() || c == '\'' || c == '%' || inner_dot {
            out.push(c);
        } else {
            out.push(' ');
        }
    }
    join_spelled(&out.split_whitespace().collect::<Vec<_>>())
}

/// Letters spelled one by one become one word, and a number said right
/// after them becomes a digit: "g t a five", "g.t.a 5" → "gta 5". A single
/// letter ("play a song") is left alone.
fn join_spelled(words: &[&str]) -> String {
    // A lone letter, or dotted letters arriving as one word ("g.t.a").
    let letters = |word: &str| -> Option<String> {
        let parts: Vec<&str> = word.split('.').filter(|part| !part.is_empty()).collect();
        let spelled = !parts.is_empty()
            && parts
                .iter()
                .all(|part| part.chars().count() == 1 && part.chars().all(char::is_alphabetic));
        spelled.then(|| parts.concat())
    };
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        let mut run = String::new();
        let mut end = index;
        while let Some(more) = words.get(end).and_then(|word| letters(word)) {
            run.push_str(&more);
            end += 1;
        }
        if run.chars().count() >= 2 {
            out.push(run);
            index = end;
            if let Some(n) = words.get(index).and_then(|word| number(word)) {
                out.push(n.to_string());
                index += 1;
            }
        } else {
            out.push(words[index].to_owned());
            index += 1;
        }
    }
    out.join(" ")
}

/// Kinds of thing people name before the name itself.
const KINDS: &[&str] = &[
    "channel", "group", "chat", "server", "community", "song", "track", "album", "artist", "band",
    "singer", "playlist", "podcast", "episode", "video", "movie", "film", "show", "series", "game",
    "app", "file", "folder", "document", "contact", "person", "user", "account", "page", "website",
    "site", "product", "book", "thing", "one",
];
/// Words that come between the kind and the name.
const NAMERS: &[&[&str]] = &[
    &["which", "is", "called"], &["that", "is", "called"], &["it's", "called"],
    &["by", "the", "name"], &["with", "the", "name"], &["which", "is"], &["that", "is"],
    &["that's"], &["called"], &["named"], &["titled"], &["by"],
];

/// The name alone, from how it was described: "a channel which is gmat
/// prep" → "gmat prep", "a song called blinding lights" → "blinding
/// lights", "the channel gmat prep" → "gmat prep". Only a description that
/// opens with an article and a kind of thing is taken apart, so "a word that
/// is spelled wrong" or "flights to dubai" stay as they are.
fn tidy_query(query: &str) -> String {
    let words: Vec<&str> = query.split(' ').filter(|word| !word.is_empty()).collect();
    let mut at = 0;
    if words.first().is_some_and(|w| matches!(*w, "a" | "an" | "the" | "some" | "my" | "this" | "that")) {
        at = 1;
    }
    let kind = words.get(at).is_some_and(|w| KINDS.contains(w))
        || words.get(at).is_some_and(|w| *w == "something" || *w == "anything");
    if !kind || (at == 0 && !matches!(words.first(), Some(&"something") | Some(&"anything"))) {
        return query.to_owned();
    }
    let mut start = at + 1;
    for namer in NAMERS {
        if words.get(start..start + namer.len()) == Some(namer) {
            start += namer.len();
            break;
        }
    }
    if start >= words.len() {
        return query.to_owned();
    }
    words[start..].join(" ")
}

fn strip_filler(text: &str) -> String {
    let mut s = text.to_owned();
    loop {
        let before = s.len();
        for filler in LEADING_FILLER {
            if let Some(rest) = after(&s, filler) {
                s = rest.to_owned();
            }
        }
        for filler in TRAILING_FILLER {
            if let Some(rest) = s.strip_suffix(filler) {
                if rest.is_empty() || rest.ends_with(' ') {
                    s = rest.trim_end().to_owned();
                }
            }
        }
        if s.len() == before {
            return s;
        }
    }
}

/// The text after `prefix` when `text` starts with it as whole words.
fn after<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(prefix)?;
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix(' ')
    }
}

/// "the spotify app" → "spotify".
fn clean_name(text: &str) -> String {
    let mut words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    while words.first().is_some_and(|w| NAME_PREFIX.contains(w)) {
        words.remove(0);
    }
    while words.len() > 1 && words.last().is_some_and(|w| NAME_SUFFIX.contains(w)) {
        words.pop();
    }
    words.join(" ")
}

fn media(s: &str) -> Option<Command> {
    let verb = match s {
        "pause" | "pause it" | "pause music" | "pause the music" | "pause the song" | "pause song"
        | "stop the music" | "stop music" | "stop playing" | "stop the song" | "pause playback" => {
            MediaVerb::Pause
        }
        "resume" | "resume music" | "resume the music" | "resume playing" | "continue"
        | "continue playing" | "unpause" | "keep playing" | "start the music" | "start music"
        | "start playing" => MediaVerb::Play,
        "next" | "next song" | "next track" | "skip" | "skip it" | "skip song" | "skip this"
        | "skip this song" | "skip the song" | "skip track" | "play the next song"
        | "play next song" | "play next" | "next one" | "change the song" | "change song" => {
            MediaVerb::Next
        }
        "previous" | "previous song" | "previous track" | "last song" | "go back a song"
        | "play the previous song" | "play previous song" | "play the last song" | "back"
        | "play that again" | "replay" | "replay the song" => MediaVerb::Previous,
        "play pause" | "toggle music" | "toggle play" => MediaVerb::PlayPause,
        _ => return None,
    };
    Some(Command::Media(verb))
}

fn play(s: &str) -> Option<Command> {
    let rest = after(s, "play")?;
    if RESUME_WORDS.contains(&rest) || rest == "music" {
        return Some(Command::Media(MediaVerb::Play));
    }
    for joiner in [" on ", " in ", " with ", " using "] {
        if let Some(at) = rest.rfind(joiner) {
            let app = clean_name(&rest[at + joiner.len()..]);
            let query = rest[..at].trim();
            if !app.is_empty() && !query.is_empty() {
                let query = if RESUME_WORDS.contains(&query) { "" } else { query };
                return Some(Command::Play {
                    query: query.to_owned(),
                    app: Some(app),
                });
            }
        }
    }
    Some(Command::Play {
        query: rest.to_owned(),
        app: None,
    })
}

fn volume(s: &str) -> Option<VolumeChange> {
    match s {
        "mute" | "mute it" | "mute the sound" | "mute sound" | "mute audio" | "mute the volume"
        | "mute volume" | "be quiet" | "silence" => return Some(VolumeChange::Mute),
        "unmute" | "unmute it" | "unmute the sound" | "unmute sound" | "unmute audio"
        | "sound on" | "unmute the volume" => return Some(VolumeChange::Unmute),
        "volume up" | "turn it up" | "turn up the volume" | "turn the volume up" | "louder"
        | "increase the volume" | "increase volume" | "raise the volume" | "raise volume"
        | "make it louder" | "volume higher" | "turn up volume" => return Some(VolumeChange::Up),
        "volume down" | "turn it down" | "turn down the volume" | "turn the volume down"
        | "quieter" | "softer" | "decrease the volume" | "decrease volume" | "lower the volume"
        | "lower volume" | "reduce the volume" | "reduce volume" | "make it quieter"
        | "turn down volume" => return Some(VolumeChange::Down),
        _ => {}
    }
    let rest = ["set the volume to", "set volume to", "set the volume at", "volume to", "change the volume to", "turn the volume to", "volume at", "volume"]
        .iter()
        .find_map(|prefix| after(s, prefix))?;
    let rest = rest.trim_end_matches(" percent").trim_end_matches('%').trim();
    number(rest).filter(|&n| n <= 100).map(|n| VolumeChange::Set(n as u8))
}

/// "50", "fifty", "twenty five", "a hundred".
fn number(text: &str) -> Option<u32> {
    if let Ok(n) = text.parse() {
        return Some(n);
    }
    let unit = |w: &str| {
        Some(match w {
            "zero" => 0, "one" => 1, "two" => 2, "three" => 3, "four" => 4, "five" => 5,
            "six" => 6, "seven" => 7, "eight" => 8, "nine" => 9, "ten" => 10, "eleven" => 11,
            "twelve" => 12, "thirteen" => 13, "fourteen" => 14, "fifteen" => 15,
            "sixteen" => 16, "seventeen" => 17, "eighteen" => 18, "nineteen" => 19,
            "twenty" => 20, "thirty" => 30, "forty" => 40, "fifty" => 50, "sixty" => 60,
            "seventy" => 70, "eighty" => 80, "ninety" => 90, "hundred" => 100,
            _ => return None,
        })
    };
    let words: Vec<&str> = text
        .split([' ', '-'])
        .filter(|w| !w.is_empty() && !matches!(*w, "a" | "and"))
        .collect();
    match words.as_slice() {
        [one] => unit(one),
        [tens, ones] => {
            let (tens, ones) = (unit(tens)?, unit(ones)?);
            (tens >= 20 && tens % 10 == 0 && ones < 10).then_some(tens + ones)
        }
        _ => None,
    }
}

fn window(s: &str) -> Option<Command> {
    const HERE: &[&str] = &["", "this", "it", "this window", "the window", "window", "that", "this app", "current window"];
    if matches!(s, "show desktop" | "show the desktop" | "minimize everything" | "minimize all" | "minimize all windows" | "go to desktop" | "go to the desktop") {
        return Some(Command::ShowDesktop);
    }
    if let Some(rest) = after(s, "maximize").or_else(|| after(s, "maximise")) {
        if HERE.contains(&rest) {
            return Some(Command::Maximize);
        }
    }
    for (verbs, make) in [
        (&["minimize", "minimise", "hide"][..], Command::Minimize as fn(Option<String>) -> Command),
        (&["close", "quit", "exit"][..], Command::Close),
    ] {
        for verb in verbs {
            if let Some(rest) = after(s, verb) {
                if HERE.contains(&rest) {
                    return Some(make(None));
                }
                let name = clean_name(rest);
                if !name.is_empty() {
                    return Some(make(Some(name)));
                }
            }
        }
    }
    None
}

fn search(s: &str) -> Option<Command> {
    let rest = ["search for", "search", "google", "look up", "find me", "search the web for", "search online for"]
        .iter()
        .find_map(|prefix| after(s, prefix))
        .filter(|rest| !rest.is_empty())?;
    for joiner in [" on ", " in "] {
        if let Some(at) = rest.rfind(joiner) {
            let site = clean_name(&rest[at + joiner.len()..]);
            let query = rest[..at].trim();
            if !site.is_empty() && !query.is_empty() {
                return Some(Command::Search {
                    query: query.to_owned(),
                    site: Some(site),
                });
            }
        }
    }
    Some(Command::Search {
        query: rest.to_owned(),
        site: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(name: &str) -> Option<Command> {
        Some(Command::Open(name.to_owned()))
    }

    #[test]
    fn opens_apps_and_sites_however_it_is_said() {
        assert_eq!(parse("Open Spotify."), open("spotify"));
        assert_eq!(parse("Can you open the Spotify app, please?"), open("spotify"));
        assert_eq!(parse("Launch Visual Studio Code."), open("visual studio code"));
        assert_eq!(parse("Switch to Chrome."), open("chrome"));
        assert_eq!(parse("Go to youtube.com."), open("youtube.com"));
        assert_eq!(parse("Hey Onyx, open up my calculator"), open("calculator"));
    }

    #[test]
    fn music_controls() {
        assert_eq!(parse("Next song."), Some(Command::Media(MediaVerb::Next)));
        assert_eq!(parse("Skip this song"), Some(Command::Media(MediaVerb::Next)));
        assert_eq!(parse("Pause the music."), Some(Command::Media(MediaVerb::Pause)));
        assert_eq!(parse("Play."), Some(Command::Media(MediaVerb::Play)));
        assert_eq!(parse("Play some music."), Some(Command::Media(MediaVerb::Play)));
        assert_eq!(parse("Previous track"), Some(Command::Media(MediaVerb::Previous)));
        assert_eq!(
            parse("Play Blinding Lights on Spotify."),
            Some(Command::Play { query: "blinding lights".into(), app: Some("spotify".into()) })
        );
        assert_eq!(
            parse("Play lo-fi beats"),
            Some(Command::Play { query: "lo fi beats".into(), app: None })
        );
        assert_eq!(
            parse("Play my music on Spotify"),
            Some(Command::Play { query: String::new(), app: Some("spotify".into()) })
        );
    }

    #[test]
    fn volume() {
        assert_eq!(parse("Volume up."), Some(Command::Volume(VolumeChange::Up)));
        assert_eq!(parse("Turn it down a bit"), Some(Command::Volume(VolumeChange::Down)));
        assert_eq!(parse("Turn it down."), Some(Command::Volume(VolumeChange::Down)));
        assert_eq!(parse("Mute."), Some(Command::Volume(VolumeChange::Mute)));
        assert_eq!(parse("Set the volume to 50%."), Some(Command::Volume(VolumeChange::Set(50))));
        assert_eq!(parse("Volume 30"), Some(Command::Volume(VolumeChange::Set(30))));
        assert_eq!(parse("Set volume to twenty five percent"), Some(Command::Volume(VolumeChange::Set(25))));
        assert_eq!(parse("Set volume to 500"), None);
    }

    #[test]
    fn typing_keeps_the_words_as_spoken() {
        assert_eq!(parse("Type see you at 5, okay?"), Some(Command::Type("See you at 5, okay?".into())));
        assert_eq!(parse("Type: Hello there."), Some(Command::Type("Hello there.".into())));
        assert_eq!(parse("Type"), None);
        // "typewriter" is not "type".
        assert_eq!(parse("Typewriter"), None);
    }

    #[test]
    fn windows() {
        assert_eq!(parse("Close this window."), Some(Command::Close(None)));
        assert_eq!(parse("Close Notepad."), Some(Command::Close(Some("notepad".into()))));
        assert_eq!(parse("Quit Spotify"), Some(Command::Close(Some("spotify".into()))));
        assert_eq!(parse("Minimize this."), Some(Command::Minimize(None)));
        assert_eq!(parse("Maximize the window"), Some(Command::Maximize));
        assert_eq!(parse("Show the desktop."), Some(Command::ShowDesktop));
    }

    #[test]
    fn search_and_system() {
        assert_eq!(
            parse("Search for flights to Dubai."),
            Some(Command::Search { query: "flights to dubai".into(), site: None })
        );
        assert_eq!(
            parse("Search lo-fi music on YouTube"),
            Some(Command::Search { query: "lo fi music".into(), site: Some("youtube".into()) })
        );
        assert_eq!(parse("Take a screenshot."), Some(Command::Screenshot));
        assert_eq!(parse("Lock my PC."), Some(Command::Lock));
    }

    #[test]
    fn several_steps_in_one_go() {
        assert_eq!(
            parse_all("Open Steam and type something."),
            Some(vec![open("steam").unwrap(), Command::Type("Something.".into())])
        );
        assert_eq!(
            parse_all("Open Notepad, then type hello and goodbye"),
            Some(vec![open("notepad").unwrap(), Command::Type("Hello and goodbye".into())])
        );
        assert_eq!(
            parse_all("Pause the music and open Spotify."),
            Some(vec![Command::Media(MediaVerb::Pause), open("spotify").unwrap()])
        );
        // One step that merely contains "and".
        assert_eq!(
            parse_all("Play rock and roll"),
            Some(vec![Command::Play { query: "rock and roll".into(), app: None }])
        );
        assert_eq!(parse_all("Type salt and pepper"), Some(vec![Command::Type("Salt and pepper".into())]));
        // A step that isn't understood stops the whole request.
        assert_eq!(parse_all("Open Notepad and dance"), Some(vec![open("notepad and dance").unwrap()]));
    }

    #[test]
    fn searches_keep_only_the_name() {
        let telegram = |query: &str| Some(Command::Search { query: query.into(), site: Some("telegram".into()) });
        assert_eq!(parse("Search for a channel which is GMAT prep on Telegram."), telegram("gmat prep"));
        assert_eq!(parse("Search for the channel GMAT Prep on Telegram"), telegram("gmat prep"));
        assert_eq!(
            parse_all("Open Telegram and search for a channel which is Gmat Prep."),
            Some(vec![Command::Open("telegram".into()), Command::Search { query: "gmat prep".into(), site: None }])
        );
        assert_eq!(
            parse("Play a song called Blinding Lights on Spotify"),
            Some(Command::Play { query: "blinding lights".into(), app: Some("spotify".into()) })
        );
        assert_eq!(parse("Play a song by Adele"), Some(Command::Play { query: "adele".into(), app: None }));
        assert_eq!(parse("Search for something called lo-fi"), Some(Command::Search { query: "lo fi".into(), site: None }));
        // Ordinary searches stay whole.
        for (said, query) in [
            ("Search for a word that is spelled wrong", "a word that is spelled wrong"),
            ("Search for song lyrics", "song lyrics"),
            ("Search for the best pizza near me", "the best pizza near me"),
            ("Search for flights to Dubai", "flights to dubai"),
        ] {
            assert_eq!(parse(said), Some(Command::Search { query: query.into(), site: None }), "{said}");
        }
    }

    #[test]
    fn spelled_letters_become_one_word() {
        let steam = |query: &str| Some(Command::Search { query: query.into(), site: Some("steam".into()) });
        assert_eq!(parse("Search for G T A five on Steam."), steam("gta 5"));
        assert_eq!(parse("Search for G.T.A. 5 on Steam"), steam("gta 5"));
        assert_eq!(parse("Search for G-T-A five on Steam"), steam("gta 5"));
        // A lone letter is a word.
        assert_eq!(parse("Play a b c by the Jackson 5"), Some(Command::Play { query: "abc by the jackson 5".into(), app: None }));
        // An ordinary number stays a word.
        assert_eq!(parse("Play five finger death punch"), Some(Command::Play { query: "five finger death punch".into(), app: None }));
    }

    #[test]
    fn anything_else_is_not_a_command() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("What's the weather like?"), None);
        assert_eq!(parse("Please."), None);
    }
}
