//! The dictation cleanup path, for model training and evaluation scripts.
//!
//! Reads one JSON object per line on stdin and answers one per line on
//! stdout, so a script can push many texts through without restarting:
//!
//! - `native_pipeline <llama-server.exe> <model.gguf> [--professional]`: what the app does
//!   with a transcript. `{"input"}` → `{"output", "rule", "accepted",
//!   "elapsed_ms"}`: rule cleanup, the model rewrite with the app's time
//!   budget, the outer guard (falling back to the rule text), then number
//!   and layout formatting.
//! - `native_pipeline --rules-only`: no model. `{"input", "target"?}` →
//!   `{"rule", "final", "target_rule"?, "guard_ok"?}`, where `guard_ok` says
//!   whether the app would accept `target` as a rewrite of the rule text.
//!
//! Prints nothing else; texts only go back to the calling script.
use islet_asr::Polisher;
use islet_core::transcript::{
    accept_rewrite, clean, finish_layout, format_numbers, unwrap_rewrite, TextOptions,
};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The app's budget for one rewrite (dictation.rs).
fn budget(text: &str) -> Duration {
    let words = text.split_whitespace().count() as u32;
    (Duration::from_millis(2500) + Duration::from_millis(60) * words).min(Duration::from_secs(15))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = TextOptions {
        filler_removal: true,
        spoken_punctuation: true,
        ..Default::default()
    };
    // `--professional` after the model: the professional rewrite prompt.
    let professional = args.iter().any(|arg| arg == "--professional");
    let polisher = if args.first().map(String::as_str) == Some("--rules-only") {
        None
    } else {
        let (Some(server), Some(model)) = (args.first(), args.get(1)) else {
            return Err("usage: native_pipeline <llama-server.exe> <model.gguf> | --rules-only".into());
        };
        let polisher = Polisher::new(PathBuf::from(server), PathBuf::from(model));
        polisher.warm()?;
        Some(polisher)
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(&line)?;
        let input = request["input"].as_str().unwrap_or_default();
        let rule = clean(input, &options);
        let reply = match &polisher {
            None => {
                let mut reply = json!({"rule": rule, "final": finish_layout(&format_numbers(&rule))});
                if let Some(target) = request["target"].as_str() {
                    let target_rule = clean(target, &options);
                    reply["guard_ok"] = json!(accept_rewrite(&rule, &target_rule));
                    reply["target_rule"] = json!(target_rule);
                }
                reply
            }
            Some(polisher) => {
                let start = Instant::now();
                let (text, accepted) = if rule.is_empty() {
                    (String::new(), false)
                } else {
                    match polisher.rewrite(&rule, budget(&rule), professional) {
                        Ok(rewrite) => {
                            let rewrite = unwrap_rewrite(&rewrite);
                            if accept_rewrite(&rule, &rewrite) {
                                (rewrite, true)
                            } else {
                                (rule.clone(), false)
                            }
                        }
                        Err(_) => (rule.clone(), false),
                    }
                };
                let output = finish_layout(&format_numbers(&text));
                json!({
                    "output": output,
                    "rule": rule,
                    "accepted": accepted,
                    "elapsed_ms": start.elapsed().as_millis() as u64,
                })
            }
        };
        writeln!(stdout, "{reply}")?;
        stdout.flush()?;
    }
    Ok(())
}
