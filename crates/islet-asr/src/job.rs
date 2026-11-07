//! Ties Onyx's helper processes (the speech worker, the AI cleanup server,
//! the file converter) to Onyx's own lifetime.
//!
//! A normal quit stops them itself, but if Onyx is killed (a crash, Task
//! Manager, an installer update) a helper would keep running with nobody to
//! stop it: the AI server alone holds about 180 MB until the PC restarts.
//! Each helper is put in one Windows job object that kills everything in it
//! when its last handle closes, which is when Onyx's process ends.
//!
//! Only helpers join: apps the assistant opens (Notepad, Spotify...) are
//! never in the job, so they stay open after Onyx quits.

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::sync::OnceLock;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// The job's handle, kept open for the whole life of the process (it is
/// never closed: the system closes it when the process ends).
static JOB: OnceLock<Option<usize>> = OnceLock::new();

fn job() -> Option<HANDLE> {
    let handle = JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls; the limit structure outlives the call.
        unsafe {
            let job = CreateJobObjectW(None, None).ok()?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .ok()?;
            Some(job.0 as usize)
        }
    });
    handle.map(|raw| HANDLE(raw as *mut _))
}

/// Makes `child` end when this process ends. Best effort: if Windows
/// refuses, the helper still runs and a normal quit still stops it.
pub fn tie_to_app(child: &Child) {
    let Some(job) = job() else {
        tracing::warn!("could not create the helper job object");
        return;
    };
    let process = HANDLE(child.as_raw_handle());
    // SAFETY: both handles are valid for the duration of the call.
    if unsafe { AssignProcessToJobObject(job, process) }.is_err() {
        tracing::warn!("could not tie a helper process to Onyx");
    }
}
