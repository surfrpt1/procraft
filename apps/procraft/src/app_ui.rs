use crate::*;

impl eframe::App for ProCraftApp {
    /// Runs on clean exit and iOS backgrounding: flush any pending
    /// settings write (settings.json is the source of truth since M49;
    /// window rect and panel widths still persist via eframe), then drop
    /// the recovery file — anything still on disk next launch means the
    /// session actually crashed. (iOS caveat: background suspension also
    /// clears it, so a later kill loses the tail — the 120 s cadence
    /// bounds that window.)
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.flush_settings_now();
        self.clear_recovery();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Advance a taper replay from the last lift. It runs before input
        // so its dabs reach this frame's composite, and it stops if
        // anything else touches the document.
        if let Some(job) = self.taper_job.as_mut() {
            match taper_step(&mut self.doc, job, &self.stroke_record) {
                TaperStep::More => ui.ctx().request_repaint(),
                TaperStep::Done => {
                    self.taper_job = None;
                    self.doc.end_stroke();
                    self.stroke_record.reset();
                    if std::mem::take(&mut self.taper_capture) {
                        self.capture_timelapse();
                    }
                }
                TaperStep::Cancelled => {
                    self.taper_job = None;
                    self.stroke_record.reset();
                    self.taper_capture = false;
                }
            }
        }
        let class = effective_layout(
            ui.ctx().viewport_rect().width(),
            cfg!(any(target_os = "ios", target_os = "android")),
        );
        let shell = if mobile_shell() {
            Shell::Mobile
        } else {
            Shell::Desktop
        };
        if self.theme_applied != Some((self.theme, class)) {
            let t = design::tokens(self.theme, class);
            // egui 0.36 keeps two style slots (dark/light) plus a
            // preference; it has no single active style to overwrite.
            // Dark maps to the dark slot, Light and Paper share the light
            // slot, and the preference is pinned so an OS theme change
            // can't swap in an unstyled slot mid-session.
            let slot = if self.theme == Theme::Dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            };
            let base = ui.ctx().style_of(slot);
            ui.ctx().set_style_of(slot, t.apply(&base));
            ui.ctx().set_theme(if self.theme == Theme::Dark {
                egui::ThemePreference::Dark
            } else {
                egui::ThemePreference::Light
            });
            self.theme_applied = Some((self.theme, class));
        }
        // Mobile surfaces are edge-to-edge, so the canvas would otherwise
        // paint under the status bar/notch. Reserve that strip at the root:
        // every later panel and the central canvas shrink from it, while
        // `mobile_chrome` anchors to the viewport and offsets by it too.
        if shell == Shell::Mobile {
            let inset = self.status_inset_top();
            if inset > 0.0 {
                egui::Panel::top("mobile-safe-top")
                    .resizable(false)
                    .exact_size(inset)
                    .frame(egui::Frame::NONE.fill(ui.visuals().panel_fill))
                    .show(ui, |_| {});
            }
        }
        // M49: the seed marker lands here — eframe 0.36's
        // `CreationContext::storage` is read-only, `Frame::storage_mut`
        // is the writable handle (None on mobile: the marker never
        // exists there, but a file, once created, always takes the
        // LoadFile branch, so mobile restore works regardless).
        if self.seed_marker_pending {
            if let Some(storage) = frame.storage_mut() {
                storage.set_string(crate::settings::MARKER_KEY, "1".to_string());
                self.seed_marker_pending = false;
            }
        }
        // M49: first thing each frame — sync the previous frame's edits.
        self.settings_sync(ui.ctx());
        // M49: remappable shortcuts through the exact-chord table
        // (Ctrl+Shift+Z is redo again), and Esc — unbound, typing-guarded
        // — clearing the selection.
        self.handle_capture(ui.ctx());
        self.dispatch_shortcuts(ui.ctx());
        self.handle_space_hand(ui.ctx());
        if !ui.ctx().text_edit_focused()
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            if let Some(sd) = self.size_drag.take() {
                // M49: an in-flight size drag reverts to its pinned base
                // (Esc means "undo this gesture" before anything else).
                self.brush.size = sd.base;
                self.painting = false;
                self.last = None;
                self.status = "Size reverted.".to_string();
            } else {
                self.doc.clear_selection();
                self.sel_anchor = None;
                self.sel_current = None;
                self.crop_anchor = None;
                self.crop_current = None;
                self.crop_rect = None;
                self.lasso_points.clear();
                self.lasso_poly.clear();
                self.sel_edges_dirty = true;
                // Armed transform handles roll back: the pins snap back to
                // the gesture start and, if a live preview session exists,
                // the pixels follow (recomputed from its base).
                let had_xform = self.xform.grab.is_some() || self.xform.base.is_some();
                self.xform.cancel_grab();
                if had_xform && !self.text_paint_blocked() {
                    xform_preview(&mut self.doc, &mut self.xform);
                }
            }
        }
        // Multi-finger taps (touch): two fingers = undo, three = redo.
        // A tap is a quick clean release with no travel; pans and pinches
        // set `moved` and never fire. Single-finger taps never arm this.
        {
            let now = ui.ctx().input(|i| i.time);
            let mt = ui.ctx().input(|i| i.multi_touch());
            if let Some(m) = mt {
                if let Some(t) = self.tap_track.as_mut() {
                    t.n = t.n.max(m.num_touches);
                    if (m.center_pos - t.c0).length() > 12.0 {
                        t.moved = true;
                    }
                } else {
                    self.tap_track = Some(TapTrack {
                        n: m.num_touches,
                        t0: now,
                        c0: m.center_pos,
                        moved: false,
                    });
                }
            } else if let Some(t) = self.tap_track.take() {
                let any_down = ui.ctx().input(|i| i.pointer.any_down());
                // M47: classification lives in the tested engine helper —
                // a palm landing mid-stroke (`painting`) never fires.
                if !any_down {
                    match classify_touch_tap(t.n, now - t.t0, t.moved, self.painting) {
                        TapAction::Undo if self.doc.undo() => {
                            self.ghost_dirty = true;
                            self.status = "Undone.".to_string();
                        }
                        TapAction::Redo if self.doc.redo() => {
                            self.ghost_dirty = true;
                            self.status = "Redone.".to_string();
                        }
                        _ => {}
                    }
                }
            }
        }
        // Autosave: silent snapshot every 120 s. Encoding an 8K document
        // deflates every tile (~600 ms), which used to freeze the frame it
        // landed on, so the encode runs on a worker thread over a cheap
        // copy-on-write clone and only the write happens here. Failures
        // surface once in the status line.
        {
            let now = ui.ctx().input(|i| i.time);
            if let Some(job) = self.autosave_job.take() {
                match job.try_recv() {
                    Ok(Ok(bytes)) => {
                        if let Err(e) = std::fs::write(&self.recovery_path, bytes) {
                            self.status = format!("Autosave failed: {e}");
                        }
                    }
                    Ok(Err(e)) => self.status = format!("Autosave failed: {e}"),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        self.autosave_job = Some(job);
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(200));
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                }
            }
            if now - self.last_autosave >= AUTOSAVE_SECS && self.autosave_job.is_none() {
                self.last_autosave = now;
                let doc = self.doc.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(doc.save_project_bytes());
                });
                self.autosave_job = Some(rx);
            }
            // eframe paints on demand, so an idle window would never reach
            // the tick above: wake once when the next snapshot is due.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    (self.last_autosave + AUTOSAVE_SECS - now).max(1.0),
                ));
        }
        // Keep frames coming while painting, selecting, lassoing, moving,
        // liquifying, shaping, or marching ants.
        if self.painting
            || self.sel_anchor.is_some()
            || self.crop_anchor.is_some()
            || !self.lasso_points.is_empty()
            || self.shape_anchor.is_some()
            || self.move_from.is_some()
            || self.liquify_from.is_some()
            || self.doc.selection.is_some()
            || self.doc.selection_mask.is_some()
        {
            ui.ctx().request_repaint();
        }
        // Animation playback: advance the current frame on an fps clock.
        // Each frame holds for its own tick count (per-frame hold).
        if self.anim_assist && self.anim_playing && !self.doc.layers.is_empty() {
            ui.ctx().request_repaint();
            let now = ui.ctx().input(|i| i.time);
            let fps = self.anim_fps.clamp(MIN_ANIM_FPS, MAX_ANIM_FPS);
            let hold = self
                .doc
                .layers
                .get(self.doc.active)
                .map(|l| l.hold)
                .unwrap_or(MIN_ANIM_HOLD)
                .clamp(MIN_ANIM_HOLD, MAX_ANIM_HOLD);
            if now - self.anim_last_t >= hold as f64 / fps as f64 {
                self.anim_last_t = now;
                // Visible frames only, so playback matches the GIF export.
                let (next, fwd) = step_playback_visible(
                    &self.doc,
                    self.doc.active,
                    self.anim_fwd,
                    self.anim_loop,
                );
                self.doc.active = next;
                self.anim_fwd = fwd;
            }
        }

        // One-time narrow-screen setup: portrait phones/tablets start
        // with both sidebars hidden so the canvas owns the screen.
        if !self.narrow_setup_done {
            self.narrow_setup_done = true;
            if ui.ctx().viewport_rect().width() < 900.0 {
                self.show_tools = false;
                self.show_layers = false;
            }
        }

        // iOS asynchronous opens: the picker was presented at tap time;
        // each frame checks whether it resolved.
        #[cfg(target_os = "ios")]
        if let Some(kind) = self.pending_open.take() {
            match rfd::poll_pick() {
                rfd::PickPoll::NoRequest | rfd::PickPoll::Waiting => {
                    self.pending_open = Some(kind);
                }
                rfd::PickPoll::Picked(path) => {
                    self.open_picked(kind, path);
                    // Deferred v4 bug: the foreground frame after the
                    // picker dismisses can drop before the rebuilt texture
                    // shows (content invisible until first tap). Force one
                    // more frame so opens appear at once.
                    ui.ctx().request_repaint();
                }
                rfd::PickPoll::Cancelled(reason) => {
                    // Reasoned cancels (no-main-thread, no window, timed
                    // out) diagnose a dead picker on device at a glance.
                    self.status = reason.unwrap_or("Pick cancelled.").to_string();
                }
            }
        }

        // Android file browser: external files dir (USB/`adb push` drop
        // point), filtered by the requested kind. Synchronous UI — there is
        // no system picker on NativeActivity. Picks reuse `open_picked`.
        // M46: scrollable rows with sizes + a count label (long USB drops
        // no longer overflow the window).
        #[cfg(target_os = "android")]
        if let Some(kind) = self.android_browse {
            let mut picked: Option<std::path::PathBuf> = None;
            let mut cancelled = false;
            let (files, dir_note) = android_import_list(kind);
            egui::Window::new("Import file")
                .collapsible(false)
                .resizable(true)
                .min_width(280.0)
                .show(ui.ctx(), |ui| {
                    ui.label("USB drop point: Android/data/com.surfrpt1.procraft/files");
                    if let Some(note) = dir_note {
                        ui.label(note);
                    } else if files.is_empty() {
                        ui.label("No matching files yet — copy some over USB first.");
                    } else {
                        ui.label(format!("{} file(s)", files.len()));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for path in &files {
                                let name = path
                                    .file_name()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let size = std::fs::metadata(path)
                                    .map(|m| file_size_label(m.len()))
                                    .unwrap_or_default();
                                let row = if size.is_empty() {
                                    name
                                } else {
                                    format!("{name} ({size})")
                                };
                                if ui.button(&row).clicked() {
                                    picked = Some(path.clone());
                                }
                            }
                        });
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            if let Some(path) = picked {
                self.android_browse = None;
                self.open_picked(kind, path);
                ui.ctx().request_repaint();
            } else if cancelled {
                self.android_browse = None;
                self.status = "Import cancelled.".to_string();
            }
        }

        // Top bar: browsers, File menu, direct edit buttons, zoom, fit,
        // rotate, then theme + About. All groups flow inline inside one
        // horizontal_wrapped so the bar folds to 2+ lines on narrow
        // screens instead of overlapping; the popups never clip like
        // the old button soup did.
        if shell == Shell::Desktop {
            egui::Panel::top("menu").show(ui, |ui| {
                // Edge-to-edge Android draws under the status bar (winit never
                // surfaces insets); pad once from the JNI readout, cached.
                #[cfg(target_os = "android")]
                {
                    if self.android_inset_top < 0.0 {
                        self.android_inset_top = rfd::android_status_bar_height()
                            .map(|px| clamp_status_inset(px))
                            .unwrap_or(0.0);
                    }
                    if self.android_inset_top > 0.0 {
                        ui.add_space(self.android_inset_top);
                    }
                }
                // Same edge-to-edge story on iOS: the wgpu surface draws under
                // the status bar and notch, so pad by the window's safe-area
                // top (read per frame; rotations change it).
                #[cfg(target_os = "ios")]
                {
                    let (top, _) = rfd::ios_safe_area_insets().unwrap_or((0, 0));
                    let pad = clamp_status_inset(top);
                    if pad > 0.0 {
                        ui.add_space(pad);
                    }
                }
                let layout = effective_layout(
                    ui.ctx().viewport_rect().width(),
                    cfg!(any(target_os = "ios", target_os = "android")),
                );
                let bar_t = design::tokens(self.theme, layout);
                if layout == BarLayout::Tablet {
                    ui.spacing_mut().interact_size.y = bar_t.control_h;
                    ui.spacing_mut().item_spacing = egui::vec2(bar_t.gap, bar_t.gap);
                }
                // Device pass 1: on touch layouts, tapping a stepper/slider inside
                // a menu closed it before the tap could land. CloseOnClickOutside
                // keeps inside taps alive while an outside tap still dismisses;
                // momentary action buttons call ui.close() themselves. Desktop
                // keeps egui's default CloseOnClick.
                let menu_config = || {
                    let close = if layout == BarLayout::Wide {
                        egui::PopupCloseBehavior::CloseOnClick
                    } else {
                        egui::PopupCloseBehavior::CloseOnClickOutside
                    };
                    egui::containers::menu::MenuConfig::new().close_behavior(close)
                };
                ui.horizontal_wrapped(|ui| {
                    // Desktop row budget: one row, always. The pills get their
                    // padding from the tokens; the stock File/zoom/combo buttons
                    // and the gaps between items get a tighter inset here so the
                    // full cluster fits the default 1120-point window. Touch
                    // keeps its finger-sized padding.
                    if layout == BarLayout::Wide {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.spacing_mut().button_padding.x = 6.0;
                    }
                    if self.pill(ui, "Gallery") {
                        self.gallery_open = !self.gallery_open;
                    }
                    if self.pill(ui, "Reference") {
                        self.ref_open = !self.ref_open;
                    }
                    egui::containers::menu::MenuButton::new("File")
                        .config(menu_config())
                        .ui(ui, |ui| {
                            self.file_menu_ui(ui, layout, &bar_t, None);
                        });
                    if self.pill(ui, "Undo") && self.doc.undo() {
                        self.status = "Undone.".to_string();
                    }
                    if self.pill(ui, "Redo") && self.doc.redo() {
                        self.status = "Redone.".to_string();
                    }
                    if self.pill(ui, "Clear layer") {
                        self.clear_layer();
                    }
                    // Phone and tablet fold the view cluster into a menu (the
                    // inline stepper starves to a character-wide column below
                    // ~600px); desktop keeps the exact inline layout and the
                    // theme pill sits just before About, inside `view_controls`.
                    // On touch the pill stays outside the fold — one tap from
                    // anywhere — right before the View menu.
                    if matches!(layout, BarLayout::Narrow | BarLayout::Tablet) {
                        if design::theme_toggle(ui, &bar_t, &mut self.theme) {
                            self.status = format!("{} theme.", self.theme.name());
                        }
                        egui::containers::menu::MenuButton::new("View")
                            .config(menu_config())
                            .ui(ui, |ui| {
                                if layout == BarLayout::Tablet {
                                    ui.spacing_mut().interact_size.y = bar_t.control_h;
                                }
                                self.view_controls(ui, true);
                            });
                    } else {
                        self.view_controls(ui, false);
                    }
                });
            });

            let tool_layout = effective_layout(
                ui.ctx().viewport_rect().width(),
                cfg!(any(target_os = "ios", target_os = "android")),
            );
            egui::Panel::top("tool-options").show(ui, |ui| {
                self.tool_options_bar(ui, tool_layout);
            });
        }

        // M33 Gallery: file browser (New / Open / Recent / Duplicate).
        // OS dialogs stay as the fallback path; recents persist in
        // ui.json (session-only on iOS, which has no disk persistence).
        if self.gallery_open {
            self.refresh_gallery_thumbs(ui.ctx());
            let mut open = self.gallery_open;
            let mut create = false;
            let mut browse = false;
            let mut open_idx: Option<usize> = None;
            let mut dup_idx: Option<usize> = None;
            let mut remove_idx: Option<usize> = None;
            let mut clear_all = false;
            let sheet = self.mobile_modal_frame(ui.ctx());
            let scrim = self.mobile_scrim(ui.ctx(), "mobile-scrim-gallery");
            fitted_modal(
                egui::Window::new("Gallery")
                    .open(&mut open)
                    .resizable(true)
                    .collapsible(false)
                    .min_width(280.0),
                sheet.as_ref(),
            )
            .show(ui.ctx(), |ui| {
                modal_scroll(ui, sheet.as_ref(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.gallery_new_w)
                                .range(1..=MAX_CANVAS_SIDE)
                                .prefix("W "),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.gallery_new_h)
                                .range(1..=MAX_CANVAS_SIDE)
                                .prefix("H "),
                        );
                        if ui.button("New canvas").clicked() {
                            create = true;
                        }
                        if ui.button("Reset").clicked() {
                            self.gallery_new_w = 1280;
                            self.gallery_new_h = 800;
                        }
                        if ui.button("Open…").clicked() {
                            browse = true;
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (label, w, h) in CANVAS_PRESETS {
                            if ui.button(label).clicked() {
                                self.gallery_new_w = w;
                                self.gallery_new_h = h;
                            }
                        }
                        ui.label(format!(
                            "8K: {} tiles/layer, painted tiles only",
                            (MAX_CANVAS_SIDE.div_ceil(TILE)).pow(2)
                        ));
                    });
                    ui.separator();
                    if self.recent.is_empty() {
                        ui.label("No recent projects yet — start one above or open a file.");
                    }
                    // Snapshot paths: row buttons mutate `recent` below.
                    for (idx, path) in self.recent.clone().iter().enumerate() {
                        ui.horizontal(|ui| {
                            if let Some(tex) = self.gallery_thumbs.get(idx).and_then(|t| t.as_ref())
                            {
                                let sz = tex.size();
                                let (w, h) = (sz[0].max(1) as f32, sz[1].max(1) as f32);
                                let s = 56.0 / w.max(h);
                                ui.image((tex.id(), egui::Vec2::new(w * s, h * s)));
                            } else {
                                let (r, _) = ui.allocate_exact_size(
                                    egui::Vec2::splat(56.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_stroke(
                                    r,
                                    4.0,
                                    ui.visuals().widgets.noninteractive.bg_stroke,
                                    egui::StrokeKind::Outside,
                                );
                            }
                            ui.vertical(|ui| {
                                let name = std::path::Path::new(path)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or(path);
                                ui.label(name);
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .add_sized([56.0, 28.0], egui::Button::new("Open"))
                                        .clicked()
                                    {
                                        open_idx = Some(idx);
                                    }
                                    if ui
                                        .add_sized([86.0, 28.0], egui::Button::new("Duplicate"))
                                        .clicked()
                                    {
                                        dup_idx = Some(idx);
                                    }
                                    let (del_w, del_t) = if self.has_symbols {
                                        (36.0, "✕")
                                    } else {
                                        (44.0, "Del")
                                    };
                                    if ui
                                        .add_sized([del_w, 28.0], egui::Button::new(del_t))
                                        .clicked()
                                    {
                                        remove_idx = Some(idx);
                                    }
                                });
                            });
                        });
                    }
                    if !self.recent.is_empty() && ui.button("Clear recents").clicked() {
                        clear_all = true;
                    }
                });
            });
            self.gallery_open = open && !scrim;
            if create {
                let (w, h) = (self.gallery_new_w, self.gallery_new_h);
                self.new_canvas(w, h);
                self.gallery_open = false;
            }
            if browse {
                #[cfg(not(target_os = "ios"))]
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("ProCraft project", &["procraft"])
                    .pick_file()
                {
                    match Document::open_project(&path) {
                        Ok(doc) => {
                            let s = path.to_string_lossy().to_string();
                            self.adopt_project(doc, &s, "Project opened");
                            self.gallery_open = false;
                        }
                        Err(e) => self.status = format!("Project open failed: {e}"),
                    }
                }
                #[cfg(target_os = "ios")]
                {
                    let _ = rfd::FileDialog::new()
                        .add_filter("ProCraft project", &["procraft"])
                        .pick_file();
                    self.pending_open = Some(PendingOpen::Project);
                    self.status = "Pick a project…".to_string();
                }
                #[cfg(target_os = "android")]
                {
                    self.android_browse = Some(PendingOpen::Project);
                }
            }
            if let Some(idx) = open_idx {
                if let Some(path) = self.recent.get(idx).cloned() {
                    match Document::open_project(std::path::Path::new(&path)) {
                        Ok(doc) => {
                            self.adopt_project(doc, &path, "Project opened");
                            self.gallery_open = false;
                        }
                        Err(e) => self.status = format!("Project open failed: {e}"),
                    }
                }
            }
            if let Some(idx) = dup_idx {
                self.duplicate_recent(idx);
                self.gallery_open = false;
            }
            if let Some(idx) = remove_idx {
                if idx < self.recent.len() {
                    self.recent.remove(idx);
                    self.gallery_keys.clear();
                }
            }
            if clear_all {
                self.recent.clear();
                self.gallery_keys.clear();
                self.gallery_thumbs.clear();
            }
        }

        // M34 Reference companion window: pinned image beside the canvas.
        // Session-only (not in the project file); sampling from it with
        // the Picker is deferred.
        if self.ref_open {
            self.refresh_ref_tex(ui.ctx());
            let mut open = self.ref_open;
            let mut load = false;
            let mut clear = false;
            let sheet = self.mobile_modal_frame(ui.ctx());
            let scrim = self.mobile_scrim(ui.ctx(), "mobile-scrim-reference");
            fitted_modal(
                egui::Window::new("Reference")
                    .open(&mut open)
                    .resizable(true)
                    .collapsible(false)
                    .min_width(200.0),
                sheet.as_ref(),
            )
            .show(ui.ctx(), |ui| {
                modal_scroll(ui, sheet.as_ref(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_sized([70.0, 28.0], egui::Button::new("Load…"))
                            .clicked()
                        {
                            load = true;
                        }
                        if self.ref_image.is_some()
                            && ui
                                .add_sized([70.0, 28.0], egui::Button::new("Clear"))
                                .clicked()
                        {
                            clear = true;
                        }
                    });
                    if let (Some(tex), Some((_, w, h))) =
                        (self.ref_tex.as_ref(), self.ref_image.as_ref())
                    {
                        // Fit the pin inside 320px, then zoom (slider).
                        let fit_px = (320.0 / (*w).max(*h).max(1) as f32).min(1.0);
                        let z = fit_px * self.ref_scale;
                        // The mobile frame is narrower than the 640px pin:
                        // cap the image to the dialog instead of clipping it.
                        let cap = sheet
                            .as_ref()
                            .map_or(640.0, |s| (s.fit.x - 56.0).min(s.fit.y - 160.0).max(120.0));
                        let dw = (*w as f32 * z).clamp(32.0, cap);
                        let dh = (*h as f32 * z).clamp(32.0, cap);
                        ui.image((tex.id(), egui::Vec2::new(dw, dh)));
                        stepped_slider(ui, &mut self.ref_scale, 0.25..=2.0, 0.25, "Zoom");
                    } else {
                        ui.label("Pin an image to paint from — nothing here yet.");
                    }
                });
            });
            self.ref_open = open && !scrim;
            if clear {
                self.ref_image = None;
                self.ref_tex = None;
                self.status = "Reference cleared.".to_string();
            }
            if load {
                #[cfg(not(target_os = "ios"))]
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        "Images",
                        &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
                    )
                    .pick_file()
                {
                    let from = format!("{}", path.display());
                    match image::open(&path).map(|i| i.to_rgba8()) {
                        Ok(img) => {
                            let (w, h) = (img.width(), img.height());
                            self.set_reference(img.into_raw(), w, h, &from);
                        }
                        Err(e) => self.status = format!("Reference load failed: {e}"),
                    }
                }
                #[cfg(target_os = "ios")]
                {
                    let _ = rfd::FileDialog::new()
                        .add_filter(
                            "Images",
                            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
                        )
                        .pick_file();
                    self.pending_open = Some(PendingOpen::Reference);
                    self.status = "Pick a reference image…".to_string();
                }
                #[cfg(target_os = "android")]
                {
                    self.android_browse = Some(PendingOpen::Reference);
                }
            }
        }

        // Recovery prompt: a previous session died with unsaved work.
        if self.recovery_offer {
            let mut open = self.recovery_offer;
            let sheet = self.mobile_modal_frame(ui.ctx());
            let scrim = self.mobile_scrim(ui.ctx(), "mobile-scrim-recovery");
            fitted_modal(
                egui::Window::new("Unsaved work found")
                    .open(&mut open)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .resizable(false)
                    .collapsible(false),
                sheet.as_ref(),
            )
            .show(ui.ctx(), |ui| {
                modal_scroll(ui, sheet.as_ref(), |ui| {
                    ui.label("The last session ended without saving. Recover the autosave?");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Recover").clicked() {
                            match Document::open_project(&self.recovery_path) {
                                Ok(doc) => {
                                    self.doc = doc;
                                    self.texture = None;
                                    self.composite.invalidate();
                                    self.ghost_dirty = true;
                                    self.anim_playing = false;
                                    self.anim_shown = None;
                                    self.lasso_points.clear();
                                    self.lasso_poly.clear();
                                    self.sel_edges_dirty = true;
                                    self.status = "Recovered autosave.".to_string();
                                }
                                Err(e) => {
                                    self.clear_recovery();
                                    self.status = format!("Recovery file was corrupt ({e}).");
                                }
                            }
                            self.recovery_offer = false;
                        }
                        if ui.button("Discard").clicked() {
                            self.clear_recovery();
                            self.recovery_offer = false;
                            self.status = "Recovery discarded.".to_string();
                        }
                    });
                });
            });
            // The X button snoozes (file kept, offered again next launch);
            // Recover/Discard already dismissed above and must stick.
            if !open || scrim {
                self.recovery_offer = false;
            }
        }

        // About box: version, builder, home page, updates. Version comes
        // from the workspace Cargo.toml at compile time, so release bumps
        // show up here with no code changes.
        if self.show_about {
            let mut open = self.show_about;
            let sheet = self.mobile_modal_frame(ui.ctx());
            let scrim = self.mobile_scrim(ui.ctx(), "mobile-scrim-about");
            fitted_modal(
                egui::Window::new("About ProCraft")
                    .open(&mut open)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .resizable(false)
                    .collapsible(false),
                sheet.as_ref(),
            )
            .show(ui.ctx(), |ui| {
                modal_scroll(ui, sheet.as_ref(), |ui| {
                    ui.heading("ProCraft");
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.label("Painting studio by surfrpt1.");
                    ui.hyperlink_to(
                        "github.com/surfrpt1/procraft",
                        "https://github.com/surfrpt1/procraft",
                    );
                    ui.separator();
                    ui.hyperlink_to(
                        "Check for updates",
                        "https://github.com/surfrpt1/procraft/releases",
                    );
                    ui.label("Original code, MIT licensed.");
                });
            });
            self.show_about = open && !scrim;
        }

        // M49: shortcuts editor window.
        if self.show_shortcuts {
            self.shortcuts_window(ui.ctx());
        }

        // Text dialog: type, style, then Render creates a text layer (or
        // Done re-renders the one being edited in place; one undo step).
        if self.text_open {
            let mut open = self.text_open;
            let sheet = self.mobile_modal_frame(ui.ctx());
            let scrim = self.mobile_scrim(ui.ctx(), "mobile-scrim-text");
            fitted_modal(
                egui::Window::new("Text")
                    .open(&mut open)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .resizable(true)
                    .collapsible(false),
                sheet.as_ref(),
            )
            .show(ui.ctx(), |ui| {
                // Scrollable: on small screens (iPad keyboard up) the
                // tall dialog clips Import/Render below the fold.
                let scroll_h = sheet
                    .as_ref()
                    .map_or(f32::INFINITY, |s| (s.fit.y - 96.0).max(160.0));
                egui::ScrollArea::vertical()
                    .max_height(scroll_h)
                    .show(ui, |ui| {
                        if self.editing_text.is_some() {
                            ui.label("Tap the canvas to move the text; edit, then Done.");
                        } else {
                            ui.label("Click the canvas to move the anchor, then Render.");
                        }
                        // Explicit focus like the layer rename: iOS shows no
                        // keyboard for an unfocused multiline field.
                        let field = ui.add(
                            egui::TextEdit::multiline(&mut self.text_body)
                                .desired_rows(3)
                                .hint_text("Type here (Enter = new line)"),
                        );
                        if !field.has_focus() {
                            field.request_focus();
                        }
                        stepped_slider(
                            ui,
                            &mut self.text_size,
                            MIN_TEXT_PX..=128.0,
                            1.0,
                            "Size px",
                        );
                        // M38 Typography: face + spacing + outline.
                        ui.horizontal_wrapped(|ui| {
                            let face = self.text_font_name.as_deref().unwrap_or(
                                if self.text_font_bytes.is_some() {
                                    "Imported"
                                } else {
                                    "Inter (bundled)"
                                },
                            );
                            ui.label(format!("Face: {face}"));
                            if ui
                                .add_sized([80.0, 28.0], egui::Button::new("Import…"))
                                .clicked()
                            {
                                #[cfg(not(target_os = "ios"))]
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Fonts", &["ttf", "otf"])
                                    .pick_file()
                                {
                                    match std::fs::read(&path) {
                                        Ok(bytes) => {
                                            let name = path
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("font")
                                                .to_string();
                                            self.status = self.absorb_font(&bytes, &name);
                                        }
                                        Err(e) => self.status = format!("Font import failed: {e}"),
                                    }
                                }
                                #[cfg(target_os = "ios")]
                                {
                                    let _ = rfd::FileDialog::new()
                                        .add_filter("Fonts", &["ttf", "otf"])
                                        .pick_file();
                                    self.pending_open = Some(PendingOpen::Font);
                                    self.status = "Pick a font…".to_string();
                                }
                                #[cfg(target_os = "android")]
                                {
                                    self.android_browse = Some(PendingOpen::Font);
                                }
                            }
                            if self.text_font_name.is_some()
                                && ui
                                    .add_sized([86.0, 28.0], egui::Button::new("Bundled"))
                                    .clicked()
                            {
                                self.text_font_name = None;
                                self.text_font_bytes = None;
                                self.status = "Back to Inter.".to_string();
                            }
                        });
                        stepped_slider(
                            ui,
                            &mut self.text_leading,
                            -MAX_TEXT_SPACING..=MAX_TEXT_SPACING,
                            1.0,
                            "Leading",
                        );
                        stepped_slider(
                            ui,
                            &mut self.text_tracking,
                            -MAX_TEXT_SPACING..=MAX_TEXT_SPACING,
                            1.0,
                            "Tracking",
                        );
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.text_outline, "Outline");
                            if self.text_outline {
                                stepped_slider(
                                    ui,
                                    &mut self.text_outline_w,
                                    1..=MAX_OUTLINE_PX,
                                    1.0,
                                    "px",
                                );
                            }
                        });
                        if self.text_outline {
                            ui.label("Outline color:");
                            stepped_slider(ui, &mut self.text_outline_rgb[0], 0..=255, 1.0, "R");
                            stepped_slider(ui, &mut self.text_outline_rgb[1], 0..=255, 1.0, "G");
                            stepped_slider(ui, &mut self.text_outline_rgb[2], 0..=255, 1.0, "B");
                        }
                        ui.horizontal_wrapped(|ui| {
                            let editing = self.editing_text.is_some();
                            if ui
                                .add_sized(
                                    [80.0, 28.0],
                                    egui::Button::new(if editing { "Done" } else { "Render" }),
                                )
                                .clicked()
                            {
                                let color = [self.color[0], self.color[1], self.color[2], 255];
                                match self.editing_text {
                                    Some(i) => {
                                        // Identity check: Done against a layer
                                        // that changed underneath (deleted,
                                        // reordered, undone) cancels rather
                                        // than corrupting.
                                        let unchanged = self.editing_loaded.as_ref().is_some_and(
                                            |(t, x, y)| {
                                                self.doc.layers.get(i).is_some_and(|l| {
                                                    l.text.as_ref().is_some_and(|b| {
                                                        *t == b.text && *x == b.x && *y == b.y
                                                    })
                                                })
                                            },
                                        );
                                        if !unchanged {
                                            self.status =
                                                "Text layer changed — edit cancelled.".to_string();
                                            self.text_open = false;
                                        } else {
                                            let (ax, ay) = self.text_anchor.unwrap_or_else(|| {
                                                self.editing_loaded
                                                    .as_ref()
                                                    .map_or((0.0, 0.0), |(_, x, y)| (*x, *y))
                                            });
                                            let prev = self.doc.layers[i].text.clone();
                                            // The checkpoint snapshots the ACTIVE
                                            // layer, and the dialog is floating —
                                            // active can drift (layer row, playback)
                                            // while it is open. Pin the edited layer
                                            // so undo restores the right one.
                                            self.doc.active = i;
                                            self.doc.checkpoint();
                                            let style = self.text_style();
                                            if let Some(b) = self.doc.layers[i].text.as_mut() {
                                                b.text = self.text_body.clone();
                                                b.size_px = self.text_size;
                                                b.color = color;
                                                b.style = style;
                                                b.x = ax;
                                                b.y = ay;
                                            }
                                            match self.doc.rerender_text_layer(i) {
                                                Ok((w, h)) => {
                                                    self.status =
                                                        format!("Text layer updated ({w}x{h}).");
                                                    self.text_open = false;
                                                    self.ghost_dirty = true;
                                                    self.capture_timelapse();
                                                }
                                                Err(e) => {
                                                    // The engine keeps the old
                                                    // pixels on a failed re-layout;
                                                    // restore the block too so a
                                                    // retry's identity check still
                                                    // matches.
                                                    self.doc.layers[i].text = prev;
                                                    self.status = format!("Text failed: {e}");
                                                }
                                            }
                                        }
                                    }
                                    None => match self.text_anchor {
                                        Some((ax, ay)) => {
                                            let style = self.text_style();
                                            let block = TextBlock {
                                                text: self.text_body.clone(),
                                                size_px: self.text_size,
                                                color,
                                                style,
                                                x: ax,
                                                y: ay,
                                                w: 0,
                                                h: 0,
                                            };
                                            let name =
                                                format!("Text {}", self.doc.layers.len() + 1);
                                            match self.doc.add_text_layer(&name, block) {
                                                Ok((_, w, h)) => {
                                                    self.status =
                                                        format!("Text layer added ({w}x{h}).");
                                                    self.text_open = false;
                                                    self.ghost_dirty = true;
                                                    self.capture_timelapse();
                                                }
                                                Err(e) => self.status = format!("Text failed: {e}"),
                                            }
                                        }
                                        None => {
                                            self.status =
                                                "Click the canvas to anchor the text first."
                                                    .to_string()
                                        }
                                    },
                                }
                            }
                            if ui
                                .add_sized([80.0, 28.0], egui::Button::new("Cancel"))
                                .clicked()
                            {
                                self.text_open = false;
                            }
                        });
                    });
            });
            // A button inside the dialog may have set `text_open = false`
            // (Render/Done/Cancel); the window ✕ flips `open`. Honor both —
            // and any close clears the edit session.
            self.text_open = self.text_open && open && !scrim;
            if !self.text_open {
                self.editing_text = None;
                self.editing_loaded = None;
            }
        }

        // Fresh id per width era ("tools" -> "tools-wide" -> here): panel
        // widths persist by id (see `persistence_path` in `main`), so a new
        // id starts at `default_size` while any saved width stays buried
        // under the old id. No max clamp:
        // the panel stays freely resizable, it just starts compact (swatch
        // grids wrap to ~3 rows, page gets the room).
        // Drag-to-collapse (phone fix): shrinking past the minimum closes
        // the panel instead of clamping; the ▶ strip reopens (drag_to_open
        // stays off so a stray edge touch never pops panels open).
        if shell == Shell::Desktop {
            let tools_was = self.show_tools;
            let mut tools_open = tools_was;
            egui::Panel::left("tools-compact")
                .default_size(240.0)
                .drag_to_open(false)
                .show_collapsible(ui, &mut tools_open, |ui| {
                    self.tools_panel_body(ui, false);
                });
            // Either the ◀ button (inside) or a drag past the minimum (panel)
            // can close; reopening is only via ▶/persistence, never a drag.
            self.show_tools = tools_open && self.show_tools;

            // Fresh id (not "layers"): panel widths persist to disk by id (see
            // `persistence_path` in `main`), and the old narrow width would
            // otherwise override `default_size` forever.
            // Same drag-to-collapse contract as the tools panel.
            let mut layers_open = self.show_layers;
            egui::Panel::right("layers-wide")
                .default_size(280.0)
                .drag_to_open(false)
                .show_collapsible(ui, &mut layers_open, |ui| {
                    self.layers_panel_body(ui);
                });
            self.show_layers = layers_open && self.show_layers;

            // Floating edge buttons reopen hidden sidebars (touch-sized).
            if !self.show_tools {
                egui::Area::new(egui::Id::new("tools-fab"))
                    .anchor(egui::Align2::LEFT_CENTER, [8.0, 0.0])
                    .show(ui.ctx(), |ui| {
                        let t = self.tokens(ui);
                        if ui
                            .add_sized([t.control_h, t.control_h], egui::Button::new("▶"))
                            .clicked()
                        {
                            self.show_tools = true;
                        }
                    });
            }
            if !self.show_layers {
                egui::Area::new(egui::Id::new("layers-fab"))
                    .anchor(egui::Align2::RIGHT_CENTER, [-8.0, 0.0])
                    .show(ui.ctx(), |ui| {
                        let t = self.tokens(ui);
                        if ui
                            .add_sized([t.control_h, t.control_h], egui::Button::new("◀"))
                            .clicked()
                        {
                            self.show_layers = true;
                        }
                    });
            }
        }

        // The mobile shell floats its status as a toast (Plan 06); until then
        // the row is simply gone so the canvas can own the bottom edge.
        if shell == Shell::Desktop {
            egui::Panel::bottom("status").show(ui, |ui| {
                ui.label(&self.status);
                // iOS: lift the label clear of the home indicator.
                #[cfg(target_os = "ios")]
                {
                    let (_, bottom) = rfd::ios_safe_area_insets().unwrap_or((0, 0));
                    let pad = clamp_status_inset(bottom);
                    if pad > 0.0 {
                        ui.add_space(pad);
                    }
                }
            });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();
            self.refresh_texture(&ctx);
            // Target B (spec §4): composite the visible rect instead of
            // sampling one canvas-sized texture. Decided once per frame;
            // `refresh_texture` already returned without an upload when set.
            let viewport = self.viewport_canvas(&ctx);
            let max_side = ctx.input(|i| i.max_texture_side);
            let (w, h) = (self.doc.w as f32, self.doc.h as f32);
            let avail = ui.available_size();
            // Viewport rect for press-origin tests (Hand grabs anywhere
            // in it, including paper margins outside the canvas rect).
            let view_rect = ui.available_rect_before_wrap();
            if self.zoom <= 0.0 {
                self.zoom = board_fit_zoom(w, h, avail);
            }
            // Pinch-to-zoom (iOS touch; Ctrl+wheel on desktop): no-op when
            // the backend reports no gesture.
            let pinch = ui.ctx().input(|i| i.zoom_delta());
            let fingers = ui.ctx().input(|i| i.multi_touch());
            let gesture = fingers.is_some();
            if (pinch - 1.0).abs() > f32::EPSILON {
                self.zoom = (self.zoom * pinch).clamp(0.1, 8.0);
            }
            // Two-finger drag pans the canvas. Touchscreens have no wheel
            // to scroll the view with, so the offset is applied manually
            // (desktop wheel-scroll leaves it at zero; a desktop trackpad
            // pinch could add to it, harmlessly — same direction).
            if let Some(m) = &fingers {
                let d = m.translation_delta;
                if d != egui::Vec2::ZERO {
                    let (cw, ch) = (w * self.zoom, h * self.zoom);
                    let (ox, oy) = pan_offset(
                        (self.view_offset.x, self.view_offset.y),
                        (d.x, d.y),
                        cw,
                        ch,
                        avail.x,
                        avail.y,
                    );
                    self.view_offset = egui::Vec2::new(ox, oy);
                }
                // Two-finger twist rotates the view about the canvas
                // center (paint math stays axis-aligned).
                if m.rotation_delta != 0.0 {
                    self.view_angle += m.rotation_delta;
                    if self.view_angle.abs() < 0.003 {
                        self.view_angle = 0.0;
                    }
                }
            }
            // A gesture aborts any in-progress stroke or drag so pinching
            // never paints, selects, moves, or grabs. A partially painted
            // stroke reverts (one undo step, from the stroke checkpoint).
            if gesture {
                if self.painting && self.stroke_painted {
                    self.stroke_painted = false;
                    if self.doc.undo() {
                        self.ghost_dirty = true;
                        self.status = "Pinch: stroke cancelled.".to_string();
                    }
                }
                self.painting = false;
                self.last = None;
                self.last_target = None;
                self.smooth_pt = None;
                self.stroke_curve.reset();
                self.assist_start = None;
                self.assist_lock = None;
                self.sel_anchor = None;
                self.sel_current = None;
                self.crop_anchor = None;
                self.crop_current = None;
                self.lasso_points.clear();
                self.shape_anchor = None;
                self.shape_current = None;
                self.move_from = None;
                self.liquify_from = None;
                self.doc.liquify_end();
                self.fill_preview = None;
                self.vp_grab = None;
                self.sym_grab = false;
                let had_xform = self.xform.grab.is_some() || self.xform.base.is_some();
                self.xform.cancel_grab();
                if had_xform && !self.text_paint_blocked() {
                    xform_preview(&mut self.doc, &mut self.xform);
                }
            }
            let z = self.zoom;
            // No scrollbar visuals over the artwork (v13 iPad: a full-width
            // bar flashed across the painting on every tap at overflow).
            // Touch pans via view_offset; desktop wheel keeps scrolling
            // while hidden - visibility was serving neither. (Bound first:
            // an inline chain reindents the whole ~950-line body.)
            let canvas_scroll = egui::ScrollArea::both()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
            // PC wheel conventions: plain wheel pans vertically, shift+wheel
            // pans horizontally, ctrl+wheel zooms (via zoom_delta below).
            // The backend passes wheel ticks through untouched, so shift
            // would otherwise scroll vertically like a plain wheel.
            if ui.ctx().input(|i| i.modifiers.shift) {
                ui.ctx().input_mut(|i| {
                    let d = i.smooth_scroll_delta;
                    let (x, y) = shift_wheel_remap(d.x, d.y, true);
                    i.smooth_scroll_delta = egui::Vec2::new(x, y);
                });
            }
            canvas_scroll.show(ui, |ui| {
                // The slot only sizes the scroll content; `resp` is built
                // below on the drawn page. Allocating it here with input
                // sense left the sensor on the un-centred slot, so the
                // page's right/bottom strips could not start a stroke.
                let (slot, _) = ui.allocate_exact_size(Vec2::new(w * z, h * z), Sense::hover());
                // A page that fits floats centred on the board; a zoomed-in
                // page keeps the viewport origin so panning still works.
                // Pan applies to the drawn canvas and every overlay/mapping
                // below (sizes are translation-invariant). At a whole-pixel
                // zoom the origin lands on a physical pixel: a fractional
                // one makes the GPU resample every edge on the page.
                let rect = canvas_page_rect(slot, avail, self.view_offset);
                let rect = egui::Rect::from_min_size(
                    snap_canvas_origin(rect.min, z, ui.ctx().pixels_per_point(), self.view_angle),
                    rect.size(),
                );
                // Presses and drags must start on the drawn page: centring
                // and pan move the artwork after layout, and the scroll
                // area clips the sensor to the visible part.
                let resp = ui.interact(rect, ui.id().with("canvas-page"), Sense::click_and_drag());
                // Rotation about the view center: canvas (x, y) maps through
                // the unrotated rect, then spins into place.
                let ang = self.view_angle;
                let cc = rect.center();
                // Board dots first: the page texture covers anything the
                // grid's radius grazes along its edge. The scroll-area
                // painter clips to the page viewport, so widen the clip to
                // the panel or the margins outside the page lose the grid.
                paint_board_dots(
                    &ui.painter().with_clip_rect(view_rect),
                    view_rect,
                    rect,
                    z,
                    ang,
                    design::board_dot(self.theme),
                );
                let to_screen = |x: f32, y: f32| {
                    rot_pt(
                        egui::Pos2::new(rect.min.x + x * z, rect.min.y + y * z),
                        cc,
                        ang,
                    )
                };
                let to_canvas = |pos: egui::Pos2| {
                    let q = unrot_pt(pos, cc, ang);
                    ((q.x - rect.min.x) / z, (q.y - rect.min.y) / z)
                };
                if viewport {
                    // Canvas rect the panel covers. Unrotated that is the
                    // tested `for_screen`; rotated, the panel's four corners
                    // mapped back, because the axis-aligned intersection of
                    // page and panel is neither inside nor outside what the
                    // rotation actually shows.
                    let view = if ang == 0.0 {
                        let (ox, oy) = to_canvas(view_rect.min);
                        canvas::ViewRect::for_screen(
                            ox,
                            oy,
                            view_rect.width(),
                            view_rect.height(),
                            w,
                            h,
                            z,
                        )
                    } else {
                        canvas::ViewRect::from_canvas_points(
                            [
                                to_canvas(view_rect.left_top()),
                                to_canvas(view_rect.right_top()),
                                to_canvas(view_rect.right_bottom()),
                                to_canvas(view_rect.left_bottom()),
                            ],
                            w,
                            h,
                        )
                    };
                    // Physical pixels: 1:1 at zoom >= 100 %, downscaled when
                    // zoomed out (the covered canvas extent caps it), never
                    // past what the GPU can hold.
                    let ppp = ctx.pixels_per_point();
                    let panel = [view_rect.width(), view_rect.height()];
                    let extent = [view.x1 - view.x0, view.y1 - view.y0];
                    let mut out = [1usize; 2];
                    for axis in 0..2 {
                        let wanted = (panel[axis] * ppp).ceil().max(1.0) as usize;
                        let covered = extent[axis].ceil().max(1.0) as usize;
                        out[axis] = wanted.min(covered).clamp(1, max_side);
                    }
                    // Mid-gesture frames reuse the previous texture at its
                    // own view rect: pinch stays responsive, and the first
                    // post-gesture frame re-renders from canvas tiles.
                    if !gesture || self.view_tex.texture().is_none() {
                        self.view_tex
                            .render(&ctx, &mut self.doc, &mut self.composite, view, out);
                    }
                    if let (Some(v), Some(t)) = (self.view_tex.view(), self.view_tex.texture()) {
                        if v.x1 > v.x0 && v.y1 > v.y0 {
                            paint_textured_quad(
                                ui.painter(),
                                t.id(),
                                [
                                    to_screen(v.x0, v.y0),
                                    to_screen(v.x1, v.y0),
                                    to_screen(v.x1, v.y1),
                                    to_screen(v.x0, v.y1),
                                ],
                                Color32::WHITE,
                            );
                        }
                    }
                } else if let Some(tex) = self.texture.as_ref() {
                    let tex = tex.id();
                    paint_textured_quad(
                        ui.painter(),
                        tex,
                        [
                            to_screen(0.0, 0.0),
                            to_screen(w, 0.0),
                            to_screen(w, h),
                            to_screen(0.0, h),
                        ],
                        Color32::WHITE,
                    );
                }
                let clamp_pt = |x: f32, y: f32| (x.clamp(0.0, w - 1.0), y.clamp(0.0, h - 1.0));
                // Mirror axis overlay so the symmetry center is visible.
                let axis_color = Color32::from_rgba_unmultiplied(220, 40, 40, 180);
                let axis = |a: egui::Pos2, b: egui::Pos2| {
                    ui.painter().line_segment([a, b], (1.0, axis_color));
                };
                let (scx, scy) = self.doc.symmetry_center_px();
                let center = to_screen(scx, scy);
                let vline = || {
                    axis(to_screen(scx, -8.0), to_screen(scx, h + 8.0));
                };
                let hline = || {
                    axis(to_screen(-8.0, scy), to_screen(w + 8.0, scy));
                };
                match self.doc.symmetry {
                    SymmetryMode::Off => {}
                    SymmetryMode::Horizontal => vline(),
                    SymmetryMode::Vertical => hline(),
                    SymmetryMode::Both => {
                        vline();
                        hline();
                    }
                    SymmetryMode::Kaleidoscope => {
                        let n = self.doc.symmetry_segments.clamp(2, 16) as usize;
                        let reach = (rect.width().powi(2) + rect.height().powi(2)).sqrt() * 0.5;
                        for k in 0..n {
                            let (s, c) = (k as f32 * std::f32::consts::PI / n as f32).sin_cos();
                            let dir = egui::Vec2::new(c * reach, s * reach);
                            axis(rot_pt(center + dir, cc, ang), rot_pt(center - dir, cc, ang));
                        }
                    }
                }
                // Mirror-center handle (draggable while Move center is
                // armed; off-canvas parks on the edge like VP handles).
                if self.doc.symmetry != SymmetryMode::Off {
                    let (p0, parked) = vp_handle_pos(rect, z, scx, scy);
                    let p = rot_pt(p0, cc, ang);
                    if parked {
                        let r = 10.0;
                        let diamond = vec![
                            p + egui::Vec2::new(0.0, -r),
                            p + egui::Vec2::new(r, 0.0),
                            p + egui::Vec2::new(0.0, r),
                            p + egui::Vec2::new(-r, 0.0),
                        ];
                        let red = Color32::from_rgb(220, 60, 60);
                        ui.painter()
                            .add(egui::Shape::closed_line(diamond, (3.0, red)));
                        ui.painter().circle_filled(p, 2.0, Color32::WHITE);
                    } else {
                        let (fill, r) = if self.sym_move {
                            (
                                Color32::from_rgb(220, 60, 60),
                                if self.sym_grab { 11.0 } else { 9.0 },
                            )
                        } else {
                            (Color32::from_rgb(220, 60, 60), 6.0)
                        };
                        ui.painter().circle_filled(p, r, fill);
                        ui.painter().circle_stroke(p, r, (2.0, Color32::WHITE));
                    }
                }
                // Brush cursor ring: nominal dab diameter at the hover point
                // (brush tools + liquify). Dark + white strokes stay visible
                // on any paper color; hover has no pressure so this is the
                // base size, dynamics spread real dabs around it.
                if let Some(size) = tool_ring_size(self.tool, self.brush.size, self.liquify_size) {
                    // While a size drag runs the ring stays pinned at the
                    // gesture's start point (the size, not the pointer,
                    // is what's moving).
                    let ring_at = self
                        .size_drag
                        .as_ref()
                        .map(|sd| sd.anchor)
                        .or_else(|| resp.hover_pos());
                    if let Some(pos) = ring_at {
                        let r = brush_ring_radius(size, z);
                        ui.painter()
                            .circle_stroke(pos, r, (2.0, Color32::from_gray(30)));
                        ui.painter().circle_stroke(pos, r, (1.0, Color32::WHITE));
                    }
                }
                // Drawing guides overlay: view-assist only, never baked in.
                if self.guides.kind != GuideKind::Off {
                    let alpha = (self.guides.opacity.clamp(0.05, 1.0) * 255.0).round() as u8;
                    let gc = Color32::from_rgba_unmultiplied(70, 130, 200, alpha);
                    for [x0, y0, x1, y1] in self.guides.overlay_lines(w, h) {
                        ui.painter()
                            .line_segment([to_screen(x0, y0), to_screen(x1, y1)], (1.0, gc));
                    }
                }
                // Drawing Assist: the engaged lock draws bright while the
                // stroke rides it (cleared on release).
                if let Some(lock) = self.assist_lock {
                    let reach = (rect.width().powi(2) + rect.height().powi(2)).sqrt();
                    let (tx, ty) = lock.through;
                    let (dx, dy) = (lock.dir.0 * reach, lock.dir.1 * reach);
                    ui.painter().line_segment(
                        [to_screen(tx - dx, ty - dy), to_screen(tx + dx, ty + dy)],
                        (1.5, Color32::from_rgb(255, 200, 60)),
                    );
                }
                // Vanishing-point handles for perspective guides (draggable
                // while Move VPs is armed in the Guides panel). Off-canvas
                // VPs park as hollow diamonds on the canvas edge so they
                // stay reachable.
                if matches!(self.guides.kind, GuideKind::OnePoint | GuideKind::TwoPoint) {
                    for (i, (vx, vy)) in self.guides.vp_list(w, h).iter().enumerate() {
                        let (p0, parked) = vp_handle_pos(rect, z, *vx, *vy);
                        let p = rot_pt(p0, cc, ang);
                        let grabbed = self.vp_grab == Some(i);
                        if parked {
                            let r = 10.0;
                            let diamond = vec![
                                p + egui::Vec2::new(0.0, -r),
                                p + egui::Vec2::new(r, 0.0),
                                p + egui::Vec2::new(0.0, r),
                                p + egui::Vec2::new(-r, 0.0),
                            ];
                            let amber = Color32::from_rgb(255, 200, 60);
                            ui.painter()
                                .add(egui::Shape::closed_line(diamond, (3.0, amber)));
                            ui.painter().circle_filled(p, 2.0, Color32::WHITE);
                        } else {
                            let (fill, r) = if self.vp_move {
                                (
                                    Color32::from_rgb(255, 200, 60),
                                    if grabbed { 11.0 } else { 9.0 },
                                )
                            } else {
                                (Color32::from_rgb(70, 130, 200), 6.0)
                            };
                            ui.painter().circle_filled(p, r, fill);
                            ui.painter().circle_stroke(p, r, (2.0, Color32::WHITE));
                        }
                    }
                }
                // Transform handles (M48): pins for the corner modes, a
                // bounds-hugging lattice for Mesh. Off-canvas points park
                // on the canvas edge as diamonds, like VP handles.
                if self.xform.mode != XformMode::Off {
                    let red = Color32::from_rgb(220, 60, 60);
                    let accent = Color32::from_rgb(255, 210, 90);
                    let sp: Vec<(egui::Pos2, bool)> = self
                        .xform
                        .points()
                        .iter()
                        .map(|&(x, y)| {
                            let (p0, parked) = vp_handle_pos(rect, z, x, y);
                            (rot_pt(p0, cc, ang), parked)
                        })
                        .collect();
                    if self.xform.mode.is_pins() {
                        if let Some(b) = self.xform.bounds {
                            let pts: Vec<egui::Pos2> = xform_bounds_corners(b)
                                .iter()
                                .map(|&(x, y)| {
                                    let (p0, _) = vp_handle_pos(rect, z, x, y);
                                    rot_pt(p0, cc, ang)
                                })
                                .collect();
                            ui.painter().add(egui::Shape::closed_line(pts, (1.0, red)));
                        }
                        if self.xform.mode == XformMode::Puppet {
                            for t in &self.xform.tris {
                                if t.iter().all(|&i| (i as usize) < self.xform.pins.len()) {
                                    for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                                        ui.painter().line_segment(
                                            [sp[a as usize].0, sp[b as usize].0],
                                            (1.0, red),
                                        );
                                    }
                                }
                            }
                        }
                        if self.xform.mode == XformMode::Free && sp.len() >= 4 {
                            if let Some((hx, hy)) = self.xform.rotate_handle(z) {
                                let (hp, _) = vp_handle_pos(rect, z, hx, hy);
                                let hp = rot_pt(hp, cc, ang);
                                let top = sp[0].0 + (sp[1].0 - sp[0].0) * 0.5;
                                ui.painter().line_segment([top, hp], (1.0, red));
                                ui.painter().circle_filled(hp, 6.0, accent);
                                ui.painter().circle_stroke(hp, 6.0, (2.0, Color32::WHITE));
                            }
                        }
                    } else if self.xform.mode == XformMode::Mesh {
                        let stride = self.xform.grid_cols as usize + 1;
                        let cols = self.xform.grid_cols as usize;
                        let rows = self.xform.grid_rows as usize;
                        if sp.len() == stride * (rows + 1) {
                            for j in 0..=rows {
                                for i in 0..=cols {
                                    if i < cols {
                                        ui.painter().line_segment(
                                            [sp[j * stride + i].0, sp[j * stride + i + 1].0],
                                            (1.0, red),
                                        );
                                    }
                                    if j < rows {
                                        ui.painter().line_segment(
                                            [sp[j * stride + i].0, sp[(j + 1) * stride + i].0],
                                            (1.0, red),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    for (i, (p, parked)) in sp.iter().enumerate() {
                        if self.xform.mode == XformMode::Mesh && !self.xform.mesh_point_active(i) {
                            continue;
                        }
                        let grabbed = self.xform.grabbed_pin() == Some(i);
                        let selected = self.xform.pin_selected(i);
                        let active = self.xform.active_pin == Some(i);
                        if *parked {
                            let r = 10.0;
                            let diamond = vec![
                                *p + egui::Vec2::new(0.0, -r),
                                *p + egui::Vec2::new(r, 0.0),
                                *p + egui::Vec2::new(0.0, r),
                                *p + egui::Vec2::new(-r, 0.0),
                            ];
                            let outline = if selected || active { accent } else { red };
                            ui.painter()
                                .add(egui::Shape::closed_line(diamond, (3.0, outline)));
                            ui.painter().circle_filled(*p, 2.0, Color32::WHITE);
                            if active {
                                ui.painter().circle_stroke(*p, 15.0, (2.0, accent));
                            }
                        } else {
                            let r = if grabbed {
                                11.0
                            } else if active {
                                10.0
                            } else {
                                9.0
                            };
                            let fill = if grabbed || selected { accent } else { red };
                            ui.painter().circle_filled(*p, r, fill);
                            ui.painter().circle_stroke(*p, r, (2.0, Color32::WHITE));
                            if active {
                                ui.painter().circle_stroke(*p, r + 4.0, (2.0, accent));
                            }
                        }
                    }
                    if let Some(((sx, sy), (ex, ey))) = self.xform.marquee {
                        let mr = egui::Rect::from_two_pos(to_screen(sx, sy), to_screen(ex, ey));
                        ui.painter().rect_filled(
                            mr,
                            0.0,
                            Color32::from_rgba_unmultiplied(255, 210, 90, 24),
                        );
                        ui.painter()
                            .rect_stroke(mr, 0.0, (1.0, accent), egui::StrokeKind::Middle);
                    }
                }
                // Onion skins over the current frame (assist only, not in
                // mask view, hidden while playing so ghosts never tint
                // the page as frames change).
                if self.anim_assist && self.onion && !self.view_mask && !self.anim_playing {
                    self.refresh_ghosts(&ctx);
                    if let Some(tex) = &self.ghost_prev {
                        paint_textured_quad(
                            ui.painter(),
                            tex.id(),
                            [
                                to_screen(0.0, 0.0),
                                to_screen(w, 0.0),
                                to_screen(w, h),
                                to_screen(0.0, h),
                            ],
                            Color32::from_rgb(255, 130, 130),
                        );
                    }
                    if let Some(tex) = &self.ghost_next {
                        paint_textured_quad(
                            ui.painter(),
                            tex.id(),
                            [
                                to_screen(0.0, 0.0),
                                to_screen(w, 0.0),
                                to_screen(w, h),
                                to_screen(0.0, h),
                            ],
                            Color32::from_rgb(130, 165, 255),
                        );
                    }
                }
                // Text anchor crosshair while placing type.
                if self.tool == Tool::Text {
                    if let Some((ax, ay)) = self.text_anchor {
                        let p = to_screen(ax, ay);
                        for (color, width) in [(Color32::BLACK, 3.0), (Color32::WHITE, 1.5)] {
                            ui.painter().line_segment(
                                [
                                    p + egui::Vec2::new(-8.0, 0.0),
                                    p + egui::Vec2::new(8.0, 0.0),
                                ],
                                (width, color),
                            );
                            ui.painter().line_segment(
                                [
                                    p + egui::Vec2::new(0.0, -8.0),
                                    p + egui::Vec2::new(0.0, 8.0),
                                ],
                                (width, color),
                            );
                        }
                    }
                }
                // Committed lasso outline stays visible on every tool (it is
                // skipped while a new loop is being dragged: the Lasso branch
                // below draws the live path instead).
                if !self.lasso_poly.is_empty() && self.lasso_points.is_empty() {
                    let pts: Vec<egui::Pos2> = self
                        .lasso_poly
                        .iter()
                        .map(|&(x, y)| to_screen(x, y))
                        .collect();
                    paint_path(ui.painter(), &pts, true);
                }
                // Mask-edge marching ants on every tool: the cached edges
                // rebuild only when the selection changed (dirty flag).
                if self.sel_edges_dirty {
                    self.rebuild_sel_edges();
                    self.sel_edges_dirty = false;
                }
                if self.doc.selection_mask.is_some() && !self.sel_edges.is_empty() {
                    let t = ctx.input(|i| i.time);
                    paint_mask_edges(
                        ui.painter(),
                        &self.sel_edges,
                        |x, y| to_screen(x as f32, y as f32),
                        t,
                    );
                }

                // Eyedropper: Alt-click samples the composited canvas color
                // into the brush on any tool (Alt held blocks painting).
                // Puppet Warp reserves Alt-click for deleting a pin, as CSP
                // does, so it bypasses the picker.
                if ctx.input(|i| i.modifiers.alt) && self.xform.mode != XformMode::Puppet {
                    if resp.drag_started() || resp.clicked() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let (x, y) = to_canvas(pos);
                            let (cx, cy) = clamp_pt(x, y);
                            if let Some(rgb) = self.doc.sample(cx as u32, cy as u32) {
                                self.color = rgb;
                                self.status =
                                    format!("Picked #{:02X}{:02X}{:02X}.", rgb[0], rgb[1], rgb[2]);
                            }
                        }
                    }
                    // Same magnifier as the Pick tool, parked clear of the cursor.
                    if let Some(pos) = resp.hover_pos() {
                        let (x, y) = to_canvas(pos);
                        let (cx, cy) = clamp_pt(x, y);
                        self.paint_picker_loupe(ui.painter(), &ctx, pos, cx, cy);
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                // Vanishing-point dragging (armed in the Guides panel):
                // grabs pause painting and move the nearest handle.
                if self.vp_move
                    && matches!(self.guides.kind, GuideKind::OnePoint | GuideKind::TwoPoint)
                {
                    if resp.drag_started() && !gesture {
                        if let Some(pos) = resp.hover_pos() {
                            let mut best: Option<(usize, f32)> = None;
                            for (i, (vx, vy)) in self.guides.vp_list(w, h).iter().enumerate() {
                                // Parked edge markers grab exactly where drawn.
                                let (p0, _) = vp_handle_pos(rect, z, *vx, *vy);
                                let p = rot_pt(p0, cc, ang);
                                let d = (p.x - pos.x).hypot(p.y - pos.y);
                                if best.is_none_or(|(_, bd)| d < bd) {
                                    best = Some((i, d));
                                }
                            }
                            // Generous grab radius so off-center taps still
                            // catch a handle; misses simply paint nothing.
                            if let Some((i, d)) = best {
                                if d <= 28.0 {
                                    self.vp_grab = Some(i);
                                }
                            }
                        }
                    } else if resp.dragged() {
                        if let (Some(i), Some(pos)) = (self.vp_grab, resp.hover_pos()) {
                            let (x, y) = to_canvas(pos);
                            self.guides.set_vp(i, x, y, w, h);
                        }
                    }
                    if resp.drag_stopped() {
                        self.vp_grab = None;
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                // Mirror-center dragging (armed in the Symmetry panel):
                // grabs pause painting and move the axes' crossing.
                if self.sym_move && self.doc.symmetry != SymmetryMode::Off {
                    if resp.drag_started() && !gesture {
                        if let Some(pos) = resp.hover_pos() {
                            let (scx, scy) = self.doc.symmetry_center_px();
                            let (p0, _) = vp_handle_pos(rect, z, scx, scy);
                            let p = rot_pt(p0, cc, ang);
                            if (p.x - pos.x).hypot(p.y - pos.y) <= 28.0 {
                                self.sym_grab = true;
                            }
                        }
                    } else if resp.dragged() && self.sym_grab {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.doc.set_symmetry_center(x, y);
                        }
                    }
                    if resp.drag_stopped() {
                        self.sym_grab = false;
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                // Transform handles (M48, armed in the Transform panel):
                // grabs pause painting. Distort/Puppet drag pins directly,
                // Skew and Perspective move their partner pins too, Free
                // adds move/scale/rotate gestures, Mesh drags in-bounds
                // lattice points. Every tick re-warps from the session
                // base (see xform_preview), so the drag previews live and
                // repeated gestures never compound resampling; release
                // just reports the already-previewed result.
                if self.xform.mode != XformMode::Off {
                    let ptr = resp
                        .interact_pointer_pos()
                        .or_else(|| resp.hover_pos())
                        .map(to_canvas);
                    if self.xform.mode == XformMode::Puppet
                        && resp.secondary_clicked()
                        && !self.text_paint_blocked()
                    {
                        if let Some(at) = ptr {
                            let r = 18.0 / z.max(0.05);
                            if self.xform.remove_pin_near(at, r, &mut self.doc) {
                                self.status =
                                    format!("Puppet pin removed ({} left).", self.xform.pins.len());
                            }
                        }
                    }
                    if resp.drag_started() && !gesture && !self.text_paint_blocked() {
                        if let Some(pos) = resp.hover_pos() {
                            let at = to_canvas(pos);
                            let alt = ctx.input(|i| i.modifiers.alt);
                            let shift = ctx.input(|i| i.modifiers.shift);
                            let mut best: Option<(usize, f32)> = None;
                            for (i, &(x, y)) in self.xform.points().iter().enumerate() {
                                if self.xform.mode == XformMode::Mesh
                                    && !self.xform.mesh_point_active(i)
                                {
                                    continue;
                                }
                                let (p0, _) = vp_handle_pos(rect, z, x, y);
                                let p = rot_pt(p0, cc, ang);
                                let d = (p.x - pos.x).hypot(p.y - pos.y);
                                if best.is_none_or(|(_, bd)| d < bd) {
                                    best = Some((i, d));
                                }
                            }
                            if let Some((i, _)) = best.filter(|&(_, d)| d <= 28.0) {
                                if self.xform.mode == XformMode::Puppet {
                                    if alt || self.xform.pin_tool == PinTool::Delete {
                                        if self.xform.remove_pin(i, &mut self.doc) {
                                            self.status = format!(
                                                "Puppet pin removed ({} left).",
                                                self.xform.pins.len()
                                            );
                                        }
                                    } else {
                                        self.xform.prepare_pin_drag(i, shift);
                                        if self.xform.pin_selected(i)
                                            && self.xform.selected_pins.len() > 1
                                        {
                                            self.xform.begin_selected(at);
                                        } else {
                                            self.xform.begin_grab(i, at);
                                        }
                                    }
                                } else {
                                    self.xform.begin_grab(i, at);
                                }
                            } else if self.xform.mode == XformMode::Puppet {
                                if !alt && !self.xform.point_in_frame(at) {
                                    self.xform.marquee = Some((at, at));
                                }
                            } else if self.xform.mode == XformMode::Free {
                                let mut grabbed = false;
                                if let Some((hx, hy)) = self.xform.rotate_handle(z) {
                                    let (p0, _) = vp_handle_pos(rect, z, hx, hy);
                                    let hp = rot_pt(p0, cc, ang);
                                    if (hp.x - pos.x).hypot(hp.y - pos.y) <= 28.0 {
                                        self.xform.begin_rotate(at);
                                        grabbed = true;
                                    }
                                }
                                if !grabbed && self.xform.point_in_frame(at) {
                                    self.xform.begin_all(at);
                                }
                            }
                        }
                    } else if resp.dragged() {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            if self.xform.grab.is_some() {
                                let (dx, dy) = (x - self.xform.grab_at.0, y - self.xform.grab_at.1);
                                let (shift, alt) =
                                    ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
                                self.xform.drag_by_mod(dx, dy, shift, alt);
                                if !self.text_paint_blocked()
                                    && xform_preview(&mut self.doc, &mut self.xform)
                                        == XformApply::Skipped
                                {
                                    self.status = "Pins overlap — transform skipped.".to_string();
                                }
                            } else if let Some((start, _)) = self.xform.marquee {
                                self.xform.marquee = Some((start, (x, y)));
                            }
                        }
                    }
                    if resp.drag_stopped() {
                        let was_grabbed = self.xform.grab.take().is_some();
                        let from = std::mem::take(&mut self.xform.grab_from);
                        if was_grabbed && from.as_slice() != self.xform.points() {
                            // Re-run once with the release positions (the
                            // last render already showed this) and commit
                            // the status message.
                            if !self.text_paint_blocked() {
                                self.status = match xform_preview(&mut self.doc, &mut self.xform) {
                                    XformApply::Skipped => {
                                        "Pins overlap — transform skipped.".to_string()
                                    }
                                    XformApply::Restored => "Transform reset.".to_string(),
                                    XformApply::Applied => match self.xform.mode {
                                        XformMode::Mesh => "Mesh warp applied.".to_string(),
                                        _ => "Transform applied.".to_string(),
                                    },
                                    XformApply::NoOp => self.status.clone(),
                                };
                            }
                        }
                        if let Some(((sx, sy), (ex, ey))) = self.xform.marquee {
                            let moved = ((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt() * z;
                            if moved > 4.0 {
                                let shift = ctx.input(|i| i.modifiers.shift);
                                self.xform.marquee_select(shift);
                                self.status = format!(
                                    "{} Puppet pin(s) selected.",
                                    self.xform.selected_pins.len()
                                );
                            } else {
                                self.xform.marquee = None;
                            }
                        }
                    }
                    if self.xform.mode == XformMode::Puppet
                        && resp.clicked()
                        && !self.text_paint_blocked()
                    {
                        // A press on a pin may have armed a grab already.
                        // A click means no real drag: drop that gesture so
                        // it cannot move pins on the next pointer tick.
                        if self.xform.grab.is_some() {
                            if self.xform.grab_from.as_slice() == self.xform.points() {
                                self.xform.cancel_grab();
                            } else {
                                self.xform.grab = None;
                                self.xform.grab_from.clear();
                            }
                        }
                        self.xform.marquee = None;
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let alt = ctx.input(|i| i.modifiers.alt);
                            let shift = ctx.input(|i| i.modifiers.shift);
                            let mut hit: Option<(usize, f32)> = None;
                            for (i, &(x, y)) in self.xform.pins.iter().enumerate() {
                                let (p0, _) = vp_handle_pos(rect, z, x, y);
                                let p = rot_pt(p0, cc, ang);
                                let d = (p.x - pos.x).hypot(p.y - pos.y);
                                if hit.is_none_or(|(_, bd)| d < bd) {
                                    hit = Some((i, d));
                                }
                            }
                            if let Some((i, _)) = hit.filter(|&(_, d)| d <= 20.0) {
                                if alt || self.xform.pin_tool == PinTool::Delete {
                                    if self.xform.remove_pin(i, &mut self.doc) {
                                        self.status = format!(
                                            "Puppet pin removed ({} left).",
                                            self.xform.pins.len()
                                        );
                                    }
                                } else {
                                    self.xform.click_pin(i, shift);
                                    self.status = format!(
                                        "{} Puppet pin(s) selected.",
                                        self.xform.selected_pins.len()
                                    );
                                }
                            } else if !alt
                                && self.xform.pin_tool == PinTool::Select
                                && self.xform.add_pin(to_canvas(pos), &self.doc)
                            {
                                self.status =
                                    format!("Puppet pin added ({}).", self.xform.pins.len());
                            }
                        }
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                // Ctrl+middle-drag zoom (pen-button workflow): vertical drag
                // distance maps onto wheel steps via `zoom_drag_factor`.
                // Skipped mid-stroke (and while primary drags) so it never
                // wrecks a line; needs no checkpoint (view-only like pan).
                if !self.painting && !resp.dragged() && ui.ctx().input(|i| i.modifiers.ctrl) {
                    use egui::PointerButton::Middle;
                    if resp.drag_started_by(Middle) {
                        self.mmb_last = resp.hover_pos();
                    } else if resp.dragged_by(Middle) {
                        if let (Some(last), Some(pos)) = (self.mmb_last, resp.hover_pos()) {
                            let dy = pos.y - last.y;
                            self.zoom = (self.zoom * zoom_drag_factor(dy)).clamp(0.1, 8.0);
                            self.mmb_last = Some(pos);
                        }
                    }
                    if resp.drag_stopped_by(Middle) {
                        self.mmb_last = None;
                    }
                } else {
                    self.mmb_last = None;
                }

                // M49 brush-size drag (default Shift, remappable): while the
                // bound modifiers are held with a paint tool, a primary drag
                // scales size by travel along the configured direction
                // (left–right / up–down / distance, see `SizeDragMode`) via
                // `size_drag_factor` (same 1.25/120px step as zoom) from the
                // pinned base; release keeps it, Esc during the drag reverts.
                let size_mod = self.shortcuts.drag_get(DragActionId::BrushSize);
                let size_mods = ui.ctx().input(|i| {
                    size_mod.matches(i.modifiers.ctrl, i.modifiers.shift, i.modifiers.alt)
                });
                let paint_tool = matches!(self.tool, Tool::Brush | Tool::Eraser | Tool::Smudge);
                if let Some(sd) = &self.size_drag {
                    if resp.dragged_by(egui::PointerButton::Primary) {
                        if let Some(pos) = resp.interact_pointer_pos().or_else(|| resp.hover_pos())
                        {
                            let mode = self.shortcuts.size_drag_mode();
                            let travel = size_drag_travel(mode, sd.anchor, pos);
                            self.brush.size = drag_size(sd.base, travel);
                        }
                    }
                    if resp.drag_stopped() {
                        self.status = format!("Size {:.0}", self.brush.size);
                        self.size_drag = None;
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                } else if paint_tool
                    && size_mods
                    && !gesture
                    && resp.drag_started_by(egui::PointerButton::Primary)
                {
                    if let Some(pos) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) {
                        self.size_drag = Some(SizeDrag {
                            base: self.brush.size,
                            anchor: pos,
                        });
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Select {
                    // Marquee selection.
                    if resp.drag_started() && !gesture {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.sel_anchor = Some(clamp_pt(x, y));
                            self.sel_current = self.sel_anchor;
                        }
                    }
                    if self.sel_anchor.is_some() && resp.dragged() {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.sel_current = Some(clamp_pt(x, y));
                        }
                    }
                    if resp.drag_stopped() {
                        if let (Some((ax, ay)), Some((bx, by))) =
                            (self.sel_anchor, self.sel_current)
                        {
                            let (x0, x1) = if ax <= bx {
                                (ax as u32, bx as u32)
                            } else {
                                (bx as u32, ax as u32)
                            };
                            let (y0, y1) = if ay <= by {
                                (ay as u32, by as u32)
                            } else {
                                (by as u32, ay as u32)
                            };
                            if x1.saturating_sub(x0) >= 2 && y1.saturating_sub(y0) >= 2 {
                                self.doc.selection = Some((x0, y0, x1, y1));
                                self.doc.selection_mask = None;
                                self.lasso_poly.clear();
                                self.sel_edges_dirty = true;
                                self.status =
                                    format!("Selected {}x{} px.", x1 - x0 + 1, y1 - y0 + 1);
                            } else {
                                self.doc.clear_selection();
                                self.sel_edges_dirty = true;
                            }
                        }
                        self.sel_anchor = None;
                        self.sel_current = None;
                    }
                    // Live rubber band + committed marching ants (mask edges
                    // when a graduated mask exists, bbox rect otherwise).
                    let band = match (self.sel_anchor, self.sel_current) {
                        (Some(a), Some(b)) => Some((a, b)),
                        _ => self.doc.selection.map(|(x0, y0, x1, y1)| {
                            ((x0 as f32, y0 as f32), ((x1 + 1) as f32, (y1 + 1) as f32))
                        }),
                    };
                    if self.sel_anchor.is_none() && self.doc.selection_mask.is_some() {
                        if self.sel_edges_dirty {
                            self.rebuild_sel_edges();
                            self.sel_edges_dirty = false;
                        }
                        if !self.sel_edges.is_empty() {
                            let t = ctx.input(|i| i.time);
                            paint_mask_edges(
                                ui.painter(),
                                &self.sel_edges,
                                |x, y| to_screen(x as f32, y as f32),
                                t,
                            );
                        }
                    } else if let Some(((ax, ay), (bx, by))) = band {
                        let (x0, x1) = if ax <= bx { (ax, bx) } else { (bx, ax) };
                        let (y0, y1) = if ay <= by { (ay, by) } else { (by, ay) };
                        paint_selection(
                            ui.painter(),
                            [
                                to_screen(x0, y0),
                                to_screen(x1, y0),
                                to_screen(x1, y1),
                                to_screen(x0, y1),
                            ],
                            ctx.input(|i| i.time),
                        );
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Lasso {
                    // Freehand loop: collect points, build the mask on release.
                    if resp.drag_started() && !gesture {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.lasso_points = vec![clamp_pt(x, y)];
                        }
                    }
                    if resp.dragged() && !gesture {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            let p = clamp_pt(x, y);
                            let push = match self.lasso_points.last() {
                                Some(&(lx, ly)) => {
                                    (p.0 - lx) * (p.0 - lx) + (p.1 - ly) * (p.1 - ly) >= 4.0
                                }
                                None => true,
                            };
                            if push {
                                self.lasso_points.push(p);
                            }
                        }
                    }
                    if !self.lasso_points.is_empty() {
                        let pts: Vec<egui::Pos2> = self
                            .lasso_points
                            .iter()
                            .map(|&(x, y)| to_screen(x, y))
                            .collect();
                        paint_path(ui.painter(), &pts, true);
                    }
                    if resp.drag_stopped() {
                        if self.lasso_points.len() >= 3 {
                            let pts = self.lasso_points.clone();
                            match self.doc.set_lasso(&pts) {
                                Some(_) => {
                                    self.lasso_poly = pts;
                                    self.status = format!(
                                        "Lasso: {} px selected.",
                                        self.doc.selection_pixel_count()
                                    );
                                }
                                None => {
                                    self.doc.clear_selection();
                                    self.lasso_poly.clear();
                                    self.status = "Lasso too small.".to_string();
                                }
                            }
                        } else {
                            self.doc.clear_selection();
                            self.lasso_poly.clear();
                        }
                        self.sel_edges_dirty = true;
                        self.lasso_points.clear();
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Fill {
                    // ColorDrop: tap fills; keep holding and drag sideways to
                    // sweep the tolerance with a live preview (one undo step
                    // for the whole gesture). `clicked` covers motionless
                    // taps: a click-and-drag widget only reports a drag once
                    // the pointer moves. Tap positions come from
                    // `interact_pointer_pos`, never `hover_pos`: a touch tap
                    // has no hover (egui-winit emits PointerGone with the
                    // release, clearing the pointer's latest position), and
                    // the tap frame must still see its point.
                    if (resp.drag_started() || resp.clicked())
                        && !gesture
                        && !self.text_paint_blocked()
                    {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let (x, y) = to_canvas(pos);
                            let (cx, cy) = clamp_pt(x, y);
                            let seed = (cx as u32, cy as u32);
                            let layer = self.doc.active;
                            // Re-tapping the same seed while the fill is
                            // still live re-applies it from the original
                            // snapshot instead of growing from the already
                            // grown outer edge.
                            let same_live = self.fill_preview.as_ref().is_some_and(|fp| {
                                fp.layer == layer
                                    && fp.seed == seed
                                    && self
                                        .doc
                                        .layers
                                        .get(fp.layer)
                                        .is_some_and(|l| l.pixels.fingerprint() == fp.live_hash)
                                    && fill_context_matches(&self.doc, fp)
                            });
                            if same_live {
                                if let Some(mut fp) = self.fill_preview.take() {
                                    fill_from_baseline(
                                        &mut self.doc,
                                        &mut fp,
                                        self.fill_tolerance,
                                        self.fill_grow,
                                    );
                                    // A sideways drag from this new press
                                    // sweeps from here, not the first tap.
                                    fp.start_x = pos.x;
                                    fp.start_tol = self.fill_tolerance;
                                    self.status = format!(
                                        "Fill preview — tolerance {}, grow {}.",
                                        fp.applied, self.fill_grow
                                    );
                                    self.fill_preview = Some(fp);
                                }
                            } else {
                                let pixels = self
                                    .doc
                                    .layers
                                    .get(layer)
                                    .map(|l| l.pixels.clone())
                                    .unwrap_or_default();
                                self.doc.checkpoint();
                                let brush = self.base_brush();
                                let color = self.current_color();
                                self.doc.flood_fill(
                                    &brush,
                                    color,
                                    seed.0,
                                    seed.1,
                                    self.fill_tolerance,
                                    self.fill_grow,
                                );
                                // Hash what our fill just left behind, not
                                // the snapshot: the sweep's guard compares
                                // the live layer against the last apply.
                                let live_hash = self
                                    .doc
                                    .layers
                                    .get(layer)
                                    .map(|l| l.pixels.fingerprint())
                                    .unwrap_or_default();
                                self.fill_preview = Some(FillPreview {
                                    layer,
                                    seed,
                                    pixels,
                                    color,
                                    opacity: brush.opacity,
                                    start_x: pos.x,
                                    start_tol: self.fill_tolerance,
                                    applied: self.fill_tolerance,
                                    live_hash,
                                    selection: self.doc.selection,
                                    mask_hash: self.doc.selection_mask.as_deref().map(pixels_hash),
                                    symmetry: self.doc.symmetry,
                                    segments: self.doc.symmetry_segments,
                                    sym_center: self.doc.symmetry_center,
                                });
                                self.status = format!(
                                    "Fill preview — drag sideways for tolerance ({}).",
                                    self.fill_tolerance
                                );
                            }
                        }
                    } else if resp.dragged() && !gesture {
                        if let Some(mut fp) = self.fill_preview.take() {
                            // Same stale-snapshot guard as filters: if the
                            // layer changed underneath (paint, undo, layer
                            // op…), drop the sweep instead of wiping work.
                            let expired = match self.doc.layers.get(fp.layer) {
                                Some(l) => l.pixels.fingerprint() != fp.live_hash,
                                None => true,
                            };
                            if expired {
                                self.status = "Fill expired — layer changed.".to_string();
                            } else {
                                if let Some(pos) = resp.hover_pos() {
                                    let tol = (fp.start_tol as i32
                                        + ((pos.x - fp.start_x) * 0.5) as i32)
                                        .clamp(0, 255)
                                        as u8;
                                    if tol != fp.applied {
                                        fill_from_baseline(
                                            &mut self.doc,
                                            &mut fp,
                                            tol,
                                            self.fill_grow,
                                        );
                                        self.fill_tolerance = tol;
                                        self.status = format!("Fill preview — tolerance {tol}.");
                                    }
                                }
                                // Keep the preview alive only while its layer exists.
                                if self.doc.layers.get(fp.layer).is_some() {
                                    self.fill_preview = Some(fp);
                                }
                            }
                        }
                    } else if resp.drag_stopped() {
                        // Keep the preview: the panel re-applies it live
                        // while Tolerance/Grow change (anchored to the
                        // original snapshot, so Grow can shrink as well as
                        // expand). It expires on tool change, pinch, or any
                        // other edit.
                        if let Some(fp) = self.fill_preview.as_ref() {
                            self.status = format!(
                                "Filled (tolerance {}, grow {}).",
                                fp.applied, self.fill_grow
                            );
                            self.capture_timelapse();
                        }
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Picker {
                    // Tap or drag to lift the composited color into the
                    // brush; the loupe shows a magnified ring so a finger
                    // never covers the target (touch-first, iPad-visible).
                    // `clicked` covers motionless taps (see the Fill note).
                    if (resp.drag_started() || resp.dragged() || resp.clicked()) && !gesture {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let (x, y) = to_canvas(pos);
                            let (cx, cy) = clamp_pt(x, y);
                            if let Some(rgb) = self.doc.sample(cx as u32, cy as u32) {
                                self.color = rgb;
                                self.status =
                                    format!("Picked #{:02X}{:02X}{:02X}.", rgb[0], rgb[1], rgb[2]);
                            }
                        }
                    }
                    // Loupe on hover and mid-drag alike.
                    if let Some(pos) = resp.hover_pos() {
                        let (x, y) = to_canvas(pos);
                        let (cx, cy) = clamp_pt(x, y);
                        self.paint_picker_loupe(ui.painter(), &ctx, pos, cx, cy);
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Hand {
                    // Pan the VIEW with any pointer (pen, mouse, finger) —
                    // never paints, checkpoints, or touches undo. The grab
                    // starts anywhere in the viewport (press origin), not
                    // just on the canvas: paper margins pan too. The origin
                    // persists for the whole press, so no latch is needed;
                    // multi-touch stays on its own path (gesture) so the
                    // two never double-pan. Same clamp as two-finger pan.
                    let (held, origin_in_view, d) = ui.ctx().input(|i| {
                        let p = &i.pointer;
                        (
                            p.any_down(),
                            p.press_origin().is_some_and(|o| view_rect.contains(o)),
                            p.delta(),
                        )
                    });
                    if held && origin_in_view && !gesture && d != egui::Vec2::ZERO {
                        let (cw, ch) = (w * self.zoom, h * self.zoom);
                        let (ox, oy) = pan_offset(
                            (self.view_offset.x, self.view_offset.y),
                            (d.x, d.y),
                            cw,
                            ch,
                            avail.x,
                            avail.y,
                        );
                        self.view_offset = egui::Vec2::new(ox, oy);
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Move {
                    // Drag the active layer around (integer steps).
                    if resp.drag_started() && !gesture && !self.text_paint_blocked() {
                        if let Some(pos) = resp.hover_pos() {
                            self.doc.checkpoint();
                            self.move_from = Some(to_canvas(pos));
                            self.move_acc = (0.0, 0.0);
                        }
                    }
                    if self.move_from.is_some() && resp.dragged() {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            let (fx, fy) = self.move_from.unwrap();
                            let ax = self.move_acc.0 + x - fx;
                            let ay = self.move_acc.1 + y - fy;
                            let (dx, dy) = (ax.trunc() as i32, ay.trunc() as i32);
                            if dx != 0 || dy != 0 {
                                self.doc.translate_layer(self.doc.active, dx, dy);
                            }
                            self.move_acc = (ax - dx as f32, ay - dy as f32);
                            self.move_from = Some((x, y));
                        }
                    }
                    if resp.drag_stopped() {
                        self.move_from = None;
                        if !self.text_paint_blocked() {
                            self.status = "Layer moved.".to_string();
                        }
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Liquify {
                    // M40 push: warp the active layer along each drag
                    // segment (one checkpoint per stroke, like paint).
                    // Touch-safe position: `hover_pos` is None on
                    // touch-tap/release frames (PointerGone).
                    let pos = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
                    if resp.drag_started() && !gesture && !self.text_paint_blocked() {
                        if let Some(pos) = pos {
                            self.doc.checkpoint();
                            self.doc.liquify_begin(self.doc.active);
                            self.liquify_from = Some(to_canvas(pos));
                        }
                    }
                    if resp.dragged() {
                        if let (Some(from), Some(pos)) = (self.liquify_from, pos) {
                            let to = to_canvas(pos);
                            // The engine splits the segment into sub-steps
                            // (an eighth of the brush size) and warps it
                            // from the stroke's snapshot, so a fast drag
                            // neither trails the pointer nor blurs what
                            // earlier pushes already warped.
                            self.doc.liquify_segment(
                                self.doc.active,
                                from,
                                to,
                                self.liquify_size,
                                self.liquify_strength,
                            );
                            // Ghosts show the neighbor frames only; warping
                            // the active frame leaves them untouched.
                            self.liquify_from = Some(to);
                        }
                    }
                    if resp.drag_stopped() {
                        self.liquify_from = None;
                        self.doc.liquify_end();
                        if !self.text_paint_blocked() {
                            self.capture_timelapse();
                            self.status = "Liquified.".to_string();
                        }
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Crop {
                    // Drag-crop rect: commits to `crop_rect` on release; Apply
                    // (in the Tools panel) crops, re-drag replaces, Esc cancels.
                    if resp.drag_started() && !gesture {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.crop_anchor = Some(clamp_pt(x, y));
                            self.crop_current = self.crop_anchor;
                        }
                    }
                    if self.crop_anchor.is_some() && resp.dragged() {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.crop_current = Some(clamp_pt(x, y));
                        }
                    }
                    if resp.drag_stopped() {
                        if let (Some((ax, ay)), Some((bx, by))) =
                            (self.crop_anchor, self.crop_current)
                        {
                            let (x0, x1) = if ax <= bx {
                                (ax as u32, bx as u32)
                            } else {
                                (bx as u32, ax as u32)
                            };
                            let (y0, y1) = if ay <= by {
                                (ay as u32, by as u32)
                            } else {
                                (by as u32, ay as u32)
                            };
                            if x1.saturating_sub(x0) >= 2 && y1.saturating_sub(y0) >= 2 {
                                self.crop_rect = Some((x0, y0, x1, y1));
                                self.status = format!(
                                    "Crop {}x{} — Apply or re-drag.",
                                    x1 - x0 + 1,
                                    y1 - y0 + 1
                                );
                            } else {
                                self.crop_rect = None;
                            }
                        }
                        self.crop_anchor = None;
                        self.crop_current = None;
                    }
                    // Live rubber band + committed rect (rotation-mapped like
                    // the selection overlay).
                    let band = match (self.crop_anchor, self.crop_current) {
                        (Some(a), Some(b)) => Some((a, b)),
                        _ => self.crop_rect.map(|(x0, y0, x1, y1)| {
                            ((x0 as f32, y0 as f32), ((x1 + 1) as f32, (y1 + 1) as f32))
                        }),
                    };
                    if let Some(((ax, ay), (bx, by))) = band {
                        let (x0, x1) = if ax <= bx { (ax, bx) } else { (bx, ax) };
                        let (y0, y1) = if ay <= by { (ay, by) } else { (by, ay) };
                        paint_selection(
                            ui.painter(),
                            [
                                to_screen(x0, y0),
                                to_screen(x1, y0),
                                to_screen(x1, y1),
                                to_screen(x0, y1),
                            ],
                            ctx.input(|i| i.time),
                        );
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Shapes {
                    // Drag a preview, rasterize on release.
                    if resp.drag_started() && !gesture && !self.text_paint_blocked() {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.shape_anchor = Some(clamp_pt(x, y));
                            self.shape_current = self.shape_anchor;
                        }
                    }
                    if self.shape_anchor.is_some() && resp.dragged() {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.shape_current = Some(clamp_pt(x, y));
                        }
                    }
                    if let (Some((ax, ay)), Some((bx, by))) =
                        (self.shape_anchor, self.shape_current)
                    {
                        let pts: Vec<egui::Pos2> =
                            shape_preview_points(self.shape_kind, ax, ay, bx, by, self.shape_sides)
                                .into_iter()
                                .map(|(x, y)| to_screen(x, y))
                                .collect();
                        paint_ants_path(ui.painter(), &pts, ctx.input(|i| i.time));
                    }
                    if resp.drag_stopped() {
                        if let (Some((ax, ay)), Some((bx, by))) =
                            (self.shape_anchor, self.shape_current)
                        {
                            self.doc.checkpoint();
                            let brush = self.base_brush();
                            let color = self.current_color();
                            match self.shape_kind {
                                ShapeKind::Line => {
                                    self.doc.shape_line(
                                        &brush,
                                        color,
                                        Some((ax, ay)),
                                        (bx, by),
                                        0.0,
                                    );
                                }
                                ShapeKind::Rect => {
                                    if self.shape_fill {
                                        self.doc.fill_rect(
                                            &brush, color, ax as u32, ay as u32, bx as u32,
                                            by as u32,
                                        );
                                    } else {
                                        self.doc.shape_rect(&brush, color, ax, ay, bx, by);
                                    }
                                }
                                ShapeKind::Ellipse => {
                                    if self.shape_fill {
                                        self.doc.fill_ellipse(&brush, color, ax, ay, bx, by);
                                    } else {
                                        self.doc.shape_ellipse(&brush, color, ax, ay, bx, by);
                                    }
                                }
                                ShapeKind::Triangle => {
                                    if self.shape_fill {
                                        self.doc.fill_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Triangle,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    } else {
                                        self.doc.shape_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Triangle,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    }
                                }
                                ShapeKind::Diamond => {
                                    if self.shape_fill {
                                        self.doc.fill_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Diamond,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    } else {
                                        self.doc.shape_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Diamond,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    }
                                }
                                ShapeKind::Star => {
                                    if self.shape_fill {
                                        self.doc.fill_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Star,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    } else {
                                        self.doc.shape_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Star,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    }
                                }
                                ShapeKind::Arrow => {
                                    self.doc.shape_arrow(&brush, color, ax, ay, bx, by);
                                }
                                ShapeKind::Polygon => {
                                    if self.shape_fill {
                                        self.doc.fill_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Polygon,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    } else {
                                        self.doc.shape_poly(
                                            &brush,
                                            color,
                                            ShapePoly::Polygon,
                                            ax,
                                            ay,
                                            bx,
                                            by,
                                            self.shape_sides,
                                        );
                                    }
                                }
                            }
                            self.status = format!("{} drawn.", self.shape_kind.name());
                            self.capture_timelapse();
                        }
                        self.shape_anchor = None;
                        self.shape_current = None;
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Text {
                    // Text layers: tapping empty canvas anchors a new block
                    // and opens the dialog; tapping existing text opens it
                    // pre-loaded for in-place editing (Procreate-style).
                    // drag_started counts as a tap too: touch jitter crosses
                    // the click-distance threshold, so an iPad tap arrives
                    // as a micro-drag and never fires `clicked` — without
                    // this the dialog could never open on device.
                    if (resp.clicked() || resp.drag_started()) && !gesture {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let (x, y) = to_canvas(pos);
                            if self.text_open {
                                // Re-anchor the open dialog (new or edit mode).
                                self.text_anchor = Some(clamp_pt(x, y));
                            } else if self.doc.paint_target != PaintTarget::Color {
                                self.status = "Switch to a color layer to edit text.".to_string();
                            } else if let Some(i) = self.doc.text_layer_at(x, y) {
                                if let Some(block) = self.doc.layers[i].text.clone() {
                                    self.doc.active = i;
                                    self.editing_text = Some(i);
                                    self.editing_loaded =
                                        Some((block.text.clone(), block.x, block.y));
                                    self.text_anchor = Some((block.x, block.y));
                                    self.text_body = block.text.clone();
                                    self.text_size = block.size_px;
                                    // A loaded face shows as "Imported" (no
                                    // stored filename), never the wrong bundle.
                                    self.text_font_name = None;
                                    self.text_font_bytes = block.style.font_bytes.clone();
                                    self.text_leading = block.style.leading;
                                    self.text_tracking = block.style.tracking;
                                    match block.style.outline {
                                        Some((c, px)) => {
                                            self.text_outline = true;
                                            self.text_outline_w = px;
                                            self.text_outline_rgb = [c.0[0], c.0[1], c.0[2]];
                                        }
                                        None => self.text_outline = false,
                                    }
                                    self.text_open = true;
                                    self.status = format!(
                                        "Editing text layer '{}'.",
                                        self.doc.layers[i].name
                                    );
                                }
                            } else {
                                self.text_anchor = Some(clamp_pt(x, y));
                                self.editing_text = None;
                                self.editing_loaded = None;
                                self.text_open = true;
                            }
                        }
                    }
                    self.painting = false;
                    self.last = None;
                    return;
                }

                if self.tool == Tool::Smudge {
                    // Finger-paint mixing: pick up color and push it along
                    // the path (mask layers steer here with a hint; the
                    // engine treats them as a no-op).
                    // A motionless tap never reports a drag; its click is
                    // stamped as a single dab below (see the Fill note).
                    let click_dab = resp.clicked();
                    if (resp.drag_started() || click_dab) && !gesture && !self.text_paint_blocked()
                    {
                        self.doc.checkpoint();
                        self.painting = true;
                        self.stroke_painted = false;
                        // Smudging edits the current frame: pause playback so
                        // the viewed frame never goes stale mid-stroke.
                        if self.anim_playing {
                            self.anim_playing = false;
                            self.status = "Paused for smudging.".to_string();
                        } else if self.doc.paint_target == PaintTarget::Mask {
                            self.status = "Smudge works on color layers.".to_string();
                        }
                        self.last = None;
                        self.leftover = 0.0;
                        if click_dab {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                let (x, y) = to_canvas(pos);
                                let (x, y) = clamp_pt(x, y);
                                let pressure = latest_touch_force(&ctx).unwrap_or(1.0);
                                let (last, leftover) = self.doc.smudge_to(
                                    &self.brush,
                                    self.smudge_strength,
                                    pressure,
                                    self.last,
                                    (x, y),
                                    self.leftover,
                                );
                                self.last = last;
                                self.leftover = leftover;
                                self.stroke_painted = true;
                            }
                            if self.stroke_painted {
                                self.stroke_painted = false;
                                self.capture_timelapse();
                            }
                            self.painting = false;
                            self.last = None;
                        }
                    }
                    if self.painting && resp.dragged() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let (x, y) = to_canvas(pos);
                            let (x, y) = clamp_pt(x, y);
                            let pressure = latest_touch_force(&ctx).unwrap_or(1.0);
                            let (last, leftover) = self.doc.smudge_to(
                                &self.brush,
                                self.smudge_strength,
                                pressure,
                                self.last,
                                (x, y),
                                self.leftover,
                            );
                            self.last = last;
                            self.leftover = leftover;
                            self.stroke_painted = true;
                        }
                    }
                    if resp.drag_stopped() {
                        if self.stroke_painted {
                            self.stroke_painted = false;
                            self.capture_timelapse();
                        }
                        self.painting = false;
                        self.last = None;
                        self.last_target = None;
                    }
                    return;
                }

                // A motionless tap never reports a drag; its click stamps a
                // single dab below (see the Fill note).
                let click_dab = resp.clicked();
                if (resp.drag_started() || click_dab) && !gesture && !self.text_paint_blocked() {
                    self.doc.checkpoint();
                    self.painting = true;
                    self.stroke_painted = false;
                    // Painting edits the current frame: pause playback so
                    // the viewed frame never goes stale mid-stroke.
                    if self.anim_playing {
                        self.anim_playing = false;
                        self.status = "Paused for painting.".to_string();
                    }
                    self.last = None;
                    self.last_target = None;
                    self.last_dyn = DabDynamics::default();
                    self.leftover = 0.0;
                    self.smooth_pt = None;
                    self.stroke_curve.reset();
                    self.stroke_record.reset();
                    self.pending_pts.clear();
                    self.smooth_pressure = 1.0;
                    self.last_t = ctx.input(|i| i.time);
                    // Drawing Assist: remember the stroke start; the lock
                    // engages on first travel and projects every dab after.
                    if self.assist && self.guides.kind != GuideKind::Off {
                        if let Some(pos) = resp.hover_pos() {
                            let (x, y) = to_canvas(pos);
                            self.assist_start = Some(clamp_pt(x, y));
                        } else {
                            self.assist_start = None;
                        }
                        self.assist_lock = None;
                    } else {
                        self.assist_start = None;
                        self.assist_lock = None;
                    }
                    // Motionless click: no dragged() frame follows, so stamp
                    // the single dab now, through the same single-point
                    // pipeline the drag path uses for a first sample.
                    if click_dab {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let (x, y) = to_canvas(pos);
                            let (x, y) = clamp_pt(x, y);
                            let (sx, sy) = self.guides.snap_point(w, h, x, y);
                            let pressure = latest_touch_force(&ctx).unwrap_or(1.0);
                            let brush = self.base_brush();
                            let color = self.current_color();
                            let from_dyn = if self.last.is_some() {
                                self.last_dyn
                            } else {
                                DabDynamics::default()
                            };
                            let to_dyn = DabDynamics {
                                pressure,
                                speed_norm: 0.0,
                                angle: self.last_dyn.angle,
                                taper: 1.0,
                            };
                            let (last, leftover) = self.doc.stroke_to_dyn(
                                &brush,
                                color,
                                self.last,
                                from_dyn,
                                (sx, sy),
                                to_dyn,
                                self.leftover,
                            );
                            self.last = last;
                            self.leftover = leftover;
                            self.last_dyn = to_dyn;
                            self.stroke_painted = true;
                            self.doc.end_stroke();
                        }
                        if self.stroke_painted {
                            self.stroke_painted = false;
                            self.capture_timelapse();
                        }
                        self.painting = false;
                        self.last = None;
                        self.last_target = None;
                        self.smooth_pt = None;
                        self.assist_start = None;
                        self.assist_lock = None;
                    }
                }
                // A flick hands us a canvas-length of travel in one frame.
                // Stamping it whole stalls the frame, and the next frame
                // then arrives later with even more samples queued, so the
                // batch is capped at `STAMP_BUDGET` and the tail carries
                // over. The release frame has no drag event and stamps the
                // rest before the tail flush, so a lift never loses ink.
                let backlog = !self.pending_pts.is_empty();
                if self.painting && (resp.dragged() || backlog) {
                    let mut pts = std::mem::take(&mut self.pending_pts);
                    let mut fresh = 0;
                    let mut live = false;
                    if resp.dragged() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            live = true;
                            // Stamp through every pointer position the
                            // backend recorded this frame (device rate), not
                            // just the frame-end hover point: fast strokes
                            // stop turning into straight chords between
                            // frames.
                            let moved: Vec<egui::Pos2> = ctx.input(|i| {
                                i.events
                                    .iter()
                                    .filter_map(|e| match e {
                                        egui::Event::PointerMoved(p) => Some(*p),
                                        _ => None,
                                    })
                                    .collect()
                            });
                            fresh = moved.len();
                            pts.extend(moved);
                            if pts.last() != Some(&pos) {
                                pts.push(pos);
                                fresh += 1;
                            }
                        }
                    }
                    if !pts.is_empty() {
                        let now = ctx.input(|i| i.time);
                        let frame_dt = (now - self.last_t).max(1.0 / 240.0) as f32;
                        self.last_t = now;
                        // A carried-over sample was paced by an earlier
                        // frame; keeping "fresh" out of the divisor leaves
                        // the live dt device-rate instead of pretending the
                        // backlog arrived this frame.
                        let dt = frame_dt / fresh.max(1) as f32;
                        let brush = self.base_brush();
                        // Pressure: Pencil/touch force when the backend
                        // forwards it, else full pressure (mice have no
                        // sensor; velocity taper below still applies).
                        let pressure = latest_touch_force(&ctx).unwrap_or(1.0);
                        // Stabilizer: exponential smoothing toward the raw
                        // pointer; 0 keeps the raw point bit-for-bit. Spread
                        // across the trail points so per-frame convergence
                        // (and with it the feel) matches the single-step form.
                        let a = brush.stabilizer.clamp(0.0, 1.0) * 0.85;
                        let a = stabilizer_substep(a, fresh.max(1));
                        let started = std::time::Instant::now();
                        let mut next = 0;
                        while next < pts.len() {
                            let pt = pts[next];
                            next += 1;
                            let (x, y) = to_canvas(pt);
                            // Clamp: off-page drags paint nothing instead of
                            // streaking the canvas edge (and poisoning the
                            // dirty rect, which used to quit the app).
                            let (x, y) = clamp_pt(x, y);
                            let target = (x, y);
                            let ((sx, sy), speed_norm) = sample_pointer(
                                self.smooth_pt,
                                self.last_target,
                                target,
                                a,
                                dt,
                                self.zoom,
                            );
                            self.smooth_pt = Some((sx, sy));
                            self.last_target = Some(target);
                            // Drawing Assist: ride an engaged ray/axis lock,
                            // else fall back to guide point-snap (shapes stay
                            // free). Projected points re-clamp so rays running
                            // off-canvas never streak the edge.
                            let (sx, sy) = if self.assist && self.guides.kind != GuideKind::Off {
                                match self.assist_lock {
                                    Some(lock) => {
                                        let (px, py) = lock.project(sx, sy);
                                        clamp_pt(px, py)
                                    }
                                    None => {
                                        let lock = self.assist_start.and_then(|s| {
                                            self.guides.assist_lock(w, h, s, (sx, sy))
                                        });
                                        match lock {
                                            Some(l) => {
                                                self.assist_lock = Some(l);
                                                let (px, py) = l.project(sx, sy);
                                                clamp_pt(px, py)
                                            }
                                            None => self.guides.snap_point(w, h, sx, sy),
                                        }
                                    }
                                }
                            } else {
                                self.guides.snap_point(w, h, sx, sy)
                            };
                            self.smooth_pressure += (pressure - self.smooth_pressure) * (1.0 - a);
                            let angle = match self.last {
                                Some((lx, ly)) => {
                                    let dx = sx - lx;
                                    let dy = sy - ly;
                                    if dx.hypot(dy) > 0.001 {
                                        dy.atan2(dx)
                                    } else {
                                        self.last_dyn.angle
                                    }
                                }
                                None => self.last_dyn.angle,
                            };
                            let from_dyn = if self.last.is_some() {
                                self.last_dyn
                            } else {
                                DabDynamics::default()
                            };
                            let base_dyn = DabDynamics {
                                pressure: self.smooth_pressure,
                                speed_norm,
                                angle,
                                taper: 1.0,
                            };
                            // Live taper: only the start ramp is known while
                            // the pen is down. The end ramp needs the final
                            // length, so it is applied once on release — the
                            // live dab must not use it, or the stroke paints
                            // thin and is repainted full-width at lift.
                            let base_len = self.stroke_record.total_len;
                            let taper_of = |from: Option<(f32, f32)>, to: (f32, f32)| {
                                let leg = from.map_or(0.0, |(fx, fy)| {
                                    ((to.0 - fx).powi(2) + (to.1 - fy).powi(2)).sqrt()
                                });
                                live_taper_at(&brush, base_len, leg)
                            };
                            let mut last_taper = from_dyn.taper;
                            let mut stamped_dyn = base_dyn;
                            let color = self.current_color();
                            // Ink the smoothed path (see StrokeCurve): the
                            // stroke's first point lands a dab, every later
                            // point stamps its curved piece in short linear
                            // steps so sparse samples still draw curves.
                            match self.stroke_curve.push((sx, sy)) {
                                None => {
                                    let from = self.last;
                                    let mut from_dyn = from_dyn;
                                    from_dyn.taper = last_taper;
                                    let to_dyn = DabDynamics {
                                        taper: taper_of(from, (sx, sy)),
                                        ..base_dyn
                                    };
                                    let (last, leftover) = self.doc.stroke_to_dyn_spaced(
                                        &brush,
                                        color,
                                        from,
                                        from_dyn,
                                        (sx, sy),
                                        to_dyn,
                                        self.leftover,
                                    );
                                    self.stroke_record.push(from, from_dyn, (sx, sy), to_dyn);
                                    self.last = last;
                                    self.leftover = leftover;
                                    stamped_dyn = to_dyn;
                                }
                                Some((p0, ctrl, p1)) => {
                                    let piece =
                                        ((p1.0 - p0.0).powi(2) + (p1.1 - p0.1).powi(2)).sqrt();
                                    let steps = stroke_steps(piece);
                                    for k in 1..=steps {
                                        let t = k as f32 / steps as f32;
                                        let (px, py) = stroke_quad_pt(p0, ctrl, p1, t);
                                        let from = self.last;
                                        let mut from_dyn = from_dyn;
                                        from_dyn.taper = last_taper;
                                        let to_dyn = DabDynamics {
                                            taper: taper_of(from, (px, py)),
                                            ..base_dyn
                                        };
                                        let (last, leftover) = self.doc.stroke_to_dyn_spaced(
                                            &brush,
                                            color,
                                            from,
                                            from_dyn,
                                            (px, py),
                                            to_dyn,
                                            self.leftover,
                                        );
                                        self.stroke_record.push(from, from_dyn, (px, py), to_dyn);
                                        self.last = last;
                                        self.leftover = leftover;
                                        last_taper = to_dyn.taper;
                                        stamped_dyn = to_dyn;
                                    }
                                }
                            }
                            self.last_dyn = stamped_dyn;
                            self.stroke_painted = true;
                            // Out of budget while the pen is still down:
                            // hand the rest to the next frame. A release
                            // (no live drag) always finishes the batch.
                            if live && started.elapsed() >= STAMP_BUDGET && next < pts.len() {
                                self.pending_pts.extend_from_slice(&pts[next..]);
                                break;
                            }
                        }
                    }
                }
                if resp.drag_stopped() {
                    if self.painting {
                        let brush = self.base_brush();
                        let color = self.current_color();
                        if flush_stroke_tail(
                            &mut self.doc,
                            &brush,
                            color,
                            &mut self.stroke_curve,
                            &mut self.last,
                            self.last_dyn,
                            &mut self.leftover,
                            &mut self.stroke_record,
                        ) {
                            self.stroke_painted = true;
                        }
                        // The replay rewinds only the stroke bbox, so grab
                        // it before closing the stroke scope.
                        let rect = self.doc.stroke_rect().map(|(x0, y0, x1, y1)| {
                            (x0.saturating_sub(2), y0.saturating_sub(2), x1 + 2, y1 + 2)
                        });
                        // Close the live stroke before the tapered replay:
                        // the replay re-renders from the checkpoint and
                        // opens its own coverage peak.
                        self.doc.end_stroke();
                        if self.stroke_painted {
                            self.taper_job = taper_begin(
                                &mut self.doc,
                                &brush,
                                color,
                                &self.stroke_record,
                                rect,
                            );
                        }
                        // A replay owns the record until it finishes (a long
                        // 8K stroke would otherwise freeze the lift frame).
                        if self.taper_job.is_none() {
                            self.stroke_record.reset();
                        }
                    }
                    if self.stroke_painted {
                        self.stroke_painted = false;
                        // The timelapse frame should show the tapered
                        // result, so a pending replay captures on finish.
                        if self.taper_job.is_none() {
                            self.capture_timelapse();
                        } else {
                            self.taper_capture = true;
                        }
                    }
                    self.painting = false;
                    self.last = None;
                    self.last_target = None;
                    self.smooth_pt = None;
                    self.assist_start = None;
                    self.assist_lock = None;
                }
            });
        });

        if shell == Shell::Mobile {
            self.mobile_chrome(ui.ctx(), class);
        }
    }
}
