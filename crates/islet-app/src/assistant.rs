//! The voice assistant: carries out a spoken [`Command`].
//!
//! Apps are found in the same list the Start menu shows (`shell:AppsFolder`),
//! so desktop programs and Store apps both work, by a forgiving name match.
//! An app that is already open is brought to the front instead of starting
//! a second copy. Nothing here logs what was said.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use islet_core::media::ArtImage;

use islet_core::command::{Command, MediaVerb, VolumeChange};
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, SHGetKnownFolderItem,
    ShellExecuteW, KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindow,
    GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, PostMessageW, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOWNORMAL, WM_CLOSE, WS_EX_TOOLWINDOW,
};

const VK_LWIN: u16 = 0x5B;
const VK_SHIFT: u16 = 0x10;
const VK_D: u16 = 0x44;
const VK_S: u16 = 0x53;
const VK_RETURN: u16 = 0x0D;
const VK_CONTROL: u16 = 0x11;
const VK_A: u16 = 0x41;
/// How long to keep looking for a just-opened app's search box.
const SEARCH_BOX_WAIT: Duration = Duration::from_secs(5);
const VK_VOLUME_DOWN: u16 = 0xAE;
const VK_VOLUME_UP: u16 = 0xAF;
/// Each volume key press moves the level 2%; a spoken "louder" is 10%.
const VOLUME_PRESSES: usize = 5;
/// After starting a player for "play my music", when to press play.
const PLAY_AFTER_LAUNCH: Duration = Duration::from_millis(3500);

/// What happened, for the island.
#[derive(Debug, Default)]
pub struct Reply {
    /// Shown on the island; `None` when the result speaks for itself (the
    /// song changes, the volume display appears).
    pub notice: Option<(String, String)>,
    pub ok: bool,
    /// A music control for the island to send.
    pub media: Option<MediaVerb>,
    /// The same, a moment later (a player that was just started).
    pub media_later: Option<(MediaVerb, Duration)>,
    /// The icon of the app it was about, shown beside the notice.
    pub icon: Option<ArtImage>,
}

impl Reply {
    fn done(title: impl Into<String>) -> Self {
        Self { notice: Some((title.into(), String::new())), ok: true, ..Self::default() }
    }
    fn quiet() -> Self {
        Self { ok: true, ..Self::default() }
    }
    fn failed(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self { notice: Some((title.into(), body.into())), ok: false, ..Self::default() }
    }
}

#[derive(Clone, Debug)]
struct AppEntry {
    name: String,
    /// What `shell:AppsFolder\` opens: a Store app's ID or a program's path.
    target: String,
    /// A desktop program's file name, lower case, to find its windows.
    exe: Option<String>,
}

pub struct Assistant {
    apps: Arc<Mutex<Option<Arc<Vec<AppEntry>>>>>,
    /// App icons by Start-menu target, fetched once each.
    icons: Mutex<HashMap<String, Option<ArtImage>>>,
}

impl Assistant {
    /// Starts listing the installed apps in the background, so the first
    /// command doesn't wait for it.
    pub fn new() -> Self {
        let apps = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&apps);
        std::thread::spawn(move || {
            let list = Arc::new(list_apps());
            tracing::info!(apps = list.len(), "assistant app list ready");
            *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(list);
        });
        Self { apps, icons: Mutex::new(HashMap::new()) }
    }

    /// The app's icon as the Start menu shows it, cached.
    fn icon(&self, app: &AppEntry) -> Option<ArtImage> {
        let mut icons = self.icons.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        icons.entry(app.target.clone()).or_insert_with(|| app_icon(&app.target)).clone()
    }

    fn with_icon(&self, reply: Reply, app: Option<&AppEntry>) -> Reply {
        Reply { icon: app.and_then(|app| self.icon(app)), ..reply }
    }

    fn apps(&self, refresh: bool) -> Arc<Vec<AppEntry>> {
        let mut slot = self.apps.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if refresh || slot.is_none() {
            *slot = Some(Arc::new(list_apps()));
        }
        slot.clone().unwrap_or_default()
    }

    /// The best installed app for a spoken name; looks again once if
    /// nothing matches (something installed since).
    fn find_app(&self, spoken: &str) -> Option<AppEntry> {
        let query = alias(spoken);
        for refresh in [false, true] {
            let apps = self.apps(refresh);
            if let Some(app) = best_match(&query, apps.iter().map(|app| app.name.as_str()))
                .map(|index| apps[index].clone())
            {
                return Some(app);
            }
        }
        None
    }

    fn find_app_cached(&self, spoken: &str) -> Option<AppEntry> {
        let apps = self.apps(false);
        best_match(&alias(spoken), apps.iter().map(|app| app.name.as_str())).map(|index| apps[index].clone())
    }

    /// After opening `name`, waits (up to `timeout`) for its window and
    /// brings it to the front, so what comes next lands in it.
    pub fn wait_for_window(&self, name: &str, timeout: Duration) -> bool {
        let app = self.find_app_cached(name);
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if let Some(&hwnd) = app_windows(name, app.as_ref()).first() {
                focus(hwnd);
                // Let the window take keyboard focus before anything is typed.
                std::thread::sleep(Duration::from_millis(400));
                return true;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        false
    }

    /// Carries out everything but typing, which the dictation controller
    /// does with its own insertion settings.
    pub fn run(&self, command: &Command) -> Reply {
        match command {
            Command::Open(name) => self.open(name),
            Command::Close(name) => self.close(name.as_deref()),
            Command::Minimize(name) => self.minimize(name.as_deref()),
            Command::Maximize => match front_window() {
                Some(hwnd) => {
                    // SAFETY: a plain window call on a live top-level window.
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_MAXIMIZE);
                    }
                    Reply::quiet()
                }
                None => Reply::failed("No window", "There's no window in front to maximize."),
            },
            Command::ShowDesktop => {
                islet_input::press_chord(&[VK_LWIN, VK_D]);
                Reply::quiet()
            }
            Command::Media(verb) => Reply { media: Some(*verb), ..Reply::quiet() },
            // Added by the island, not here.
            Command::AddTask(_) => Reply::quiet(),
            Command::Play { query, app } => self.play(query, app.as_deref()),
            Command::Volume(change) => volume(*change),
            Command::Search { query, site } => {
                let url = search_url(query, site.as_deref());
                if open_target(&url) {
                    Reply::done(match site {
                        Some(site) => format!("Searching {}", title_case(site)),
                        None => "Searching the web".to_owned(),
                    })
                } else {
                    Reply::failed("Couldn't search", "No browser opened.")
                }
            }
            Command::Screenshot => {
                islet_input::press_chord(&[VK_LWIN, VK_SHIFT, VK_S]);
                Reply::quiet()
            }
            Command::Lock => {
                // SAFETY: no preconditions; locking is asynchronous.
                let locked = unsafe { windows::Win32::System::Shutdown::LockWorkStation() }.is_ok();
                if locked { Reply::quiet() } else { Reply::failed("Couldn't lock", "Windows refused.") }
            }
            Command::Type(_) => Reply::quiet(),
        }
    }

    /// "Open X and search for Y": X's own search. Apps with a search link
    /// open it; any other app gets its search box found and filled in.
    ///
    /// An installed app is searched the way you would: its window comes to
    /// the front, the search box is clicked into, anything already there is
    /// replaced, the words are typed and Enter is pressed. A website or a
    /// browser uses its search link; so does an app whose box can't be found.
    pub fn search_in(&self, app: &str, query: &str, method: islet_input::Method, wait: Duration) -> Reply {
        let entry = self.find_app_cached(app);
        let label = entry.as_ref().map_or_else(|| title_case(app), |entry| entry.name.clone());
        let searching = || self.with_icon(Reply::done(format!("Searching {label} for \u{201c}{query}\u{201d}")), entry.as_ref());
        let by_link = || app_search_url(app, query).is_some_and(|url| open_target(&url));
        if entry.as_ref().is_none_or(|entry| is_browser(&entry.name)) {
            return if by_link() { searching() } else { Reply::failed(format!("Couldn't search {label}"), "") };
        }
        if self.wait_for_window(app, wait) && self.type_in_search_box(app, entry.as_ref(), query, method) {
            return searching();
        }
        if by_link() {
            return searching();
        }
        self.with_icon(Reply::failed(format!("No search box in {label}"), ""), entry.as_ref())
    }

    /// Clicks into the app's search box and types `query`. A freshly opened
    /// app fills in its window a moment after it appears, so this keeps
    /// looking for a few seconds. Nothing is typed unless the app's own
    /// window is in front with its search box focused.
    fn type_in_search_box(&self, app: &str, entry: Option<&AppEntry>, query: &str, method: islet_input::Method) -> bool {
        let started = std::time::Instant::now();
        while started.elapsed() < SEARCH_BOX_WAIT {
            let windows = app_windows(app, entry);
            if let Some(&window) = windows.first() {
                if focus_search_box(window) {
                    std::thread::sleep(Duration::from_millis(150));
                    // SAFETY: a plain query.
                    let front = unsafe { GetForegroundWindow() };
                    if !windows.contains(&front) {
                        return false;
                    }
                    islet_input::press_chord(&[VK_CONTROL, VK_A]);
                    if islet_input::insert_text_with_clipboard(query, method, false).is_err() {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(80));
                    islet_input::press_chord(&[VK_RETURN]);
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(400));
        }
        false
    }

    /// An installed app (not a browser), whose own search box can be used.
    pub fn is_app(&self, name: &str) -> bool {
        self.find_app_cached(name).is_some_and(|entry| !is_browser(&entry.name))
    }

    /// Opens what was named. Extra words after a name ("Steam app for me")
    /// don't stop it: shorter and shorter versions are tried.
    fn open(&self, name: &str) -> Reply {
        let words: Vec<&str> = name.split_whitespace().collect();
        for refresh in [false, true] {
            if refresh {
                self.apps(true);
            }
            for length in (1..=words.len()).rev() {
                if let Some(reply) = self.try_open(&words[..length].join(" ")) {
                    return reply;
                }
            }
        }
        Reply::failed(format!("Couldn't find {}", title_case(name)), "")
    }

    fn try_open(&self, name: &str) -> Option<Reply> {
        let app = self.find_app_cached(name);
        let label = app.as_ref().map_or_else(|| title_case(name), |app| app.name.clone());
        if let Some(&hwnd) = app_windows(name, app.as_ref()).first() {
            focus(hwnd);
            return Some(self.with_icon(Reply::done(format!("Switched to {label}")), app.as_ref()));
        }
        if let Some(app) = &app {
            if open_target(&format!("shell:AppsFolder\\{}", app.target)) {
                return Some(self.with_icon(Reply::done(format!("Opening {}", app.name)), Some(app)));
            }
        }
        let url = site_url(name)?;
        open_target(&url).then(|| Reply::done(format!("Opening {label}")))
    }

    fn close(&self, name: Option<&str>) -> Reply {
        let app = name.and_then(|name| self.find_app(name));
        let targets = match name {
            None => front_window().into_iter().collect::<Vec<_>>(),
            Some(name) => app_windows(name, app.as_ref()),
        };
        if targets.is_empty() {
            return match name {
                Some(name) => Reply::failed(format!("{} isn't open", title_case(name)), ""),
                None => Reply::failed("No window", "There's no window in front to close."),
            };
        }
        for hwnd in &targets {
            // A polite close: the app can still ask to save.
            // SAFETY: posting to a window handle is safe even if it is gone.
            unsafe {
                let _ = PostMessageW(Some(*hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        match name {
            Some(name) => {
                let label = app.as_ref().map_or_else(|| title_case(name), |app| app.name.clone());
                self.with_icon(Reply::done(format!("Closing {label}")), app.as_ref())
            }
            None => Reply::quiet(),
        }
    }

    fn minimize(&self, name: Option<&str>) -> Reply {
        let targets = match name {
            None => front_window().into_iter().collect::<Vec<_>>(),
            Some(name) => app_windows(name, self.find_app(name).as_ref()),
        };
        if targets.is_empty() {
            return Reply::failed("Nothing to minimize", "");
        }
        for hwnd in targets {
            // SAFETY: a plain window call.
            unsafe {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
        }
        Reply::quiet()
    }

    /// "Play ..." with something to find. Spotify, if it is installed and
    /// wasn't ruled out, opens its search; otherwise YouTube Music does.
    fn play(&self, query: &str, app: Option<&str>) -> Reply {
        let wants = app.map(alias);
        let spotify = || self.find_app("spotify").filter(|app| app.name.to_lowercase().contains("spotify"));
        let use_spotify = match wants.as_deref() {
            Some(name) => name.contains("spotify"),
            None => spotify().is_some(),
        };
        if query.is_empty() {
            // "Play my music on Spotify": start the player, then press play.
            let Some(name) = app else {
                return Reply { media: Some(MediaVerb::Play), ..Reply::quiet() };
            };
            let opened = self.open(name);
            if !opened.ok {
                return opened;
            }
            let running = opened.notice.as_ref().is_some_and(|(title, _)| title.starts_with("Switched"));
            return Reply {
                media: running.then_some(MediaVerb::Play),
                media_later: (!running).then_some((MediaVerb::Play, PLAY_AFTER_LAUNCH)),
                ..opened
            };
        }
        if use_spotify && open_target(&format!("spotify:search:{}", encode(query, "%20"))) {
            let done = Reply::done(format!("Finding \u{201c}{query}\u{201d} on Spotify"));
            return self.with_icon(done, spotify().as_ref());
        }
        let url = match wants.as_deref() {
            Some(name) if name.contains("youtube") && !name.contains("music") => {
                format!("https://www.youtube.com/results?search_query={}", encode(query, "+"))
            }
            _ => format!("https://music.youtube.com/search?q={}", encode(query, "+")),
        };
        if open_target(&url) {
            Reply::done(format!("Finding \u{201c}{query}\u{201d}"))
        } else {
            Reply::failed("Couldn't play that", "No player or browser opened.")
        }
    }
}

fn volume(change: VolumeChange) -> Reply {
    match change {
        VolumeChange::Up => {
            islet_input::press_chord(&[VK_VOLUME_UP; VOLUME_PRESSES]);
            Reply::quiet()
        }
        VolumeChange::Down => {
            islet_input::press_chord(&[VK_VOLUME_DOWN; VOLUME_PRESSES]);
            Reply::quiet()
        }
        VolumeChange::Set(level) => {
            if islet_audio::volume::set_master_once(Some(f32::from(level) / 100.0), None) {
                Reply::done(format!("Volume {level}%"))
            } else {
                Reply::failed("Couldn't change the volume", "No output device.")
            }
        }
        VolumeChange::Mute | VolumeChange::Unmute => {
            let mute = change == VolumeChange::Mute;
            if islet_audio::volume::set_master_once(None, Some(mute)) {
                Reply::done(if mute { "Muted" } else { "Sound on" })
            } else {
                Reply::failed("Couldn't change the volume", "No output device.")
            }
        }
    }
}

// ----- matching ---------------------------------------------------------

/// Everyday names for apps whose real names differ.
fn alias(spoken: &str) -> String {
    let lower = spoken.trim().to_lowercase();
    match lower.as_str() {
        "vs code" | "vscode" | "code" | "visual studio code" | "vs" => "visual studio code",
        "explorer" | "files" | "file manager" | "my files" | "file explorer" => "file explorer",
        "cmd" | "command line" => "command prompt",
        "powershell" | "power shell" => "windows powershell",
        "chrome" | "google chrome" => "google chrome",
        "edge" => "microsoft edge",
        "store" | "app store" | "microsoft store" => "microsoft store",
        "word" | "microsoft word" => "word",
        "excel" | "microsoft excel" => "excel",
        "power point" | "powerpoint" => "powerpoint",
        "settings" | "windows settings" => "settings",
        "whats app" | "what's app" => "whatsapp",
        "calc" => "calculator",
        _ => return lower,
    }
    .to_owned()
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_browser(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["chrome", "edge", "firefox", "brave", "opera", "vivaldi", "arc", "browser"]
        .iter()
        .any(|browser| lower.split_whitespace().any(|word| word == *browser))
}

/// Entries the Start menu lists that are never what someone means.
fn is_extra(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["uninstall", "readme", "read me", "documentation", "release notes", "help", "manual", "license", "website", "what's new", "changelog"]
        .iter()
        .any(|word| lower.contains(word))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let substitute = previous + usize::from(ca != cb);
            previous = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

/// How well an app name fits what was said; 0 is no match.
fn score(query: &str, name: &str) -> u32 {
    let (q, n) = (words(query), words(name));
    if q.is_empty() || n.is_empty() {
        return 0;
    }
    let (qj, nj) = (q.concat(), n.concat());
    let base = if q == n {
        1000
    } else if qj == nj {
        950
    } else if n.starts_with(&q) {
        800
    } else if q.iter().all(|w| n.contains(w)) {
        600
    } else if nj.starts_with(&qj) && qj.len() >= 3 {
        500
    } else if q.len() == 1 && qj.len() >= 5 && n.iter().any(|w| edit_distance(&qj, w) <= if qj.len() >= 8 { 2 } else { 1 }) {
        // A word misheard by a letter or two ("spotfy").
        300
    } else {
        return 0;
    };
    if is_extra(name) { base / 10 } else { base }
}

/// The index of the best-fitting name; ties go to the shorter name.
fn best_match<'a>(query: &str, names: impl Iterator<Item = &'a str>) -> Option<usize> {
    names
        .enumerate()
        .map(|(index, name)| (score(query, name), std::cmp::Reverse(name.len()), index))
        .filter(|(score, _, _)| *score >= 300)
        .max()
        .map(|(_, _, index)| index)
}

/// Well-known websites, for names with no installed app.
fn site_url(name: &str) -> Option<String> {
    let lower = name.trim().to_lowercase();
    if !lower.contains(' ') && lower.contains('.') {
        // Always a web address: a name like "setup.bat" must never be opened
        // as a file on this PC.
        let site = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")).unwrap_or(&lower);
        return Some(format!("https://{site}"));
    }
    let joined: String = lower.split_whitespace().collect();
    let url = match joined.as_str() {
        "youtube" => "https://www.youtube.com",
        "youtubemusic" => "https://music.youtube.com",
        "gmail" | "email" | "mail" => "https://mail.google.com",
        "google" => "https://www.google.com",
        "googledrive" | "drive" => "https://drive.google.com",
        "googledocs" | "docs" => "https://docs.google.com",
        "googlemaps" | "maps" => "https://maps.google.com",
        "github" => "https://github.com",
        "netflix" => "https://www.netflix.com",
        "reddit" => "https://www.reddit.com",
        "twitter" => "https://x.com",
        "instagram" => "https://www.instagram.com",
        "facebook" => "https://www.facebook.com",
        "linkedin" => "https://www.linkedin.com",
        "amazon" => "https://www.amazon.com",
        "chatgpt" => "https://chatgpt.com",
        "claude" => "https://claude.ai",
        "wikipedia" => "https://www.wikipedia.org",
        "whatsapp" | "whatsappweb" => "https://web.whatsapp.com",
        "spotify" => "https://open.spotify.com",
        _ => return None,
    };
    Some(url.to_owned())
}

/// Apps (and sites) whose own search opens from a link.
fn app_search_url(app: &str, query: &str) -> Option<String> {
    let joined: String = words(&alias(app)).concat();
    Some(match joined.as_str() {
        "spotify" => format!("spotify:search:{}", encode(query, "%20")),
        "youtube" => format!("https://www.youtube.com/results?search_query={}", encode(query, "+")),
        "youtubemusic" => format!("https://music.youtube.com/search?q={}", encode(query, "+")),
        "netflix" => format!("https://www.netflix.com/search?q={}", encode(query, "+")),
        "amazon" => format!("https://www.amazon.com/s?k={}", encode(query, "+")),
        "wikipedia" => format!("https://en.wikipedia.org/w/index.php?search={}", encode(query, "+")),
        "github" => format!("https://github.com/search?q={}", encode(query, "+")),
        "reddit" => format!("https://www.reddit.com/search/?q={}", encode(query, "+")),
        "microsoftstore" => format!("ms-windows-store://search/?query={}", encode(query, "%20")),
        "steam" => format!("steam://openurl/https://store.steampowered.com/search/?term={}", encode(query, "+")),
        "fileexplorer" => format!("search-ms:query={}", encode(query, "%20")),
        "googlechrome" | "microsoftedge" | "firefox" | "google" => {
            format!("https://www.google.com/search?q={}", encode(query, "+"))
        }
        _ => return None,
    })
}

/// Finds the search box in a window through UI Automation (what screen
/// readers use) and puts the keyboard focus in it. Only a field that says
/// it is for searching counts, so nothing is ever typed into another one.
fn focus_search_box(window: HWND) -> bool {
    let handle = window.0 as isize;
    let (sender, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // SAFETY: this thread owns its apartment until CoUninitialize; every
        // UI Automation object is dropped before then.
        let found = unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                false
            } else {
                let found = search_box(HWND(handle as *mut _), true).unwrap_or(false);
                CoUninitialize();
                found
            }
        };
        let _ = sender.send(found);
    });
    // A big app's control tree can be slow; don't hang the assistant.
    result.recv_timeout(Duration::from_secs(3)).unwrap_or(false)
}

unsafe fn search_box(window: HWND, focus: bool) -> windows::core::Result<bool> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ComboBoxControlTypeId,
        UIA_ControlTypePropertyId, UIA_EditControlTypeId,
    };
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
    let root = automation.ElementFromHandle(window)?;
    for control in [UIA_EditControlTypeId, UIA_ComboBoxControlTypeId] {
        let condition = automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(control.0))?;
        let fields = root.FindAll(TreeScope_Descendants, &condition)?;
        for index in 0..fields.Length()? {
            let field = fields.GetElement(index)?;
            if field.CurrentIsOffscreen().is_ok_and(|offscreen| offscreen.as_bool())
                || !field.CurrentIsEnabled().is_ok_and(|enabled| enabled.as_bool())
            {
                continue;
            }
            // Hidden fields report no size (Spotify carries an invisible
            // browser address bar).
            let rect = field.CurrentBoundingRectangle().unwrap_or_default();
            if rect.right - rect.left < 4 || rect.bottom - rect.top < 4 {
                continue;
            }
            let label = [
                field.CurrentName().map(|name| name.to_string()).unwrap_or_default(),
                field.CurrentAutomationId().map(|id| id.to_string()).unwrap_or_default(),
                field.CurrentHelpText().map(|help| help.to_string()).unwrap_or_default(),
            ]
            .join(" ")
            .to_lowercase();
            if is_search_label(&label) {
                if focus {
                    field.SetFocus()?;
                }
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// What a search box calls itself: "Search", "Find", or a prompt such as
/// Spotify's "What do you want to play?".
fn is_search_label(label: &str) -> bool {
    ["search", "find", "what do you want", "look up", "look for"]
        .iter()
        .any(|word| label.contains(word))
        && !label.contains("address")
}

fn search_url(query: &str, site: Option<&str>) -> String {
    let joined: String = site.unwrap_or("").split_whitespace().collect::<String>().to_lowercase();
    match joined.as_str() {
        "youtube" => format!("https://www.youtube.com/results?search_query={}", encode(query, "+")),
        "youtubemusic" => format!("https://music.youtube.com/search?q={}", encode(query, "+")),
        "spotify" => format!("spotify:search:{}", encode(query, "%20")),
        "amazon" => format!("https://www.amazon.com/s?k={}", encode(query, "+")),
        "wikipedia" => format!("https://en.wikipedia.org/w/index.php?search={}", encode(query, "+")),
        "github" => format!("https://github.com/search?q={}", encode(query, "+")),
        "reddit" => format!("https://www.reddit.com/search/?q={}", encode(query, "+")),
        "" | "google" | "the web" | "web" | "internet" | "theinternet" | "online" => {
            format!("https://www.google.com/search?q={}", encode(query, "+"))
        }
        _ => format!("https://www.google.com/search?q={}", encode(&format!("{query} {joined}"), "+")),
    }
}

fn encode(text: &str, space: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            b' ' => space.to_owned(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ----- Windows ------------------------------------------------------------

fn open_target(target: &str) -> bool {
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let result = unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    result.0 as usize > 32
}

/// Every app in the Start menu's "All apps" list, desktop and Store alike.
fn list_apps() -> Vec<AppEntry> {
    std::thread::spawn(|| {
        // SAFETY: this thread owns its apartment until CoUninitialize; every
        // COM object is dropped before that.
        unsafe {
            if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
                return Vec::new();
            }
            let apps = enumerate_apps().unwrap_or_default();
            CoUninitialize();
            apps
        }
    })
    .join()
    .unwrap_or_default()
}

unsafe fn enumerate_apps() -> windows::core::Result<Vec<AppEntry>> {
    let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
    let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
    let mut apps = Vec::new();
    loop {
        let mut batch = [None];
        let mut fetched = 0;
        if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
            break;
        }
        let Some(item) = batch[0].take() else { break };
        let (Some(name), Some(target)) = (shell_name(&item, SIGDN_NORMALDISPLAY), shell_name(&item, SIGDN_PARENTRELATIVEPARSING)) else {
            continue;
        };
        let exe = target
            .to_lowercase()
            .ends_with(".exe")
            .then(|| target.rsplit(['\\', '/']).next().unwrap_or(&target).to_lowercase());
        apps.push(AppEntry { name, target, exe });
    }
    Ok(apps)
}

unsafe fn shell_name(item: &IShellItem, kind: windows::Win32::UI::Shell::SIGDN) -> Option<String> {
    let name = item.GetDisplayName(kind).ok()?;
    let text = name.to_string().ok();
    CoTaskMemFree(Some(name.0 as *const _));
    text.filter(|text| !text.is_empty())
}

/// Icon pixels this size or larger read sharply at the notice's 36 px.
const ICON_PIXELS: i32 = 96;

/// An app's icon as the Start menu draws it, as premultiplied BGRA.
fn app_icon(target: &str) -> Option<ArtImage> {
    shell_icon(&format!("shell:AppsFolder\\{target}"))
}

/// The icon the shell shows for `name` (a `shell:` path or a file), as
/// premultiplied BGRA. Blocks for the few milliseconds it takes.
pub(crate) fn shell_icon(name: &str) -> Option<ArtImage> {
    let target = name.to_owned();
    std::thread::spawn(move || {
        // SAFETY: this thread owns its apartment until CoUninitialize; the
        // factory is dropped and the bitmap deleted before that.
        unsafe {
            if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
                return None;
            }
            let icon = load_icon(&target);
            CoUninitialize();
            icon
        }
    })
    .join()
    .ok()
    .flatten()
}

unsafe fn load_icon(target: &str) -> Option<ArtImage> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };
    let path: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None).ok()?;
    let bitmap = factory
        .GetImage(SIZE { cx: ICON_PIXELS, cy: ICON_PIXELS }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)
        .ok()?;
    let mut info = BITMAP::default();
    let read = GetObjectW(bitmap.into(), std::mem::size_of::<BITMAP>() as i32, Some((&mut info as *mut BITMAP).cast()));
    let (width, height) = (info.bmWidth, info.bmHeight.abs());
    let mut pixels = vec![0u8; (width.max(0) * height * 4) as usize];
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = CreateCompatibleDC(None);
    let lines = if read > 0 && width > 0 && height > 0 {
        GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut header, DIB_RGB_COLORS)
    } else {
        0
    };
    let _ = DeleteDC(dc);
    let _ = DeleteObject(bitmap.into());
    if lines <= 0 {
        return None;
    }
    to_premultiplied(&mut pixels);
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ("app icon", target).hash(&mut hasher);
    Some(ArtImage { id: hasher.finish(), width: width as u32, height: height as u32, pixels: pixels.into() })
}

/// Icons arrive with straight alpha, premultiplied alpha, or (old ones) none
/// at all; the renderer wants premultiplied.
fn to_premultiplied(pixels: &mut [u8]) {
    if pixels.chunks_exact(4).all(|p| p[3] == 0) {
        for p in pixels.chunks_exact_mut(4) {
            p[3] = 255;
        }
        return;
    }
    let straight = pixels.chunks_exact(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]);
    if straight {
        for p in pixels.chunks_exact_mut(4) {
            let a = u16::from(p[3]);
            for channel in &mut p[..3] {
                *channel = ((u16::from(*channel) * a + 127) / 255) as u8;
            }
        }
    }
}

struct Window {
    hwnd: HWND,
    title: String,
    exe: String,
}

/// The visible top-level windows, front to back, without Onyx's own.
fn top_windows() -> Vec<Window> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = &mut *(lparam.0 as *mut Vec<HWND>);
        list.push(hwnd);
        true.into()
    }
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into the Vec it is handed, which
    // outlives the call.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<HWND> as isize));
    }
    // SAFETY: no preconditions.
    let own = unsafe { GetCurrentProcessId() };
    handles
        .into_iter()
        .filter_map(|hwnd| {
            // SAFETY: plain queries on a window handle; a stale one fails.
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
                    return None;
                }
                if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                    return None;
                }
                // The desktop and the taskbar are never an app's window:
                // closing the desktop would ask to shut Windows down.
                if is_shell_window(hwnd) {
                    return None;
                }
                let mut cloaked = 0u32;
                let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
                if cloaked != 0 {
                    return None;
                }
                let length = GetWindowTextLengthW(hwnd);
                if length <= 0 {
                    return None;
                }
                let mut buffer = vec![0u16; length as usize + 1];
                let copied = GetWindowTextW(hwnd, &mut buffer);
                let title = String::from_utf16_lossy(&buffer[..copied.max(0) as usize]);
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid == own {
                    return None;
                }
                Some(Window { hwnd, title, exe: process_exe(pid).unwrap_or_default() })
            }
        })
        .collect()
}

fn process_exe(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed before returning; the buffer outlives
    // the call.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut length).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| {
            let path = String::from_utf16_lossy(&buffer[..length as usize]);
            path.rsplit('\\').next().unwrap_or(&path).to_lowercase()
        })
    }
}

/// The windows that belong to an app, front first.
fn app_windows(spoken: &str, app: Option<&AppEntry>) -> Vec<HWND> {
    let windows = top_windows();
    // A desktop program: its own process's windows.
    if let Some(exe) = app.and_then(|app| app.exe.as_deref()) {
        let found: Vec<HWND> = windows.iter().filter(|w| w.exe == exe).map(|w| w.hwnd).collect();
        if !found.is_empty() {
            return found;
        }
    }
    // The program names it could run as: "spotify" is spotify.exe, and a
    // few apps run under another name.
    let spoken_name = alias(spoken);
    let mut programs = vec![words(&spoken_name).concat()];
    if let Some(app) = app {
        programs.push(words(&app.name).concat());
        programs.extend(known_program(&app.name.to_lowercase()).map(str::to_owned));
    }
    programs.extend(known_program(&spoken_name).map(str::to_owned));
    programs.retain(|program| !program.is_empty());
    let name = app.map(|app| words(&app.name));
    windows
        .iter()
        .filter(|w| {
            let stem: String = words(w.exe.trim_end_matches(".exe")).concat();
            let title = words(&w.title);
            // A Store app's window carries its name at the end of the title
            // ("Inbox - Mail"), as do many others ("... - Visual Studio Code").
            programs.contains(&stem)
                || name.as_ref().is_some_and(|name| !name.is_empty() && title.ends_with(name))
                || (app.is_none() && title.ends_with(&words(spoken)))
        })
        .map(|w| w.hwnd)
        .collect()
}

/// Apps whose program file is named differently from the app.
fn known_program(name: &str) -> Option<&'static str> {
    Some(match name {
        "visual studio code" => "code",
        "microsoft edge" => "msedge",
        "google chrome" => "chrome",
        "file explorer" => "explorer",
        "terminal" | "windows terminal" => "windowsterminal",
        "command prompt" => "cmd",
        "task manager" => "taskmgr",
        "word" => "winword",
        "powerpoint" => "powerpnt",
        "outlook" => "olk",
        "settings" => "systemsettings",
        _ => return None,
    })
}

fn is_shell_window(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    // SAFETY: the buffer outlives the call.
    let length = unsafe { GetClassNameW(hwnd, &mut class) };
    let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
    matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// The window the user is working in, if it is an ordinary window.
fn front_window() -> Option<HWND> {
    // SAFETY: plain queries.
    unsafe {
        let hwnd = GetForegroundWindow();
        (!hwnd.is_invalid() && !is_shell_window(hwnd)).then_some(hwnd)
    }
}

fn focus(hwnd: HWND) {
    // SAFETY: plain window calls. The empty mouse input makes this process
    // the last to send input, which Windows requires before it lets a
    // background app bring a window to the front.
    unsafe {
        let nudge = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: islet_input::EXTRA_INFO },
            },
        };
        SendInput(&[nudge], std::mem::size_of::<INPUT>() as i32);
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick<'a>(query: &str, names: &[&'a str]) -> Option<&'a str> {
        best_match(&alias(query), names.iter().copied()).map(|index| names[index])
    }

    #[test]
    fn spoken_names_find_the_right_app() {
        let names = [
            "Google Chrome", "Chrome Remote Desktop", "Spotify", "Visual Studio Code",
            "Uninstall Spotify", "Notepad", "File Explorer", "WhatsApp", "Microsoft Edge",
            "Calculator", "Microsoft Store",
        ];
        assert_eq!(pick("chrome", &names), Some("Google Chrome"));
        assert_eq!(pick("spotify", &names), Some("Spotify"));
        assert_eq!(pick("spotfy", &names), Some("Spotify"));
        assert_eq!(pick("vs code", &names), Some("Visual Studio Code"));
        assert_eq!(pick("code", &names), Some("Visual Studio Code"));
        assert_eq!(pick("files", &names), Some("File Explorer"));
        assert_eq!(pick("whats app", &names), Some("WhatsApp"));
        assert_eq!(pick("edge", &names), Some("Microsoft Edge"));
        assert_eq!(pick("calc", &names), Some("Calculator"));
        assert_eq!(pick("store", &names), Some("Microsoft Store"));
        assert_eq!(pick("photoshop", &names), None);
    }

    /// Lists this PC's apps and resolves a few names. Run by hand:
    /// `cargo test -p islet-app real_apps -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_apps() {
        let apps = list_apps();
        println!("{} apps", apps.len());
        for name in ["spotify", "chrome", "calculator"] {
            let app = best_match(&alias(name), apps.iter().map(|app| app.name.as_str())).map(|i| &apps[i]);
            let icon = app.and_then(|app| app_icon(&app.target));
            println!("{name} icon: {:?}", icon.map(|icon| (icon.width, icon.height, icon.pixels.chunks_exact(4).filter(|p| p[3] > 0).count())));
        }
        for name in ["notepad", "calculator", "chrome", "spotify", "vs code", "files", "settings", "edge", "whatsapp", "terminal"] {
            let found = best_match(&alias(name), apps.iter().map(|app| app.name.as_str())).map(|i| &apps[i]);
            println!("{name:>10} -> {:?}", found.map(|app| (&app.name, &app.exe)));
        }
        for window in top_windows().iter().filter(|w| w.exe == "explorer.exe" || w.exe == "spotify.exe") {
            // SAFETY: test-only; COM is set up on this thread for the call.
            let found = unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                search_box(window.hwnd, false)
            };
            println!("search box in {}: {found:?}", window.exe);
        }
        let open: Vec<String> = top_windows().iter().map(|w| w.exe.clone()).collect();
        println!("open windows by program: {open:?}");
        for name in ["spotify", "firefox", "files", "task manager"] {
            let app = best_match(&alias(name), apps.iter().map(|app| app.name.as_str())).map(|i| apps[i].clone());
            println!("{name:>12} has {} window(s)", app_windows(name, app.as_ref()).len());
        }
    }

    #[test]
    fn icon_alpha_ends_up_premultiplied() {
        let mut straight = vec![200, 100, 50, 128];
        to_premultiplied(&mut straight);
        assert_eq!(straight, [100, 50, 25, 128]);
        let mut already = vec![60, 40, 20, 128];
        to_premultiplied(&mut already);
        assert_eq!(already, [60, 40, 20, 128]);
        let mut opaque = vec![10, 20, 30, 0];
        to_premultiplied(&mut opaque);
        assert_eq!(opaque, [10, 20, 30, 255]);
    }

    #[test]
    fn apps_with_their_own_search() {
        assert_eq!(app_search_url("spotify", "lo fi beats").as_deref(), Some("spotify:search:lo%20fi%20beats"));
        assert_eq!(app_search_url("store", "whatsapp").as_deref(), Some("ms-windows-store://search/?query=whatsapp"));
        assert_eq!(app_search_url("chrome", "rust").as_deref(), Some("https://www.google.com/search?q=rust"));
        assert_eq!(app_search_url("notepad", "x"), None);
        assert!(is_browser("Google Chrome") && is_browser("Microsoft Edge") && is_browser("Firefox"));
        assert!(!is_browser("Spotify") && !is_browser("Microsoft Store"));
        assert!(is_search_label("what do you want to play?"));
        assert!(is_search_label(" search .rustup textbox "));
        assert!(!is_search_label("address and search bar view_1012 ask google or type a url"));
        assert!(!is_search_label("name system.itemnamedisplay"));
    }

    #[test]
    fn websites_and_searches() {
        assert_eq!(site_url("youtube.com").as_deref(), Some("https://youtube.com"));
        // Never a local file, whatever the name starts with.
        assert_eq!(site_url("httpsetup.bat").as_deref(), Some("https://httpsetup.bat"));
        assert_eq!(site_url("http://example.com").as_deref(), Some("https://example.com"));
        assert_eq!(site_url("YouTube").as_deref(), Some("https://www.youtube.com"));
        assert_eq!(site_url("g mail").as_deref(), Some("https://mail.google.com"));
        assert_eq!(site_url("my notes"), None);
        assert_eq!(search_url("lo fi music", Some("youtube")), "https://www.youtube.com/results?search_query=lo+fi+music");
        assert_eq!(search_url("rust & c", None), "https://www.google.com/search?q=rust+%26+c");
    }
}
