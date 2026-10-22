use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Keeps the non-blocking log writer alive for the lifetime of the process.
pub struct LogGuard {
    _worker: WorkerGuard,
}

/// Initializes rolling file logs in `log_dir` and installs a panic hook.
///
/// Nothing sensitive may be logged by callers. This module only controls the
/// sink; call sites are responsible for never logging keystrokes, transcripts,
/// clipboard contents, or raw agent payloads.
pub fn init(log_dir: &Path) -> Result<LogGuard, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(log_dir)?;

    let file_appender = tracing_appender::rolling::daily(log_dir, "islet.log");
    let (writer, worker) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_thread_names(true)
        .init();

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "unhandled panic");
        default_hook(info);
    }));

    Ok(LogGuard { _worker: worker })
}
