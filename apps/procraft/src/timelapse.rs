use crate::*;

impl ProCraftApp {
    /// Time-lapse card above the Animation group: the recording switch sits
    /// one tap from the Layers panel instead of three taps deep inside the
    /// group (plan 12). Recording is on by default.
    pub(crate) fn lapse_card(&mut self, ui: &mut egui::Ui, t: &design::Tokens) {
        let on = self.lapse_on;
        let frames = self.lapse_frames.len();
        let mut toggle = false;
        let mut export = false;
        let mut clear = false;
        design::section_card_actions(
            ui,
            t,
            Some("Time-lapse"),
            false,
            |ui| {
                if design::pill_button(
                    ui,
                    t,
                    design::PillSpec {
                        icon: None,
                        glyph: None,
                        label: Some(if on { "On" } else { "Off" }),
                        selected: on,
                        enabled: true,
                    },
                )
                .clicked()
                {
                    toggle = true;
                }
            },
            |ui| {
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    if on {
                        ui.painter().circle_filled(dot.center(), 4.5, t.danger);
                    } else {
                        ui.painter().circle_stroke(
                            dot.center(),
                            4.5,
                            egui::Stroke::new(1.0, t.text_lo),
                        );
                    }
                    ui.label(if frames == 0 {
                        "Starts with your first stroke".to_string()
                    } else {
                        format!("{frames} frames recorded")
                    });
                });
                if frames > 0 {
                    ui.horizontal(|ui| {
                        if ui.button("Export lapse").clicked() {
                            export = true;
                        }
                        if ui.button("Clear").clicked() {
                            clear = true;
                        }
                    });
                }
            },
        );
        if toggle {
            self.lapse_on = !on;
            self.status = if self.lapse_on {
                "Time-lapse recording on.".to_string()
            } else {
                "Time-lapse recording off.".to_string()
            };
        }
        if export {
            self.export_lapse();
        }
        if clear {
            self.lapse_frames.clear();
            self.status = "Time-lapse cleared.".to_string();
        }
    }

    /// Save the staged time-lapse frames as a GIF.
    pub(crate) fn export_lapse(&mut self) {
        if self.lapse_frames.is_empty() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("GIF image", &["gif"])
            .save_file()
        {
            let path = ensure_extension(path, "gif");
            match export_timelapse(&self.lapse_frames, 10) {
                Ok(bytes) => match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        self.status = format!(
                            "Time-lapse saved {} ({} frames).",
                            path.display(),
                            self.lapse_frames.len()
                        );
                        #[cfg(target_os = "ios")]
                        rfd::ios_share_file(&path);
                        #[cfg(target_os = "android")]
                        rfd::android_scan_file(&path);
                    }
                    Err(e) => self.status = format!("Time-lapse save failed: {e}"),
                },
                Err(e) => self.status = format!("Time-lapse export failed: {e}"),
            }
        } else {
            note_save_declined(&mut self.status, "Time-lapse");
        }
    }

    /// Export the visible layers as a GIF.
    pub(crate) fn gif_export_ui(&mut self, ui: &mut egui::Ui) {
        if !ui.button("Export GIF").clicked() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("GIF image", &["gif"])
            .save_file()
        {
            let path = ensure_extension(path, "gif");
            match export_gif_ext(&self.doc, self.anim_fps, self.anim_loop) {
                Ok(bytes) => match std::fs::write(&path, &bytes) {
                    Ok(()) => {
                        let shown = self.doc.layers.iter().filter(|l| l.visible).count();
                        let exported = if self.anim_loop == LoopMode::PingPong && shown > 2 {
                            2 * shown - 2
                        } else {
                            shown
                        };
                        let held = self.doc.layers.iter().filter(|l| l.hold > 1).count();
                        let extra = match (self.anim_loop, held) {
                            (LoopMode::PingPong, 0) => ", ping-pong".to_string(),
                            (LoopMode::PingPong, _) => format!(", ping-pong, {held} held"),
                            (LoopMode::Forward, 0) => String::new(),
                            (LoopMode::Forward, _) => format!(", {held} held"),
                        };
                        self.status = format!(
                            "GIF saved {} ({} frames, hidden skipped{extra}).",
                            path.display(),
                            exported
                        );
                        #[cfg(target_os = "ios")]
                        rfd::ios_share_file(&path);
                        #[cfg(target_os = "android")]
                        rfd::android_scan_file(&path);
                    }
                    Err(e) => self.status = format!("GIF save failed: {e}"),
                },
                Err(e) => self.status = format!("GIF export failed: {e}"),
            }
        } else {
            note_save_declined(&mut self.status, "GIF");
        }
    }

    /// Import a GIF as the document (one layer per frame).
    pub(crate) fn gif_import_ui(&mut self, ui: &mut egui::Ui) {
        if !ui.button("Import GIF").clicked() {
            return;
        }
        #[cfg(not(target_os = "ios"))]
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("GIF image", &["gif"])
            .pick_file()
        {
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| import_gif_document(&b))
            {
                Ok((doc, fps)) => {
                    self.status = format!(
                        "GIF imported {} ({} frames, {} fps).",
                        path.display(),
                        doc.layers.len(),
                        fps
                    );
                    self.adopt_gif(doc, fps);
                }
                Err(e) => self.status = format!("GIF import failed: {e}"),
            }
        }
        #[cfg(target_os = "ios")]
        {
            let _ = rfd::FileDialog::new()
                .add_filter("GIF image", &["gif"])
                .pick_file();
            self.pending_open = Some(PendingOpen::Gif);
            self.status = "Pick GIF…".to_string();
        }
        #[cfg(target_os = "android")]
        {
            self.android_browse = Some(PendingOpen::Gif);
        }
    }

    /// M41 Time-lapse: stage a downscaled composite snapshot while
    /// recording. The min-interval gate plus the frame cap bound a
    /// session to small thumbs (ring-evicted past the cap); callers
    /// invoke this at every commit point and it stays cheap otherwise.
    /// Session-only — staged frames never enter the project file.
    pub(crate) fn capture_timelapse(&mut self) {
        if !self.lapse_on || self.lapse_last.elapsed() < LAPSE_MIN_DT {
            return;
        }
        self.lapse_last = std::time::Instant::now();
        let (bytes, tw, th) = sampled_thumb(&self.doc, LAPSE_SIDE);
        if bytes.is_empty() {
            return;
        }
        if self.lapse_frames.len() >= MAX_LAPSE_FRAMES {
            self.lapse_frames.remove(0);
        }
        self.lapse_frames.push((bytes, tw as u32, th as u32));
    }

    /// Adopt an imported GIF as the document (one layer per frame, active
    /// on frame 1): enables Assist and resets the view, ghosts and
    /// playback so the first frame shows at once.
    pub(crate) fn adopt_gif(&mut self, doc: Document, fps: u32) {
        self.doc = doc;
        self.clear_recovery();
        self.anim_fps = fps;
        self.anim_assist = true;
        self.anim_playing = false;
        self.anim_fwd = true;
        self.anim_shown = None;
        self.ghost_dirty = true;
        self.texture = None;
        self.composite.invalidate();
        self.lasso_points.clear();
        self.lasso_poly.clear();
    }
}

/// M41 Time-lapse: minimum time between staged snapshots (with the
/// frame cap, this bounds a session to ~180 small thumbs, not gigabytes).
pub(crate) const LAPSE_MIN_DT: std::time::Duration = std::time::Duration::from_secs(10);
