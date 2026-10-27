//! Downloading, verifying, and removing model files.
//!
//! Files are fetched over HTTPS into `<name>.part`, resumed with `Range`
//! requests after an interruption, checked against the pinned SHA-256, and only
//! then renamed into place. A corrupt or truncated download is never used.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::registry::{AddOn, ModelFile, ModelSpec, SILERO_VAD, SUPPORT_DIR};

const CHUNK: usize = 256 * 1024;
const ATTEMPTS: u32 = 4;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Debug)]
pub enum ManagerError {
    Io(io::Error),
    Http(String),
    HashMismatch {
        file: String,
    },
    /// The server sent a different amount of data than the registry expects.
    SizeMismatch {
        file: String,
    },
    Cancelled,
}

impl fmt::Display for ManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "file error: {error}"),
            Self::Http(message) => write!(f, "download failed: {message}"),
            Self::HashMismatch { file } => {
                write!(f, "{file} does not match its expected checksum")
            }
            Self::SizeMismatch { file } => write!(f, "{file} has an unexpected size"),
            Self::Cancelled => write!(f, "download cancelled"),
        }
    }
}

impl std::error::Error for ManagerError {}

impl From<io::Error> for ManagerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// What is on disk for a model (sizes only; see [`ModelManager::verify`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelStatus {
    Missing,
    /// Some files are present or partially downloaded.
    Partial {
        bytes: u64,
    },
    Installed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

pub struct ModelManager {
    root: PathBuf,
}

impl ModelManager {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn model_dir(&self, spec: &ModelSpec) -> PathBuf {
        self.root.join(spec.dir)
    }

    #[must_use]
    pub fn vad_path(&self) -> PathBuf {
        self.root.join(SUPPORT_DIR).join(SILERO_VAD.name)
    }

    /// Whether every file of `spec`, and the VAD model, exists with the
    /// expected size. Cheap enough to call on every settings repaint.
    #[must_use]
    pub fn status(&self, spec: &ModelSpec) -> ModelStatus {
        let dir = self.model_dir(spec);
        let mut present = 0u64;
        let mut complete = true;
        for file in spec.files {
            let size = file_size(&dir.join(file.name));
            let part = file_size(&dir.join(part_name(file)));
            if size == Some(file.size) {
                present += file.size;
            } else {
                complete = false;
                present += part.unwrap_or(0).min(file.size);
            }
        }
        if !self.vad_ready() {
            complete = false;
        }
        match (complete, present) {
            (true, _) => ModelStatus::Installed,
            (false, 0) => ModelStatus::Missing,
            (false, bytes) => ModelStatus::Partial { bytes },
        }
    }

    #[must_use]
    pub fn addon_dir(&self, addon: &AddOn) -> PathBuf {
        self.root.join(addon.dir)
    }

    /// The program to run for `addon`, inside its unpacked runtime.
    #[must_use]
    pub fn addon_program(&self, addon: &AddOn) -> PathBuf {
        self.addon_dir(addon)
            .join(AddOn::RUNTIME_DIR)
            .join(addon.program)
    }

    #[must_use]
    pub fn addon_model(&self, addon: &AddOn) -> PathBuf {
        self.addon_dir(addon).join(addon.model)
    }

    fn runtime_unpacked(&self, addon: &AddOn) -> bool {
        addon.archive.is_none() || self.addon_program(addon).is_file()
    }

    /// Like [`Self::status`], for an add-on. An archive counts as present
    /// once it has been unpacked (the zip itself is removed).
    #[must_use]
    pub fn addon_status(&self, addon: &AddOn) -> ModelStatus {
        let dir = self.addon_dir(addon);
        let mut present = 0u64;
        let mut complete = true;
        for file in addon.files {
            if Some(file.name) == addon.archive && self.runtime_unpacked(addon) {
                present += file.size;
                continue;
            }
            match file_size(&dir.join(file.name)) {
                Some(size) if size == file.size && Some(file.name) != addon.archive => {
                    present += size;
                }
                size => {
                    complete = false;
                    let part = file_size(&dir.join(part_name(file))).unwrap_or(0);
                    present += size.unwrap_or(part).min(file.size);
                }
            }
        }
        match (complete, present) {
            (true, _) => ModelStatus::Installed,
            (false, 0) => ModelStatus::Missing,
            (false, bytes) => ModelStatus::Partial { bytes },
        }
    }

    /// Downloads an add-on and unpacks its runtime. Resumes like
    /// [`Self::download`].
    pub fn download_addon(
        &self,
        addon: &AddOn,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<(), ManagerError> {
        let dir = self.addon_dir(addon);
        fs::create_dir_all(&dir)?;
        let total = addon.total_size();
        let mut done = 0u64;
        progress(Progress { done, total });
        for file in addon.files {
            let is_archive = Some(file.name) == addon.archive;
            if !(is_archive && self.runtime_unpacked(addon)) {
                download_file(file, &dir, done, total, progress, cancel)?;
                if is_archive {
                    unpack(&dir.join(file.name), &dir.join(AddOn::RUNTIME_DIR))?;
                    let _ = fs::remove_file(dir.join(file.name));
                }
            }
            done += file.size;
            progress(Progress { done, total });
        }
        Ok(())
    }

    /// Removes an add-on's files.
    pub fn delete_addon(&self, addon: &AddOn) -> io::Result<()> {
        match fs::remove_dir_all(self.addon_dir(addon)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn vad_ready(&self) -> bool {
        file_size(&self.vad_path()) == Some(SILERO_VAD.size)
    }

    /// Recomputes the SHA-256 of every installed file. Slow (it reads the
    /// whole model); run it on a worker thread.
    pub fn verify(&self, spec: &ModelSpec) -> Result<(), ManagerError> {
        let dir = self.model_dir(spec);
        for file in spec.files {
            verify_file(&dir.join(file.name), file)?;
        }
        verify_file(&self.vad_path(), &SILERO_VAD)
    }

    /// Removes a model's files (not the shared VAD model).
    pub fn delete(&self, spec: &ModelSpec) -> io::Result<()> {
        match fs::remove_dir_all(self.model_dir(spec)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Downloads everything `spec` needs (including the VAD model). Existing
    /// complete files are kept; partial ones resume. `progress` is called a few
    /// times a second. Setting `cancel` stops the download and keeps the
    /// partial files for next time.
    pub fn download(
        &self,
        spec: &ModelSpec,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<(), ManagerError> {
        let dir = self.model_dir(spec);
        let support = self.root.join(SUPPORT_DIR);
        fs::create_dir_all(&dir)?;
        fs::create_dir_all(&support)?;

        let total = spec.total_size() + SILERO_VAD.size;
        let mut done = 0u64;
        progress(Progress { done, total });

        let jobs = spec
            .files
            .iter()
            .map(|file| (file, dir.as_path()))
            .chain(std::iter::once((&SILERO_VAD, support.as_path())));
        for (file, destination) in jobs {
            download_file(file, destination, done, total, progress, cancel)?;
            done += file.size;
            progress(Progress { done, total });
        }
        Ok(())
    }
}

/// Unpacks the program and libraries of a runtime zip into `target`
/// (flattened; tools and docs the app never runs are skipped). Unpacks into a
/// temporary folder first so a half-written runtime is never used.
fn unpack(archive: &Path, target: &Path) -> Result<(), ManagerError> {
    let staging = target.with_extension("unpacking");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let mut zip = zip::ZipArchive::new(File::open(archive)?)
        .map_err(|error| ManagerError::Io(io::Error::other(error)))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| ManagerError::Io(io::Error::other(error)))?;
        let Some(name) = entry
            .enclosed_name()
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
        else {
            continue;
        };
        let lower = name.to_string_lossy().to_lowercase();
        let wanted = lower.ends_with(".dll")
            || lower == "llama-server.exe"
            || lower.starts_with("license");
        if !entry.is_file() || !wanted {
            continue;
        }
        let mut out = File::create(staging.join(&name))?;
        io::copy(&mut entry, &mut out)?;
    }
    let _ = fs::remove_dir_all(target);
    fs::rename(&staging, target)?;
    Ok(())
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
}

fn part_name(file: &ModelFile) -> String {
    format!("{}.part", file.name)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut reader = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn verify_file(path: &Path, file: &ModelFile) -> Result<(), ManagerError> {
    if file_size(path) != Some(file.size) {
        return Err(ManagerError::SizeMismatch {
            file: file.name.to_owned(),
        });
    }
    if sha256_file(path)? == file.sha256 {
        Ok(())
    } else {
        Err(ManagerError::HashMismatch {
            file: file.name.to_owned(),
        })
    }
}

/// Downloads one file with resume, retries, and checksum verification.
fn download_file(
    file: &ModelFile,
    directory: &Path,
    base_done: u64,
    total: u64,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), ManagerError> {
    let target = directory.join(file.name);
    if file_size(&target) == Some(file.size) {
        return Ok(());
    }
    let part = directory.join(part_name(file));

    let mut last_error = None;
    for attempt in 1..=ATTEMPTS {
        if cancel.load(Ordering::Relaxed) {
            return Err(ManagerError::Cancelled);
        }
        match fetch_into(file, &part, base_done, total, progress, cancel) {
            Ok(()) => {
                // The hash covers any bytes kept from an earlier run too.
                if sha256_file(&part)? != file.sha256 {
                    let _ = fs::remove_file(&part);
                    return Err(ManagerError::HashMismatch {
                        file: file.name.to_owned(),
                    });
                }
                if target.exists() {
                    fs::remove_file(&target)?;
                }
                fs::rename(&part, &target)?;
                return Ok(());
            }
            Err(ManagerError::Http(message)) => {
                tracing::warn!(file = file.name, attempt, "download interrupted: {message}");
                last_error = Some(ManagerError::Http(message));
                std::thread::sleep(Duration::from_millis(400 * u64::from(attempt)));
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| ManagerError::Http("unknown error".to_owned())))
}

/// One HTTP attempt, appending to `part`.
fn fetch_into(
    file: &ModelFile,
    part: &Path,
    base_done: u64,
    total: u64,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), ManagerError> {
    let mut have = file_size(part).unwrap_or(0);
    if have > file.size {
        fs::remove_file(part)?;
        have = 0;
    }
    if have == file.size {
        return Ok(());
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build();
    let mut request = agent.get(file.url);
    if have > 0 {
        request = request.set("Range", &format!("bytes={have}-"));
    }
    let response = request
        .call()
        .map_err(|error| ManagerError::Http(error.to_string()))?;

    let mut output = match response.status() {
        206 => OpenOptions::new().append(true).open(part)?,
        // The server ignored the range (or there was nothing to resume).
        200 => {
            have = 0;
            File::create(part)?
        }
        status => return Err(ManagerError::Http(format!("server answered {status}"))),
    };

    let mut reader = response.into_reader();
    let mut buffer = vec![0u8; CHUNK];
    let mut last_report = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            output.flush()?;
            return Err(ManagerError::Cancelled);
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|error| ManagerError::Http(error.to_string()))?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        have += read as u64;
        if have > file.size {
            return Err(ManagerError::SizeMismatch {
                file: file.name.to_owned(),
            });
        }
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            last_report = Instant::now();
            progress(Progress {
                done: base_done + have,
                total,
            });
        }
    }
    output.flush()?;
    if have != file.size {
        // A dropped connection looks like a short read: retry and resume.
        return Err(ManagerError::Http(format!(
            "connection closed after {have} of {} bytes",
            file.size
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("islet-manager-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    /// A one-file HTTP server that honours `Range`, optionally dropping the
    /// connection after `cut_after` bytes on the first request.
    struct Server {
        url: String,
        requests: Arc<AtomicUsize>,
        ranges: Arc<std::sync::Mutex<Vec<String>>>,
    }

    fn serve(body: Vec<u8>, cut_first_after: Option<usize>, honour_range: bool) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let ranges = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (counter, seen) = (Arc::clone(&requests), Arc::clone(&ranges));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut start = 0usize;
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        seen.lock().unwrap().push(value.trim().to_owned());
                        if honour_range {
                            start = value.trim().trim_end_matches('-').parse().unwrap_or(0);
                        }
                    }
                }
                let slice = &body[start..];
                let (status, extra) = if start > 0 {
                    (
                        "206 Partial Content",
                        format!(
                            "Content-Range: bytes {start}-{}/{}\r\n",
                            body.len() - 1,
                            body.len()
                        ),
                    )
                } else {
                    ("200 OK", String::new())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                    slice.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let send = match (index, cut_first_after) {
                    (0, Some(cut)) => &slice[..cut.min(slice.len())],
                    _ => slice,
                };
                let _ = stream.write_all(send);
            }
        });
        Server {
            url,
            requests,
            ranges,
        }
    }

    fn file_for(body: &[u8], url: String, sha_override: Option<&str>) -> ModelFile {
        ModelFile {
            name: "file.bin",
            url: leak(url),
            size: body.len() as u64,
            sha256: leak(sha_override.map_or_else(|| hex(&Sha256::digest(body)), str::to_owned)),
        }
    }

    fn run(file: &ModelFile, dir: &Path, cancel: &AtomicBool) -> Result<(), ManagerError> {
        download_file(file, dir, 0, file.size, &mut |_| {}, cancel)
    }

    #[test]
    fn downloads_and_verifies_a_file() {
        let body = payload(700_000);
        let server = serve(body.clone(), None, true);
        let dir = temp_dir("plain");
        let file = file_for(&body, server.url.clone(), None);

        run(&file, &dir, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(dir.join("file.bin")).unwrap(), body);
        assert!(!dir.join("file.bin.part").exists());
        // Already present: no second request.
        run(&file, &dir, &AtomicBool::new(false)).unwrap();
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn resumes_after_a_dropped_connection() {
        let body = payload(900_000);
        let server = serve(body.clone(), Some(300_000), true);
        let dir = temp_dir("resume");
        let file = file_for(&body, server.url.clone(), None);

        run(&file, &dir, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(dir.join("file.bin")).unwrap(), body);
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);
        assert_eq!(server.ranges.lock().unwrap().as_slice(), ["300000-"]);
    }

    #[test]
    fn restarts_when_the_server_ignores_ranges() {
        let body = payload(400_000);
        let server = serve(body.clone(), Some(100_000), false);
        let dir = temp_dir("norange");
        let file = file_for(&body, server.url.clone(), None);

        run(&file, &dir, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(dir.join("file.bin")).unwrap(), body);
    }

    #[test]
    fn rejects_and_discards_a_corrupt_download() {
        let body = payload(100_000);
        let server = serve(body.clone(), None, true);
        let dir = temp_dir("corrupt");
        let wrong = "0".repeat(64);
        let file = file_for(&body, server.url.clone(), Some(&wrong));

        let error = run(&file, &dir, &AtomicBool::new(false)).unwrap_err();
        assert!(
            matches!(error, ManagerError::HashMismatch { .. }),
            "{error}"
        );
        assert!(!dir.join("file.bin").exists());
        assert!(!dir.join("file.bin.part").exists());
    }

    #[test]
    fn cancelling_keeps_the_partial_file() {
        let body = payload(100_000);
        let server = serve(body.clone(), None, true);
        let dir = temp_dir("cancel");
        let file = file_for(&body, server.url.clone(), None);

        let error = run(&file, &dir, &AtomicBool::new(true)).unwrap_err();
        assert!(matches!(error, ManagerError::Cancelled));
        assert!(!dir.join("file.bin").exists());
    }

    #[test]
    fn status_reflects_files_on_disk() {
        let dir = temp_dir("status");
        let manager = ModelManager::new(&dir);
        let spec = crate::registry::PARAKEET_V2;
        assert_eq!(manager.status(&spec), ModelStatus::Missing);

        // A partial download of the biggest file counts as progress.
        let model_dir = manager.model_dir(&spec);
        fs::create_dir_all(&model_dir).unwrap();
        fs::write(model_dir.join("encoder.int8.onnx.part"), vec![0u8; 1000]).unwrap();
        assert_eq!(manager.status(&spec), ModelStatus::Partial { bytes: 1000 });
        assert!(!manager.vad_ready());
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
