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
    if let Err(e) = run_native(options) {
        crash::record(&format!("eframe::run_native failed: {e}"));
    }
}

fn run_native(options: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native(
        "ProCraft",
        options,
        Box::new(|cc| Ok(Box::new(ProCraftApp::new(cc)))),
    )
}

/// Desktop/iOS entry (the `[[bin]]` wrapper calls this).
pub fn run() {
    run_with(build_options());
}

/// Backend order for Android. Kept apart from `eframe::Renderer` because
/// only the Android build enables both variants; the host tests pin the
/// ordering.
#[cfg(any(target_os = "android", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    Wgpu,
    Glow,
}

/// Wgpu first, glow (GLES/EGL) as the fallback for drivers that cannot
/// create a wgpu surface. A recorded fallback starts on glow instead,
/// and still probes wgpu if glow itself fails (self-healing after a
/// driver update).
#[cfg(any(target_os = "android", test))]
pub(crate) fn backend_attempts(fallback_recorded: bool) -> &'static [Backend] {
    if fallback_recorded {
        &[Backend::Glow, Backend::Wgpu]
    } else {
        &[Backend::Wgpu, Backend::Glow]
    }
}

/// Marker naming the GLES fallback, inside the settings directory.
#[cfg(target_os = "android")]
const FALLBACK_FILE: &str = "renderer.txt";

#[cfg(target_os = "android")]
fn fallback_path() -> Option<std::path::PathBuf> {
    crate::settings::settings_dir().map(|dir| dir.join(FALLBACK_FILE))
}

/// True when wgpu already failed once and glow is the stored choice.
#[cfg(target_os = "android")]
pub(crate) fn renderer_fallback_active() -> bool {
    fallback_path().is_some_and(|path| path.exists())
}

#[cfg(target_os = "android")]
fn record_glow_fallback() {
    if let Some(path) = fallback_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, "glow\n");
    }
}

/// Forget the fallback so the next launch probes wgpu again.
#[cfg(target_os = "android")]
pub(crate) fn clear_renderer_fallback() {
    if let Some(path) = fallback_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Android entry: `NativeActivity` spawns this on its own thread (not
/// `main`); the `AndroidApp` handle is required for winit's event loop.
///
/// `run_and_return` keeps the winit event loop in eframe's thread-local
/// after a failed attempt, so the glow retry reuses it instead of trying
/// to build a second loop on the same thread.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub(crate) fn android_main(app: android_activity::AndroidApp) {
    if let Some(p) = app.internal_data_path() {
        crate::settings::set_android_data_dir(p);
    }
    crash::install();
    for backend in backend_attempts(renderer_fallback_active()) {
        let mut options = build_options();
        options.renderer = match backend {
            Backend::Wgpu => eframe::Renderer::Wgpu,
            Backend::Glow => eframe::Renderer::Glow,
        };
        options.run_and_return = true;
        options.android_app = Some(app.clone());
        match run_native(options) {
            Ok(()) => return,
            Err(e) => {
                crash::record(&format!("eframe::run_native failed with {backend:?}: {e}"));
                match backend {
                    // Later launches start on glow directly; About offers a
                    // retry of the hardware renderer.
                    Backend::Wgpu => record_glow_fallback(),
                    // The recorded choice failed (driver update, lost GLES
                    // support): probe wgpu first again next launch.
                    Backend::Glow => clear_renderer_fallback(),
                }
            }
        }
    }
}
