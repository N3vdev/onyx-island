//! Actual native cleanup-path verification; never prints input/output text.
use islet_asr::{ModelManager, ModelStatus, Polisher, AI_CLEANUP};
use islet_core::transcript::{clean, TextOptions};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let root = args
        .windows(2)
        .find(|pair| pair[0] == "--model-root")
        .map(|pair| PathBuf::from(&pair[1]))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("Islet/models")
        });
    let manager = ModelManager::new(root);
    if args.iter().any(|arg| arg == "--install") {
        manager.download_addon(
            &AI_CLEANUP,
            &mut |_| {},
            &std::sync::atomic::AtomicBool::new(false),
        )?;
    }
    assert_eq!(manager.addon_status(&AI_CLEANUP), ModelStatus::Installed);
    let polisher = Polisher::new(
        manager.addon_program(&AI_CLEANUP),
        manager.addon_model(&AI_CLEANUP),
    );
    polisher.warm()?;
    let path = std::env::args()
        .nth(1)
        .ok_or("expected synthetic fixture path")?;
    let cases: serde_json::Value = if path == "-" {
        serde_json::from_reader(std::io::stdin().lock())?
    } else {
        serde_json::from_reader(std::fs::File::open(path)?)?
    };
    let options = TextOptions {
        filler_removal: true,
        spoken_punctuation: true,
        ..Default::default()
    };
    let mut passed = 0;
    for case in cases.as_array().unwrap() {
        let input = clean(case["input"].as_str().unwrap(), &options);
        let start = Instant::now();
        let output = polisher.rewrite(&input, Duration::from_secs(15), false)?;
        assert!(islet_core::transcript::accept_rewrite(&input, &output));
        let output = islet_core::transcript::format_numbers(&output);
        assert!(islet_core::transcript::accept_rewrite(&input, &output));
        let lower = output.to_lowercase();
        assert!(case["required"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| lower.contains(&v.as_str().unwrap().to_lowercase())));
        assert!(case["forbidden"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| !lower.contains(&v.as_str().unwrap().to_lowercase())));
        if case["require_question"].as_bool().unwrap_or(true) {
            assert!(output.contains('?'));
        }
        if let Some(expected) = case["bullet_count"].as_u64() {
            let count = output
                .lines()
                .filter(|line| {
                    ["- ", "* ", "\u{2022} "]
                        .iter()
                        .any(|prefix| line.trim_start().starts_with(prefix))
                })
                .count();
            assert_eq!(count as u64, expected);
        }
        if let Some(groups) = case["bullet_groups"].as_array() {
            let lines: Vec<_> = lower
                .lines()
                .filter(|line| {
                    ["- ", "* ", "\u{2022} "]
                        .iter()
                        .any(|prefix| line.trim_start().starts_with(prefix))
                })
                .collect();
            assert_eq!(lines.len(), groups.len());
            for (line, group) in lines.iter().zip(groups) {
                assert!(group
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|term| line.contains(&term.as_str().unwrap().to_lowercase())));
            }
        }
        if let Some(groups) = case["any_of"].as_array() {
            for group in groups {
                assert!(group
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| lower.contains(&v.as_str().unwrap().to_lowercase())));
            }
        }
        println!(
            "case={} native_pass=true elapsed_ms={}",
            case["id"].as_str().unwrap(),
            start.elapsed().as_millis()
        );
        passed += 1;
    }
    polisher.shutdown();
    if std::env::args().any(|arg| arg == "--enable") {
        let path = PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Islet/config.toml");
        let mut config = islet_core::Config::load(&path)?;
        config.dictation.ai_cleanup = true;
        config.save_atomic(&path)?;
        println!("Gemma cleanup enabled in saved settings.");
    }
    println!(
        "model={} cases_passed={passed} runtime_stopped={}",
        AI_CLEANUP.name,
        !polisher.is_running()
    );
    Ok(())
}
