//! M49 settings persistence: per-platform file location, atomic save,
//! corrupt-file quarantine, and the one-time seed policy. All IO lives
//! here so `lib.rs` only wires it; the pure decision logic is pinned by
//! host tests.

use procraft_engine::Settings;

/// eframe storage key recording that the one-time seed already happened.
pub const MARKER_KEY: &str = "m49_seeded";

/// File name inside the settings directory.
pub const SETTINGS_FILE: &str = "settings.json";

/// Quiet period before a coalesced zoom write is flushed (seconds).
const ZOOM_FLUSH_SECS: f64 = 1.5;

/// How the app should initialize its settings this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inits {
    /// A settings.json exists: load it (parse errors quarantine + default).
    LoadFile,
    /// No file, no marker: first run since M49 — seed from legacy UI state.
    SeedLegacy,
    /// No file, marker set: already seeded once ever — plain defaults.
    UseDefaults,
}

/// Seed-once policy: the marker (set the first time we seed) stops a
/// later missing/corrupt file from resurrecting legacy UI state forever.
pub fn init_action(file_exists: bool, marker_set: bool) -> Inits {
    if file_exists {
        Inits::LoadFile
    } else if marker_set {
        Inits::UseDefaults
    } else {
        Inits::SeedLegacy
    }
}

/// Coalesced zoom write gate: dirty and quiet for the whole period.
pub fn should_flush_zoom(dirty: bool, last_change: f64, now: f64) -> bool {
    dirty && now - last_change >= ZOOM_FLUSH_SECS
}

/// Platform settings directory (created lazily on save). Never `None`:
/// a device without the usual env vars or an Activity data dir (cold
/// boot before JNI hands it over, stripped Android images) falls back to
/// the temp dir so preferences degrade instead of silently vanishing.
pub fn settings_dir() -> Option<std::path::PathBuf> {
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
            .map(|mut dir| {
                dir.push("procraft");
                dir
            })
            .or_else(|| Some(temp_fallback()))
    }
    #[cfg(target_os = "android")]
    {
        ANDROID_DATA_DIR
            .get()
            .cloned()
            .or_else(|| Some(temp_fallback()))
    }
    #[cfg(target_os = "ios")]
    {
        std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|mut dir| {
                dir.push("Documents");
                dir
            })
            .or_else(|| Some(temp_fallback()))
    }
}

/// Last-resort settings root, with the reason logged so a lost path is
/// visible in `logcat`/`stderr` rather than silent.
fn temp_fallback() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("procraft");
    eprintln!(
        "ProCraft: no platform settings directory; using {}",
        dir.display()
    );
    dir
}

#[cfg(target_os = "android")]
static ANDROID_DATA_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Android: record the Activity's internal data dir (first call wins;
/// `android_main` hands it over once at startup).
#[cfg(target_os = "android")]
pub fn set_android_data_dir(p: std::path::PathBuf) {
    let _ = ANDROID_DATA_DIR.set(p);
}

/// The settings file's full path inside `dir`.
pub fn settings_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(SETTINGS_FILE)
}

/// Read + parse the settings file; Err is a human-readable io/parse note.
pub fn read(dir: &std::path::Path) -> Result<Settings, String> {
    let text = std::fs::read_to_string(settings_path(dir)).map_err(|e| e.to_string())?;
    Settings::from_json(&text)
}

/// Save by writing `settings.json.tmp` then renaming over the real file
/// (std rename replaces an existing file on both Windows and Unix).
pub fn save_now(dir: &std::path::Path, s: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = settings_path(dir);
    let tmp = path.with_file_name(format!("{SETTINGS_FILE}.tmp"));
    std::fs::write(&tmp, s.to_json_pretty()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Move a corrupt settings.json aside to `settings.json.bad` (replacing
/// any older quarantine) so the next save starts clean. Returns the
/// quarantine path, or None when the file could not be moved.
pub fn quarantine(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = settings_path(dir);
    let bad = path.with_file_name(format!("{SETTINGS_FILE}.bad"));
    let _ = std::fs::remove_file(&bad);
    std::fs::rename(&path, &bad).ok().map(|_| bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_action_covers_the_three_ways_in() {
        assert_eq!(init_action(true, false), Inits::LoadFile);
        assert_eq!(init_action(true, true), Inits::LoadFile);
        assert_eq!(init_action(false, false), Inits::SeedLegacy);
        assert_eq!(init_action(false, true), Inits::UseDefaults); // marker: seed once, ever
    }

    #[test]
    fn zoom_flushes_only_after_the_quiet_period() {
        assert!(!should_flush_zoom(false, 100.0, 200.0));
        assert!(!should_flush_zoom(true, 100.0, 101.0));
        assert!(should_flush_zoom(true, 100.0, 101.5));
        assert!(should_flush_zoom(true, 100.0, 130.0));
    }

    #[test]
    fn settings_path_is_the_app_dir() {
        let d = std::path::Path::new("base");
        assert_eq!(settings_path(d), d.join("settings.json"));
        assert_eq!(SETTINGS_FILE, "settings.json");
        assert_eq!(MARKER_KEY, "m49_seeded");
    }

    #[test]
    fn settings_dir_never_returns_none() {
        // A device with no APPDATA/HOME and no Activity data dir still
        // gets a usable root so preferences degrade instead of vanishing.
        assert!(settings_dir().is_some());
    }

    #[test]
    fn save_now_round_trips_through_a_sibling_temp_file() {
        let dir = std::env::temp_dir().join(format!("procraft-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings {
            zoom: 2.5,
            ..Settings::default()
        };
        save_now(&dir, &s).unwrap();
        assert!(dir.join(SETTINGS_FILE).is_file());
        // The temp file is renamed over the real one, not left behind.
        assert!(!dir.join(format!("{SETTINGS_FILE}.tmp")).exists());
        assert_eq!(read(&dir).unwrap().zoom, 2.5);
        // Quarantine moves the file aside to a sibling `.bad`.
        let bad = quarantine(&dir).unwrap();
        assert_eq!(
            bad.file_name().unwrap().to_string_lossy(),
            format!("{SETTINGS_FILE}.bad")
        );
        assert!(!dir.join(SETTINGS_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
