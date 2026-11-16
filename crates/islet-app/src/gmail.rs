//! One-time codes from Gmail: connect once, and a code that arrives shows on
//! the island.
//!
//! Connecting uses Google's own sign-in page in the browser (authorization
//! code with PKCE, back to a one-time listener on 127.0.0.1) and asks for
//! read-only access to mail, nothing else. The key it hands over (the refresh
//! token) is kept encrypted for this Windows user (DPAPI).
//!
//! The Google sign-in app is Onyx's own, baked in at build time (see
//! `build.rs`): users only press Connect and allow it. A development build
//! without one can use a client file in Onyx's settings folder instead.
//!
//! Checking is cheap: Gmail is asked what changed since the last look, which
//! is a few hundred bytes when nothing did, over one kept connection. It asks
//! every 15 s while the PC is in use, every 3 minutes once it is left alone,
//! and not at all on Battery Saver. Only new messages in the inbox from the
//! last 10 minutes are read, and only for a code: mail, codes and tokens never
//! reach the log or the disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use sha2::Digest;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWNORMAL};

use crate::spotify::{encode, protect, random_text, shell_open, wait_for_code};

/// Posted to the island when there is news (see [`Gmail::take`]).
pub const WM_APP_GMAIL: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 37;

const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// Reading mail; no sending, changing or deleting.
const SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
const SIGN_IN_WAIT: Duration = Duration::from_secs(180);
/// How often Gmail is asked while the PC is in use, and once it is left.
const ACTIVE_EVERY: Duration = Duration::from_secs(15);
const IDLE_EVERY: Duration = Duration::from_secs(180);
/// No keyboard or mouse for this long: the PC is left alone.
const IDLE_AFTER: Duration = Duration::from_secs(120);
/// Older mail is not a code still wanted.
const FRESH: Duration = Duration::from_secs(600);
/// Messages read per look at most (a burst of mail is not all codes).
const PER_LOOK: usize = 5;

/// Onyx's Google sign-in app, from the build ("" when it has none).
const BUILT_IN: &str = include_str!(concat!(env!("OUT_DIR"), "/google-client.json"));
/// A development stand-in for it, in the settings folder.
const CLIENT_FILE: &str = "google-client.json";
const KEY_FILE: &str = "gmail.key";
const STATUS_FILE: &str = "gmail-status.txt";

/// What Settings shows (it cannot read the key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// This build has no Google sign-in app.
    NoClient,
    Disconnected,
    Connecting,
    Connected,
}

pub enum News {
    Code { code: String, from: String },
    Connected,
    ConnectFailed(&'static str),
    SignedOut,
    /// Google ended the sign-in (revoked, or a Testing app's week is up).
    Expired,
}

enum Job {
    Connect,
    Disconnect,
    Shutdown,
}

#[derive(Default)]
struct Shared {
    news: Vec<News>,
    stopping: bool,
}

pub struct Gmail {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Job>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Gmail {
    pub fn start(island: HWND, dir: PathBuf) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = unbounded();
        let worker_shared = shared.clone();
        let island = island.0 as isize;
        let thread = std::thread::Builder::new()
            .name("gmail".to_owned())
            .spawn(move || run(&rx, &worker_shared, island, &dir))
            .ok();
        Self { shared, tx, thread }
    }

    pub fn connect(&self) {
        let _ = self.tx.send(Job::Connect);
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(Job::Disconnect);
    }

    pub fn take(&self) -> Vec<News> {
        self.shared.lock().map(|mut shared| std::mem::take(&mut shared.news)).unwrap_or_default()
    }

    /// A key is saved from an earlier sign-in.
    pub fn saved(dir: &Path) -> bool {
        dir.join(KEY_FILE).exists()
    }
}

impl Drop for Gmail {
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

/// Onyx's Google sign-in app.
struct Client {
    id: String,
    secret: String,
}

impl Client {
    /// The one built in, or else a development stand-in.
    fn load(dir: &Path) -> Option<Self> {
        Self::parse(BUILT_IN).or_else(|| Self::parse(&std::fs::read_to_string(dir.join(CLIENT_FILE)).ok()?))
    }

    fn parse(text: &str) -> Option<Self> {
        let json: serde_json::Value = serde_json::from_str(text).ok()?;
        let app = json.get("installed").unwrap_or(&json);
        let id = app["client_id"].as_str()?.trim().to_owned();
        let secret = app["client_secret"].as_str().unwrap_or("").trim().to_owned();
        id.ends_with(".apps.googleusercontent.com").then_some(Self { id, secret })
    }
}

struct Session {
    dir: PathBuf,
    refresh: Option<String>,
    access: Option<(String, Instant)>,
    agent: Option<ureq::Agent>,
    /// Gmail's change counter as of the last look.
    history: Option<String>,
}

/// Why a request failed.
#[derive(Debug, PartialEq)]
enum Fail {
    Offline,
    SignedOut,
    /// The change counter is too old: start again from now.
    Stale,
    Other,
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

    fn token(&mut self, client: &Client) -> Result<String, Fail> {
        if let Some((token, until)) = &self.access {
            if Instant::now() + Duration::from_secs(30) < *until {
                return Ok(token.clone());
            }
        }
        let refresh = self.refresh.clone().ok_or(Fail::SignedOut)?;
        let response = self.agent().post(TOKEN).send_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &client.id),
            ("client_secret", &client.secret),
        ]);
        let body: serde_json::Value = match response {
            Ok(response) => read_json(response).ok_or(Fail::Other)?,
            // Revoked, or the client was changed: sign in again.
            Err(ureq::Error::Status(400 | 401, _)) => return Err(Fail::SignedOut),
            Err(ureq::Error::Status(_, _)) => return Err(Fail::Other),
            Err(_) => return Err(Fail::Offline),
        };
        self.take_tokens(&body)
    }

    fn take_tokens(&mut self, body: &serde_json::Value) -> Result<String, Fail> {
        let access = body["access_token"].as_str().ok_or(Fail::Other)?.to_owned();
        let lasts = body["expires_in"].as_u64().unwrap_or(3600);
        self.access = Some((access.clone(), Instant::now() + Duration::from_secs(lasts)));
        if let Some(refresh) = body["refresh_token"].as_str() {
            self.refresh = Some(refresh.to_owned());
            let _ = save_key(&self.dir, refresh);
        }
        Ok(access)
    }

    fn get(&mut self, client: &Client, url: &str) -> Result<serde_json::Value, Fail> {
        let token = self.token(client)?;
        match self.agent().get(url).set("Authorization", &format!("Bearer {token}")).call() {
            Ok(response) => read_json(response).ok_or(Fail::Other),
            Err(ureq::Error::Status(401, _)) => {
                self.access = None;
                Err(Fail::Other)
            }
            Err(ureq::Error::Status(404, _)) => Err(Fail::Stale),
            Err(ureq::Error::Status(_, _)) => Err(Fail::Other),
            Err(_) => Err(Fail::Offline),
        }
    }

    /// New codes since the last look.
    fn look(&mut self, client: &Client) -> Result<Vec<(String, String)>, Fail> {
        let Some(since) = self.history.clone() else {
            // The first look only learns where "now" is.
            let profile = self.get(client, &format!("{API}/profile?fields=historyId"))?;
            self.history = profile["historyId"].as_str().map(str::to_owned);
            return Ok(Vec::new());
        };
        let url = format!(
            "{API}/history?startHistoryId={since}&historyTypes=messageAdded&labelId=INBOX&fields=historyId,history(messagesAdded(message(id)))"
        );
        let changes = match self.get(client, &url) {
            Err(Fail::Stale) => {
                self.history = None;
                return Ok(Vec::new());
            }
            other => other?,
        };
        if let Some(now) = changes["historyId"].as_str() {
            self.history = Some(now.to_owned());
        }
        let ids: Vec<String> = changes["history"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|entry| entry["messagesAdded"].as_array().into_iter().flatten())
            .filter_map(|added| added["message"]["id"].as_str().map(str::to_owned))
            .collect();
        let mut codes = Vec::new();
        // The newest last in the list: read those first.
        for id in ids.iter().rev().take(PER_LOOK) {
            let url = format!("{API}/messages/{}?format=full", encode(id));
            let Ok(message) = self.get(client, &url) else { continue };
            if let Some(found) = code_of(&message) {
                codes.push(found);
            }
        }
        codes.reverse();
        Ok(codes)
    }
}

/// A message's code and sender, if it is fresh and has one.
fn code_of(message: &serde_json::Value) -> Option<(String, String)> {
    let sent_ms: u64 = message["internalDate"].as_str()?.parse().ok()?;
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    if now_ms.saturating_sub(sent_ms) > FRESH.as_millis() as u64 {
        return None;
    }
    let payload = &message["payload"];
    let header = |name: &str| {
        payload["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|header| header["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .and_then(|header| header["value"].as_str())
            .unwrap_or("")
            .to_owned()
    };
    let subject = header("Subject");
    let snippet = message["snippet"].as_str().unwrap_or("");
    let mut text = decode_entities(snippet);
    if let Some(body) = body_text(payload) {
        text.push(' ');
        text.push_str(&body.chars().take(3_000).collect::<String>());
    }
    let code = islet_core::otp::find_code(&subject, &text)?;
    // Who sent it is the domain Gmail verified, never the display name,
    // which is anyone's to write.
    let results: Vec<&str> = payload["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|header| header["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("Authentication-Results")))
        .filter_map(|header| header["value"].as_str())
        .collect();
    let from = gmails_result(&results)
        .and_then(|result| verified_domain(result, &header("From")))
        .unwrap_or_else(|| "an unverified sender".to_owned());
    Some((code, from))
}

const GMAIL_MX: &str = "mx.google.com";

/// Gmail's own Authentication-Results: the topmost, and the only one that
/// says it is Gmail's (a sender may add their own below; more than one is not
/// trusted at all).
fn gmails_result<'a>(results: &[&'a str]) -> Option<&'a str> {
    let claims_gmail = |result: &&str| result.split(';').next().is_some_and(|id| id.trim().eq_ignore_ascii_case(GMAIL_MX));
    let first = *results.first()?;
    (claims_gmail(&first) && results.iter().filter(|result| claims_gmail(result)).count() == 1).then_some(first)
}

/// The header without its (comments), which carry text a sender chose;
/// None if they don't balance.
fn without_comments(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.checked_sub(1)?,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    (depth == 0).then_some(out)
}

/// The From domain, exactly as Gmail's DMARC check passed it: Gmail's
/// result ("mx.google.com; ... dmarc=pass (...) header.from=google.com"), for
/// the one plain address the From header gives. Anything unclear is not
/// verified: quoted text (a sender's), or other than exactly one DMARC result.
fn verified_domain(result: &str, from: &str) -> Option<String> {
    let result = without_comments(&result.to_ascii_lowercase())?;
    if result.contains('"') || result.matches("dmarc=").count() != 1 {
        return None;
    }
    let mut parts = result.split(';');
    if parts.next()?.trim() != GMAIL_MX {
        return None;
    }
    let dmarc = parts.map(str::trim).find(|part| part.starts_with("dmarc="))?;
    let mut words = dmarc.split_whitespace();
    if words.next()? != "dmarc=pass" {
        return None;
    }
    let domain = words.find_map(|word| word.strip_prefix("header.from="))?;
    let plain = |text: &str| {
        text.contains('.') && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
    };
    if !plain(domain) {
        return None;
    }
    // One address: "Name <a@b>" or "a@b", nothing more.
    let from = from.trim();
    let address = match from.find('<') {
        Some(open) if from.rfind('<') == Some(open) => from[open + 1..].strip_suffix('>')?,
        Some(_) => return None,
        None => from,
    };
    let (local, from_domain) = address.trim().split_once('@')?;
    if local.is_empty() || from_domain.contains(['@', ',', ' ']) {
        return None;
    }
    from_domain.eq_ignore_ascii_case(domain).then(|| domain.to_owned())
}

/// The plain text of a message, or its HTML with the tags taken out.
fn body_text(payload: &serde_json::Value) -> Option<String> {
    let mut plain = None;
    let mut html = None;
    let mut stack = vec![payload];
    while let Some(part) = stack.pop() {
        let kind = part["mimeType"].as_str().unwrap_or("");
        if let Some(data) = part["body"]["data"].as_str() {
            if kind == "text/plain" && plain.is_none() {
                plain = decode_part(data);
            } else if kind == "text/html" && html.is_none() {
                html = decode_part(data).map(|html| strip_html(cut(&html, HTML_LIMIT)));
            }
        }
        if let Some(parts) = part["parts"].as_array() {
            stack.extend(parts.iter().rev());
        }
    }
    plain.or(html)
}

/// A code is near the top of a mail; HTML past this is not read.
const HTML_LIMIT: usize = 64 * 1024;

/// `text` up to `limit` bytes, at a character boundary.
fn cut(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn decode_part(data: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data.trim_end_matches('=')).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Text of an HTML mail: tags, styles and scripts out, entities decoded.
///
/// One forward pass: each byte is looked at a bounded number of times.
fn strip_html(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len() / 2);
    let mut at = 0;
    while let Some(open) = html[at..].find('<').map(|offset| at + offset) {
        out.push_str(&html[at..open]);
        out.push(' ');
        // A style, script or head block goes whole, up to its closing tag.
        let block = ["style", "script", "head"].into_iter().find(|name| starts_with_ignore_case(&bytes[open + 1..], name));
        let mut from = open;
        if let Some(name) = block {
            from = find_close(bytes, open + 1 + name.len(), name).unwrap_or(bytes.len());
        }
        at = html.get(from..).and_then(|rest| rest.find('>')).map_or(html.len(), |close| from + close + 1);
    }
    out.push_str(&html[at..]);
    decode_entities(&out)
}

fn starts_with_ignore_case(bytes: &[u8], word: &str) -> bool {
    bytes.len() >= word.len() && bytes[..word.len()].eq_ignore_ascii_case(word.as_bytes())
}

/// Where `</name` starts at or after `from`.
fn find_close(bytes: &[u8], from: usize, name: &str) -> Option<usize> {
    (from..bytes.len()).find(|&at| {
        bytes[at] == b'<' && bytes.get(at + 1) == Some(&b'/') && starts_with_ignore_case(&bytes[at + 2..], name)
    })
}

fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn read_json(response: ureq::Response) -> Option<serde_json::Value> {
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(response.into_reader(), 4_000_000), &mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn post(island: isize) {
    // SAFETY: a non-blocking post to the island window.
    unsafe {
        let _ = PostMessageW(Some(HWND(island as *mut _)), WM_APP_GMAIL, WPARAM(0), LPARAM(0));
    }
}

/// How long until the next look: sooner while the PC is in use, never on
/// Battery Saver (looked at again in a minute).
fn next_look() -> Option<Duration> {
    let mut power = SYSTEM_POWER_STATUS::default();
    // SAFETY: plain reads of system state into local structs.
    unsafe {
        if GetSystemPowerStatus(&mut power).is_ok() && power.SystemStatusFlag & 1 != 0 {
            return None;
        }
        let mut input = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        let idle = if GetLastInputInfo(&mut input).as_bool() {
            Duration::from_millis(u64::from(GetTickCount().wrapping_sub(input.dwTime)))
        } else {
            Duration::ZERO
        };
        Some(if idle < IDLE_AFTER { ACTIVE_EVERY } else { IDLE_EVERY })
    }
}

fn run(rx: &Receiver<Job>, shared: &Arc<Mutex<Shared>>, island: isize, dir: &Path) {
    let mut session = Session { dir: dir.to_owned(), refresh: load_key(dir), access: None, agent: None, history: None };
    let tell = |news: News| {
        if let Ok(mut shared) = shared.lock() {
            shared.news.push(news);
        }
        post(island);
    };
    let client = Client::load(dir);
    write_status(dir, match (&client, &session.refresh) {
        (None, _) => Status::NoClient,
        (Some(_), None) => Status::Disconnected,
        (Some(_), Some(_)) => Status::Connected,
    });
    let mut client = client;
    let mut failures: u32 = 0;
    let mut wait = Duration::ZERO;
    loop {
        let job = match rx.recv_timeout(wait) {
            Ok(job) => Some(job),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match job {
            Some(Job::Shutdown) => break,
            Some(Job::Disconnect) => {
                session.refresh = None;
                session.access = None;
                session.history = None;
                let _ = std::fs::remove_file(dir.join(KEY_FILE));
                write_status(dir, if client.is_some() { Status::Disconnected } else { Status::NoClient });
                tell(News::SignedOut);
            }
            Some(Job::Connect) => {
                client = Client::load(dir);
                let Some(app) = &client else {
                    write_status(dir, Status::NoClient);
                    tell(News::ConnectFailed("Gmail sign-in isn't set up in this build of Onyx"));
                    wait = Duration::from_secs(3600);
                    continue;
                };
                write_status(dir, Status::Connecting);
                let stopping = || shared.lock().map_or(true, |shared| shared.stopping);
                match sign_in(&mut session, app, &stopping) {
                    Ok(()) => {
                        session.history = None;
                        write_status(dir, Status::Connected);
                        tell(News::Connected);
                    }
                    Err(why) => {
                        write_status(dir, if session.refresh.is_some() { Status::Connected } else { Status::Disconnected });
                        tell(News::ConnectFailed(why));
                    }
                }
            }
            None => {}
        }
        // Time for a look (or right after connecting).
        let (Some(app), true) = (&client, session.refresh.is_some()) else {
            wait = Duration::from_secs(3600);
            continue;
        };
        let Some(every) = next_look() else {
            // Battery Saver: let the connection go, and look again later.
            session.agent = None;
            wait = Duration::from_secs(60);
            continue;
        };
        match session.look(app) {
            Ok(codes) => {
                failures = 0;
                for (code, from) in codes {
                    tracing::info!("gmail: a code arrived");
                    tell(News::Code { code, from });
                }
                wait = every;
            }
            Err(Fail::SignedOut) => {
                session.refresh = None;
                let _ = std::fs::remove_file(dir.join(KEY_FILE));
                write_status(dir, Status::Disconnected);
                tell(News::Expired);
                wait = Duration::from_secs(3600);
            }
            Err(_) => {
                // Offline or Gmail is busy: back off, up to the idle pace.
                failures = failures.saturating_add(1);
                session.agent = None;
                wait = (every * 2u32.saturating_pow(failures.min(4))).min(IDLE_EVERY);
            }
        }
    }
}

fn sign_in(session: &mut Session, client: &Client, stopping: &dyn Fn() -> bool) -> Result<(), &'static str> {
    let (Some(verifier), Some(state)) = (random_text(64), random_text(16)) else {
        return Err("Couldn't start sign-in");
    };
    // Any free port: Google allows any on 127.0.0.1 for a Desktop app.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "Couldn't start sign-in")?;
    listener.set_nonblocking(true).map_err(|_| "Couldn't start sign-in")?;
    let port = listener.local_addr().map_err(|_| "Couldn't start sign-in")?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let url = format!(
        "{AUTH}?client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt=consent",
        encode(&client.id),
        encode(&redirect),
        encode(SCOPE)
    );
    if !shell_open(&url, SW_SHOWNORMAL.0) {
        return Err("Couldn't open the browser");
    }
    let code = wait_for_code(&listener, "/", &state, "Gmail", Instant::now() + SIGN_IN_WAIT, stopping)?;
    drop(listener);
    let response = session.agent().post(TOKEN).send_form(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("client_id", &client.id),
        ("client_secret", &client.secret),
        ("code_verifier", &verifier),
    ]);
    let body = match response {
        Ok(response) => read_json(response).ok_or("Google's answer was unreadable")?,
        Err(ureq::Error::Status(_, _)) => return Err("Google refused the sign-in"),
        Err(_) => return Err("Couldn't reach Google"),
    };
    if !body["scope"].as_str().is_some_and(|granted| granted.split(' ').any(|scope| scope == SCOPE)) {
        return Err("Onyx needs permission to read your mail for codes");
    }
    session.take_tokens(&body).map(|_| ()).map_err(|_| "Google's answer was unreadable")?;
    if session.refresh.is_none() {
        return Err("Google didn't give a lasting sign-in: try again");
    }
    Ok(())
}

fn save_key(dir: &Path, key: &str) -> std::io::Result<()> {
    let sealed = protect(key.as_bytes(), true).ok_or_else(|| std::io::Error::other("DPAPI"))?;
    let path = dir.join(KEY_FILE);
    let temporary = path.with_extension("key.tmp");
    std::fs::write(&temporary, sealed)?;
    std::fs::rename(&temporary, &path)
}

fn load_key(dir: &Path) -> Option<String> {
    let sealed = std::fs::read(dir.join(KEY_FILE)).ok()?;
    String::from_utf8(protect(&sealed, false)?).ok()
}

/// For Settings, which cannot read the key.
fn write_status(dir: &Path, status: Status) {
    let text = match status {
        Status::NoClient => "no-client",
        Status::Disconnected => "disconnected",
        Status::Connecting => "connecting",
        Status::Connected => "connected",
    };
    let _ = std::fs::write(dir.join(STATUS_FILE), text);
}

/// Copies a code to the clipboard, kept out of clipboard history and the
/// cloud clipboard (it is a secret, and soon stale).
pub fn copy_code(hwnd: HWND, code: &str) -> bool {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = code.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the clipboard is opened and closed here; each block is
    // allocated, filled while locked, and handed to the clipboard (which then
    // owns it) or freed if that fails.
    unsafe {
        let put = |format: u32, bytes: &[u8]| -> bool {
            let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { return false };
            let pointer = GlobalLock(memory) as *mut u8;
            if pointer.is_null() {
                let _ = GlobalFree(Some(memory));
                return false;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            let _ = GlobalUnlock(memory);
            if SetClipboardData(format, Some(HANDLE(memory.0))).is_err() {
                let _ = GlobalFree(Some(HGLOBAL(memory.0)));
                return false;
            }
            true
        };
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let text: Vec<u8> = wide.iter().flat_map(|unit| unit.to_le_bytes()).collect();
        let ok = put(CF_UNICODETEXT, &text);
        if ok {
            let zero = 0u32.to_le_bytes();
            for name in [windows::core::w!("CanIncludeInClipboardHistory"), windows::core::w!("CanUploadToCloudClipboard")] {
                let format = RegisterClipboardFormatW(name);
                if format != 0 {
                    put(format, &zero);
                }
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_file_from_google_cloud_is_read() {
        let file = r#"{"installed":{"client_id":"123-abc.apps.googleusercontent.com","project_id":"onyx","client_secret":"GOCSPX-x","redirect_uris":["http://localhost"]}}"#;
        let client = Client::parse(file).unwrap();
        assert_eq!(client.id, "123-abc.apps.googleusercontent.com");
        assert_eq!(client.secret, "GOCSPX-x");
        assert!(Client::parse(r#"{"installed":{"client_id":"something-else"}}"#).is_none());
        assert!(Client::parse("not json").is_none());
    }

    #[test]
    fn only_the_domain_gmail_verified_names_the_sender() {
        let gmail = "mx.google.com; dkim=pass header.i=@accounts.google.com; spf=pass; dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=accounts.google.com";
        let verified = |from: &str| verified_domain(gmail, from);
        assert_eq!(verified("Google <no-reply@accounts.google.com>").as_deref(), Some("accounts.google.com"));
        assert_eq!(verified("no-reply@Accounts.Google.com").as_deref(), Some("accounts.google.com"));
        // The display name, a second address, or another domain is not it.
        assert_eq!(verified("Google <x@google.xyz.io>"), None);
        assert_eq!(verified("<a@accounts.google.com> <b@evil.io>"), None);
        assert_eq!(verified("a@accounts.google.com, b@evil.io"), None);
        // Failed, or a result Gmail didn't write.
        assert_eq!(verified_domain(&gmail.replace("dmarc=pass", "dmarc=fail"), "a@accounts.google.com"), None);
        assert_eq!(verified_domain(&gmail.replace("mx.google.com", "mx.evil.io"), "a@accounts.google.com"), None);
        assert_eq!(verified_domain("", "a@accounts.google.com"), None);
        // Text a sender controls can't bring in a pass of its own.
        let injected = "mx.google.com; spf=pass smtp.mailfrom=\"x;dmarc=pass header.from=accounts.google.com\"@evil.io; dmarc=fail header.from=evil.io";
        assert_eq!(verified_domain(injected, "a@accounts.google.com"), None);
        let in_comment = "mx.google.com; spf=pass (x; dmarc=pass header.from=accounts.google.com) smtp.mailfrom=evil.io";
        assert_eq!(verified_domain(in_comment, "a@accounts.google.com"), None);
        assert_eq!(verified_domain("mx.google.com; dmarc=pass (p=NONE (x) header.from=accounts.google.com", "a@accounts.google.com"), None);
    }

    #[test]
    fn only_gmails_own_topmost_result_is_read() {
        let gmail = "mx.google.com; dmarc=pass header.from=example.com";
        let theirs = "mx.example.com; dmarc=pass header.from=example.com";
        assert_eq!(gmails_result(&[gmail, theirs]), Some(gmail));
        assert_eq!(gmails_result(&[theirs, gmail]), None);
        // A sender's own line claiming to be Gmail's: neither is trusted.
        assert_eq!(gmails_result(&[gmail, "mx.google.com; dmarc=pass header.from=google.com"]), None);
        assert_eq!(gmails_result(&[]), None);
    }

    #[test]
    fn html_is_stripped_in_one_pass() {
        let words = |html: &str| strip_html(html).split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(words("<HEAD><Style>p{}</STYLE></head><p>Code&nbsp;<b>4821</b></p>"), ["Code", "4821"]);
        assert_eq!(words("a <script>x<y</script> b"), ["a", "b"]);
        assert_eq!(words("a <b"), ["a"]);
        // Many tags, or a style never closed, stay fast.
        let big = "<b>".repeat(200_000);
        let started = std::time::Instant::now();
        let _ = strip_html(cut(&big, HTML_LIMIT));
        let _ = strip_html(&format!("<style>{big}"));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_fresh_message_gives_its_code_and_an_old_one_does_not() {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("<html><style>p{}</style><p>Your code is <b>731904</b></p></html>");
        let message = |sent_ms: u64, dmarc: &str| {
            serde_json::json!({
                "internalDate": sent_ms.to_string(),
                "snippet": "Use this to sign in",
                "payload": {
                    "mimeType": "multipart/alternative",
                    "headers": [
                        {"name": "Subject", "value": "Sign in to Example"},
                        {"name": "From", "value": "Google <hi@example.com>"},
                        {"name": "Authentication-Results", "value": format!("mx.google.com; spf=pass; dmarc={dmarc} header.from=example.com")},
                    ],
                    "parts": [{"mimeType": "text/html", "body": {"data": body}}],
                },
            })
        };
        assert_eq!(code_of(&message(now_ms - 5_000, "pass")), Some(("731904".into(), "example.com".into())));
        assert_eq!(code_of(&message(now_ms - 5_000, "fail")), Some(("731904".into(), "an unverified sender".into())));
        assert_eq!(code_of(&message(now_ms - 3_600_000, "pass")), None);
    }

    #[test]
    fn html_mail_becomes_its_text() {
        let text = strip_html("<head><title>x</title></head><style>.a{}</style><p>Code&nbsp;<b>4821</b></p><script>var a=1;</script>");
        assert!(text.contains("Code"), "{text}");
        assert!(text.contains("4821"));
        assert!(!text.contains("var a") && !text.contains(".a{}") && !text.contains("title"), "{text}");
    }
}
