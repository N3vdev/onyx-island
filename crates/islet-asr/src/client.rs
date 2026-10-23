//! The island's handle on `islet-asr-worker.exe`.
//!
//! The worker is a child process: unloading the model is simply ending the
//! process, which returns every byte of its memory to Windows. Replies
//! arrive on [`AsrClient::events`]; a reader thread turns the worker's stdout
//! into [`AsrEvent`]s.

use std::io::{self, BufReader, BufWriter, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::protocol::{Final, LoadRequest, Reply, Request, Segment};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long a polite `Quit` gets before the worker is killed.
const QUIT_GRACE: Duration = Duration::from_millis(1_500);

#[derive(Clone, Debug, PartialEq)]
pub enum AsrEvent {
    Loaded {
        load_ms: u64,
    },
    Segment(Segment),
    Final(Final),
    Error(String),
    /// The worker ended without being asked to (a crash).
    Exited,
}

struct Running {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    reader: JoinHandle<()>,
    /// Set before an intentional shutdown so the reader stays quiet.
    expected_exit: Arc<AtomicBool>,
}

pub struct AsrClient {
    worker_exe: PathBuf,
    running: Option<Running>,
    events_tx: Sender<AsrEvent>,
    events_rx: Receiver<AsrEvent>,
}

/// `islet-asr-worker.exe` next to the running executable.
#[must_use]
pub fn default_worker_path() -> PathBuf {
    std::env::current_exe()
        .map(|exe| exe.with_file_name("islet-asr-worker.exe"))
        .unwrap_or_else(|_| PathBuf::from("islet-asr-worker.exe"))
}

impl AsrClient {
    #[must_use]
    pub fn new(worker_exe: impl Into<PathBuf>) -> Self {
        let (events_tx, events_rx) = unbounded();
        Self {
            worker_exe: worker_exe.into(),
            running: None,
            events_tx,
            events_rx,
        }
    }

    #[must_use]
    pub fn worker_exe(&self) -> &Path {
        &self.worker_exe
    }

    #[must_use]
    pub fn events(&self) -> &Receiver<AsrEvent> {
        &self.events_rx
    }

    /// Whether a worker process is alive (it may still be loading).
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Starts the worker if needed and asks it to load a model. The result
    /// arrives as [`AsrEvent::Loaded`] or [`AsrEvent::Error`].
    pub fn load(&mut self, request: LoadRequest) -> io::Result<()> {
        if self.running.is_none() {
            self.spawn()?;
        }
        self.send(&Request::Load(request))
    }

    pub fn begin(&mut self) -> io::Result<()> {
        self.send(&Request::Begin)
    }

    pub fn audio(&mut self, samples: &[f32]) -> io::Result<()> {
        self.send(&Request::Audio(samples.to_vec()))
    }

    pub fn end(&mut self) -> io::Result<()> {
        self.send(&Request::End)
    }

    pub fn cancel(&mut self) -> io::Result<()> {
        self.send(&Request::Cancel)
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> io::Result<()> {
        self.send(&Request::Transcribe(samples.to_vec()))
    }

    /// Ends the worker (unloading the model) and waits for it to exit.
    pub fn shutdown(&mut self) {
        let Some(mut running) = self.running.take() else {
            return;
        };
        running.expected_exit.store(true, Ordering::Release);
        let _ = Request::Quit.write_to(&mut running.stdin);
        drop(running.stdin);

        let deadline = Instant::now() + QUIT_GRACE;
        loop {
            match running.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                _ => {
                    let _ = running.child.kill();
                    let _ = running.child.wait();
                    break;
                }
            }
        }
        let _ = running.reader.join();
    }

    fn spawn(&mut self) -> io::Result<()> {
        let mut child = Command::new(&self.worker_exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no worker stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no worker stdout"))?;

        let expected_exit = Arc::new(AtomicBool::new(false));
        let reader_flag = Arc::clone(&expected_exit);
        let events = self.events_tx.clone();
        let reader = std::thread::Builder::new()
            .name("islet-asr-reader".to_owned())
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                loop {
                    let event = match Reply::read_from(&mut stdout) {
                        Ok(Some(Reply::Loaded(loaded))) => AsrEvent::Loaded {
                            load_ms: loaded.load_ms,
                        },
                        Ok(Some(Reply::Segment(segment))) => AsrEvent::Segment(segment),
                        Ok(Some(Reply::Final(done))) => AsrEvent::Final(done),
                        Ok(Some(Reply::Error(message))) => AsrEvent::Error(message),
                        Ok(None) | Err(_) => {
                            if !reader_flag.load(Ordering::Acquire) {
                                let _ = events.send(AsrEvent::Exited);
                            }
                            return;
                        }
                    };
                    if events.send(event).is_err() {
                        return;
                    }
                }
            })?;

        self.running = Some(Running {
            child,
            stdin: BufWriter::with_capacity(256 * 1024, stdin),
            reader,
            expected_exit,
        });
        Ok(())
    }

    fn send(&mut self, request: &Request) -> io::Result<()> {
        let Some(running) = self.running.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "speech worker is not running",
            ));
        };
        let result = request
            .write_to(&mut running.stdin)
            .and_then(|()| running.stdin.flush());
        if let Err(error) = result {
            // The pipe broke: the worker is gone. Reap it and report once.
            self.drop_dead_worker();
            return Err(error);
        }
        Ok(())
    }

    fn drop_dead_worker(&mut self) {
        if let Some(mut running) = self.running.take() {
            let _ = running.child.kill();
            let _ = running.child.wait();
            // The reader reports `Exited` itself when it sees the pipe close;
            // joining after the kill cannot block.
            let _ = running.reader.join();
        }
    }
}

impl Drop for AsrClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}
