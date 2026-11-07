//! Optional AI cleanup: a small instruction-following language model that
//! rewrites a finished dictation (drops what the speaker corrected, fixes
//! grammar, lays out lists).
//!
//! The model runs in `llama-server.exe` from the downloaded llama.cpp
//! runtime, a separate process like the speech worker: the island never
//! links it, "unloading" is ending the process, and a crash only costs the
//! rewrite. The server listens on a loopback port only.

use std::fmt;
use std::net::TcpListener;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// `CREATE_NO_WINDOW`: the server must not flash a console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Windows' "a DLL was not found" exit code (the VC++ runtime is missing).
const STATUS_DLL_NOT_FOUND: u32 = 0xC000_0135;
const START_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum PolishError {
    /// The runtime could not start; usually the VC++ runtime is missing.
    MissingRuntime,
    Start(String),
    Request(String),
    TimedOut,
}

impl fmt::Display for PolishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRuntime => write!(
                f,
                "AI cleanup needs the Microsoft Visual C++ Runtime (x64) installed"
            ),
            Self::Start(message) => write!(f, "AI cleanup could not start: {message}"),
            Self::Request(message) => write!(f, "AI cleanup failed: {message}"),
            Self::TimedOut => write!(f, "AI cleanup took too long"),
        }
    }
}

impl std::error::Error for PolishError {}

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Owns the `llama-server` process. Methods block; call them off the
/// dictation thread.
pub struct Polisher {
    server_exe: PathBuf,
    model: PathBuf,
    threads: usize,
    server: Mutex<Option<Server>>,
}

impl Polisher {
    #[must_use]
    pub fn new(server_exe: PathBuf, model: PathBuf) -> Self {
        let threads = std::thread::available_parallelism()
            .map_or(2, |count| count.get())
            .clamp(2, 4);
        Self {
            server_exe,
            model,
            threads,
            server: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.server.lock().is_ok_and(|server| server.is_some())
    }

    /// Starts the server if it is not running (loading takes about a second),
    /// so a rewrite right after a dictation does not wait for it.
    pub fn warm(&self) -> Result<(), PolishError> {
        let mut server = self
            .server
            .lock()
            .map_err(|_| PolishError::Start("lock poisoned".to_owned()))?;
        self.ensure(&mut server).map(|_| ())
    }

    /// Ends the server process, freeing its memory.
    pub fn shutdown(&self) {
        if let Ok(mut server) = self.server.lock() {
            *server = None;
        }
    }

    /// Rewrites `text`, giving up after `budget` (including any start-up).
    pub fn rewrite(&self, text: &str, budget: Duration) -> Result<String, PolishError> {
        let deadline = Instant::now() + budget;
        let mut server = self
            .server
            .lock()
            .map_err(|_| PolishError::Start("lock poisoned".to_owned()))?;
        let port = self.ensure(&mut server)?;
        let left = deadline.saturating_duration_since(Instant::now());
        if left < Duration::from_millis(200) {
            return Err(PolishError::TimedOut);
        }
        let pieces = crate::chunks::plan(text);
        let mut result = String::new();
        let mut kept = 0usize;
        for piece in &pieces {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let proposed = if piece.infer && remaining >= Duration::from_millis(200) {
                request(port, piece.text, remaining)
            } else {
                Err(PolishError::TimedOut)
            };
            match proposed {
                Ok(rewrite) => {
                    if let Some(reason) =
                        islet_core::transcript::rewrite_rejection(piece.text, &rewrite)
                    {
                        tracing::info!(?reason, "Onyx cleanup chunk retained rule-cleaned text");
                        result.push_str(piece.text);
                    } else {
                        result.push_str(&rewrite);
                        kept += 1;
                    }
                }
                Err(reason) => {
                    tracing::info!(%reason, "Onyx cleanup chunk retained rule-cleaned text");
                    result.push_str(piece.text);
                }
            }
            result.push_str(piece.separator);
        }
        let accepted = islet_core::transcript::accept_rewrite(text, result.trim());
        tracing::info!(
            chunks = pieces.len(),
            kept,
            accepted,
            "Onyx cleanup completed"
        );
        Ok(if accepted {
            result.trim().to_owned()
        } else {
            text.to_owned()
        })
    }

    fn ensure(&self, server: &mut Option<Server>) -> Result<u16, PolishError> {
        if let Some(running) = server {
            if running.child.try_wait().ok().flatten().is_none() {
                return Ok(running.port);
            }
            *server = None;
        }
        let started = start(&self.server_exe, &self.model, self.threads)?;
        let port = started.port;
        *server = Some(started);
        Ok(port)
    }
}

fn free_port() -> Result<u16, PolishError> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| PolishError::Start(error.to_string()))
}

fn start(exe: &Path, model: &Path, threads: usize) -> Result<Server, PolishError> {
    let port = free_port()?;
    let child = Command::new(exe)
        .arg("-m")
        .arg(model)
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args([
            "-t",
            &threads.to_string(),
            "-c",
            "4096",
            "-ngl",
            "0",
            "--no-webui",
        ])
        .current_dir(exe.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| PolishError::Start(error.to_string()))?;
    crate::job::tie_to_app(&child);
    let mut server = Server { child, port };

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(500))
        .build();
    let began = Instant::now();
    loop {
        if let Ok(Some(status)) = server.child.try_wait() {
            // Exit codes are NTSTATUS values; compare the bit pattern.
            let code = status.code().map(|code| code as u32);
            return Err(if code == Some(STATUS_DLL_NOT_FOUND) {
                PolishError::MissingRuntime
            } else {
                PolishError::Start(format!("exited with {status}"))
            });
        }
        // 200 once the model is loaded; 503 while it loads.
        if agent
            .get(&format!("http://127.0.0.1:{port}/health"))
            .call()
            .is_ok()
        {
            tracing::info!(
                load_ms = began.elapsed().as_millis() as u64,
                "AI cleanup model loaded"
            );
            return Ok(server);
        }
        if began.elapsed() > START_TIMEOUT {
            let _ = server.child.kill();
            return Err(PolishError::Start("timed out loading the model".to_owned()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn messages(text: &str) -> Vec<Value> {
    vec![json!({"role": "user", "content": format!("Clean up this dictation:\n{text}")})]
}

fn request(port: u16, text: &str, budget: Duration) -> Result<String, PolishError> {
    use std::io::BufRead;
    let deadline = Instant::now() + budget;
    let body = json!({"messages": messages(text), "temperature": 0,
        "max_tokens": (text.split_whitespace().count() * 3 + 24).clamp(64, 192),
        "cache_prompt": false, "stream": true});
    let response = ureq::AgentBuilder::new()
        .timeout(budget)
        .build()
        .post(&format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|_| PolishError::Request("local inference request failed".into()))?;
    let mut output = String::new();
    let mut finished = false;
    for line in std::io::BufReader::new(response.into_reader()).lines() {
        if Instant::now() >= deadline {
            return Err(PolishError::TimedOut);
        }
        let line =
            line.map_err(|_| PolishError::Request("local inference stream failed".into()))?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        if data.trim() == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|_| PolishError::Request("invalid inference event".into()))?;
        if let Some(part) = value["choices"][0]["delta"]["content"].as_str() {
            output.push_str(part);
        }
        if let Some(reason) = value["choices"][0]["finish_reason"].as_str() {
            if reason != "stop" {
                return Err(PolishError::Request("rewrite incomplete".into()));
            }
            finished = true;
        }
        if crate::chunks::repeating(text, &output) || output.len() > 400.max(text.len() * 2 + 160) {
            return Err(PolishError::Request(
                "repeating or excessive rewrite".into(),
            ));
        }
    }
    if !finished || output.trim().is_empty() {
        return Err(PolishError::Request("incomplete rewrite".into()));
    }
    Ok(output.trim().to_owned())
}

#[cfg(test)]
fn decode_reply(value: Value) -> Result<String, PolishError> {
    if value["choices"][0]["finish_reason"].as_str() == Some("length") {
        return Err(PolishError::Request(
            "rewrite reached the output limit".to_owned(),
        ));
    }
    value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| PolishError::Request("no text in the reply".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_truncated_rewrite_is_never_typed_as_a_complete_reply() {
        let value = json!({"choices": [{"finish_reason": "length", "message": {"content": "Keep this draft"}}]});
        assert!(matches!(decode_reply(value), Err(PolishError::Request(_))));
        let value = json!({"choices": [{"finish_reason": "stop", "message": {"content": "Keep this draft."}}]});
        assert_eq!(decode_reply(value).unwrap(), "Keep this draft.");
    }

    #[test]
    fn prompt_matches_gemma_training() {
        let all = messages("Hello there.");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["role"], "user");
        assert_eq!(all[0]["content"], "Clean up this dictation:\nHello there.");
    }

    #[test]
    fn a_missing_server_is_a_start_error() {
        let polisher = Polisher::new(
            PathBuf::from("Z:/does/not/exist/llama-server.exe"),
            PathBuf::from("model.gguf"),
        );
        assert!(matches!(polisher.warm(), Err(PolishError::Start(_))));
        assert!(!polisher.is_running());
    }
}
