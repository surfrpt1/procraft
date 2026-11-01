//! Cold-start crash trail (issue #2): a panic before the first frame
//! leaves nothing on a phone's screen and no console on desktop, so the
//! report goes to stderr (logcat) and an append-only `crash_report.log`
//! beside the settings file. The hook is installed before eframe starts.

use std::fmt::Write as _;

/// Report text for one failure: message, source location, backtrace.
fn format_report(payload: &str, location: Option<&str>, backtrace: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "ProCraft failure");
    let _ = writeln!(out, "  message: {payload}");
    let _ = writeln!(out, "  at: {}", location.unwrap_or("<startup>"));
    if !backtrace.is_empty() {
        let _ = writeln!(out, "  backtrace:");
        let _ = writeln!(out, "{backtrace}");
    }
    out
}

/// Cap before the current log is rotated aside, so a crash loop cannot
/// fill the device's storage.
const MAX_LOG_BYTES: u64 = 128 * 1024;

/// Append a report to `crash_report.log` in the settings directory. Best
/// effort: a missing directory or unwritable disk must never turn a
/// diagnosable failure into a second one.
fn append_log(report: &str) {
    use std::io::Write as _;
    let Some(dir) = crate::settings::settings_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("crash_report.log");
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
    if too_big {
        let old = dir.join("crash_report.old.log");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&path, &old);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(report.as_bytes());
    }
}

/// Install the panic hook once, at process start. The log location is
/// resolved when the panic fires, so on Android it lands in the Activity
/// data dir that `android_main` records just before eframe starts.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let report = format_report(payload, location.as_deref(), &backtrace);
        eprint!("{report}");
        append_log(&report);
        previous(info);
    }));
}

/// Log a non-panic startup failure (backend init, window creation) to the
/// same sink as panics.
pub fn record(message: &str) {
    let report = format_report(message, None, "");
    eprint!("{report}");
    append_log(&report);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_carries_message_location_and_backtrace() {
        let text = format_report("boom", Some("src/lib.rs:12:3"), " 0: frame");
        assert!(text.contains("message: boom"));
        assert!(text.contains("at: src/lib.rs:12:3"));
        assert!(text.contains("backtrace:\n 0: frame"));
        // Startup failures have no location: they still produce a report.
        let startup = format_report("no display", None, "");
        assert!(startup.contains("at: <startup>"));
        assert!(!startup.contains("backtrace"));
    }
}
