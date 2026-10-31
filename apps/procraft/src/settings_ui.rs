use crate::*;

impl ProCraftApp {
    /// M49: current fields as a Settings value (the shape written to
    /// settings.json). The live brush belongs to the active tool, so it is
    /// folded back into its slot on a copy before the slots are written.
    pub(crate) fn settings_snapshot_from_fields(&self) -> Settings {
        let mut slots = self.tool_brushes;
        slots.save(self.tool, &self.brush);
        let mut s = Settings {
            version: SETTINGS_VERSION,
            theme: self.theme.as_str().to_string(),
            show_tools: self.show_tools,
            show_layers: self.show_layers,
            zoom: self.zoom,
            recent: self.recent.clone(),
            shortcuts: self.shortcuts.to_stored(),
            strip_collapsed: self.strip_collapsed,
            strip_dock: self.strip_dock.as_str().to_string(),
            time_lapse: self.lapse_on,
            liquify_size: self.liquify_size,
            liquify_strength: self.liquify_strength,
            smudge_strength: self.smudge_strength,
            ..Settings::default()
        };
        slots.write_settings(&mut s);
        s
    }

    /// M49: adopt a loaded file. Unknown theme ids keep the current
    /// theme; a non-finite/positive zoom keeps the current zoom (legacy
    /// keys' semantics); shortcut-file warnings become the settings note.
    pub(crate) fn apply_settings(&mut self, s: &Settings) {
        if let Some(theme) = Theme::from_str(&s.theme) {
            self.theme = theme;
        }
        self.show_tools = s.show_tools;
        self.show_layers = s.show_layers;
        if s.zoom.is_finite() && s.zoom > 0.0 {
            self.zoom = s.zoom;
        }
        self.recent = s.recent.iter().take(MAX_RECENT).cloned().collect();
        self.strip_collapsed = s.strip_collapsed;
        self.strip_dock = StripDock::from_str(&s.strip_dock);
        self.lapse_on = s.time_lapse;
        // Tool dials are clamped here, not trusted from the file: the
        // sliders cannot produce out-of-range or non-finite values, so a
        // loaded one must not either. The live brush then adopts the
        // active tool's slot (Brush at startup).
        self.tool_brushes = ToolBrushes::from_settings(s);
        self.tool_brushes.load(self.tool, &mut self.brush);
        self.liquify_size = clamp_dial(
            s.liquify_size,
            LIQUIFY_SIZE_MIN,
            LIQUIFY_SIZE_MAX,
            DEFAULT_LIQUIFY_SIZE,
        );
        self.liquify_strength = clamp_dial(s.liquify_strength, 0.05, 1.0, DEFAULT_LIQUIFY_STRENGTH);
        self.smudge_strength = clamp_dial(s.smudge_strength, 0.0, 1.0, DEFAULT_SMUDGE_STRENGTH);
        let (shortcuts, mut warns) = Shortcuts::from_stored(&s.shortcuts);
        self.shortcuts = shortcuts;
        if s.version > SETTINGS_VERSION {
            warns.insert(
                0,
                format!(
                    "Settings file is newer (v{}); it will be saved as v{}.",
                    s.version, SETTINGS_VERSION
                ),
            );
        }
        if !warns.is_empty() {
            self.settings_note = Some(warns.join(" "));
        }
    }

    /// M49: write settings now (no-op when the platform gave us no dir).
    pub(crate) fn save_settings_file(&self, s: &Settings) -> Result<(), String> {
        match &self.settings_dir {
            Some(dir) => crate::settings::save_now(dir, s),
            None => Ok(()),
        }
    }

    /// M49: once-per-frame settings sync. Discrete edits (theme, panels,
    /// recents, shortcuts) save immediately; the zoom level and the
    /// per-tool brush dials coalesce and only land once
    /// `should_flush_coalesced` says the pointer went quiet. A failed write
    /// leaves the edit pending and is retried after a short cooldown, so a
    /// transient disk error never silently drops a change.
    pub(crate) fn settings_sync(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let cur = self.settings_snapshot_from_fields();
        if coalesced_differ(&cur, &self.settings_snapshot) {
            self.coalesced_dirty = true;
            self.coalesced_changed_at = now;
            // Advance the coalesced fields so the discrete differ below
            // stays quiet; they are rewritten with everything else when
            // the quiet period passes.
            self.settings_snapshot.zoom = cur.zoom;
            self.settings_snapshot.brush = cur.brush;
            self.settings_snapshot.eraser = cur.eraser;
            self.settings_snapshot.smudge = cur.smudge;
            self.settings_snapshot.liquify_size = cur.liquify_size;
            self.settings_snapshot.liquify_strength = cur.liquify_strength;
            self.settings_snapshot.smudge_strength = cur.smudge_strength;
        }
        let retry_ok = match self.save_failed_at {
            None => true,
            Some(t) => now - t >= SAVE_RETRY_SECS,
        };
        if settings_differ(&cur, &self.settings_snapshot) {
            if retry_ok {
                match self.save_settings_file(&cur) {
                    Ok(()) => {
                        self.settings_snapshot = cur.clone();
                        self.coalesced_dirty = false; // snapshot refreshed wholesale
                        self.save_note = None;
                        self.save_failed_at = None;
                    }
                    Err(err) => {
                        self.save_failed_at = Some(now);
                        self.note_save_failure(err);
                    }
                }
            }
        } else if crate::settings::should_flush_coalesced(
            self.coalesced_dirty,
            self.coalesced_changed_at,
            now,
        ) && retry_ok
        {
            match self.save_settings_file(&cur) {
                Ok(()) => {
                    self.settings_snapshot = cur;
                    self.coalesced_dirty = false;
                    self.save_note = None;
                    self.save_failed_at = None;
                }
                Err(err) => {
                    self.save_failed_at = Some(now);
                    self.note_save_failure(err);
                }
            }
        }
    }

    /// M49: surface a failed settings write once via the status line and
    /// the settings note; the edit stays pending and is retried.
    pub(crate) fn note_save_failure(&mut self, err: String) {
        let note = format!("Couldn't save settings ({err}); will retry.");
        if self.save_note.is_none() {
            self.status = note.clone();
        }
        self.save_note = Some(note);
    }

    /// M49: flush immediately when anything is pending (clean exit and
    /// iOS backgrounding — no timing gate here). A failure keeps the
    /// edit pending so the next frame re-attempts it.
    pub(crate) fn flush_settings_now(&mut self) {
        let cur = self.settings_snapshot_from_fields();
        if settings_differ(&cur, &self.settings_snapshot) || self.coalesced_dirty {
            match self.save_settings_file(&cur) {
                Ok(()) => {
                    self.settings_snapshot = cur;
                    self.coalesced_dirty = false;
                    self.save_note = None;
                    self.save_failed_at = None;
                }
                Err(err) => {
                    self.save_failed_at = None; // retry promptly next frame
                    self.note_save_failure(err);
                }
            }
        }
    }
}
