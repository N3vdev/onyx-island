//! Deciding whether a media session is music.
//!
//! GSMTC cannot tell: browsers report every tab as `Music` and expose no URL.
//! Music apps are trusted (unless they flag the content as video). For
//! browsers, the platform layer supplies the titles of all open tabs; the
//! tab whose title contains the playing track names the site ("... -
//! YouTube", "... | SoundCloud"). Music sites count; youtube.com counts only
//! for music videos; anything else does not.

/// Sites shown by default. Matched against the site name in the tab title.
pub const DEFAULT_MUSIC_SITES: &[&str] = &[
    "YouTube Music",
    "SoundCloud",
    "Deezer",
    "Spotify",
    "Apple Music",
    "TIDAL",
    "Bandcamp",
    "Amazon Music",
    "Audiomack",
    "Mixcloud",
    "JioSaavn",
    "Gaana",
    "Pandora",
    "Qobuz",
    "Napster",
];

/// Executable names (without `.exe`, lowercase) treated as web browsers.
pub const BROWSER_PROCESSES: &[&str] = &[
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "opera_gx",
    "vivaldi",
    "arc",
    "zen",
    "floorp",
    "librewolf",
    "waterfox",
    "thorium",
    "chromium",
    "yandex",
    "browser",
    "iexplore",
];

/// Whether a session's display name ("Firefox", "Edge", "Zen Browser")
/// belongs to a web browser.
#[must_use]
pub fn is_browser_name(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "chrome",
        "edge",
        "firefox",
        "brave",
        "opera",
        "vivaldi",
        "arc",
        "zen",
        "floorp",
        "librewolf",
        "waterfox",
        "thorium",
        "chromium",
        "yandex",
        "browser",
    ];
    let name = name.to_lowercase();
    NAMES.iter().any(|browser| contains_word(&name, browser))
}

/// Apps whose media sessions are voice notes and calls, never music.
const CHAT_APPS: &[&str] = &[
    "whatsapp", "telegram", "discord", "teams", "zoom", "skype", "signal", "messenger", "slack",
];

/// Whether a session's app id or name is a chat app.
#[must_use]
pub fn is_chat_app(app: &str) -> bool {
    let app = app.to_lowercase();
    CHAT_APPS.iter().any(|chat| app.contains(chat))
}

/// What the platform layer knows about one session.
#[derive(Clone, Copy, Debug)]
pub struct SessionFacts<'a> {
    /// The session's app id (or name).
    pub app: &'a str,
    pub is_browser: bool,
    /// The app itself marked the content as video (native apps only; browsers
    /// always say music).
    pub flagged_video: bool,
    pub title: &'a str,
    pub artist: &'a str,
}

/// The user's music filter, normalised once from config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MusicFilter {
    pub music_only: bool,
    /// Lowercase site names, longest first so "youtube music" beats
    /// "youtube".
    sites: Vec<String>,
}

impl MusicFilter {
    #[must_use]
    pub fn new(music_only: bool, sites: &[String]) -> Self {
        let mut sites: Vec<String> = sites
            .iter()
            .map(|site| site_token(site))
            .filter(|site| !site.is_empty())
            .collect();
        sites.sort_by_key(|site| std::cmp::Reverse(site.len()));
        sites.dedup();
        Self { music_only, sites }
    }

    /// Whether the session should appear on the island.
    #[must_use]
    pub fn is_music(&self, facts: SessionFacts<'_>, tab_titles: &[String]) -> bool {
        if !self.music_only {
            return true;
        }
        if !facts.is_browser {
            return !facts.flagged_video && !is_chat_app(facts.app);
        }
        let title = normalize(facts.title);
        let playing_tab = (!title.is_empty())
            .then(|| {
                tab_titles
                    .iter()
                    .find(|tab| normalize(tab).contains(&title))
            })
            .flatten();
        match playing_tab {
            Some(tab) => match self.site_of(tab) {
                Site::Listed => true,
                Site::YouTube => looks_like_music(facts.title, facts.artist),
                Site::Other => false,
            },
            // Music sites often show the song in the player but keep their
            // own name as the page title. If no tab names the track, accept
            // it when a listed music site is open.
            None => tab_titles
                .iter()
                .any(|tab| self.site_of(tab) == Site::Listed),
        }
    }

    /// The youtube.com tab playing this session, when the title heuristic
    /// rejected it. The platform layer can then look for the channel's
    /// "Official Artist Channel" badge on that page.
    #[must_use]
    pub fn doubtful_youtube_tab<'t>(
        &self,
        facts: SessionFacts<'_>,
        tab_titles: &'t [String],
    ) -> Option<&'t String> {
        if !self.music_only || !facts.is_browser {
            return None;
        }
        let title = normalize(facts.title);
        if title.is_empty() {
            return None;
        }
        tab_titles
            .iter()
            .find(|tab| normalize(tab).contains(&title))
            .filter(|tab| self.site_of(tab) == Site::YouTube)
            .filter(|_| !looks_like_music(facts.title, facts.artist))
    }

    /// Whether a tab title belongs to one of the listed music sites.
    #[must_use]
    pub fn is_music_site_tab(&self, tab_title: &str) -> bool {
        self.music_only && self.site_of(tab_title) == Site::Listed
    }

    /// Whether `tab_title` is the tab playing the track titled `track`.
    #[must_use]
    pub fn tab_names_track(tab_title: &str, track: &str) -> bool {
        let track = normalize(track);
        !track.is_empty() && normalize(tab_title).contains(&track)
    }

    fn site_of(&self, tab_title: &str) -> Site {
        let tab = normalize(tab_title);
        if self.sites.iter().any(|site| contains_word(&tab, site)) {
            return Site::Listed;
        }
        if tab.ends_with("- youtube") || tab == "youtube" {
            return Site::YouTube;
        }
        Site::Other
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Site {
    Listed,
    YouTube,
    Other,
}

/// Turns a site entry ("https://www.jiosaavn.com/song/x", "JioSaavn",
/// "music.apple.com") into the lowercase name expected in tab titles.
#[must_use]
pub fn site_token(entry: &str) -> String {
    let entry = entry.trim();
    let looks_like_url = entry.contains("://") || (entry.contains('.') && !entry.contains(' '));
    if !looks_like_url {
        return normalize(entry);
    }
    let host = entry
        .split("://")
        .nth(1)
        .unwrap_or(entry)
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut labels: Vec<&str> = host.split('.').filter(|label| !label.is_empty()).collect();
    if labels.len() > 1 {
        labels.pop(); // TLD
    }
    // Drop generic prefixes; "music.apple" keeps both words.
    while labels.len() > 1 && matches!(labels[0], "www" | "m" | "open" | "web" | "listen" | "play")
    {
        labels.remove(0);
    }
    if labels.len() == 2 && labels[0] == "music" {
        // music.apple.com -> "apple music", music.youtube.com -> "youtube music"
        return format!("{} music", labels[1]);
    }
    labels
        .last()
        .map_or_else(String::new, |label| (*label).to_owned())
}

/// Heuristic for youtube.com: is this video a song?
#[must_use]
pub fn looks_like_music(title: &str, channel: &str) -> bool {
    let title_lower = title.to_lowercase();
    let channel_lower = channel.to_lowercase();

    // Auto-generated artist channels and label channels are always music.
    if channel_lower.ends_with(" - topic") || channel_lower.contains("vevo") {
        return true;
    }

    if is_episode(&title_lower) {
        return false;
    }

    const NOT_MUSIC: &[&str] = &[
        "trailer",
        "teaser",
        "podcast",
        "episode",
        "ep.",
        "stand-up",
        "standup",
        "stand up comedy",
        "full show",
        "roast",
        "sketch",
        "prank",
        "challenge",
        "reaction",
        "reacts",
        "review",
        "tutorial",
        "how to",
        "vlog",
        "gameplay",
        "walkthrough",
        "interview",
        "#shorts",
        "unboxing",
        "explained",
        "documentary",
        "highlights",
        "news",
        "livestream",
        "stream highlights",
        "full match",
        "lecture",
    ];
    if NOT_MUSIC.iter().any(|marker| title_lower.contains(marker)) {
        return false;
    }

    const MUSIC: &[&str] = &[
        "official video",
        "official music video",
        "official audio",
        "official visualizer",
        "official lyric",
        "lyric video",
        "lyrics",
        "visualizer",
        "(audio)",
        "[audio]",
        "music video",
        "(mv)",
        "[mv]",
        // A bracketed credit is a song's; a bare "ft." also lists show guests
        // ("Show S2 EP8 ft. ...") and is only trusted below with "Artist - Song".
        "(feat.",
        "[feat.",
        "(ft.",
        "[ft.",
        "remix",
        "prod.",
        "slowed",
        "sped up",
        "reverb",
        "instrumental",
        "acoustic version",
        "live session",
        "(live)",
        "full album",
        "album stream",
        "official song",
        " - topic",
    ];
    if MUSIC.iter().any(|marker| title_lower.contains(marker)) {
        return true;
    }

    // "Artist - Song" where the channel is the artist, or that shape with a
    // featured artist ("Artist - Song ft. Other").
    if let Some((left, _)) = title_lower.split_once(" - ") {
        let left = left.trim();
        let channel = channel_lower.trim();
        if !left.is_empty()
            && !channel.is_empty()
            && (left.contains(channel) || channel.contains(left))
        {
            return true;
        }
        if title_lower.contains("feat.") || title_lower.contains("ft.") {
            return true;
        }
    }
    false
}

/// Episode numbering: "EP8", "Ep 12", "S2E5", "S2 EP8", "Season 3".
fn is_episode(title_lower: &str) -> bool {
    let words: Vec<&str> = title_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let digits = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    words.iter().enumerate().any(|(index, word)| {
        let next = words.get(index + 1).copied().unwrap_or("");
        // "ep8", "ep 8", "season 3"
        if let Some(rest) = word.strip_prefix("ep") {
            if digits(rest) || (rest.is_empty() && digits(next)) {
                return true;
            }
        }
        if *word == "season" && digits(next) {
            return true;
        }
        // "s2e5"
        word.strip_prefix('s')
            .and_then(|rest| rest.split_once('e'))
            .is_some_and(|(season, episode)| digits(season) && digits(episode))
    })
}

fn normalize(text: &str) -> String {
    // Lowercase, unify dashes and whitespace, drop the "(2) " unread badge
    // some sites prefix to the page title.
    let lower = text.to_lowercase().replace(['\u{2013}', '\u{2014}'], "-");
    let collapsed = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed
        .strip_prefix('(')
        .and_then(|rest| rest.split_once(") "))
    {
        Some((count, rest)) if count.chars().all(|c| c.is_ascii_digit()) => rest.to_owned(),
        _ => collapsed,
    }
}

/// `needle` appears in `haystack` on word boundaries (so "tidal" does not
/// match "tidally").
fn contains_word(haystack: &str, needle: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric();
    haystack.match_indices(needle).any(|(start, _)| {
        let before = haystack[..start].chars().next_back();
        let after = haystack[start + needle.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> MusicFilter {
        let sites: Vec<String> = DEFAULT_MUSIC_SITES
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        MusicFilter::new(true, &sites)
    }

    fn browser<'a>(title: &'a str, artist: &'a str) -> SessionFacts<'a> {
        SessionFacts {
            app: "Firefox",
            is_browser: true,
            flagged_video: false,
            title,
            artist,
        }
    }

    fn tabs(titles: &[&str]) -> Vec<String> {
        titles.iter().map(|t| (*t).to_owned()).collect()
    }

    #[test]
    fn music_apps_count_unless_they_flag_video() {
        let app = SessionFacts {
            app: "Spotify.exe",
            is_browser: false,
            flagged_video: false,
            title: "White Iverson",
            artist: "Post Malone",
        };
        assert!(filter().is_music(app, &[]));
        assert!(!filter().is_music(
            SessionFacts {
                flagged_video: true,
                ..app
            },
            &[]
        ));
    }

    #[test]
    fn chat_apps_are_never_music() {
        let whatsapp = SessionFacts {
            app: "5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App",
            is_browser: false,
            flagged_video: false,
            title: "(65) WhatsApp",
            artist: "",
        };
        assert!(!filter().is_music(whatsapp, &[]));
        assert!(is_chat_app("Discord"));
        assert!(!is_chat_app("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"));
    }

    #[test]
    fn the_real_report_x_video_is_not_music_but_the_youtube_song_is() {
        let open = tabs(&[
            "Inbox (2) - someone@gmail.com - Gmail",
            "(2) Don Toliver - Secondhand (feat. Rema) [Official Visualizer] - YouTube",
            "Home / X",
        ]);
        assert!(!filter().is_music(browser("Home / X", ""), &open));
        assert!(filter().is_music(
            browser(
                "Don Toliver - Secondhand (feat. Rema) [Official Visualizer]",
                "Don Toliver"
            ),
            &open
        ));
    }

    #[test]
    fn non_music_youtube_videos_are_hidden() {
        let open = tabs(&["How to fix a bike chain - YouTube"]);
        assert!(!filter().is_music(browser("How to fix a bike chain", "Park Tool"), &open));
        let open = tabs(&["Dune: Part Three | Official Trailer - YouTube"]);
        assert!(!filter().is_music(
            browser("Dune: Part Three | Official Trailer", "Warner Bros."),
            &open
        ));
    }

    #[test]
    fn music_sites_count_whatever_the_title() {
        let open = tabs(&["Stream Lofi beat by someone | Listen online for free on SoundCloud"]);
        assert!(filter().is_music(browser("Lofi beat", "someone"), &open));
        let open = tabs(&["Blinding Lights • The Weeknd - YouTube Music"]);
        assert!(filter().is_music(browser("Blinding Lights", "The Weeknd"), &open));
        // Deezer keeps its own name as the title: no tab names the song.
        let open = tabs(&["Deezer", "Home / X"]);
        assert!(filter().is_music(browser("Starboy", "The Weeknd"), &open));
    }

    #[test]
    fn unknown_sites_are_not_music_until_added() {
        let open = tabs(&["Kesariya - JioSaavn"]);
        let none = MusicFilter::new(true, &[]);
        assert!(!none.is_music(browser("Kesariya", "Arijit Singh"), &open));
        let custom = MusicFilter::new(true, &["https://www.jiosaavn.com/song/abc".to_owned()]);
        assert!(custom.is_music(browser("Kesariya", "Arijit Singh"), &open));
    }

    #[test]
    fn listing_youtube_trusts_all_of_youtube() {
        let open = tabs(&["How to fix a bike chain - YouTube"]);
        let filter = MusicFilter::new(true, &["youtube.com".to_owned()]);
        assert!(filter.is_music(browser("How to fix a bike chain", "Park Tool"), &open));
    }

    #[test]
    fn filter_off_shows_everything() {
        let filter = MusicFilter::new(false, &[]);
        assert!(filter.is_music(browser("Home / X", ""), &tabs(&["Home / X"])));
    }

    #[test]
    fn site_tokens_from_names_and_urls() {
        assert_eq!(site_token("JioSaavn"), "jiosaavn");
        assert_eq!(
            site_token("https://www.jiosaavn.com/song/x?y=1"),
            "jiosaavn"
        );
        assert_eq!(site_token("soundcloud.com"), "soundcloud");
        assert_eq!(
            site_token("https://music.apple.com/in/album/1"),
            "apple music"
        );
        assert_eq!(site_token("music.youtube.com"), "youtube music");
        assert_eq!(site_token("open.spotify.com"), "spotify");
        assert_eq!(site_token("  Amazon Music "), "amazon music");
        assert_eq!(site_token(""), "");
    }

    #[test]
    fn youtube_music_heuristics() {
        assert!(looks_like_music("Blinding Lights", "The Weeknd - Topic"));
        assert!(looks_like_music("Starboy", "TheWeekndVEVO"));
        assert!(looks_like_music(
            "Post Malone - Circles (Official Audio)",
            "Post Malone"
        ));
        assert!(looks_like_music(
            "Kesariya Lyrics | Brahmastra",
            "Sony Music India"
        ));
        assert!(looks_like_music("Drake - Passionfruit", "Drake"));
        assert!(!looks_like_music(
            "MKBHD reacts to iPhone 17",
            "Marques Brownlee"
        ));
        assert!(!looks_like_music("Minecraft Gameplay Part 3", "Some Gamer"));
        assert!(!looks_like_music("My morning routine", "Some Vlogger"));
        assert!(!looks_like_music(
            "Podcast Episode 12 - Music Industry",
            "Talk Show"
        ));
    }

    #[test]
    fn shows_with_guests_are_not_music() {
        // The real report: a comedy show episode whose guests are listed "ft.".
        let title =
            "INDIA\u{2019}S GOT LATENT S2 EP8 ft. MC Stan, Tanmay Bhat, Madhur Virli, Arpit Bala";
        assert!(!looks_like_music(title, "Samay Raina"));
        let open = tabs(&[&format!("{title} - YouTube")]);
        assert!(!filter().is_music(browser(title, "Samay Raina"), &open));

        assert!(!looks_like_music(
            "The Late Show Ep 12 ft. Some Guest",
            "Late Show"
        ));
        assert!(!looks_like_music(
            "Hot Ones S21E3 ft. A Rapper",
            "First We Feast"
        ));
        assert!(!looks_like_music(
            "Season 3 Finale ft. Everyone",
            "Some Show"
        ));
        assert!(!looks_like_music(
            "Stand-up special ft. friends",
            "Comedian"
        ));
    }

    #[test]
    fn only_rejected_youtube_tracks_are_doubtful() {
        let show = "INDIA\u{2019}S GOT LATENT S2 EP8 ft. MC Stan";
        let open = tabs(&[
            &format!("{show} - YouTube"),
            "Drake - Passionfruit - YouTube",
            "Kesariya - JioSaavn",
        ]);
        let doubtful = filter().doubtful_youtube_tab(browser(show, "Samay Raina"), &open);
        assert_eq!(doubtful, Some(&open[0]));
        // Already music by its title, or not on YouTube: nothing to check.
        assert!(filter()
            .doubtful_youtube_tab(browser("Drake - Passionfruit", "Drake"), &open)
            .is_none());
        assert!(filter()
            .doubtful_youtube_tab(browser("Kesariya", "Arijit Singh"), &open)
            .is_none());
    }

    #[test]
    fn song_credits_still_count() {
        assert!(looks_like_music("Secondhand (feat. Rema)", "Don Toliver"));
        assert!(looks_like_music("Calm Down [ft. Selena Gomez]", "Rema"));
        assert!(looks_like_music(
            "Rema - Calm Down ft. Selena Gomez",
            "Mavin Records"
        ));
        // Words that merely start with "ep" or "s" are not episode numbers.
        assert!(looks_like_music("Epic - Slowed", "Someone"));
        assert!(!is_episode("epic slowed version"));
        assert!(!is_episode("s club 7 never had a dream"));
        assert!(is_episode("s2e5 recap"));
    }

    #[test]
    fn browser_names() {
        for name in [
            "Firefox",
            "Chrome",
            "Edge",
            "Brave",
            "Zen Browser",
            "Opera GX",
            "Browser",
        ] {
            assert!(is_browser_name(name), "{name}");
        }
        for name in ["Spotify", "Media Player", "foobar2000", "Arcade Music"] {
            assert!(!is_browser_name(name), "{name}");
        }
    }

    #[test]
    fn word_boundaries_and_badges() {
        assert!(contains_word("blinding lights - tidal", "tidal"));
        assert!(!contains_word("tidally locked planets", "tidal"));
        assert_eq!(normalize("(12) Song – Artist"), "song - artist");
        assert_eq!(normalize("(Live) Song"), "(live) song");
    }
}
