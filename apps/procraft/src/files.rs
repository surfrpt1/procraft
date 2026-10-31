use crate::*;

/// Android: accepted extensions per open kind (mirrors the desktop filters).
#[cfg(target_os = "android")]
pub(crate) fn pending_open_exts(kind: PendingOpen) -> &'static [&'static str] {
    match kind {
        PendingOpen::Image | PendingOpen::Reference | PendingOpen::ImagePalette => {
            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"]
        }
        PendingOpen::Project => &["procraft"],
        PendingOpen::Psd => &["psd"],
        PendingOpen::Font => &["ttf", "otf"],
        PendingOpen::Brushes => BRUSH_IMPORT_EXTS,
        PendingOpen::Palette => &["pcpalette", "json", "gpl"],
        PendingOpen::DualTip => TIP_IMPORT_EXTS,
        PendingOpen::Gif => &["gif"],
    }
}

/// Android: importables for `kind` plus a note when the dir is unreadable.
/// Returns `(files, note)`: `note` is `Some` only when listing failed.
#[cfg(target_os = "android")]
pub(crate) fn android_import_list(kind: PendingOpen) -> (Vec<std::path::PathBuf>, Option<String>) {
    let exts = pending_open_exts(kind);
    let dir = match rfd::android_external_files() {
        Ok(d) => d,
        Err(reason) => return (Vec::new(), Some(format!("Files unavailable: {reason}."))),
    };
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file() && ext_matches(p.extension().and_then(|e| e.to_str()), exts)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    (out, None)
}

/// Latest Apple Pencil / touch pressure (0.0..=1.0) seen in this frame's
/// input events, if the backend forwarded any. egui 0.36 exposes pressure
/// only on `Event::Touch.force` — desktop mice always yield `None`, and
/// tilt is not exposed at all (documented M14 limitation).
pub(crate) fn latest_touch_force(ctx: &egui::Context) -> Option<f32> {
    ctx.input(|i| {
        let mut force = None;
        for e in &i.events {
            if let egui::Event::Touch {
                force: f, phase, ..
            } = e
            {
                if *phase == egui::TouchPhase::End || *phase == egui::TouchPhase::Cancel {
                    continue;
                }
                if let Some(v) = f {
                    force = Some(v.clamp(0.0, 1.0));
                }
            }
        }
        force
    })
}

/// Mobile open: what to load once the picker resolves (iOS document
/// picker) or the in-app browser picks (Android external files).
#[cfg(any(target_os = "ios", target_os = "android"))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingOpen {
    Image,
    Project,
    Psd,
    Brushes,
    Palette,
    ImagePalette,
    Gif,
    Reference,
    Font,
    DualTip,
}

/// Append `.ext` when the chosen path has none (rfd doesn't force it).
pub(crate) fn ensure_extension(mut path: std::path::PathBuf, ext: &str) -> std::path::PathBuf {
    if path.extension().is_none() {
        path.set_extension(ext);
    }
    path
}

/// Mobile save dialogs never cancel: `None` means the sandbox path could
/// not be resolved (storage unmounted, no external files dir), so the tap
/// must explain itself. Desktop `None` is a plain dialog cancel and stays
/// quiet.
pub(crate) fn note_save_declined(status: &mut String, what: &str) {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        if let Some(reason) = rfd::last_error() {
            *status = format!("{what} save failed: {reason}");
        }
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (status, what);
    }
}

/// M33 Gallery: how many recent projects to remember (newest first).
pub(crate) const MAX_RECENT: usize = 10;

/// M33 Gallery: thumbnail longest side in px.
pub(crate) const GALLERY_THUMB_SIDE: u32 = 96;

/// Gallery new-canvas presets: (label, width, height). Every side stays
/// within [`MAX_CANVAS_SIDE`]; the 8K entry is the square export target.
pub(crate) const CANVAS_PRESETS: [(&str, u32, u32); 3] = [
    ("1080p", 1920, 1080),
    ("4K", 4096, 4096),
    ("8K", 8192, 8192),
];

/// M33 Gallery: record a project path as most-recent (dedup, truncate).
pub(crate) fn push_recent(recent: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    recent.retain(|p| p != path);
    recent.insert(0, path.to_string());
    recent.truncate(MAX_RECENT);
}

/// M49: cooldown before retrying a failed settings write (seconds).
pub(crate) const SAVE_RETRY_SECS: f64 = 2.0;

/// M49: what makes two settings snapshots differ in the discrete sense.
/// Zoom is excluded — it coalesces through `should_flush_zoom` and gets
/// written by its own quiet-period path.
pub(crate) fn settings_differ(a: &Settings, b: &Settings) -> bool {
    a.version != b.version
        || a.theme != b.theme
        || a.show_tools != b.show_tools
        || a.show_layers != b.show_layers
        || a.recent != b.recent
        || a.shortcuts != b.shortcuts
        || a.strip_collapsed != b.strip_collapsed
        || a.strip_dock != b.strip_dock
        || a.time_lapse != b.time_lapse
}

pub(crate) fn page_presets() -> Vec<(&'static str, [u8; 3])> {
    vec![
        ("White", [255, 255, 255]),
        ("Black", [0, 0, 0]),
        ("Gray", [128, 128, 128]),
        ("Warm", [240, 230, 210]),
        ("Blue", [52, 120, 200]),
    ]
}
