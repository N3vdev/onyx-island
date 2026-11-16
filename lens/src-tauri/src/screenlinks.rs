//! Things on screen you can act on directly: QR codes (found in the frame
//! and offered as chips) and links in selected text (offered as an "Open"
//! action). Both go through one classifier so a QR code and a typed URL are
//! treated the same way.

use image::RgbaImage;
use regex::Regex;
use std::sync::OnceLock;

use crate::state::AppState;

const MAX_CODES: usize = 4;
const MAX_LINK_LEN: usize = 2048;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Web,
    Email,
    Wifi,
    /// A 2FA setup code (`otpauth://`): copy only, and never shown in full.
    Secret,
    Text,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenLink {
    pub kind: LinkKind,
    /// What the chip/button says, e.g. "example.com" or "Wi-Fi: Home".
    pub label: String,
    /// What Copy puts on the clipboard (a Wi-Fi code's password, say).
    pub copy_text: String,
    /// What Open opens, for web and email only.
    pub target: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenCode {
    pub link: ScreenLink,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Free. Finds QR codes in the frame the overlay is showing.
#[tauri::command]
pub async fn scan_codes(state: tauri::State<'_, AppState>) -> Result<Vec<ScreenCode>, String> {
    let Some(frame) = state.frame_cache() else { return Ok(Vec::new()) };
    tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let image = image::load_from_memory(&frame.png_bytes)
            .map_err(|err| format!("Could not read the capture: {err}"))?
            .to_rgba8();
        let codes = find_codes(&image);
        tracing::info!(elapsed_ms = started.elapsed().as_millis() as u64, count = codes.len(), "qr scan");
        Ok(codes)
    })
    .await
    .map_err(|err| format!("qr worker failed: {err}"))?
}

/// Free. Whether a piece of selected text is a single link worth opening.
#[tauri::command]
pub fn classify_text(text: String) -> Option<ScreenLink> {
    from_recognized_text(&text)
}

pub fn find_codes(image: &RgbaImage) -> Vec<ScreenCode> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let raw = image.as_raw();
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let p = (y * w + x) * 4;
        ((raw[p] as u32 * 299 + raw[p + 1] as u32 * 587 + raw[p + 2] as u32 * 114) / 1000) as u8
    });
    let mut codes = Vec::new();
    for grid in prepared.detect_grids().into_iter().take(MAX_CODES * 2) {
        let Ok((_, content)) = grid.decode() else { continue };
        if content.trim().is_empty() {
            continue;
        }
        let xs = grid.bounds.iter().map(|p| p.x as f64);
        let ys = grid.bounds.iter().map(|p| p.y as f64);
        let (x0, x1) = xs.fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        let (y0, y1) = ys.fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        codes.push(ScreenCode { link: classify(&content), x: x0, y: y0, width: x1 - x0, height: y1 - y0 });
        if codes.len() == MAX_CODES {
            break;
        }
    }
    codes
}

/// Any decoded content (a QR code's text) → what it is.
pub fn classify(content: &str) -> ScreenLink {
    let text = content.trim();
    web(text)
        .or_else(|| email(text))
        .or_else(|| wifi(text))
        .or_else(|| secret(text))
        .unwrap_or_else(|| ScreenLink { kind: LinkKind::Text, label: ellipsize(text, 48), copy_text: text.to_string(), target: None })
}

/// OCR'd text → a link, only if the whole thing is one. OCR tends to put
/// spaces around the dots and slashes of a URL ("example . com / path"), so
/// those are closed up first; ordinary prose with a dot in it doesn't
/// survive the checks that follow.
pub fn from_recognized_text(text: &str) -> Option<ScreenLink> {
    static GAPS: OnceLock<Regex> = OnceLock::new();
    let gaps = GAPS.get_or_init(|| Regex::new(r"\s*([./:@])\s*").unwrap());
    let trimmed = strip_wrappers(text.trim());
    if trimmed.is_empty() || trimmed.len() > MAX_LINK_LEN || trimmed.lines().count() > 2 {
        return None;
    }
    let joined = gaps.replace_all(trimmed, "$1").replace(['\n', '\r'], "");
    if joined.contains(char::is_whitespace) {
        return None;
    }
    web(&joined).or_else(|| email(&joined))
}

fn strip_wrappers(text: &str) -> &str {
    text.trim_start_matches(['(', '[', '{', '<', '"', '\'', '\u{201c}', '\u{2018}'])
        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'', '\u{201d}', '\u{2019}'])
}

fn web(text: &str) -> Option<ScreenLink> {
    static SCHEME: OnceLock<Regex> = OnceLock::new();
    static BARE: OnceLock<Regex> = OnceLock::new();
    let scheme = SCHEME.get_or_init(|| Regex::new(r"(?i)^https?://[^\s/$.?#][^\s]*$").unwrap());
    // A bare domain only counts with a common top-level domain, so "file.rs"
    // or "e.g" never turn into links.
    let bare = BARE.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:www\.)?(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+(?:com|net|org|edu|gov|io|ai|app|dev|co|me|info|biz|xyz|tech|site|online|store|shop|blog|news|tv|uk|us|ca|au|de|fr|in|jp|nl|es|it|se|no|ch|eu|nz|ie|be|at|dk|fi|pl|br|mx)(?:[/?#][^\s]*)?$",
        )
        .unwrap()
    });
    let target = if scheme.is_match(text) {
        text.to_string()
    } else if bare.is_match(text) {
        format!("https://{text}")
    } else {
        return None;
    };
    let shown = target.split("://").nth(1).unwrap_or(&target).trim_start_matches("www.").trim_end_matches('/');
    Some(ScreenLink { kind: LinkKind::Web, label: ellipsize(shown, 40), copy_text: target.clone(), target: Some(target) })
}

fn email(text: &str) -> Option<ScreenLink> {
    static EMAIL: OnceLock<Regex> = OnceLock::new();
    let pattern = EMAIL.get_or_init(|| Regex::new(r"(?i)^(?:mailto:)?([a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,})$").unwrap());
    let address = pattern.captures(text)?.get(1)?.as_str().to_string();
    Some(ScreenLink { kind: LinkKind::Email, label: ellipsize(&address, 40), copy_text: address.clone(), target: Some(format!("mailto:{address}")) })
}

/// `WIFI:T:WPA;S:name;P:password;;` - values escape `\ ; , : "` with `\`.
fn wifi(text: &str) -> Option<ScreenLink> {
    let body = text.strip_prefix("WIFI:").or_else(|| text.strip_prefix("wifi:"))?;
    let mut fields: Vec<(char, String)> = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in body.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ';' {
            let mut parts = current.splitn(2, ':');
            if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
                if let Some(k) = key.chars().next().filter(|_| key.len() == 1) {
                    fields.push((k.to_ascii_uppercase(), value.to_string()));
                }
            }
            current.clear();
        } else {
            current.push(ch);
        }
    }
    let field = |k: char| fields.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone());
    let name = field('S')?;
    let password = field('P').unwrap_or_default();
    Some(ScreenLink {
        kind: LinkKind::Wifi,
        label: format!("Wi-Fi: {}", ellipsize(&name, 32)),
        copy_text: if password.is_empty() { name } else { password },
        target: None,
    })
}

fn secret(text: &str) -> Option<ScreenLink> {
    if !text.to_ascii_lowercase().starts_with("otpauth://") {
        return None;
    }
    Some(ScreenLink { kind: LinkKind::Secret, label: "2FA setup code".to_string(), copy_text: text.to_string(), target: None })
}

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_links_with_and_without_a_scheme() {
        let full = classify("https://www.example.com/path?q=1");
        assert_eq!(full.kind, LinkKind::Web);
        assert_eq!(full.target.as_deref(), Some("https://www.example.com/path?q=1"));
        assert_eq!(full.label, "example.com/path?q=1");

        let bare = classify("github.com/rust-lang");
        assert_eq!(bare.target.as_deref(), Some("https://github.com/rust-lang"));
        // Not a common TLD, and file names are not links.
        assert_eq!(classify("main.rs").kind, LinkKind::Text);
        assert_eq!(classify("e.g").kind, LinkKind::Text);
    }

    #[test]
    fn emails_and_mailto() {
        let mail = classify("mailto:Someone@Example.org");
        assert_eq!(mail.kind, LinkKind::Email);
        assert_eq!(mail.target.as_deref(), Some("mailto:Someone@Example.org"));
        assert_eq!(classify("hi@there.io").copy_text, "hi@there.io");
    }

    #[test]
    fn wifi_codes_copy_the_password_and_honour_escapes() {
        let wifi = classify(r"WIFI:T:WPA;S:Home\;Net;P:pa\:ss;;");
        assert_eq!(wifi.kind, LinkKind::Wifi);
        assert_eq!(wifi.label, "Wi-Fi: Home;Net");
        assert_eq!(wifi.copy_text, "pa:ss");
        assert_eq!(wifi.target, None);
    }

    #[test]
    fn two_factor_codes_are_never_opened_or_shown() {
        let otp = classify("otpauth://totp/Acme:me?secret=ABC&issuer=Acme");
        assert_eq!(otp.kind, LinkKind::Secret);
        assert_eq!(otp.target, None);
        assert!(!otp.label.contains("ABC"));
    }

    #[test]
    fn recognized_text_is_only_a_link_when_all_of_it_is() {
        // OCR's spaces around dots and slashes are closed up.
        let link = from_recognized_text("example . com / docs").expect("a link");
        assert_eq!(link.target.as_deref(), Some("https://example.com/docs"));
        assert_eq!(from_recognized_text("(https://a.io/x).").unwrap().target.as_deref(), Some("https://a.io/x"));
        assert!(from_recognized_text("Visit example.com for more").is_none());
        assert!(from_recognized_text("The end. Next line").is_none());
        assert!(from_recognized_text("").is_none());
    }

    #[test]
    fn finds_and_reads_a_qr_code_in_a_frame() {
        let code = qrcode::QrCode::new(b"https://example.com/qr").unwrap();
        let width = code.width() as u32;
        let modules = code.to_colors();
        let (scale, origin) = (6u32, (120u32, 90u32));
        let image = RgbaImage::from_fn(480, 360, |x, y| {
            let inside = x >= origin.0 && y >= origin.1 && x < origin.0 + width * scale && y < origin.1 + width * scale;
            let dark = inside && {
                let (mx, my) = ((x - origin.0) / scale, (y - origin.1) / scale);
                modules[(my * width + mx) as usize] == qrcode::Color::Dark
            };
            if dark { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) }
        });
        let codes = find_codes(&image);
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].link.target.as_deref(), Some("https://example.com/qr"));
        assert!((codes[0].x - 120.0).abs() < 8.0 && (codes[0].y - 90.0).abs() < 8.0, "bounds {:?}", (codes[0].x, codes[0].y));

        let blank = RgbaImage::from_pixel(200, 200, image::Rgba([255, 255, 255, 255]));
        assert!(find_codes(&blank).is_empty());
    }
}
