//! Spotify through its Web API: connect once, then search and play from
//! the island.
//!
//! Connecting uses Spotify's own sign-in page (the "authorization code with
//! PKCE" flow, which needs no secret): the browser comes back to a one-time
//! listener on 127.0.0.1, and the key it hands over (the refresh token) is
//! kept encrypted for this Windows user (DPAPI). Tokens never reach the log.
//!
//! Only Premium accounts can start songs from another app. Onyx asks for
//! the fewest permissions that playing needs (control playback, and see
//! the devices to play on), not the account's details: it offers search
//! once connected, and if Spotify turns a song down for want of Premium,
//! remembers the account is free and offers it no more.
//!
//! One thread does the talking. It waits for work, costing nothing, and lets
//! its connection to Spotify go after a minute without any.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use islet_core::media::ArtImage;
use islet_core::spotify::SearchResult;
use sha2::Digest;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL, HWND, LPARAM, WPARAM};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL};

/// Posted to the island when there is news (see [`Spotify::take`]).
pub const WM_APP_SPOTIFY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 36;

/// Onyx's app on Spotify's developer dashboard (not a secret: PKCE needs none).
const CLIENT_ID: &str = "ac0754754a50491f9be1191345d8cc3c";
const PORT: u16 = 43210;
const REDIRECT: &str = "http://127.0.0.1:43210/callback";
/// Starting a song, and finding a Spotify app to start it on. Nothing else.
const SCOPES: &str = "user-read-playback-state user-modify-playback-state";
const API: &str = "https://api.spotify.com/v1";
const ACCOUNTS: &str = "https://accounts.spotify.com";
/// How long the sign-in page may take.
const SIGN_IN_WAIT: Duration = Duration::from_secs(180);
/// The connection to Spotify is let go after this long without work.
const IDLE: Duration = Duration::from_secs(60);
/// Results asked for (they fill the search view).
const RESULTS: usize = islet_core::spotify::ROWS;

/// What Onyx knows of the Spotify account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Account {
    /// Not looked at yet this session.
    #[default]
    Unknown,
    Disconnected,
    /// The sign-in page is open.
    Connecting,
    Free,
    Premium,
}

/// News for the island.
#[derive(Clone, Debug, PartialEq)]
pub enum News {
    /// Signed in (or the sign-in failed: `None`, with why).
    Connected { premium: bool },
    ConnectFailed(&'static str),
    Results { query: String, results: Vec<SearchResult> },
    SearchFailed { query: String },
    /// A result's cover arrived.
    Art { uri: String, art: ArtImage },
    Played { uri: String },
    PlayFailed { uri: String, why: &'static str },
}

enum Job {
    Check,
    Connect,
    Disconnect,
    Search(String),
    Play(String),
    Shutdown,
}

#[derive(Default)]
struct Shared {
    account: Account,
    news: Vec<News>,
    /// The sign-in page being waited on, to open again on a second click.
    sign_in_page: Option<String>,
    /// Onyx is closing: a sign-in being waited on gives up.
    stopping: bool,
}

pub struct Spotify {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Job>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Spotify {
    /// Starts the worker; it looks at the saved account straight away.
    pub fn start(island: HWND, dir: PathBuf) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = unbounded();
        let worker_shared = shared.clone();
        let island = island.0 as isize;
        let thread = std::thread::Builder::new()
            .name("spotify".to_owned())
            .spawn(move || run(&rx, &worker_shared, island, &dir))
            .ok();
        let _ = tx.send(Job::Check);
        Self { shared, tx, thread }
    }

    pub fn account(&self) -> Account {
        self.shared.lock().map(|shared| shared.account).unwrap_or_default()
    }

    /// Opens Spotify's sign-in page in the browser; while one is already
    /// waited on (closed, or it showed an error), opens that one again.
    pub fn connect(&self) {
        let again = self.shared.lock().ok().and_then(|mut shared| {
            let page = shared.sign_in_page.clone().filter(|_| shared.account == Account::Connecting);
            shared.account = Account::Connecting;
            page
        });
        match again {
            Some(page) => {
                shell_open(&page, SW_SHOWNORMAL.0);
            }
            None => {
                let _ = self.tx.send(Job::Connect);
            }
        }
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(Job::Disconnect);
    }

    pub fn search(&self, query: &str) {
        let _ = self.tx.send(Job::Search(query.to_owned()));
    }

    pub fn play(&self, uri: &str) {
        let _ = self.tx.send(Job::Play(uri.to_owned()));
    }

    /// What happened since last asked.
    pub fn take(&self) -> Vec<News> {
        self.shared.lock().map(|mut shared| std::mem::take(&mut shared.news)).unwrap_or_default()
    }

    /// A key is saved from an earlier sign-in.
    pub fn saved(dir: &std::path::Path) -> bool {
        key_path(dir).exists()
    }
}

impl Drop for Spotify {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.stopping = true;
        }
        let _ = self.tx.send(Job::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why a request failed.
#[derive(Debug, PartialEq)]
enum Fail {
    /// No network, or Spotify is down.
    Offline,
    /// The saved key no longer works (revoked, or expired after months).
    SignedOut,
    /// Premium only.
    NotPremium,
    NoDevice,
    Other,
}

struct Session {
    dir: PathBuf,
    refresh: Option<String>,
    access: Option<(String, Instant)>,
    agent: Option<ureq::Agent>,
}

impl Session {
    fn agent(&mut self) -> ureq::Agent {
        self.agent
            .get_or_insert_with(|| {
                ureq::AgentBuilder::new()
                    .user_agent(concat!("Onyx/", env!("CARGO_PKG_VERSION")))
                    .timeout_connect(Duration::from_secs(6))
                    .timeout_read(Duration::from_secs(10))
                    .build()
            })
            .clone()
    }

    /// A working access token, refreshed when it is about to run out.
    fn token(&mut self) -> Result<String, Fail> {
        if let Some((token, until)) = &self.access {
            if Instant::now() + Duration::from_secs(30) < *until {
                return Ok(token.clone());
            }
        }
        let refresh = self.refresh.clone().ok_or(Fail::SignedOut)?;
        let response = self
            .agent()
            .post(&format!("{ACCOUNTS}/api/token"))
            .send_form(&[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", CLIENT_ID)]);
        let body: serde_json::Value = match response {
            Ok(response) => response.into_json().map_err(|_| Fail::Other)?,
            Err(ureq::Error::Status(400 | 401, _)) => {
                self.forget();
                return Err(Fail::SignedOut);
            }
            Err(ureq::Error::Status(_, _)) => return Err(Fail::Other),
            Err(_) => return Err(Fail::Offline),
        };
        self.take_tokens(&body)
    }

    /// Keeps the tokens of a token response (a new refresh token replaces
    /// the saved one; without one, the old one stays good).
    fn take_tokens(&mut self, body: &serde_json::Value) -> Result<String, Fail> {
        let access = body["access_token"].as_str().ok_or(Fail::Other)?.to_owned();
        let lasts = body["expires_in"].as_u64().unwrap_or(3600);
        if let Some(refresh) = body["refresh_token"].as_str() {
            if self.refresh.as_deref() != Some(refresh) {
                self.refresh = Some(refresh.to_owned());
                if save_key(&self.dir, refresh).is_err() {
                    tracing::warn!("could not save the Spotify sign-in");
                }
            }
        }
        self.access = Some((access.clone(), Instant::now() + Duration::from_secs(lasts)));
        Ok(access)
    }

    fn forget(&mut self) {
        self.refresh = None;
        self.access = None;
        let _ = std::fs::remove_file(key_path(&self.dir));
    }

    /// A Web API call, signed; once more with a fresh token if Spotify
    /// says the token is no good.
    fn call(&mut self, method: &str, path: &str, body: Option<&serde_json::Value>) -> Result<(u16, serde_json::Value), Fail> {
        for attempt in 0..2 {
            let token = self.token()?;
            let request = self.agent().request(method, &format!("{API}{path}")).set("Authorization", &format!("Bearer {token}"));
            let response = match body {
                Some(body) => request.send_json(body.clone()),
                None if method == "PUT" => request.set("Content-Length", "0").call(),
                None => request.call(),
            };
            match response {
                Ok(response) => {
                    let status = response.status();
                    let json = response.into_json().unwrap_or(serde_json::Value::Null);
                    return Ok((status, json));
                }
                Err(ureq::Error::Status(401, _)) if attempt == 0 => self.access = None,
                Err(ureq::Error::Status(status, response)) => {
                    let json = response.into_json().unwrap_or(serde_json::Value::Null);
                    return Ok((status, json));
                }
                Err(_) => return Err(Fail::Offline),
            }
        }
        Err(Fail::SignedOut)
    }
}

fn run(rx: &Receiver<Job>, shared: &Arc<Mutex<Shared>>, island: isize, dir: &std::path::Path) {
    // SAFETY: this thread's own apartment, for decoding covers.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let mut session = Session { dir: dir.to_owned(), refresh: load_key(dir), access: None, agent: None };
    let mut covers: HashMap<String, ArtImage> = HashMap::new();
    let tell = |news: News| {
        if let Ok(mut shared) = shared.lock() {
            shared.news.push(news);
        }
        post(island);
    };
    let set_account = |account: Account| {
        if let Ok(mut shared) = shared.lock() {
            shared.account = account;
        }
        write_status(dir, account);
        post(island);
    };
    let mut waiting: std::collections::VecDeque<Job> = std::collections::VecDeque::new();
    loop {
        let job = match waiting.pop_front().map_or_else(|| rx.recv_timeout(IDLE), Ok) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => {
                // Idle: let the connection (and its TLS state) go.
                session.agent = None;
                session.access = None;
                if covers.len() > 60 {
                    covers.clear();
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Typing sends a search per pause: only the newest matters. Other
        // work queued behind it waits its turn.
        let job = match job {
            Job::Search(mut query) => {
                while let Ok(next) = rx.try_recv() {
                    match next {
                        Job::Search(newer) => query = newer,
                        other => waiting.push_back(other),
                    }
                }
                Job::Search(query)
            }
            other => other,
        };
        match job {
            Job::Shutdown => break,
            Job::Check => match session.refresh.is_some() {
                false => set_account(Account::Disconnected),
                true => match account_kind(&mut session) {
                    Ok(premium) => set_account(if premium { Account::Premium } else { Account::Free }),
                    Err(Fail::SignedOut) => set_account(Account::Disconnected),
                    // Offline: look again on the next sign of Spotify.
                    Err(_) => set_account(Account::Unknown),
                },
            },
            Job::Connect => match {
                let _ = std::fs::remove_file(dir.join("spotify-status.txt"));
                sign_in(
                    &mut session,
                    &|page| {
                        if let Ok(mut shared) = shared.lock() {
                            shared.sign_in_page = page;
                        }
                    },
                    &|| shared.lock().map_or(true, |shared| shared.stopping),
                )
            } {
                Ok(()) => match account_kind(&mut session) {
                    Ok(premium) => {
                        set_account(if premium { Account::Premium } else { Account::Free });
                        tell(News::Connected { premium });
                    }
                    Err(_) => {
                        set_account(Account::Unknown);
                        tell(News::ConnectFailed("Couldn't reach Spotify"));
                    }
                },
                Err(why) => {
                    set_account(if session.refresh.is_some() { Account::Unknown } else { Account::Disconnected });
                    tell(News::ConnectFailed(why));
                }
            },
            Job::Disconnect => {
                session.forget();
                set_account(Account::Disconnected);
            }
            Job::Search(query) => match search(&mut session, &query) {
                Ok(found) => {
                    let urls: Vec<(String, Option<String>)> =
                        found.iter().map(|(result, url)| (result.uri.clone(), url.clone())).collect();
                    let results: Vec<SearchResult> = found
                        .into_iter()
                        .map(|(mut result, url)| {
                            result.art = url.and_then(|url| covers.get(&url).cloned());
                            result
                        })
                        .collect();
                    tell(News::Results { query: query.clone(), results });
                    for (uri, url) in urls {
                        let Some(url) = url.filter(|url| !covers.contains_key(url)) else { continue };
                        // A newer search waiting: its covers matter more.
                        if !rx.is_empty() {
                            break;
                        }
                        if let Some(art) = fetch_cover(&mut session, &url) {
                            covers.insert(url, art.clone());
                            tell(News::Art { uri, art });
                        }
                    }
                }
                Err(Fail::SignedOut) => {
                    set_account(Account::Disconnected);
                    tell(News::SearchFailed { query });
                }
                Err(_) => tell(News::SearchFailed { query }),
            },
            Job::Play(uri) => match play(&mut session, &uri) {
                Ok(()) => tell(News::Played { uri }),
                Err(fail) => {
                    if fail == Fail::SignedOut {
                        set_account(Account::Disconnected);
                    }
                    if fail == Fail::NotPremium {
                        // Remembered (in the status file), so search is
                        // not offered again.
                        set_account(Account::Free);
                    }
                    let why = match fail {
                        Fail::NotPremium => "Playing from Onyx needs Spotify Premium",
                        Fail::NoDevice => "Open Spotify on this PC or your phone, then try again",
                        Fail::Offline => "Spotify can't be reached right now",
                        Fail::SignedOut => "Connect Spotify again",
                        Fail::Other => "Spotify didn't start it",
                    };
                    tell(News::PlayFailed { uri, why });
                }
            },
        }
    }
    if com {
        // SAFETY: balances CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
}

fn post(island: isize) {
    // SAFETY: a non-blocking post to the island window.
    unsafe {
        let _ = PostMessageW(Some(HWND(island as *mut _)), WM_APP_SPOTIFY, WPARAM(0), LPARAM(0));
    }
}

/// Whether the sign-in works, and the account is not known to be free
/// (a song turned down for want of Premium marks it so). Asks for the
/// devices (a permission Onyx has anyway), never the profile.
fn account_kind(session: &mut Session) -> Result<bool, Fail> {
    match session.call("GET", "/me/player/devices", None)? {
        (200, _) => Ok(!known_free(&session.dir)),
        (401, _) => Err(Fail::SignedOut),
        // Not on the app's list of users (development mode).
        (403, _) => Err(Fail::SignedOut),
        _ => Err(Fail::Other),
    }
}

/// Songs for `query`, each with its smallest cover that is still sharp.
fn search(session: &mut Session, query: &str) -> Result<Vec<(SearchResult, Option<String>)>, Fail> {
    let path = format!("/search?type=track&limit={RESULTS}&q={}", encode(query));
    let (status, body) = session.call("GET", &path, None)?;
    if status != 200 {
        return Err(if status == 401 { Fail::SignedOut } else { Fail::Other });
    }
    Ok(parse_tracks(&body))
}

fn parse_tracks(body: &serde_json::Value) -> Vec<(SearchResult, Option<String>)> {
    let Some(items) = body["tracks"]["items"].as_array() else { return Vec::new() };
    items
        .iter()
        .filter_map(|track| {
            let uri = track["uri"].as_str()?.to_owned();
            let title = track["name"].as_str()?.to_owned();
            let artist = track["artists"]
                .as_array()
                .map(|artists| artists.iter().filter_map(|artist| artist["name"].as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            // Images come largest first; the last at least 64 px wide.
            let cover = track["album"]["images"].as_array().and_then(|images| {
                images
                    .iter()
                    .rev()
                    .find(|image| image["width"].as_u64().is_none_or(|width| width >= 64))
                    .and_then(|image| image["url"].as_str())
                    .map(str::to_owned)
            });
            Some((
                SearchResult {
                    uri,
                    title,
                    artist,
                    duration_ms: track["duration_ms"].as_u64().unwrap_or(0) as u32,
                    explicit: track["explicit"].as_bool().unwrap_or(false),
                    art: None,
                },
                cover,
            ))
        })
        .collect()
}

fn fetch_cover(session: &mut Session, url: &str) -> Option<ArtImage> {
    // Only Spotify's own image host.
    if !url.starts_with("https://i.scdn.co/") {
        return None;
    }
    let response = session.agent().get(url).call().ok()?;
    let mut bytes = Vec::new();
    response.into_reader().take(2_000_000).read_to_end(&mut bytes).ok()?;
    islet_media::decode_image(&bytes)
}

/// Starts `uri` on the device playing, else on this PC's Spotify (started,
/// minimized, if it is not running), else on any device of the account.
fn play(session: &mut Session, uri: &str) -> Result<(), Fail> {
    let body = serde_json::json!({ "uris": [uri] });
    match session.call("PUT", "/me/player/play", Some(&body))?.0 {
        200..=299 => return Ok(()),
        401 => return Err(Fail::SignedOut),
        403 => return Err(Fail::NotPremium),
        404 => {}
        _ => return Err(Fail::Other),
    }
    // Nothing is playing anywhere: pick a device.
    let mut device = pick_device(session)?;
    if device.is_none() {
        // Spotify on this PC, kept out of the way.
        shell_open("spotify:", SW_SHOWMINNOACTIVE.0);
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(800));
            device = pick_device(session)?;
            if device.is_some() {
                break;
            }
        }
    }
    let device = device.ok_or(Fail::NoDevice)?;
    match session.call("PUT", &format!("/me/player/play?device_id={}", encode(&device)), Some(&body))?.0 {
        200..=299 => Ok(()),
        403 => Err(Fail::NotPremium),
        404 => Err(Fail::NoDevice),
        _ => Err(Fail::Other),
    }
}

/// This PC's Spotify first, then any device.
fn pick_device(session: &mut Session) -> Result<Option<String>, Fail> {
    let (status, body) = session.call("GET", "/me/player/devices", None)?;
    if status != 200 {
        return Err(Fail::Other);
    }
    let devices = body["devices"].as_array().cloned().unwrap_or_default();
    let usable = |device: &&serde_json::Value| !device["is_restricted"].as_bool().unwrap_or(false);
    let computer = devices.iter().filter(usable).find(|device| device["type"].as_str() == Some("Computer"));
    Ok(computer
        .or_else(|| devices.iter().find(usable))
        .and_then(|device| device["id"].as_str())
        .map(str::to_owned))
}

/// Spotify's sign-in page in the browser, and the answer it sends back to
/// a listener on this PC; the code it carries is swapped for tokens.
fn sign_in(
    session: &mut Session,
    waiting_on: &dyn Fn(Option<String>),
    stopping: &dyn Fn() -> bool,
) -> Result<(), &'static str> {
    let result = sign_in_with(session, waiting_on, stopping);
    waiting_on(None);
    result
}

fn sign_in_with(
    session: &mut Session,
    waiting_on: &dyn Fn(Option<String>),
    stopping: &dyn Fn() -> bool,
) -> Result<(), &'static str> {
    // Before the port opens: no secrets, no sign-in.
    let (Some(verifier), Some(state)) = (random_text(64), random_text(16)) else {
        return Err("Couldn't start sign-in");
    };
    let listener = TcpListener::bind(("127.0.0.1", PORT)).map_err(|_| "Another app is using the sign-in port")?;
    listener.set_nonblocking(true).map_err(|_| "Couldn't start sign-in")?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let url = format!(
        "{ACCOUNTS}/authorize?client_id={CLIENT_ID}&response_type=code&redirect_uri={}&code_challenge_method=S256&code_challenge={challenge}&state={state}&scope={}",
        encode(REDIRECT),
        encode(SCOPES)
    );
    if !shell_open(&url, SW_SHOWNORMAL.0) {
        return Err("Couldn't open the browser");
    }
    waiting_on(Some(url.clone()));
    let deadline = Instant::now() + SIGN_IN_WAIT;
    let code = loop {
        if Instant::now() > deadline {
            return Err("Sign-in timed out");
        }
        if stopping() {
            return Err("Sign-in was cancelled");
        }
        let Ok((mut stream, _)) = listener.accept() else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut request = Vec::new();
        let mut chunk = [0u8; 2048];
        // A slow sender gets 5 s in all, not 5 s per read.
        let read_until = Instant::now() + Duration::from_secs(5);
        while request.len() < 16_384 && Instant::now() < read_until && !request.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => request.extend_from_slice(&chunk[..read]),
            }
        }
        let request = String::from_utf8_lossy(&request);
        let target = request.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("");
        let Some(query) = target.strip_prefix("/callback?") else {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        };
        let params = parse_query(query);
        if params.get("state") != Some(&state) {
            // Not this sign-in's answer (a stale tab, or a page poking the
            // port): it does not end the wait.
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            continue;
        }
        let ok = params.contains_key("code");
        let page = if ok {
            "Onyx is connected to Spotify. You can close this tab."
        } else {
            "Onyx wasn't connected to Spotify. You can close this tab."
        };
        let html = format!(
            "<!doctype html><meta charset=utf-8><title>Onyx</title><body style=\"font:16px system-ui;background:#0b0b0d;color:#f4f5fa;display:grid;place-items:center;height:90vh\"><p>{page}</p>"
        );
        let _ = stream.write_all(
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len())
                .as_bytes(),
        );
        match params.get("code") {
            Some(code) => break code.clone(),
            None => return Err("Sign-in was cancelled"),
        }
    };
    drop(listener);
    let response = session.agent().post(&format!("{ACCOUNTS}/api/token")).send_form(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", REDIRECT),
        ("client_id", CLIENT_ID),
        ("code_verifier", &verifier),
    ]);
    let body: serde_json::Value = match response {
        Ok(response) => response.into_json().map_err(|_| "Spotify's answer was unreadable")?,
        Err(ureq::Error::Status(_, _)) => return Err("Spotify refused the sign-in"),
        Err(_) => return Err("Couldn't reach Spotify"),
    };
    session.take_tokens(&body).map(|_| ()).map_err(|_| "Spotify's answer was unreadable")
}

fn shell_open(target: &str, show: i32) -> bool {
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: plain ShellExecute of a URL or URI.
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD(show),
        )
    };
    result.0 as usize > 32
}

/// Unguessable text from the OS's random numbers; `None` if they are not
/// available (there is no safe stand-in).
fn random_text(bytes: usize) -> Option<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer))
}

/// Percent-encoding for a URL's query.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((decode(key), decode(value)))
        })
        .collect()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |byte: u8| char::from(byte).to_digit(16).map(|digit| digit as u8);
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high << 4 | low);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn key_path(dir: &std::path::Path) -> PathBuf {
    dir.join("spotify.key")
}

/// The refresh token, encrypted for this Windows user.
fn save_key(dir: &std::path::Path, key: &str) -> std::io::Result<()> {
    let sealed = protect(key.as_bytes(), true).ok_or_else(|| std::io::Error::other("DPAPI"))?;
    let path = key_path(dir);
    let temporary = path.with_extension("key.tmp");
    std::fs::write(&temporary, sealed)?;
    std::fs::rename(&temporary, &path)
}

fn load_key(dir: &std::path::Path) -> Option<String> {
    let sealed = std::fs::read(key_path(dir)).ok()?;
    String::from_utf8(protect(&sealed, false)?).ok()
}

/// DPAPI: seal (`true`) or open data for the signed-in Windows user.
fn protect(data: &[u8], seal: bool) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr().cast_mut() };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the output buffer is LocalAlloc'd by Windows and freed here.
    unsafe {
        let done = if seal {
            CryptProtectData(&input, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        done.ok()?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Some(bytes)
    }
}

/// A song was turned down for want of Premium on this sign-in.
fn known_free(dir: &std::path::Path) -> bool {
    std::fs::read_to_string(dir.join("spotify-status.txt")).is_ok_and(|status| status.trim() == "free")
}

/// For Settings, which cannot read the key: connected, and Premium or not.
fn write_status(dir: &std::path::Path, account: Account) {
    let status = match account {
        Account::Premium => "premium",
        Account::Free => "free",
        Account::Disconnected => "disconnected",
        Account::Connecting | Account::Unknown => return,
    };
    let _ = std::fs::write(dir.join("spotify-status.txt"), status);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_encoded_and_decoded() {
        assert_eq!(encode("lo fi & beats"), "lo%20fi%20%26%20beats");
        let params = parse_query("code=AQB%2Bx-y_z&state=abc&error=access_denied");
        assert_eq!(params["code"], "AQB+x-y_z");
        assert_eq!(params["state"], "abc");
        assert_eq!(decode("a+b%2"), "a b%2");
    }

    #[test]
    fn search_results_come_out_with_their_smallest_sharp_cover() {
        let body = serde_json::json!({ "tracks": { "items": [{
            "uri": "spotify:track:1", "name": "Somewhere Only We Know", "duration_ms": 237000, "explicit": false,
            "artists": [{ "name": "Keane" }],
            "album": { "images": [
                { "url": "https://i.scdn.co/640", "width": 640 },
                { "url": "https://i.scdn.co/300", "width": 300 },
                { "url": "https://i.scdn.co/64", "width": 64 }
            ] }
        }, { "name": "no uri" }] } });
        let found = parse_tracks(&body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.artist, "Keane");
        assert_eq!(found[0].1.as_deref(), Some("https://i.scdn.co/64"));
    }

    #[test]
    fn the_key_round_trips_through_dpapi() {
        let sealed = protect(b"refresh-token", true).unwrap();
        assert_ne!(sealed, b"refresh-token");
        assert_eq!(protect(&sealed, false).unwrap(), b"refresh-token");
    }

    #[test]
    fn random_text_is_long_and_varies() {
        let (a, b) = (random_text(64).unwrap(), random_text(64).unwrap());
        assert!(a.len() >= 43 && a != b);
    }
}
