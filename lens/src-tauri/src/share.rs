//! Share (free): hands a selection's PNG to the Windows share sheet via
//! `IDataTransferManagerInterop` — the Win32-side interop for
//! `DataTransferManager`, since the UWP-only
//! `DataTransferManager::GetForCurrentView()` needs a `CoreWindow` this app
//! doesn't have.
//!
//! Anchored to a small, ordinary window created just for this
//! (`ensure_share_anchor_window`), not the overlay itself. Lifting the
//! overlay's `skipTaskbar` style alone wasn't enough to make
//! `ShowShareUIForWindow` display anything — the overlay is transparent,
//! always-on-top, frameless *and* skip-taskbar all at once, and Windows
//! Shell surfaces are documented to be fussy about several of those
//! individually. Rather than keep guessing which exact combination is the
//! blocker, this anchors to a plain top-level window with none of them,
//! which removes every one of those variables at once.
//!
//! Per Microsoft's own desktop-interop samples, both `GetForWindow` and
//! `ShowShareUIForWindow` must run on the thread that owns the window's
//! message loop — the same Win32 affinity rule `pin.rs`/`show_overlay`
//! already work around, so this reuses the exact async-command +
//! `tokio::oneshot` bridge `pin.rs` settled on after its own deadlock
//! lesson (see its module doc): awaiting a oneshot yields the calling task
//! instead of blocking whatever thread actually runs this command.
//!
//! `ShareSession` holds onto the `DataTransferManager` this obtains for as
//! long as the app runs, rather than dropping it once the command returns.
//! `ShowShareUIForWindow` keeps its native flyout open, and the
//! `DataRequested` event it eventually fires arrives well after this
//! function has already returned — nothing here plays until then. Whether
//! `GetForWindow` hands back an instance the OS itself keeps alive for the
//! window's lifetime isn't something to gamble on from outside; keeping our
//! own strong reference removes that risk for one extra field. For the same
//! reason, the anchor window itself is never closed from this side — the
//! flyout is a Shell-owned popup that may well be an owned child of it, so
//! tearing the anchor down while a share is still in progress risks taking
//! the flyout with it. It's an ordinary window with a close button; the
//! user closes it when they're done.

use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use windows::core::HSTRING;
use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Foundation::Collections::IVectorView;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference};
use windows::Storage::{IStorageItem, StorageFolder};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IDataTransferManagerInterop;

use crate::commands::{decode_selection_png, PIN_MAX_IMAGE_BYTES};

const SHARE_ANCHOR_LABEL: &str = "share-anchor";

/// Keeps the previous share's `DataTransferManager` alive, plus the token of
/// the handler registered on it. Sound to store despite
/// `DataTransferManager` not being `Send`/`Sync` (a raw COM pointer with no
/// marshaling): it is only ever touched inside `share_via_windows`, which
/// always runs on the main thread that obtained it — other threads only
/// move the box around, never call through it.
struct Registration {
    manager: DataTransferManager,
    token: EventRegistrationToken,
}
unsafe impl Send for Registration {}
unsafe impl Sync for Registration {}

#[derive(Default)]
pub struct ShareSession(Mutex<Option<Registration>>);

/// The shared image as a real file, wrapped as the collection
/// `SetStorageItemsReadOnly` wants. Sound to move across threads: it comes
/// from `StorageFolder::GetItemsAsync`, whose results are free-threaded
/// (agile) — it's built on a blocking worker and only ever read back on the
/// main thread.
struct ShareFiles(IVectorView<IStorageItem>);
unsafe impl Send for ShareFiles {}
unsafe impl Sync for ShareFiles {}

/// Many share targets don't accept a raw bitmap at all — WhatsApp's own
/// manifest, for one, declares only files, URLs and text — so a share that
/// offers just `SetBitmap` silently filters them out of the picker. Writes
/// the PNG into its own fresh folder and lists that folder: that listing is
/// the one place windows-rs 0.58 gives a ready-made `IVectorView<IStorageItem>`
/// (building one by hand needs `windows-collections`, which pulls in an
/// incompatible `windows-core`). The previous share's folder is removed first
/// — surely done being read by the time a new share starts.
fn prepare_share_files(png_bytes: &[u8]) -> Result<ShareFiles, String> {
    let root = std::env::temp_dir().join("circle-search-share");
    let _ = std::fs::remove_dir_all(&root);
    let dir = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).map_err(|err| format!("Could not create the share folder: {err}"))?;
    std::fs::write(dir.join("Onyx Lens selection.png"), png_bytes)
        .map_err(|err| format!("Could not write the shared file: {err}"))?;

    let folder = StorageFolder::GetFolderFromPathAsync(&HSTRING::from(&*dir.to_string_lossy()))
        .and_then(|op| op.get())
        .map_err(|err| format!("Could not open the share folder: {err}"))?;
    let items = folder
        .GetItemsAsyncOverloadDefaultStartAndCount()
        .and_then(|op| op.get())
        .map_err(|err| format!("Could not list the shared file: {err}"))?;
    Ok(ShareFiles(items))
}

#[tauri::command]
pub async fn share_selection(app: AppHandle, image_data_url: String) -> Result<(), String> {
    let png_bytes = decode_selection_png(&image_data_url, PIN_MAX_IMAGE_BYTES)?;

    // Off the main thread: file I/O plus two blocking WinRT waits. A failure
    // here only costs the file-based targets (WhatsApp and the like), the
    // bitmap share still goes ahead.
    let bytes_for_files = png_bytes.clone();
    let files = match tokio::task::spawn_blocking(move || prepare_share_files(&bytes_for_files)).await {
        Ok(Ok(files)) => Some(files),
        Ok(Err(err)) => {
            tracing::warn!(%err, "sharing as a bitmap only; file-based targets won't be offered");
            None
        }
        Err(err) => {
            tracing::warn!(%err, "share file preparation panicked; sharing as a bitmap only");
            None
        }
    };

    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let app_for_main = app.clone();
    app.run_on_main_thread(move || {
        let outcome = share_via_windows(&app_for_main, png_bytes, files);
        if let Err(err) = &outcome {
            tracing::error!(%err, "share failed");
        }
        let _ = result_tx.send(outcome);
    })
    .map_err(|err| format!("Could not schedule the share request: {err}"))?;

    result_rx.await.map_err(|_| "The share request never responded".to_string())?
}

/// Reuses an already-open anchor window (bringing it forward) rather than
/// opening a second one on a later share — same reasoning as `pin.rs`'s
/// `get_webview_window` lookups.
fn ensure_share_anchor_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(SHARE_ANCHOR_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(window);
    }
    let window = WebviewWindowBuilder::new(app, SHARE_ANCHOR_LABEL, WebviewUrl::App("share.html".into()))
        .title("Share")
        .inner_size(380.0, 180.0)
        .resizable(false)
        .center()
        .build()
        .map_err(|err| format!("Could not open the share window: {err}"))?;
    let _ = window.set_focus();
    Ok(window)
}

/// The actual interop call — split out from the command so the
/// `run_on_main_thread` closure above stays a one-liner and this can be read
/// (and eventually tested) as an ordinary function of its inputs.
fn share_via_windows(app: &AppHandle, png_bytes: Vec<u8>, files: Option<ShareFiles>) -> Result<(), String> {
    let window = ensure_share_anchor_window(app)?;
    // Same version-drift unwrap/rewrap as pin.rs's own hwnd() call (see its
    // Cargo.toml comment on the Share features) — tauri's own `windows`
    // dependency has drifted to a newer minor version than this crate's
    // direct one, so `Window::hwnd()` returns a nominally different HWND
    // type despite being identical in layout.
    let hwnd = window.hwnd().map_err(|err| format!("Could not reach the share window: {err}"))?;
    let hwnd = HWND(hwnd.0);

    let interop: IDataTransferManagerInterop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()
        .map_err(|err| format!("Could not reach the Windows share system: {err}"))?;
    let manager: DataTransferManager =
        unsafe { interop.GetForWindow(hwnd) }.map_err(|err| format!("Could not attach sharing to this window: {err}"))?;

    // GetDeferral + Complete is the documented contract for DataRequested —
    // taking one even though this fill is synchronous so the request can't
    // be considered abandoned before SetBitmap lands, which otherwise risks
    // the picker proceeding with an empty package and showing no (or far
    // fewer) matching targets.
    let handler = TypedEventHandler::new(
        move |_sender: &Option<DataTransferManager>, args: &Option<DataRequestedEventArgs>| -> windows::core::Result<()> {
            let Some(args) = args else { return Ok(()) };
            let request = args.Request()?;
            let deferral = request.GetDeferral()?;
            // A failure here only ever reaches this log — WinRT has no
            // channel back to the Tauri command, which already returned
            // once ShowShareUIForWindow was called.
            let fill = (|| -> windows::core::Result<()> {
                let package = request.Data()?;
                package.Properties()?.SetTitle(&HSTRING::from("Onyx Lens"))?;

                let stream = InMemoryRandomAccessStream::new()?;
                let writer = DataWriter::CreateDataWriter(&stream)?;
                writer.WriteBytes(&png_bytes)?;
                writer.StoreAsync()?.get()?;
                stream.Seek(0)?;
                package.SetBitmap(&RandomAccessStreamReference::CreateFromStream(&stream)?)?;
                if let Some(files) = &files {
                    package.SetStorageItemsReadOnly(&files.0)?;
                }
                Ok(())
            })();
            if let Err(err) = &fill {
                tracing::error!(%err, "could not fill the share request");
            }
            deferral.Complete()?;
            fill
        },
    );
    // GetForWindow hands back the same manager for the same window every
    // time, so without this each Share click would stack another handler
    // and a later share could fire an earlier click's image.
    let mut session = app.state::<ShareSession>().inner().0.lock().unwrap();
    if let Some(previous) = session.take() {
        let _ = previous.manager.RemoveDataRequested(previous.token);
    }
    let token = manager
        .DataRequested(&handler)
        .map_err(|err| format!("Could not prepare the shared image: {err}"))?;
    *session = Some(Registration { manager, token });
    drop(session);

    unsafe { interop.ShowShareUIForWindow(hwnd) }.map_err(|err| format!("Could not open the share menu: {err}"))
}
