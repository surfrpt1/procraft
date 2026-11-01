use crate::*;

/// Window icon: original layer-stack mark drawn in VectorCraft
/// (see `assets/icon-generator.py`, rendered to `assets/icon-48.png`).
pub(crate) fn app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../../../assets/icon-48.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

pub(crate) fn build_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1400.0, 900.0]);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    // Disk persistence for window rect, panel widths (by panel id), and
    // the UI state saved in `App::save` (theme, panels, zoom). Mobile
    // keeps it off (same as iOS: sandboxed, session-only).
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let persistence_path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .map(|mut dir| {
            dir.push("procraft");
            let _ = std::fs::create_dir_all(&dir);
            dir.push("ui.json");
            dir
        });
    #[cfg(any(target_os = "ios", target_os = "android"))]
    let persistence_path = None;
    eframe::NativeOptions {
        viewport,
        persistence_path,
        ..Default::default()
    }
}

pub(crate) fn run_with(options: eframe::NativeOptions) {
    // Before eframe: a cold-start panic must leave a trail (logcat on
    // Android, crash_report.log on every platform).
    crash::install();
    let started = eframe::run_native(
        "ProCraft",
        options,
        Box::new(|cc| Ok(Box::new(ProCraftApp::new(cc)))),
    );
    if let Err(e) = started {
        crash::record(&format!("eframe::run_native failed: {e}"));
    }
}

/// Desktop/iOS entry (the `[[bin]]` wrapper calls this).
pub fn run() {
    run_with(build_options());
}

/// Android entry: `NativeActivity` spawns this on its own thread (not
/// `main`); the `AndroidApp` handle is required for winit's event loop.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub(crate) fn android_main(app: android_activity::AndroidApp) {
    if let Some(p) = app.internal_data_path() {
        crate::settings::set_android_data_dir(p);
    }
    let mut options = build_options();
    options.android_app = Some(app);
    run_with(options);
}
