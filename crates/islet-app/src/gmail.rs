//! Mail from Gmail on the island: one-time codes and, if chosen in Settings,
//! new emails, from every connected account.
//!
//! Connecting uses Google's own sign-in page in the browser (authorization
//! code with PKCE, back to a one-time listener on 127.0.0.1) and asks for
//! read-only access to mail, nothing else. The key each account hands over
//! (its refresh token) is kept encrypted for this Windows user (DPAPI).
//!
//! The Google sign-in app is Onyx's own, baked in at build time (see
//! `build.rs`): users only press Connect and allow it. A development build
//! without one can use a client file in Onyx's settings folder instead.
//!
//! Checking is cheap: Gmail is asked what changed since the last look, which
//! is a few hundred bytes when nothing did, over one kept connection. It asks
//! every 15 s while the PC is in use, every 3 minutes once it is left alone,
//! and not at all on Battery Saver. Only new messages in the inbox from the
//! last 10 minutes are read, for a code or to show the email as chosen: mail,
//! codes and tokens never reach the log or the disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use islet_core::config::MailConfig;
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
const REVOKE: &str = "https://oauth2.googleapis.com/revoke";
const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
/// Reading mail; no sending, changing or deleting.
const SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
/// Where an email opens.
pub const GMAIL_WEB: &str = "https://mail.google.com/";
const SIGN_IN_WAIT: Duration = Duration::from_secs(180);
/// How often Gmail is asked while the PC is in use, and once it is left.
const ACTIVE_EVERY: Duration = Duration::from_secs(15);
const IDLE_EVERY: Duration = Duration::from_secs(180);
/// No keyboard or mouse for this long: the PC is left alone.
const IDLE_AFTER: Duration = Duration::from_secs(120);
/// Older mail is not a code still wanted, nor news.
const FRESH: Duration = Duration::from_secs(600);
/// Messages read per look at most (a burst of mail is not all news).
const PER_LOOK: usize = 5;
/// Accounts connected at most.
pub const MOST_ACCOUNTS: usize = 5;

/// Onyx's Google sign-in app, from the build ("" when it has none).
const BUILT_IN: &str = include_str!(concat!(env!("OUT_DIR"), "/google-client.json"));
/// A development stand-in for it, in the settings folder.
const CLIENT_FILE: &str = "google-client.json";
const KEY_FILE: &str = "gmail.key";
const STATUS_FILE: &str = "gmail-status.txt";

pub enum News {
    /// `account` names the account it came to when several are connected.
    Code { code: String, from: String, account: String },
    Email { from: String, subject: String, link: String, account: String },
    Connected(String),
    ConnectFailed(&'static str),
    SignedOut,
    /// Google ended an account's sign-in (revoked, or a Testing app's week
    /// is up).
    Expired(String),
}

enum Job {
    Connect,
    /// One account (by its place in the list Settings shows), or all.
    Disconnect(Option<usize>),
    Shutdown,
}

#[derive(Default)]
struct Shared {
    news: Vec<News>,
    stopping: bool,
    /// What shows: codes only, every email, or the custom choices.
    filter: MailConfig,
}

pub struct Gmail {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Job>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Gmail {
    pub fn start(island: HWND, dir: PathBuf, filter: MailConfig) -> Self {
        let shared = Arc::new(Mutex::new(Shared { filter, ..Shared::default() }));
        let (tx, rx) = unbounded();
        let worker_shared = shared.clone();
        let island = island.0 as isize;
        let thread = std::thread::Builder::new()
            .name("gmail".to_owned())
            .spawn(move || run(&rx, &worker_shared, island, &dir))
            .ok();
        Self { shared, tx, thread }
    }

    /// Adds an account (or signs one in again).
    pub fn connect(&self) {
        let _ = self.tx.send(Job::Connect);
    }

    pub fn disconnect(&self, which: Option<usize>) {
        let _ = self.tx.send(Job::Disconnect(which));
    }

    pub fn set_filter(&self, filter: &MailConfig) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.filter = filter.clone();
        }
    }

    pub fn take(&self) -> Vec<News> {
        self.shared.lock().map(|mut shared| std::mem::take(&mut shared.news)).unwrap_or_default()
    }

    /// An account is saved from an earlier sign-in.
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

/// A connected Gmail account.
#[derive(Clone, Debug, PartialEq)]
struct Account {
    /// Its address (empty until Gmail first says, for one saved before
    /// accounts were named).
    email: String,
    refresh: String,
    access: Option<(String, Instant)>,
    /// Gmail's change counter as of the last look.
    history: Option<String>,
}

impl Account {
    fn new(email: String, refresh: String) -> Self {
        Self { email, refresh, access: None, history: None }
    }
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

fn new_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(concat!("Onyx/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Duration::from_secs(6))
        .timeout_read(Duration::from_secs(10))
        .build()
}

fn token(agent: &ureq::Agent, client: &Client, account: &mut Account) -> Result<String, Fail> {
    if let Some((token, until)) = &account.access {
        if Instant::now() + Duration::from_secs(30) < *until {
            return Ok(token.clone());
        }
    }
    let response = agent.post(TOKEN).send_form(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", &account.refresh),
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
    take_tokens(account, &body)
}

fn take_tokens(account: &mut Account, body: &serde_json::Value) -> Result<String, Fail> {
    let access = body["access_token"].as_str().ok_or(Fail::Other)?.to_owned();
    let lasts = body["expires_in"].as_u64().unwrap_or(3600);
    account.access = Some((access.clone(), Instant::now() + Duration::from_secs(lasts)));
    if let Some(refresh) = body["refresh_token"].as_str() {
        refresh.clone_into(&mut account.refresh);
    }
    Ok(access)
}

fn get(agent: &ureq::Agent, client: &Client, account: &mut Account, url: &str) -> Result<serde_json::Value, Fail> {
    let token = token(agent, client, account)?;
    match agent.get(url).set("Authorization", &format!("Bearer {token}")).call() {
        Ok(response) => read_json(response).ok_or(Fail::Other),
        Err(ureq::Error::Status(401, _)) => {
            account.access = None;
            Err(Fail::Other)
        }
        Err(ureq::Error::Status(404, _)) => Err(Fail::Stale),
        Err(ureq::Error::Status(_, _)) => Err(Fail::Other),
        Err(_) => Err(Fail::Offline),
    }
}

/// What came to `account` since the last look, as `filter` chooses;
/// `several` accounts are connected, so the news names this one.
fn look(
    agent: &ureq::Agent,
    client: &Client,
    account: &mut Account,
    filter: &MailConfig,
    several: bool,
) -> Result<Vec<News>, Fail> {
    let Some(since) = account.history.clone() else {
        // The first look only learns where "now" is (and whose account).
        let profile = get(agent, client, account, &format!("{API}/profile?fields=historyId,emailAddress"))?;
        account.history = profile["historyId"].as_str().map(str::to_owned);
        if account.email.is_empty() {
            if let Some(email) = profile["emailAddress"].as_str() {
                email.clone_into(&mut account.email);
            }
        }
        return Ok(Vec::new());
    };
    let url = format!(
        "{API}/history?startHistoryId={since}&historyTypes=messageAdded&labelId=INBOX&fields=historyId,history(messagesAdded(message(id)))"
    );
    let changes = match get(agent, client, account, &url) {
        Err(Fail::Stale) => {
            account.history = None;
            return Ok(Vec::new());
        }
        other => other?,
    };
    if let Some(now) = changes["historyId"].as_str() {
        account.history = Some(now.to_owned());
    }
    let ids: Vec<String> = changes["history"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["messagesAdded"].as_array().into_iter().flatten())
        .filter_map(|added| added["message"]["id"].as_str().map(str::to_owned))
        .collect();
    let label = if several { account.email.clone() } else { String::new() };
    let mut news = Vec::new();
    // The newest last in the list: read those first.
    for id in ids.iter().rev().take(PER_LOOK) {
        let url = format!("{API}/messages/{}?format=full", encode(id));
        let Ok(message) = get(agent, client, account, &url) else { continue };
        if let Some(found) = read_message(&message, filter, &account.email, &label) {
            news.push(found);
        }
    }
    news.reverse();
    Ok(news)
}

/// What a fresh message shows as: its code, or the email itself, as `filter`
/// chooses. `email` is the account's address (for the link), `label` how the
/// card names the account.
fn read_message(message: &serde_json::Value, filter: &MailConfig, email: &str, label: &str) -> Option<News> {
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
    let from = header("From");
    if filter.shows_codes() {
        if let Some(code) = code_of(message, &subject, &from) {
            return Some(News::Code { code: code.0, from: code.1, account: label.to_owned() });
        }
    }
    let labels: Vec<&str> = message["labelIds"].as_array().into_iter().flatten().filter_map(|label| label.as_str()).collect();
    let important = labels.contains(&"IMPORTANT");
    // Primary: so marked, or no inbox tabs at all.
    let primary = labels.contains(&"CATEGORY_PERSONAL") || !labels.iter().any(|label| label.starts_with("CATEGORY_"));
    if !filter.shows_email(important, primary, plain_address(&from).unwrap_or("")) {
        return None;
    }
    let thread = message["threadId"].as_str().or_else(|| message["id"].as_str())?;
    Some(News::Email {
        from: display_name(&from),
        subject: one_line(&subject, 140),
        link: email_link(email, thread),
        account: label.to_owned(),
    })
}

/// The code in a message and who sent it (the domain Gmail verified, never
/// the display name, which is anyone's to write).
fn code_of(message: &serde_json::Value, subject: &str, from: &str) -> Option<(String, String)> {
    let payload = &message["payload"];
    let snippet = message["snippet"].as_str().unwrap_or("");
    let mut text = decode_entities(snippet);
    if let Some(body) = body_text(payload) {
        text.push(' ');
        text.push_str(&body.chars().take(3_000).collect::<String>());
    }
    let code = islet_core::otp::find_code(subject, &text)?;
    let results: Vec<&str> = payload["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|header| header["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("Authentication-Results")))
        .filter_map(|header| header["value"].as_str())
        .collect();
    let sender = gmails_result(&results)
        .and_then(|result| verified_domain(result, from))
        .unwrap_or_else(|| "an unverified sender".to_owned());
    Some((code, sender))
}

/// "Ana <ana@example.com>" or "ana@example.com": the address.
fn plain_address(from: &str) -> Option<&str> {
    let from = from.trim();
    let address = match from.rfind('<') {
        Some(open) => from[open + 1..].trim_end().strip_suffix('>')?,
        None => from,
    };
    let address = address.trim();
    address.contains('@').then_some(address)
}

/// The name an email's sender gives ("Ana Lima"), or their address.
fn display_name(from: &str) -> String {
    let name = from.rfind('<').map_or("", |open| &from[..open]).trim().trim_matches('"').trim();
    let shown = if name.is_empty() { plain_address(from).unwrap_or(from) } else { name };
    one_line(shown, 60)
}

/// `text` on one line, without control characters, at most `most` characters.
fn one_line(text: &str, most: usize) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(most).collect()
}

/// The email's conversation in Gmail, in the account it came to.
fn email_link(email: &str, thread: &str) -> String {
    let thread: String = thread.chars().filter(char::is_ascii_alphanumeric).collect();
    if email.is_empty() {
        format!("{GMAIL_WEB}mail/#all/{thread}")
    } else {
        format!("{GMAIL_WEB}mail/?authuser={}#all/{thread}", encode(email))
    }
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
    let mut accounts = load_accounts(dir);
    let mut agent = new_agent();
    let tell = |news: News| {
        if let Ok(mut shared) = shared.lock() {
            shared.news.push(news);
        }
        post(island);
    };
    let mut client = Client::load(dir);
    write_status(dir, client.is_some(), false, &accounts);
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
            Some(Job::Disconnect(which)) => {
                let gone: Vec<Account> = match which {
                    None => std::mem::take(&mut accounts),
                    Some(index) if index < accounts.len() => vec![accounts.remove(index)],
                    Some(_) => Vec::new(),
                };
                // Google forgets the access too, not only this PC (best
                // effort: offline, the key is still deleted here).
                for account in &gone {
                    let _ = agent.post(REVOKE).send_form(&[("token", &account.refresh)]);
                }
                let _ = save_accounts(dir, &accounts);
                write_status(dir, client.is_some(), false, &accounts);
                if accounts.is_empty() {
                    tell(News::SignedOut);
                }
            }
            Some(Job::Connect) => {
                client = Client::load(dir);
                if let Some(app) = &client {
                    if accounts.len() >= MOST_ACCOUNTS {
                        tell(News::ConnectFailed("Onyx checks up to 5 Gmail accounts: remove one first"));
                    } else {
                        write_status(dir, true, true, &accounts);
                        let stopping = || shared.lock().map_or(true, |shared| shared.stopping);
                        match sign_in(&agent, app, &stopping) {
                            Ok(account) => {
                                let email = account.email.clone();
                                // The same account signed in again replaces
                                // its old key.
                                match accounts.iter_mut().find(|known| known.email.eq_ignore_ascii_case(&email)) {
                                    Some(known) => *known = account,
                                    None => accounts.push(account),
                                }
                                let _ = save_accounts(dir, &accounts);
                                tell(News::Connected(email));
                            }
                            Err(why) => tell(News::ConnectFailed(why)),
                        }
                    }
                } else {
                    tell(News::ConnectFailed("Gmail sign-in isn't set up in this build of Onyx"));
                }
                write_status(dir, client.is_some(), false, &accounts);
            }
            None => {}
        }
        // Time for a look (or right after connecting).
        let Some(app) = &client else {
            wait = Duration::from_secs(3600);
            continue;
        };
        if accounts.is_empty() {
            wait = Duration::from_secs(3600);
            continue;
        }
        let Some(every) = next_look() else {
            // Battery Saver: let the connection go, and look again later.
            agent = new_agent();
            wait = Duration::from_secs(60);
            continue;
        };
        let filter = shared.lock().map(|shared| shared.filter.clone()).unwrap_or_default();
        let several = accounts.len() > 1;
        let mut failed = false;
        let mut changed = false;
        let mut expired = Vec::new();
        for (index, account) in accounts.iter_mut().enumerate() {
            let before = (account.email.clone(), account.refresh.clone());
            match look(&agent, app, account, &filter, several) {
                Ok(found) => {
                    for news in found {
                        tracing::info!("gmail: {} arrived", if matches!(news, News::Code { .. }) { "a code" } else { "an email" });
                        tell(news);
                    }
                }
                Err(Fail::SignedOut) => expired.push(index),
                Err(_) => failed = true,
            }
            changed |= (account.email.as_str(), account.refresh.as_str()) != (before.0.as_str(), before.1.as_str());
        }
        for index in expired.into_iter().rev() {
            tell(News::Expired(accounts.remove(index).email));
            changed = true;
        }
        if changed {
            let _ = save_accounts(dir, &accounts);
            write_status(dir, true, false, &accounts);
        }
        if failed {
            // Offline or Gmail is busy: back off, up to the idle pace.
            failures = failures.saturating_add(1);
            agent = new_agent();
            wait = (every * 2u32.saturating_pow(failures.min(4))).min(IDLE_EVERY);
        } else {
            failures = 0;
            wait = every;
        }
    }
}

fn sign_in(agent: &ureq::Agent, client: &Client, stopping: &dyn Fn() -> bool) -> Result<Account, &'static str> {
    let (Some(verifier), Some(state)) = (random_text(64), random_text(16)) else {
        return Err("Couldn't start sign-in");
    };
    // Any free port: Google allows any on 127.0.0.1 for a Desktop app.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "Couldn't start sign-in")?;
    listener.set_nonblocking(true).map_err(|_| "Couldn't start sign-in")?;
    let port = listener.local_addr().map_err(|_| "Couldn't start sign-in")?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    // "select_account": adding a second account must let the user pick it.
    let url = format!(
        "{AUTH}?client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt={}",
        encode(&client.id),
        encode(&redirect),
        encode(SCOPE),
        encode("select_account consent")
    );
    if !shell_open(&url, SW_SHOWNORMAL.0) {
        return Err("Couldn't open the browser");
    }
    let code = wait_for_code(&listener, "/", &state, "Gmail", Instant::now() + SIGN_IN_WAIT, stopping)?;
    drop(listener);
    let response = agent.post(TOKEN).send_form(&[
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
    let Some(refresh) = body["refresh_token"].as_str() else {
        return Err("Google didn't give a lasting sign-in: try again");
    };
    let mut account = Account::new(String::new(), refresh.to_owned());
    take_tokens(&mut account, &body).map_err(|_| "Google's answer was unreadable")?;
    // Whose account it is, and where "now" is in it.
    let profile = get(agent, client, &mut account, &format!("{API}/profile?fields=historyId,emailAddress"))
        .map_err(|_| "Couldn't reach Gmail")?;
    let email = profile["emailAddress"].as_str().ok_or("Gmail's answer was unreadable")?;
    email.clone_into(&mut account.email);
    account.history = profile["historyId"].as_str().map(str::to_owned);
    Ok(account)
}

/// Every account's address and key, sealed for this Windows user; no file
/// once none is left.
fn save_accounts(dir: &Path, accounts: &[Account]) -> std::io::Result<()> {
    let path = dir.join(KEY_FILE);
    if accounts.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let list: Vec<serde_json::Value> = accounts
        .iter()
        .map(|account| serde_json::json!({ "email": account.email, "refresh": account.refresh }))
        .collect();
    let text = serde_json::json!({ "accounts": list }).to_string();
    let sealed = protect(text.as_bytes(), true).ok_or_else(|| std::io::Error::other("DPAPI"))?;
    let temporary = path.with_extension("key.tmp");
    std::fs::write(&temporary, sealed)?;
    std::fs::rename(&temporary, &path)
}

fn load_accounts(dir: &Path) -> Vec<Account> {
    let Some(text) = std::fs::read(dir.join(KEY_FILE)).ok().and_then(|sealed| protect(&sealed, false)).and_then(|plain| String::from_utf8(plain).ok())
    else {
        return Vec::new();
    };
    parse_accounts(&text)
}

/// The saved list; or one key on its own, as saved before there could be
/// several (its address is learned on the first look).
fn parse_accounts(text: &str) -> Vec<Account> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
        let key = text.trim();
        return if key.is_empty() { Vec::new() } else { vec![Account::new(String::new(), key.to_owned())] };
    };
    json["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|account| {
            let refresh = account["refresh"].as_str()?.to_owned();
            Some(Account::new(account["email"].as_str().unwrap_or("").to_owned(), refresh))
        })
        .take(MOST_ACCOUNTS)
        .collect()
}

/// For Settings, which cannot read the keys: whether this build can sign in,
/// whether a sign-in is under way, and the accounts' addresses.
fn write_status(dir: &Path, available: bool, connecting: bool, accounts: &[Account]) {
    let emails: Vec<&str> = accounts.iter().map(|account| account.email.as_str()).collect();
    let text = serde_json::json!({ "available": available, "connecting": connecting, "accounts": emails }).to_string();
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

    fn message(sent_ms: u64, subject: &str, from: &str, dmarc: &str, labels: &[&str], html: &str) -> serde_json::Value {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(html);
        serde_json::json!({
            "id": "18c2f0a1b2",
            "threadId": "18c2f0a1b2",
            "labelIds": labels,
            "internalDate": sent_ms.to_string(),
            "snippet": "",
            "payload": {
                "mimeType": "multipart/alternative",
                "headers": [
                    {"name": "Subject", "value": subject},
                    {"name": "From", "value": from},
                    {"name": "Authentication-Results", "value": format!("mx.google.com; spf=pass; dmarc={dmarc} header.from=example.com")},
                ],
                "parts": [{"mimeType": "text/html", "body": {"data": body}}],
            },
        })
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
    }

    #[test]
    fn a_fresh_message_gives_its_code_and_an_old_one_does_not() {
        let codes = MailConfig::default();
        let code = |sent_ms: u64, dmarc: &str| {
            let html = "<html><style>p{}</style><p>Your code is <b>731904</b></p></html>";
            match read_message(&message(sent_ms, "Sign in to Example", "Google <hi@example.com>", dmarc, &["INBOX"], html), &codes, "", "") {
                Some(News::Code { code, from, .. }) => Some((code, from)),
                _ => None,
            }
        };
        assert_eq!(code(now_ms() - 5_000, "pass"), Some(("731904".into(), "example.com".into())));
        assert_eq!(code(now_ms() - 5_000, "fail"), Some(("731904".into(), "an unverified sender".into())));
        assert_eq!(code(now_ms() - 3_600_000, "pass"), None);
    }

    #[test]
    fn emails_show_only_as_chosen() {
        use islet_core::config::MailShow;
        let lunch = message(
            now_ms() - 5_000,
            "Lunch on\r\nFriday?",
            "\"Ana Lima\" <ana@example.com>",
            "pass",
            &["INBOX", "CATEGORY_PERSONAL"],
            "<p>Are you free?</p>",
        );
        let mut filter = MailConfig::default();
        assert!(read_message(&lunch, &filter, "me@gmail.com", "").is_none(), "codes only by default");
        filter.show = MailShow::All;
        let Some(News::Email { from, subject, link, account }) = read_message(&lunch, &filter, "me@gmail.com", "me@gmail.com") else {
            panic!("all mail shows");
        };
        assert_eq!((from.as_str(), subject.as_str(), account.as_str()), ("Ana Lima", "Lunch on Friday?", "me@gmail.com"));
        assert_eq!(link, "https://mail.google.com/mail/?authuser=me%40gmail.com#all/18c2f0a1b2");
        filter.show = MailShow::Custom;
        assert!(read_message(&lunch, &filter, "", "").is_none(), "not important, primary off, not a chosen sender");
        filter.custom_senders = vec!["example.com".into()];
        assert!(matches!(read_message(&lunch, &filter, "", ""), Some(News::Email { .. })));
        // A code still shows as a code when all mail shows.
        filter.show = MailShow::All;
        let code = message(now_ms() - 5_000, "Your code", "a@example.com", "pass", &["INBOX"], "<p>Your code is 731904</p>");
        assert!(matches!(read_message(&code, &filter, "", ""), Some(News::Code { .. })));
    }

    #[test]
    fn accounts_are_saved_together_and_an_old_single_key_still_loads() {
        let saved = parse_accounts(r#"{"accounts":[{"email":"a@gmail.com","refresh":"1//a"},{"email":"b@gmail.com","refresh":"1//b"}]}"#);
        assert_eq!(saved.iter().map(|account| account.email.as_str()).collect::<Vec<_>>(), ["a@gmail.com", "b@gmail.com"]);
        assert_eq!(parse_accounts("1//old-key\n"), vec![Account::new(String::new(), "1//old-key".into())]);
        assert!(parse_accounts("").is_empty());
    }

    #[test]
    fn senders_and_links_are_tidy() {
        assert_eq!(display_name("Ana <ana@example.com>"), "Ana");
        assert_eq!(display_name("ana@example.com"), "ana@example.com");
        assert_eq!(plain_address("\"Lima, Ana\" <ana@example.com>"), Some("ana@example.com"));
        assert_eq!(plain_address("nobody"), None);
        assert_eq!(email_link("", "18c2\"/x"), "https://mail.google.com/mail/#all/18c2x");
        assert!(email_link("a@b.c", "1").starts_with(GMAIL_WEB));
    }

    #[test]
    fn html_mail_becomes_its_text() {
        let text = strip_html("<head><title>x</title></head><style>.a{}</style><p>Code&nbsp;<b>4821</b></p><script>var a=1;</script>");
        assert!(text.contains("Code"), "{text}");
        assert!(text.contains("4821"));
        assert!(!text.contains("var a") && !text.contains(".a{}") && !text.contains("title"), "{text}");
    }
}
