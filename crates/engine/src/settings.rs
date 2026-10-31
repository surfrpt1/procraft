//! App-wide persisted settings envelope: versioned, serde-based, with
//! forgiving parsing (missing fields default, unknown fields ignored).
//! Pure data (no GUI types).

use serde::{Deserialize, Serialize};

use crate::brush::Brush;
use crate::shortcuts::StoredShortcuts;

/// Schema version written into every settings file.
pub const SETTINGS_VERSION: u32 = 1;

/// Defaults for the dials that have no brush slot behind them. The app
/// seeds its live fields from these so stored and in-memory defaults agree.
pub const DEFAULT_LIQUIFY_SIZE: f32 = 128.0;
pub const DEFAULT_LIQUIFY_STRENGTH: f32 = 0.5;
pub const DEFAULT_SMUDGE_STRENGTH: f32 = 0.5;

/// Per-tool brush values that survive a relaunch. Mirrors the app's
/// runtime brush slots (size/opacity/softness only; spacing, pressure,
/// velocity, taper and the stabilizer stay shared between tools).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolBrushSettings {
    pub size: f32,
    pub opacity: f32,
    pub softness: f32,
}

impl Default for ToolBrushSettings {
    fn default() -> Self {
        // Stay in lockstep with a fresh brush so the first run and a
        // missing file land on the same values.
        let brush = Brush::default();
        ToolBrushSettings {
            size: brush.size,
            opacity: brush.opacity,
            softness: brush.softness,
        }
    }
}

/// Everything ProCraft persists across runs. `#[serde(default)]` makes
/// every field optional on read (missing -> default) and unknown fields
/// are ignored, so older/newer files stay loadable.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version (see SETTINGS_VERSION).
    pub version: u32,
    /// Theme id ("dark", "light", "paper").
    pub theme: String,
    /// View-menu toggles.
    pub show_tools: bool,
    pub show_layers: bool,
    /// Persisted canvas zoom (clamped by the app on load).
    pub zoom: f32,
    /// Recent files, newest first.
    pub recent: Vec<String>,
    /// Sparse shortcut overrides (only non-default entries).
    pub shortcuts: StoredShortcuts,
    /// Phone strip: collapsed to its pocket pill, and which bottom third it
    /// docks to ("left", "center", "right").
    pub strip_collapsed: bool,
    pub strip_dock: String,
    /// Time-lapse recording; on by default so a new canvas stages snapshots
    /// from the first stroke.
    pub time_lapse: bool,
    /// Per-tool brush slots (clamped by the app on load).
    pub brush: ToolBrushSettings,
    pub eraser: ToolBrushSettings,
    pub smudge: ToolBrushSettings,
    /// Liquify and Smudge dials (clamped by the app on load).
    pub liquify_size: f32,
    pub liquify_strength: f32,
    pub smudge_strength: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            theme: "dark".into(),
            show_tools: true,
            show_layers: true,
            zoom: 1.0,
            recent: Vec::new(),
            shortcuts: StoredShortcuts::default(),
            strip_collapsed: false,
            strip_dock: "center".into(),
            time_lapse: true,
            brush: ToolBrushSettings::default(),
            eraser: ToolBrushSettings::default(),
            smudge: ToolBrushSettings::default(),
            liquify_size: DEFAULT_LIQUIFY_SIZE,
            liquify_strength: DEFAULT_LIQUIFY_STRENGTH,
            smudge_strength: DEFAULT_SMUDGE_STRENGTH,
        }
    }
}

impl Settings {
    /// Parse a settings JSON document; Err is a human-readable message.
    pub fn from_json(s: &str) -> Result<Settings, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }

    /// Pretty JSON for on-disk storage (empty string only if serde fails,
    /// which cannot happen for these plain types).
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_pinned() {
        let s = Settings::default();
        assert_eq!(s.version, 1);
        assert_eq!(s.theme, "dark");
        assert!(s.show_tools && s.show_layers);
        assert_eq!(s.zoom, 1.0);
        assert!(s.recent.is_empty());
        assert_eq!(
            s.shortcuts,
            crate::shortcuts::Shortcuts::default().to_stored()
        );
        assert!(!s.strip_collapsed);
        assert_eq!(s.strip_dock, "center");
        // Time-lapse is Procreate's default: record from the first stroke.
        assert!(s.time_lapse);
        // Tool slots follow a fresh brush; the free dials keep their old
        // in-memory defaults.
        let brush = Brush::default();
        for slot in [s.brush, s.eraser, s.smudge] {
            assert_eq!(slot.size, brush.size);
            assert_eq!(slot.opacity, brush.opacity);
            assert_eq!(slot.softness, brush.softness);
        }
        assert_eq!(s.liquify_size, 128.0);
        assert_eq!(s.liquify_strength, 0.5);
        assert_eq!(s.smudge_strength, 0.5);
    }

    #[test]
    fn json_round_trip_is_lossless() {
        let s = Settings {
            theme: "dark".into(),
            show_tools: false,
            zoom: 2.5,
            recent: vec!["a.procraft".into(), "b.procraft".into()],
            strip_collapsed: true,
            strip_dock: "left".into(),
            time_lapse: false,
            brush: ToolBrushSettings {
                size: 64.0,
                opacity: 0.75,
                softness: 0.4,
            },
            eraser: ToolBrushSettings {
                size: 8.0,
                opacity: 1.0,
                softness: 0.0,
            },
            liquify_size: 256.0,
            liquify_strength: 0.9,
            smudge_strength: 0.25,
            ..Settings::default()
        };
        let s2 = Settings::from_json(&s.to_json_pretty()).unwrap();
        assert_eq!(s2, s);
    }

    #[test]
    fn partial_and_extra_json_fall_back_to_defaults() {
        let s = Settings::from_json(r#"{"theme":"light","future_field":42}"#).unwrap();
        assert_eq!(s.theme, "light");
        assert_eq!(s.zoom, 1.0); // missing field -> default
        assert!(s.time_lapse); // older files keep recording
        assert!(!s.shortcuts.bindings.contains_key("undo")); // none customized
                                                             // Files from before brush persistence start from a fresh brush.
        assert_eq!(s.brush, ToolBrushSettings::default());
        assert_eq!(s.liquify_size, DEFAULT_LIQUIFY_SIZE);
        assert_eq!(s.smudge_strength, DEFAULT_SMUDGE_STRENGTH);
        // A partially written slot keeps the defaults for its missing keys.
        let partial = Settings::from_json(r#"{"brush":{"size":64}}"#).unwrap();
        assert_eq!(partial.brush.size, 64.0);
        assert_eq!(partial.brush.opacity, ToolBrushSettings::default().opacity);
        assert_eq!(
            partial.brush.softness,
            ToolBrushSettings::default().softness
        );
    }

    #[test]
    fn wrong_types_are_a_clean_error() {
        assert!(Settings::from_json(r#"{"version":"one"}"#).is_err());
        assert!(Settings::from_json("not json").is_err());
    }
}
