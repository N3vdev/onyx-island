use std::env;
use std::path::PathBuf;

/// Filesystem locations used by the island process.
///
/// Config lives under roaming app data so it can be backed up; logs and large
/// models stay under local app data. Both directories are per-user.
#[derive(Clone, Debug)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub model_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Self {
        let roaming = env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let local = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| roaming.clone());

        let config_dir = roaming.join("Islet");
        let data_dir = local.join("Islet");
        let config_file = config_dir.join("config.toml");
        let log_dir = data_dir.join("logs");
        let model_dir = data_dir.join("models");

        Self {
            config_dir,
            config_file,
            data_dir,
            log_dir,
            model_dir,
        }
    }
}
