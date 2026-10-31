//! ProCraft file dialogs with the `rfd` 0.17 call shape the app uses.
//!
//! - Desktop: this crate is not compiled (real `rfd` is used instead).
//! - Android: declining shape only, until the Storage Access bridge lands.
//! - iOS: `rfd` has no backend, so this crate backs the same calls with a
//!   `UIDocumentPickerViewController` bridge. Opens use Import mode (single
//!   tap picks; the system copies into the sandbox, so no access claims are
//!   needed) and resolve ASYNCHRONOUSLY: `pick_file` presents and returns
//!   immediately, the app polls [`poll_pick`] each frame. Blocking the main
//!   thread for the result is deliberately avoided (watchdog kills a wedged
//!   UI). Saves resolve into the sandboxed Documents folder and the app
//!   presents the share sheet afterwards for the real destination.

#[cfg(any(target_os = "ios", target_os = "android"))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(target_os = "ios", target_os = "android"))]
use std::sync::Mutex;

/// Why the last dialog call produced no path. Mobile saves have no cancel
/// button in this shim, so `None` always means the sandbox path could not
/// be resolved and the app should show this reason instead of silence.
#[cfg(any(target_os = "ios", target_os = "android"))]
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[cfg(any(target_os = "ios", target_os = "android"))]
fn set_error(reason: Option<String>) {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = reason;
    }
}

/// Reason the last [`FileDialog`] call returned `None`, cleared by the
/// next successful call. `None` means a plain user cancel.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().ok().and_then(|slot| slot.clone())
}

/// Builder matching the `rfd::FileDialog` calls in `apps/procraft`.
#[derive(Clone, Debug, Default)]
pub struct FileDialog {
    filters: Vec<String>,
}

impl FileDialog {
    /// Fresh dialog builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept a file-type filter (becomes picker document types on iOS).
    pub fn add_filter(mut self, _name: &str, extensions: &[&str]) -> Self {
        self.filters
            .extend(extensions.iter().map(|e| e.to_ascii_lowercase()));
        self
    }

    /// Open dialog. Presents the picker and returns immediately (`None`);
    /// the result arrives via [`poll_pick`]. Presenting twice while one is
    /// pending keeps the first request.
    pub fn pick_file(self) -> Option<PathBuf> {
        #[cfg(target_os = "ios")]
        {
            ios_bridge::present_import(&self.filters);
            None
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = &self.filters;
            None
        }
    }

    /// Save dialog. No native panel: resolves to a uniquified path in the
    /// sandboxed Documents folder (iOS) or the external files dir with the
    /// first filter as extension (Android, so the media scan categorizes it).
    /// The app writes the bytes itself and then calls [`ios_share_file`] /
    /// [`android_scan_file`] for the real destination.
    pub fn save_file(self) -> Option<PathBuf> {
        #[cfg(target_os = "ios")]
        {
            let path = ios_bridge::documents_path(&unique_stem());
            set_error(
                path.is_none()
                    .then(|| "the app Documents folder is unavailable".to_string()),
            );
            path
        }
        #[cfg(target_os = "android")]
        {
            let dir = match android_bridge::external_files_dir() {
                Ok(dir) => dir,
                Err(reason) => {
                    set_error(Some(format!(
                        "{reason} (app storage: Android/data/com.surfrpt1.procraft/files/)"
                    )));
                    return None;
                }
            };
            set_error(None);
            let stem = unique_stem();
            let name = match self.filters.first() {
                Some(ext) => format!("{stem}.{ext}"),
                None => stem,
            };
            Some(dir.join(name))
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let _ = &self.filters;
            None
        }
    }
}

/// Asynchronous pick state for [`poll_pick`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickPoll {
    /// No request in flight.
    NoRequest,
    /// Picker on screen, waiting for the user.
    Waiting,
    /// User picked a file (sandboxed, readable).
    Picked(PathBuf),
    /// User cancelled (`None`), presentation failed, or the wait timed
    /// out (both with a reason) — surfaced in the status line so a dead
    /// picker reports WHY instead of vanishing silently.
    Cancelled(Option<&'static str>),
}

/// Check a pending pick. Call once per frame after `pick_file`.
pub fn poll_pick() -> PickPoll {
    #[cfg(target_os = "ios")]
    {
        ios_bridge::poll()
    }
    #[cfg(not(target_os = "ios"))]
    {
        PickPoll::NoRequest
    }
}

/// Present the iOS share sheet for an already-written file (call right after
/// saving, on the UI thread). Returns false when no window is available.
/// iOS only; the desktop `rfd` has no such function (call sites cfg-gate it).
#[cfg(target_os = "ios")]
pub fn ios_share_file(path: &Path) -> bool {
    ios_bridge::share_file(path)
}

/// External files dir for the in-app browser (call to list importables).
/// Android only; errors carry the reason for the status line.
#[cfg(target_os = "android")]
pub fn android_external_files() -> Result<PathBuf, &'static str> {
    android_bridge::external_files_dir()
}

/// Media-scan an already-written file (call right after saving).
/// Best-effort: false only means "not indexed", the file is still saved.
/// Android only; the desktop `rfd` has no such function (call sites cfg-gate it).
#[cfg(target_os = "android")]
pub fn android_scan_file(path: &Path) -> bool {
    android_bridge::scan_file(path)
}

/// Status-bar height in px for edge-to-edge padding (call once, cache).
/// Android only; errors carry the reason, callers fall back to 0.
#[cfg(target_os = "android")]
pub fn android_status_bar_height() -> Result<u32, &'static str> {
    android_bridge::status_bar_height()
}

/// Window safe-area insets (top, bottom) in points (call per frame; a cheap
/// property read, correct across rotation). iOS only; errors carry the
/// reason, callers fall back to 0.
#[cfg(target_os = "ios")]
pub fn ios_safe_area_insets() -> Result<(u32, u32), &'static str> {
    ios_bridge::safe_area_insets()
}

/// Timestamped stem so saves never silently overwrite.
#[cfg(any(target_os = "ios", target_os = "android"))]
fn unique_stem() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("ProCraft-{ms}")
}

#[cfg(target_os = "android")]
mod android_bridge;
#[cfg(target_os = "ios")]
mod ios_bridge;
