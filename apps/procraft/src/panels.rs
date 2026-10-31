use crate::*;

impl ProCraftApp {
    /// Zoom/rotate/About cluster shared by the wide bar (inline) and the
    /// narrow/tablet bar (inside a View menu). `in_menu` dismisses the popup
    /// after momentary actions, matching the File menu convention; inline
    /// clicks must not close anything.
    pub(crate) fn view_controls(&mut self, ui: &mut egui::Ui, in_menu: bool) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("−").clicked() {
                self.zoom = (self.zoom / 1.25).clamp(0.1, 8.0);
            }
            // The ladder is 1.25x off a fractional fit zoom, so 100 % is not
            // on it. The percentage is the way there (and where the canvas
            // lands on whole pixels instead of being resampled).
            if ui
                .add(
                    egui::Label::new(format!("{}%", (self.zoom * 100.0).round() as u32))
                        .sense(Sense::click()),
                )
                .on_hover_text("Click for 100%")
                .clicked()
            {
                self.zoom = 1.0;
            }
            if ui.button("+").clicked() {
                self.zoom = (self.zoom * 1.25).clamp(0.1, 8.0);
            }
            if self.pill(ui, "Fit") {
                if in_menu {
                    ui.close();
                }
                self.zoom = 0.0; // recomputed in the canvas view
                self.view_offset = egui::Vec2::ZERO;
                self.view_angle = 0.0;
            }
        });
        let mut deg = self.view_angle.to_degrees();
        // Desktop keeps the rail short so the cluster shares one row; touch
        // menus have the width to keep the full rail.
        let rotate_rail = if self.tokens(ui).class == BarLayout::Wide {
            90.0
        } else {
            140.0
        };
        ui.scope(|ui| {
            ui.spacing_mut().slider_width = rotate_rail;
            stepped_slider(ui, &mut deg, -180.0..=180.0, 1.0, "Rotate°");
        });
        self.view_angle = deg.to_radians();
        if self.pill(ui, "Reset") {
            if in_menu {
                ui.close();
            }
            self.view_angle = 0.0;
        }
        // The wide bar carries the one theme control right before About.
        // Touch bars render it outside this cluster (inside the View menu it
        // would need two taps).
        if !in_menu {
            let t = self.tokens(ui);
            if design::theme_toggle(ui, &t, &mut self.theme) {
                self.status = format!("{} theme.", self.theme.name());
            }
        }
        let about = if self.has_symbols {
            "ⓘ About"
        } else {
            "About"
        };
        if self.pill(ui, about) {
            if in_menu {
                ui.close();
            }
            self.show_about = !self.show_about;
        }
        let shortcuts = if self.has_symbols {
            "⌨ Shortcuts"
        } else {
            "Shortcuts"
        };
        if self.pill(ui, shortcuts) {
            if in_menu {
                ui.close();
            }
            self.show_shortcuts = !self.show_shortcuts;
        }
    }

    /// Tools panel body shared by the desktop dock and the mobile drawer.
    pub(crate) fn tools_panel_body(&mut self, ui: &mut egui::Ui, compact: bool) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let t = self.tokens(ui);
            let mut close_tools = false;
            design::section_card_actions(
                ui,
                &t,
                Some("Tool"),
                false,
                |ui| {
                    if ui.add_sized([40.0, 24.0], egui::Button::new("<")).clicked() {
                        close_tools = true;
                    }
                },
                |ui| {
                let was_fill = self.tool == Tool::Fill;
                // A one-column list on the dock; the mobile drawer packs two
                // columns, or three when it is shot (landscape) so every tool
                // fits without scrolling through a half-empty sheet.
                let cols = tool_grid_columns(ui.available_width(), compact, mobile_shell());
                let entries: Vec<(Tool, &'static str, String)> = TOOL_ORDER
                    .iter()
                    .map(|&tool| {
                        (
                            tool,
                            tool_label(tool, self.has_symbols),
                            self.tool_hint(tool_id_for(tool)),
                        )
                    })
                    .collect();
                if cols > 1 {
                    let gap = ui.spacing().item_spacing.x;
                    let cell_w =
                        ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32)
                            .max(96.0);
                    let row_h = t.control_h;
                    for row in entries.chunks(cols) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for (tool, label, hint) in row {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(cell_w, row_h),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.radio_value(&mut self.tool, *tool, *label)
                                            .on_hover_text(hint);
                                    },
                                );
                            }
                        });
                    }
                } else {
                    for (tool, label, hint) in entries {
                        ui.radio_value(&mut self.tool, tool, label)
                            .on_hover_text(hint);
                    }
                }
                if was_fill && self.tool != Tool::Fill {
                    self.fill_preview = None;
                }
                if self.tool == Tool::Fill {
                    let tol_before = self.fill_tolerance;
                    let grow_before = self.fill_grow;
                    stepped_slider(ui, &mut self.fill_tolerance, 0..=255, 1.0, "Tolerance");
                    stepped_slider(ui, &mut self.fill_grow, 0..=16, 1.0, "Grow");
                    if (self.fill_tolerance != tol_before || self.fill_grow != grow_before)
                        && self.fill_preview.is_some()
                    {
                        self.reapply_live_fill();
                    }
                    ui.label("Tap fills; hold + drag sideways to sweep tolerance. Tolerance and Grow stay live after a fill; Grow paints the anti-aliased edge and stops at dark line art.");
                }
                if self.tool == Tool::Picker {
                    ui.label("Tap the canvas to lift its color; the loupe magnifies the target.");
                }
                if self.tool == Tool::Smudge {
                    stepped_slider(ui, &mut self.smudge_strength, 0.0..=1.0, 0.05, "Strength");
                    ui.label("Drag to push paint around (color layers).");
                }
                if self.tool == Tool::Liquify {
                    stepped_slider(ui, &mut self.liquify_size, 8.0..=512.0, 1.0, "Size");
                    stepped_slider(ui, &mut self.liquify_strength, 0.05..=1.0, 0.05, "Strength");
                    ui.label("Drag to push pixels (one undo step per stroke).");
                }
                if self.tool == Tool::Crop {
                    ui.label("Drag a rect on the canvas, then Apply.");
                    if let Some((x0, y0, x1, y1)) = self.crop_rect {
                        ui.label(format!("Crop {}x{} px.", x1 - x0 + 1, y1 - y0 + 1));
                        if ui.button("Apply crop").clicked() {
                            match self.doc.crop(x0, y0, x1, y1) {
                                Ok(()) => {
                                    let (w, h) = (self.doc.w, self.doc.h);
                                    self.after_canvas_change();
                                    self.status =
                                        format!("Cropped to {w}x{h} (history cleared).");
                                }
                                Err(e) => self.status = format!("Crop failed: {e}"),
                            }
                        }
                        if ui.button("Cancel crop").clicked() {
                            self.crop_rect = None;
                            self.status = "Crop cancelled.".to_string();
                        }
                    }
                    ui.collapsing("Canvas size", |ui| {
                        // Re-adopt the real dims after an open/crop/apply; typing
                        // never changes the doc, so edits are never clobbered.
                        if self.canvas_sync != (self.doc.w, self.doc.h) {
                            self.canvas_sync = (self.doc.w, self.doc.h);
                            self.canvas_w = self.doc.w;
                            self.canvas_h = self.doc.h;
                        }
                        stepped_slider(ui, &mut self.canvas_w, 1..=MAX_CANVAS_SIDE, 1.0, "Width");
                        stepped_slider(ui, &mut self.canvas_h, 1..=MAX_CANVAS_SIDE, 1.0, "Height");
                        ui.label("Anchor (old canvas sits here):");
                        for ay in -1..=1 {
                            ui.horizontal(|ui| {
                                for ax in -1..=1 {
                                    let mark =
                                        if self.canvas_anchor == (ax, ay) { "■" } else { "·" };
                                    if ui
                                        .add_sized([32.0, 24.0], egui::Button::new(mark))
                                        .clicked()
                                    {
                                        self.canvas_anchor = (ax, ay);
                                    }
                                }
                            });
                        }
                        if ui.button("Apply size").clicked() {
                            let (ax, ay) = self.canvas_anchor;
                            let place = |new: u32, old: u32, a: i32| {
                                let rest = new as i32 - old as i32;
                                match a {
                                    -1 => 0,
                                    1 => rest,
                                    _ => rest / 2,
                                }
                            };
                            let dx = place(self.canvas_w, self.doc.w, ax);
                            let dy = place(self.canvas_h, self.doc.h, ay);
                            match self.doc.rehome_canvas(self.canvas_w, self.canvas_h, dx, dy) {
                                Ok(()) => {
                                    let (w, h) = (self.doc.w, self.doc.h);
                                    self.after_canvas_change();
                                    self.status =
                                        format!("Canvas {w}x{h} (history cleared).");
                                }
                                Err(e) => self.status = format!("Resize failed: {e}"),
                            }
                        }
                        ui.label("Crop and resize clear undo history.");
                    });
                }
                if self.tool == Tool::Shapes {
                    ui.horizontal_wrapped(|ui| {
                        for k in ShapeKind::ALL {
                            ui.radio_value(&mut self.shape_kind, k, k.name());
                        }
                    });
                    if self.shape_kind.fillable() {
                        ui.checkbox(&mut self.shape_fill, "Fill");
                    }
                    if self.shape_kind == ShapeKind::Polygon {
                        stepped_slider(ui, &mut self.shape_sides, 3..=12, 1.0, "Sides");
                    }
                }
            });
            if close_tools {
                self.show_tools = false;
            }
            let has_selection = self.doc.selection.is_some() || self.doc.selection_mask.is_some();
            ui.add_space(t.gap);
            design::section_card(ui, &t, None, false, |ui| {
                if ui
                    .add_enabled(has_selection, egui::Button::new("Clear selection"))
                    .clicked()
                {
                    self.doc.clear_selection();
                    self.lasso_poly.clear();
                    self.sel_edges_dirty = true;
                    self.status = "Selection cleared.".to_string();
                }
                ui.collapsing("Selection Studio", |ui| {
                    if !has_selection {
                        ui.label("No selection yet — drag Marquee/Lasso on the canvas, or Select opaque.");
                    }
                    stepped_slider(ui, &mut self.sel_feather, 0..=32, 1.0, "Feather px");
                    if ui.button("Apply feather").clicked() {
                        if self.doc.feather_selection(self.sel_feather) {
                            self.status = format!("Feathered ({} px).", self.doc.selection_pixel_count());
                        } else {
                            self.status = "No selection to feather.".to_string();
                        }
                        self.sel_edges_dirty = true;
                    }
                    stepped_slider(ui, &mut self.sel_grow, -32..=32, 1.0, "Grow px");
                    if ui.button("Apply grow").clicked() {
                        if self.doc.grow_selection(self.sel_grow) {
                            self.status = format!("Grown ({} px).", self.doc.selection_pixel_count());
                        } else {
                            self.status = "No selection to grow.".to_string();
                        }
                        self.sel_edges_dirty = true;
                    }
                    if ui.button("Smooth").clicked() {
                        if self.doc.smooth_selection() {
                            self.status = format!("Smoothed ({} px).", self.doc.selection_pixel_count());
                        } else {
                            self.status = "No selection to smooth.".to_string();
                        }
                        self.sel_edges_dirty = true;
                    }
                    if ui.button("Invert").clicked() {
                        if self.doc.invert_selection() {
                            self.status = format!("Inverted ({} px).", self.doc.selection_pixel_count());
                        } else {
                            self.status = "No selection to invert.".to_string();
                        }
                        self.sel_edges_dirty = true;
                    }
                    stepped_slider(ui, &mut self.sel_opaque_t, 0..=255, 1.0, "Opacity >");
                    if ui.button("Select opaque").clicked() {
                        if self.doc.select_opaque(self.sel_opaque_t) {
                            self.status = format!("Opaque selected ({} px).", self.doc.selection_pixel_count());
                        } else {
                            self.status = "Layer is empty.".to_string();
                        }
                        self.sel_edges_dirty = true;
                    }
                    stepped_slider(ui, &mut self.sel_stroke_w, 1..=16, 1.0, "Outline px");
                    if ui.button("Paint outline").clicked() && !self.text_paint_blocked() {
                        self.doc.checkpoint();
                        let brush = self.base_brush();
                        let color = self.current_color();
                        if self.doc.stroke_selection(&brush, color, self.sel_stroke_w) {
                            self.status = "Outlined.".to_string();
                        } else {
                            self.status = "No selection to outline.".to_string();
                        }
                    }
                    // M35 Selection upgrades: save/load slot (session-only;
                    // Load refuses size mismatches) plus nudge/flip transform.
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Save selection").clicked() {
                            if self.doc.selection.is_some() || self.doc.selection_mask.is_some()
                            {
                                self.saved_sel = Some((
                                    (self.doc.w, self.doc.h),
                                    self.doc.selection,
                                    self.doc.selection_mask.clone(),
                                ));
                                self.status = format!(
                                    "Selection saved ({} px).",
                                    self.doc.selection_pixel_count()
                                );
                            } else {
                                self.status = "No selection to save.".to_string();
                            }
                        }
                        if ui
                            .add_enabled(self.saved_sel.is_some(), egui::Button::new("Load selection"))
                            .clicked()
                        {
                            let dims_ok = self
                                .saved_sel
                                .as_ref()
                                .is_some_and(|(dims, _, _)| *dims == (self.doc.w, self.doc.h));
                            if dims_ok {
                                if let Some((_, rect, mask)) = self.saved_sel.clone() {
                                    self.doc.selection = rect;
                                    self.doc.selection_mask = mask;
                                    self.sel_edges_dirty = true;
                                    self.status = format!(
                                        "Selection loaded ({} px).",
                                        self.doc.selection_pixel_count()
                                    );
                                }
                            } else {
                                self.status =
                                    "Saved selection is a different canvas size.".to_string();
                            }
                        }
                    });
                    if let Some(((w, h), _, _)) = &self.saved_sel {
                        ui.label(format!("Saved slot: {w}x{h} canvas."));
                    }
                    stepped_slider(ui, &mut self.sel_nudge_x, -64..=64, 1.0, "Nudge x");
                    stepped_slider(ui, &mut self.sel_nudge_y, -64..=64, 1.0, "Nudge y");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Apply nudge").clicked() {
                            let (dx, dy) = (self.sel_nudge_x, self.sel_nudge_y);
                            if self.doc.translate_selection(dx, dy) {
                                self.status = format!(
                                    "Nudged ({dx},{dy}, {} px).",
                                    self.doc.selection_pixel_count()
                                );
                            } else {
                                self.status = "Nudge cleared the selection.".to_string();
                            }
                            self.sel_edges_dirty = true;
                        }
                    if ui.button(if self.has_symbols { "⇄ Flip H" } else { "Flip H" }).clicked() {
                            if self.doc.flip_selection_h() {
                                self.status = "Flipped horizontal.".to_string();
                            } else {
                                self.status = "No selection to flip.".to_string();
                            }
                            self.sel_edges_dirty = true;
                        }
                    if ui.button(if self.has_symbols { "⇅ Flip V" } else { "Flip V" }).clicked() {
                            if self.doc.flip_selection_v() {
                                self.status = "Flipped vertical.".to_string();
                            } else {
                                self.status = "No selection to flip.".to_string();
                            }
                            self.sel_edges_dirty = true;
                        }
                    });
                });
            });
            ui.add_space(t.gap);
            design::section_card(ui, &t, Some("Brushes"), false, |ui| {
                // Theme ink: light strokes on dark theme, dark strokes otherwise.
                let ink = match self.theme {
                    Theme::Dark => Color::rgb(215, 215, 215),
                    _ => Color::rgb(70, 70, 70),
                };
                if self.preview_theme != Some(self.theme)
                    || self.preset_textures.len() != self.preview_count()
                    || (self.preview_scale - ui.ctx().pixels_per_point()).abs() > 0.01
                {
                    let ctx = ui.ctx().clone();
                    self.build_preset_previews(&ctx, ink);
                    self.preview_theme = Some(self.theme);
                }
                let builtin = procraft_engine::presets();
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 3.0);
                    for (idx, (name, preset)) in builtin.into_iter().enumerate() {
                        // Name and thumbnail both apply the brush; the active
                        // brush reads highlighted.
                        let active = self.brush_slot_active(BrushSlot::Builtin(idx));
                        ui.horizontal(|ui| {
                            if ui.add_sized([64.0, 22.0], egui::Button::new(name).selected(active)).clicked() {
                                self.choose_brush(BrushSlot::Builtin(idx), &preset);
                            }
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                if let Some(tex) = self.preset_textures.get(idx) {
                                    let thumb = ui.add(
                                        egui::Image::new((tex.id(), Vec2::new(PREVIEW_W, PREVIEW_H)))
                                            .sense(Sense::click()),
                                    );
                                    if thumb.clicked() {
                                        self.choose_brush(BrushSlot::Builtin(idx), &preset);
                                    }
                                }
                            });
                        });
                    }
                });
                ui.separator();
                ui.collapsing("My brushes", |ui| {
                    ui.horizontal(|ui| {
                        // Fixed widths: a free-size text edit would keep pushing the
                        // whole left panel wider every frame.
                        ui.add_sized(
                            [110.0, 22.0],
                            egui::TextEdit::singleline(&mut self.new_brush_name).hint_text("Name"),
                        );
                        if ui.add_sized([90.0, 22.0], egui::Button::new("Add current")).clicked() {
                            let want = self.new_brush_name.trim().to_string();
                            if want.is_empty() {
                                self.status = "Name the brush first.".to_string();
                            } else if self.custom_brushes.len() >= MAX_LIBRARY_BRUSHES {
                                self.status = format!("Brush library is full ({MAX_LIBRARY_BRUSHES}).");
                            } else {
                                let name = self.unique_brush_name(&want);
                                let brush = Brush { eraser: false, ..self.brush.clone() };
                                self.custom_brushes.push(BrushPreset { name: name.clone(), brush });
                                self.brush_slot =
                                    Some(BrushSlot::Custom(self.custom_brushes.len() - 1));
                                self.new_brush_name.clear();
                                self.status = format!("Brush '{name}' added.");
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Import").clicked() {
                            #[cfg(not(target_os = "ios"))]
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Brushes", BRUSH_IMPORT_EXTS)
                                .pick_file()
                            {
                                match load_brush_file_report(&path) {
                                    Ok(libs) => self.status = self.absorb_brushes(libs, &path),
                                    Err(e) => self.status = format!("Brush import failed: {e}"),
                                }
                            }
                            #[cfg(target_os = "ios")]
                            {
                                let _ = rfd::FileDialog::new()
                                    .add_filter("Brushes", BRUSH_IMPORT_EXTS)
                                    .pick_file();
                                self.pending_open = Some(PendingOpen::Brushes);
                                self.status = "Pick brushes…".to_string();
                            }
                            #[cfg(target_os = "android")]
                            {
                                self.android_browse = Some(PendingOpen::Brushes);
                            }
                        }
                        if ui.button("Export set").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("ProCraft brushes", &["pcbrush"])
                                .save_file()
                            {
                                let path = ensure_extension(path, "pcbrush");
                                match save_brush_library(&path, &self.custom_brushes) {
                                    Ok(()) => {
                                        self.status = format!("Brushes saved {}", path.display());
                                        #[cfg(target_os = "ios")]
                                        rfd::ios_share_file(&path);
                                        #[cfg(target_os = "android")]
                                        rfd::android_scan_file(&path);
                                    }
                                    Err(e) => self.status = format!("Brush save failed: {e}"),
                                }
                            } else {
                                note_save_declined(&mut self.status, "Brush set");
                            }
                        }
                        if ui.button("Tip PNG").clicked() {
                            // Grayscale tip of the current brush for Photoshop,
                            // Procreate, Krita and GIMP.
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("PNG image", &["png"])
                                .save_file()
                            {
                                let path = ensure_extension(path, "png");
                                let brush = Brush { eraser: false, ..self.brush.clone() };
                                match export_tip_png(&path, &brush) {
                                    Ok(()) => {
                                        self.status = format!("Tip saved {}", path.display());
                                        #[cfg(target_os = "ios")]
                                        rfd::ios_share_file(&path);
                                        #[cfg(target_os = "android")]
                                        rfd::android_scan_file(&path);
                                    }
                                    Err(e) => self.status = format!("Tip save failed: {e}"),
                                }
                            } else {
                                note_save_declined(&mut self.status, "Brush tip");
                            }
                        }
                    });
                    let custom_count = self.custom_brushes.len();
                    if custom_count > 0 {
                        let base = procraft_engine::presets().len();
                        let mut remove: Option<usize> = None;
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(6.0, 3.0);
                            for idx in 0..custom_count {
                                let (name, brush) = {
                                    let p = &self.custom_brushes[idx];
                                    (p.name.clone(), p.brush.clone())
                                };
                                let active = self.brush_slot_active(BrushSlot::Custom(idx));
                                ui.horizontal(|ui| {
                                    if ui.add_sized([64.0, 22.0], egui::Button::new(&name).selected(active)).clicked() {
                                        self.choose_brush(BrushSlot::Custom(idx), &brush);
                                    }
                                    egui::Frame::group(ui.style()).show(ui, |ui| {
                                        if let Some(tex) = self.preset_textures.get(base + idx) {
                                            let thumb = ui.add(
                                                egui::Image::new((tex.id(), Vec2::new(PREVIEW_W, PREVIEW_H)))
                                                    .sense(Sense::click()),
                                            );
                                            if thumb.clicked() {
                                                self.choose_brush(BrushSlot::Custom(idx), &brush);
                                            }
                                        }
                                    });
                                    let preset_del = if self.has_symbols { "✕" } else { "Del" };
                                    if ui.small_button(preset_del).clicked() {
                                        remove = Some(idx);
                                    }
                                });
                            }
                        });
                        if let Some(idx) = remove {
                            if let Some(name) = self.remove_custom_brush(idx) {
                                self.status = format!("Brush '{name}' removed.");
                            }
                        }
                    }
                });
            });
            ui.add_space(t.gap);
            design::section_card(ui, &t, Some("Color"), false, |ui| {
                let card = ui.scope(|ui| {
                    stepped_slider(ui, &mut self.picker_size, 140.0..=420.0, 5.0, "Picker px");
                    let mut c = Color32::from_rgb(self.color[0], self.color[1], self.color[2]);
                    // The SV square + hue strip size themselves from `slider_width`,
                    // not the panel width: drive it from the slider above.
                    let picker_width = self.picker_size;
                    ui.scope(|ui| {
                        ui.spacing_mut().slider_width = picker_width;
                        if egui::color_picker::color_picker_color32(
                            ui,
                            &mut c,
                            egui::color_picker::Alpha::Opaque,
                        ) {
                            self.color = [c.r(), c.g(), c.b()];
                        }
                    });
                });
                if self.mobile_scroll_color {
                    ui.scroll_to_rect(card.response.rect, Some(egui::Align::Center));
                    self.mobile_scroll_color = false;
                }
            });
            ui.add_space(t.gap);
            design::section_card(ui, &t, Some("Swatches"), false, |ui| {
                ui.label("Starter (click applies):");
                swatch_grid(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(SWATCH_GAP, SWATCH_GAP);
                        for rgb in starter_palette().colors {
                            let resp = swatch_button(ui, rgb, self.color == rgb);
                            if resp.clicked() {
                                self.color = rgb;
                            }
                        }
                    });
                });
                // Actions first, then the active palette on its own line:
                // one long `Palettes: < name >` row overflowed the card and
                // clipped the name at the right edge.
                ui.horizontal_wrapped(|ui| {
                    ui.label("Palettes:");
                    let n = self.palettes.palettes.len();
                    if ui.button("<").clicked() && n > 0 {
                        self.palettes.set_active((self.palettes.active + n - 1) % n);
                    }
                    if ui.button(">").clicked() && n > 0 {
                        self.palettes.set_active((self.palettes.active + 1) % n);
                    }
                    if ui.add_enabled(n < MAX_PALETTES, egui::Button::new("+ New")).clicked() {
                        let name = self.palettes.unique_name("Palette");
                        match self.palettes.add(&name) {
                            Ok(_) => self.status = format!("Palette {name} added."),
                            Err(e) => self.status = format!("Palette not added: {e}"),
                        }
                    }
                });
                let n = self.palettes.palettes.len();
                let (pname, pcount) = self
                    .palettes
                    .active()
                    .map(|p| (p.name.clone(), p.colors.len()))
                    .unwrap_or_else(|| ("-".to_string(), 0));
                let pos = if n == 0 { 0 } else { self.palettes.active + 1 };
                ui.label(format!("{pname} ({pos}/{n} · {pcount} swatches)"));
                ui.horizontal(|ui| {
                    if !self.pal_renaming {
                        if ui.button("Rename").clicked() {
                            self.pal_name_edit =
                                self.palettes.active().map(|p| p.name.clone()).unwrap_or_default();
                            self.pal_renaming = true;
                        }
                        if ui.button("Delete").clicked() {
                            let idx = self.palettes.active;
                            let name =
                                self.palettes.active().map(|p| p.name.clone()).unwrap_or_default();
                            self.palettes.remove(idx);
                            self.status = format!("Palette {name} deleted.");
                        }
                    } else {
                        ui.text_edit_singleline(&mut self.pal_name_edit);
                        if ui.button("Save").clicked() {
                            let idx = self.palettes.active;
                            let edit = self.pal_name_edit.clone();
                            match self.palettes.rename(idx, &edit) {
                                Ok(()) => {
                                    self.pal_renaming = false;
                                    self.status = format!("Palette renamed {edit}.");
                                }
                                Err(e) => self.status = format!("Rename failed: {e}"),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.pal_renaming = false;
                        }
                    }
                });
                ui.label("Active palette (click applies, right-click removes):");
                let swatches =
                    self.palettes.active().map(|p| p.colors.clone()).unwrap_or_default();
                if swatches.is_empty() {
                    ui.label("Empty — store the current color with +.");
                } else {
                    let mut remove = None;
                    swatch_grid(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(SWATCH_GAP, SWATCH_GAP);
                            for (idx, rgb) in swatches.iter().enumerate() {
                                let resp = swatch_button(ui, *rgb, self.color == *rgb);
                                if resp.clicked() {
                                    self.color = *rgb;
                                }
                                if resp.secondary_clicked() {
                                    remove = Some(idx);
                                }
                            }
                        });
                    });
                    if let Some(idx) = remove {
                        self.palettes.remove_color(idx);
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("+ Current").clicked() {
                        if swatches.contains(&self.color) {
                            self.status = "That color is already stored.".to_string();
                        } else if swatches.len() >= MAX_PALETTE_COLORS {
                            self.status = format!("Swatch set is full ({MAX_PALETTE_COLORS}).");
                        } else {
                            let _ = self.palettes.push_color(self.color);
                            self.status = "Color stored.".to_string();
                        }
                    }
                    if ui.button("Import").clicked() {
                        #[cfg(not(target_os = "ios"))]
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Palettes", &["pcpalette", "json", "gpl"])
                            .pick_file()
                        {
                            match load_palette_file(&path) {
                                Ok(pal) => {
                                    self.status = self.absorb_colors(pal, &format!("{}", path.display()))
                                }
                                Err(e) => self.status = format!("Palette import failed: {e}"),
                            }
                        }
                        #[cfg(target_os = "ios")]
                        {
                            let _ = rfd::FileDialog::new()
                                .add_filter("Palettes", &["pcpalette", "json", "gpl"])
                                .pick_file();
                            self.pending_open = Some(PendingOpen::Palette);
                            self.status = "Pick palette…".to_string();
                        }
                        #[cfg(target_os = "android")]
                        {
                            self.android_browse = Some(PendingOpen::Palette);
                        }
                    }
                    if ui.button("Export").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("ProCraft palette", &["pcpalette"])
                            .save_file()
                        {
                            let path = ensure_extension(path, "pcpalette");
                            let pal = self.palettes.active().cloned().unwrap_or(Palette {
                                name: "My colors".to_string(),
                                colors: Vec::new(),
                            });
                            match save_palette(&path, &pal) {
                                Ok(()) => {
                                    self.status = format!("Palette saved {}", path.display());
                                    #[cfg(target_os = "ios")]
                                    rfd::ios_share_file(&path);
                                    #[cfg(target_os = "android")]
                                    rfd::android_scan_file(&path);
                                }
                                Err(e) => self.status = format!("Palette save failed: {e}"),
                            }
                        } else {
                            note_save_declined(&mut self.status, "Palette");
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Clear").clicked() {
                        self.palettes.clear_active();
                        self.status = "Active palette cleared.".to_string();
                    }
                    if ui.button("From image").clicked() {
                        #[cfg(not(target_os = "ios"))]
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter(
                                "Images",
                                &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
                            )
                            .pick_file()
                        {
                            let stem = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("Image")
                                .to_string();
                            let from = format!("{}", path.display());
                            match image::open(&path).map(|i| i.to_rgba8()) {
                                Ok(img) => {
                                    let (w, h) = (img.width(), img.height());
                                    self.status =
                                        self.image_palette_status(&stem, img.as_raw(), w, h, &from);
                                }
                                Err(e) => self.status = format!("Image open failed: {e}"),
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
                            self.pending_open = Some(PendingOpen::ImagePalette);
                            self.status = "Pick an image…".to_string();
                        }
                        #[cfg(target_os = "android")]
                        {
                            self.android_browse = Some(PendingOpen::ImagePalette);
                        }
                    }
                    if ui.button("From canvas").clicked() {
                        let flat = self.doc.flatten();
                        let (w, h) = (self.doc.w, self.doc.h);
                        self.status = self.image_palette_status("Canvas", &flat, w, h, "canvas");
                    }
                });
                ui.label("Harmony of the current color (click applies):");
                swatch_grid(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(SWATCH_GAP, SWATCH_GAP);
                        let cur = self.color;
                        let mut harmonies = vec![complementary(cur)];
                        harmonies.extend(triadic(cur));
                        harmonies.extend(analogous(cur));
                        for rgb in harmonies {
                            if swatch_button(ui, rgb, self.color == rgb).clicked() {
                                self.color = rgb;
                            }
                        }
                    });
                });
            });
            ui.add_space(t.gap);
            design::section_card(ui, &t, None, false, |ui| {
                stepped_slider(
                    ui,
                    &mut self.brush.size,
                    BRUSH_SIZE_MIN..=BRUSH_SIZE_MAX,
                    1.0,
                    "Size",
                );
                stepped_slider(ui, &mut self.brush.opacity, 0.05..=1.0, 0.05, "Opacity");
                stepped_slider(ui, &mut self.brush.softness, 0.0..=1.0, 0.05, "Softness");
                stepped_slider(ui, &mut self.brush.spacing, 0.05..=1.0, 0.05, "Spacing");
                ui.collapsing("Pressure (touch / Pencil)", |ui| {
                    stepped_slider(ui, &mut self.brush.pressure_size, -1.0..=1.0, 0.05, "Size");
                    stepped_slider(ui, &mut self.brush.pressure_opacity, -1.0..=1.0, 0.05, "Opacity");
                    ui.label("Mouse has no pressure sensor: it paints at full pressure.");
                });
                ui.collapsing("Velocity", |ui| {
                    stepped_slider(ui, &mut self.brush.velocity_size, 0.0..=1.0, 0.05, "Thin");
                    stepped_slider(ui, &mut self.brush.velocity_opacity, 0.0..=1.0, 0.05, "Fade");
                    ui.label("Fast moves thin and fade (this is the old Dynamics, split).");
                });
                ui.collapsing("Taper", |ui| {
                    stepped_slider(ui, &mut self.brush.taper.start, 0.0..=1.0, 0.05, "Start");
                    stepped_slider(ui, &mut self.brush.taper.end, 0.0..=1.0, 0.05, "End");
                    stepped_slider(ui, &mut self.brush.taper.size, 0.0..=1.0, 0.05, "Size");
                    stepped_slider(ui, &mut self.brush.taper.shape, 0.0..=1.0, 0.05, "Tip shape");
                    stepped_slider(ui, &mut self.brush.taper.opacity, 0.0..=1.0, 0.05, "Opacity");
                    stepped_slider(ui, &mut self.brush.taper.pressure, 0.0..=1.0, 0.05, "Pressure");
                    ui.label("Taper lengths are fractions of the stroke; applied on release.");
                });
                ui.collapsing("Texture", |ui| {
                    stepped_slider(ui, &mut self.brush.grain, 0.0..=1.0, 0.05, "Grain");
                    stepped_slider(ui, &mut self.brush.grain_scale, 0.25..=4.0, 0.05, "Grain size");
                });
                ui.collapsing("Depth", |ui| {
                    // M39 per-brush blend mode (Normal = classic source-over).
                    ui.horizontal_wrapped(|ui| {
                        for m in BlendMode::ALL {
                            ui.radio_value(&mut self.brush.blend, m, m.name());
                        }
                    });
                    stepped_slider(ui, &mut self.brush.wet_edge, 0.0..=1.0, 0.05, "Wet edge");
                    // M39 dual-tip: a stamp from any brush file textures the dab.
                    ui.horizontal_wrapped(|ui| {
                        match &self.brush.dual_tip {
                            Some(t) => ui.label(format!("Dual: {}x{} tip", t.w, t.h)),
                            None => ui.label("Dual: off"),
                        };
                        if ui.button("Import…").clicked() {
                            #[cfg(not(target_os = "ios"))]
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Brushes", TIP_IMPORT_EXTS)
                                .pick_file()
                            {
                                self.status = self.absorb_dual_tip(&path);
                            }
                            #[cfg(target_os = "ios")]
                            {
                                let _ = rfd::FileDialog::new()
                                    .add_filter("Brushes", TIP_IMPORT_EXTS)
                                    .pick_file();
                                self.pending_open = Some(PendingOpen::DualTip);
                                self.status = "Pick a dual tip…".to_string();
                            }
                            #[cfg(target_os = "android")]
                            {
                                self.android_browse = Some(PendingOpen::DualTip);
                            }
                        }
                        if self.brush.dual_tip.is_some() && ui.button("Clear").clicked() {
                            self.brush.dual_tip = None;
                            self.status = "Dual tip cleared.".to_string();
                        }
                    });
                    if self.brush.dual_tip.is_some() {
                        stepped_slider(ui, &mut self.brush.dual_scale, 0.1..=4.0, 0.05, "Dual size");
                    }
                });
                ui.collapsing("Dab shape", |ui| {
                    stepped_slider(ui, &mut self.brush.roundness, 0.1..=1.0, 0.05, "Roundness");
                    ui.checkbox(&mut self.brush.angle_follow, "Follow direction");
                    stepped_slider(ui, &mut self.brush.scatter, 0.0..=1.0, 0.05, "Scatter");
                });
                ui.collapsing("Smoothing", |ui| {
                    stepped_slider(ui, &mut self.brush.stabilizer, 0.0..=1.0, 0.05, "Stabilizer");
                });
                ui.separator();
                ui.collapsing("Page", |ui| {
                    ui.horizontal(|ui| {
                        for (name, rgb) in page_presets() {
                            let active = self.doc.background[..3] == rgb;
                            if ui.add(egui::Button::new(name).selected(active)).clicked() {
                                self.set_page_color(rgb);
                            }
                        }
                    });
                    if egui::color_picker::color_edit_button_srgb(ui, &mut self.page_custom).changed() {
                        self.set_page_color(self.page_custom);
                    }
                });
                ui.separator();
                ui.collapsing("Guides", |ui| {
                    let mut kind_changed = false;
                    ui.horizontal_wrapped(|ui| {
                        for k in GuideKind::ALL {
                            if ui.radio_value(&mut self.guides.kind, k, k.name()).changed() {
                                kind_changed = true;
                            }
                        }
                    });
                    if kind_changed {
                        self.vp_move = false;
                        self.vp_grab = None;
                    }
                    if self.guides.kind != GuideKind::Off {
                        match self.guides.kind {
                            GuideKind::Square | GuideKind::Isometric => {
                                stepped_slider(ui, &mut self.guides.spacing, 8.0..=256.0, 1.0, "Spacing");
                            }
                            GuideKind::OnePoint | GuideKind::TwoPoint => {
                                if ui.checkbox(&mut self.vp_move, "Move VPs (drag handles on canvas)").changed()
                                {
                                    self.vp_grab = None;
                                    self.status = if self.vp_move {
                                        "Drag a handle; off-canvas VPs park as diamonds on the edge. Painting pauses while armed.".to_string()
                                    } else {
                                        "Vanishing-point move off.".to_string()
                                    };
                                }
                                if ui.button("Reset VPs").clicked() {
                                    self.guides.reset_vps();
                                    self.status = "Vanishing points reset to defaults.".to_string();
                                }
                            }
                            GuideKind::Off => {}
                        }
                        ui.checkbox(&mut self.guides.snap, "Snap while painting");
                        if self.guides.snap {
                            stepped_slider(ui, &mut self.guides.snap_px, 2.0..=32.0, 1.0, "Snap range (px)");
                        }
                        ui.checkbox(&mut self.assist, "Drawing Assist (lock strokes to guides)");
                        if self.assist {
                            ui.label("Rays/axes engage as you draw; taps stay free.");
                        }
                        stepped_slider(ui, &mut self.guides.opacity, 0.05..=1.0, 0.05, "Opacity");
                    }
                });
            });
            if self.doc.selection_mask.is_some() {
                ui.separator();
                ui.label(format!("Lasso: {} px", self.doc.selection_pixel_count()));
            } else if let Some((x0, y0, x1, y1)) = self.doc.selection {
                ui.separator();
                ui.label(format!("Selected {}x{} px", x1 - x0 + 1, y1 - y0 + 1));
            }
            });
    }

    /// Layers panel body shared by the desktop dock and the mobile drawer.
    /// Add/duplicate/merge/rasterize row, shared by the dock and the mobile
    /// drawer (which wraps it instead of clipping at the drawer edge).
    pub(crate) fn layer_action_row(&mut self, ui: &mut egui::Ui) {
        if ui.button("Add layer").clicked() {
            self.ghost_dirty = true;
            self.doc
                .add_layer(format!("Layer {}", self.doc.layers.len() + 1));
        }
        if ui.button("Duplicate").clicked() {
            let active = self.doc.active;
            self.ghost_dirty = true;
            self.doc.duplicate_layer(active);
            self.status = "Layer duplicated.".to_string();
        }
        let can_merge = self.doc.active > 0 && self.doc.layers.len() > 1;
        if ui
            .add_enabled(can_merge, egui::Button::new("Merge down"))
            .clicked()
        {
            let active = self.doc.active;
            if self.doc.merge_down(active) {
                self.ghost_dirty = true;
                self.status = "Merged down.".to_string();
            }
        }
        let can_raster = self
            .doc
            .layers
            .get(self.doc.active)
            .is_some_and(|l| l.is_text());
        if ui
            .add_enabled(can_raster, egui::Button::new("Rasterize"))
            .clicked()
        {
            let active = self.doc.active;
            self.doc.checkpoint();
            if self.doc.rasterize_text_layer(active) {
                self.ghost_dirty = true;
                self.status = "Text rasterized.".to_string();
            }
        }
    }

    /// Name for a fresh group that does not collide with an existing one
    /// ("Group 3" after "Group 3" was disbanded).
    pub(crate) fn next_group_name(&self) -> String {
        let mut n = 1;
        while self
            .doc
            .groups
            .iter()
            .any(|g| g.name == format!("Group {n}"))
        {
            n += 1;
        }
        format!("Group {n}")
    }

    /// One group header: collapse and visibility toggles, inline rename,
    /// move-active-in and ungroup actions, and an opacity slider while the
    /// group is expanded. Requests land in `acts` so the panel can apply
    /// them after the row loop.
    pub(crate) fn group_header_row(
        &mut self,
        ui: &mut egui::Ui,
        depth: usize,
        id: u32,
        members: usize,
        acts: &mut GroupRowActions,
    ) {
        let Some((name, visible, collapsed, opacity)) = self
            .doc
            .group(id)
            .map(|g| (g.name.clone(), g.visible, g.collapsed, g.opacity))
        else {
            return;
        };
        let header = ui.horizontal_wrapped(|ui| {
            ui.add_space(depth as f32 * 12.0);
            if ui
                .small_button(if collapsed { "+" } else { "−" })
                .on_hover_text(if collapsed {
                    "Expand group"
                } else {
                    "Collapse group"
                })
                .clicked()
            {
                acts.collapse = Some(id);
            }
            let mut vis = visible;
            if ui
                .toggle_value(&mut vis, "👁")
                .on_hover_text("Toggle group visibility")
                .changed()
            {
                acts.eye = Some(id);
            }
            if self.group_renaming == Some(id) {
                // Fixed width, so the field cannot shove OK/X off a narrow
                // drawer; the wrapped row moves the buttons to the next
                // line if the field still does not fit.
                let buttons_w = 2.0 * 44.0 + 2.0 * ui.spacing().item_spacing.x;
                let field_w = (ui.available_width() - buttons_w).clamp(72.0, 260.0);
                let resp = ui.add_sized(
                    [field_w, 28.0],
                    egui::TextEdit::singleline(&mut self.group_name_edit),
                );
                if !resp.has_focus() {
                    resp.request_focus();
                }
                let enter = ui.ctx().input(|inp| inp.key_pressed(egui::Key::Enter));
                let mut commit = resp.lost_focus() || enter;
                let mut cancel = false;
                let (ok_label, x_label) = if self.has_symbols {
                    ("✓", "✕")
                } else {
                    ("OK", "X")
                };
                if ui
                    .add_sized([44.0, 28.0], egui::Button::new(ok_label))
                    .clicked()
                {
                    commit = true;
                }
                if ui
                    .add_sized([44.0, 28.0], egui::Button::new(x_label))
                    .clicked()
                {
                    cancel = true;
                }
                if commit && !cancel {
                    let edited = std::mem::take(&mut self.group_name_edit);
                    acts.rename = Some((
                        id,
                        if edited.trim().is_empty() {
                            name.clone()
                        } else {
                            edited
                        },
                    ));
                }
                if commit || cancel {
                    self.group_renaming = None;
                    self.group_name_edit.clear();
                }
            } else {
                // Double-tap renames, matching the layer rows.
                let resp = ui
                    .selectable_label(false, &name)
                    .on_hover_text("Double-tap to rename");
                if resp.double_clicked() {
                    self.group_renaming = Some(id);
                    self.group_name_edit = name.clone();
                }
                if let Some(badge) = group_member_badge(members) {
                    let hint = if members == 1 {
                        "1 layer in this group".to_string()
                    } else {
                        format!("{members} layers in this group")
                    };
                    ui.weak(badge).on_hover_text(hint);
                }
            }
            if members == 0 {
                ui.weak("empty");
            }
            // Secondary actions sit behind one menu so the header stays a
            // single line in the narrow drawer.
            let can_up = self.doc.can_move_group(id, true);
            let can_down = self.doc.can_move_group(id, false);
            let mut start_rename = false;
            ui.menu_button(if self.has_symbols { "⋯" } else { "..." }, |ui| {
                if ui.button("Rename group").clicked() {
                    start_rename = true;
                    ui.close();
                }
                if ui
                    .button("Add active layer")
                    .on_hover_text("Move the active layer into this group")
                    .clicked()
                {
                    acts.add = Some(id);
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(can_up, egui::Button::new("Move up"))
                    .on_disabled_hover_text("Already at the top of the stack")
                    .clicked()
                {
                    acts.reorder = Some((id, true));
                    ui.close();
                }
                if ui
                    .add_enabled(can_down, egui::Button::new("Move down"))
                    .on_disabled_hover_text("Already at the bottom of the stack")
                    .clicked()
                {
                    acts.reorder = Some((id, false));
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("Ungroup")
                    .on_hover_text("Disband group (keeps layers)")
                    .clicked()
                {
                    acts.ungroup = Some(id);
                    ui.close();
                }
            })
            .response
            .on_hover_text("Group actions");
            if start_rename {
                self.group_renaming = Some(id);
                self.group_name_edit = name.clone();
            }
        });
        if take_scroll_request(&mut self.group_scroll, id) {
            header.response.scroll_to_me(Some(egui::Align::Center));
        }
        // Collapsed and empty groups keep a single line.
        if !collapsed && members > 0 {
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 12.0);
                let mut op = opacity;
                stepped_slider(ui, &mut op, 0.0..=1.0, 0.05, "Opacity");
                if (op - opacity).abs() > f32::EPSILON {
                    acts.opacity = Some((id, op));
                }
            });
        }
    }

    pub(crate) fn layers_panel_body(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Indexes are the only layer identity, so a count change (add,
            // duplicate, merge, delete, undo) invalidates the stored
            // selection; fall back to the active layer.
            let len = self.doc.layers.len();
            if self.layer_count_marker != len {
                self.layer_count_marker = len;
                self.layer_selection = vec![self.doc.active];
            }
            self.layer_selection.retain(|&i| i < len);
            if !self.layer_selection.contains(&self.doc.active) {
                self.layer_selection = vec![self.doc.active];
            }
            let t = self.tokens(ui);
            let mut close_layers = false;
            design::section_card_actions(
                ui,
                &t,
                Some("Layers"),
                false,
                |ui| {
                    if ui.add_sized([40.0, 24.0], egui::Button::new(">")).clicked() {
                        close_layers = true;
                    }
                },
                |ui| {
                    // Overlay drawers are narrower than the dock, so the
                    // action row wraps there; the dock keeps one line.
                    if mobile_shell() {
                        ui.horizontal_wrapped(|ui| self.layer_action_row(ui));
                    } else {
                        ui.horizontal(|ui| self.layer_action_row(ui));
                    }
                    let selected_count = self.layer_selection.len();
                    if ui
                        .button("New group")
                        .on_hover_text(if selected_count > 1 {
                            "Group the selected layers in a new group"
                        } else {
                            "Group the active layer in a new group"
                        })
                        .clicked()
                    {
                        let selection = self.layer_selection.clone();
                        let name = self.next_group_name();
                        let parent_name = self
                            .doc
                            .shared_group(&selection)
                            .and_then(|id| self.doc.group(id))
                            .map(|g| g.name.clone());
                        let targets = match selection.as_slice() {
                            [only] => self.doc.layers[*only].name.clone(),
                            many => format!("{} layers", many.len()),
                        };
                        if let Some(id) = self.doc.group_layers(&selection, name.clone()) {
                            self.ghost_dirty = true;
                            self.status =
                                group_created_status(&name, parent_name.as_deref(), &targets);
                            self.group_scroll = Some(id);
                        }
                    }
                    let mut remove = None;
                    let mut move_out = None;
                    let mut select = None;
                    let mut touched = false;
                    let mut acts = GroupRowActions::default();
                    for row in layer_panel_rows(&self.doc) {
                        let (i, depth) = match row {
                            LayerPanelRow::Group { id, depth, members } => {
                                self.group_header_row(ui, depth, id, members, &mut acts);
                                continue;
                            }
                            LayerPanelRow::Layer { index, depth } => (index, depth),
                        };
                        let layer = &mut self.doc.layers[i];
                        ui.horizontal(|ui| {
                            ui.add_space(depth as f32 * 12.0);
                            let mut vis = layer.visible;
                            if ui
                                .toggle_value(&mut vis, "👁")
                                .on_hover_text("Toggle layer visibility")
                                .changed()
                            {
                                layer.visible = vis;
                                touched = true;
                            }
                            let selected = self.layer_selection.contains(&i);
                            if Some(i) == self.renaming {
                                // Double-clicked: edit the name in a staged buffer so
                                // Cancel can discard it. Commits via Save, Enter, or
                                // focus loss (touch keyboards often send neither, so
                                // the buttons are the reliable path on iPad).
                                let resp = ui.text_edit_singleline(&mut self.renaming_text);
                                if !resp.has_focus() {
                                    resp.request_focus();
                                }
                                let enter = ui.ctx().input(|inp| inp.key_pressed(egui::Key::Enter));
                                let mut commit = resp.lost_focus() || enter;
                                let mut cancel = false;
                                let (ok_label, x_label) = if self.has_symbols {
                                    ("✓", "✕")
                                } else {
                                    ("OK", "X")
                                };
                                if ui
                                    .add_sized([44.0, 28.0], egui::Button::new(ok_label))
                                    .clicked()
                                {
                                    commit = true;
                                }
                                if ui
                                    .add_sized([44.0, 28.0], egui::Button::new(x_label))
                                    .clicked()
                                {
                                    cancel = true;
                                }
                                if commit && !cancel {
                                    if self.renaming_text.trim().is_empty() {
                                        self.renaming_text = format!("Layer {}", i + 1);
                                    }
                                    layer.name = std::mem::take(&mut self.renaming_text);
                                }
                                if commit || cancel {
                                    self.renaming = None;
                                    self.renaming_text.clear();
                                }
                            } else {
                                let resp =
                                    ui.selectable_label(selected, &layer.name).on_hover_text(
                                        "Ctrl/Shift-click or long-press to select several layers",
                                    );
                                let long = resp.long_touched();
                                let mods = ui.ctx().input(|inp| inp.modifiers);
                                if resp.clicked() {
                                    self.layer_selection = next_layer_selection(
                                        &self.layer_selection,
                                        self.doc.active,
                                        i,
                                        long || mods.command || mods.ctrl,
                                        mods.shift && !long,
                                    );
                                    select = Some(i);
                                    if self.layer_selection.len() > 1 {
                                        self.status = format!(
                                            "{} layers selected.",
                                            self.layer_selection.len()
                                        );
                                    }
                                }
                                if resp.double_clicked() {
                                    select = Some(i);
                                    self.renaming = Some(i);
                                    self.renaming_text = layer.name.clone();
                                }
                            }
                            if layer.group.is_some()
                                && ui
                                    .small_button(if self.has_symbols { "↥" } else { "Out" })
                                    .on_hover_text("Move layer out of its group")
                                    .clicked()
                            {
                                move_out = Some(i);
                            }
                            if ui
                                .add_sized([44.0, 24.0], egui::Button::new("del"))
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            if layer.is_text() {
                                ui.label("T");
                            }
                            if layer.mask.is_some() {
                                ui.label("M");
                            }
                            if layer.clip {
                                ui.label("C");
                            }
                        });
                    }
                    // A request the rows did not consume (its group sits under
                    // a collapsed ancestor) is dropped rather than saved for a
                    // surprise scroll when that ancestor is expanded.
                    self.group_scroll = None;
                    if let Some(i) = select {
                        self.doc.active = i;
                        // A fresh full upload (matters for the mask viewer).
                        self.doc.mark_full_dirty();
                        if !self.doc.is_layer_shown(i) {
                            self.status = "Active layer is in a hidden group.".to_string();
                        }
                    }
                    let report = apply_group_actions(&mut self.doc, &acts);
                    if report.dirty {
                        self.ghost_dirty = true;
                        self.doc.mark_full_dirty();
                    }
                    if let Some(status) = report.status {
                        self.status = status;
                    }
                    if touched {
                        self.ghost_dirty = true;
                        self.doc.mark_full_dirty();
                    }
                    if let Some(i) = remove {
                        self.ghost_dirty = true;
                        self.doc.remove_layer(i);
                    }
                    if let Some(i) = move_out {
                        let name = self.doc.layers[i].name.clone();
                        if self.doc.move_layer_out_of_group(i) {
                            self.ghost_dirty = true;
                            self.status = format!("{name} moved out of the group.");
                        }
                    }
                },
            );
            if close_layers {
                self.show_layers = false;
            }
            ui.add_space(t.gap);
            design::section_card(ui, &t, None, false, |ui| {
                if let Some(layer) = self.doc.layers.get_mut(self.doc.active) {
                    ui.label(format!("Opacity: {}", layer.name));
                    let before = layer.opacity;
                    stepped_slider(ui, &mut layer.opacity, 0.0..=1.0, 0.05, "");
                    let mut blend = layer.blend;
                    let mut moved = 0;
                    egui::ComboBox::from_label("Blend")
                        .selected_text(blend.name())
                        .show_ui(ui, |ui| {
                            for m in BlendMode::ALL {
                                ui.selectable_value(&mut blend, m, m.name());
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button("Up").clicked() {
                            moved = 1;
                        }
                        if ui.button("Down").clicked() {
                            moved = -1;
                        }
                    });
                    let active = self.doc.active;
                    let mut dirty = (self.doc.layers[active].opacity - before).abs() > f32::EPSILON;
                    if self.doc.layers[active].blend != blend {
                        self.doc.layers[active].blend = blend;
                        dirty = true;
                    }
                    let reordered = (moved > 0 && active + 1 < self.doc.layers.len())
                        || (moved < 0 && active > 0);
                    if reordered {
                        let target = if moved > 0 { active + 1 } else { active - 1 };
                        self.doc.move_layer(active, target);
                        // Rows shift under the indexes: keep only the active row.
                        self.layer_selection = vec![self.doc.active];
                    }
                    if dirty {
                        self.ghost_dirty = true;
                        self.doc.mark_full_dirty();
                    }
                    // Masks, clipping and paint target for the active layer.
                    let has_mask = self.doc.layers[active].mask.is_some();
                    ui.horizontal(|ui| {
                        if ui
                            .button(if has_mask { "Clear mask" } else { "Add mask" })
                            .clicked()
                        {
                            self.ghost_dirty = true;
                            if has_mask {
                                self.doc.clear_mask(active);
                                self.view_mask = false;
                                self.status = "Mask cleared.".to_string();
                            } else {
                                self.doc.add_mask(active);
                                self.status = "Mask added (white reveals). Paint on Mask to edit."
                                    .to_string();
                            }
                        }
                        if ui.checkbox(&mut self.view_mask, "View").changed() {
                            self.doc.mark_full_dirty();
                            self.status = if self.view_mask {
                                "Mask view: white reveals, black conceals.".to_string()
                            } else {
                                "Canvas view.".to_string()
                            };
                        }
                    });
                    ui.horizontal(|ui| {
                        let mut clip = self.doc.layers[active].clip;
                        if ui.checkbox(&mut clip, "Clip to below").changed() {
                            self.ghost_dirty = true;
                            self.doc.layers[active].clip = clip;
                            self.doc.mark_full_dirty();
                            self.status = if clip {
                                "Layer clips to below.".to_string()
                            } else {
                                "Clip off.".to_string()
                            };
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Paint:");
                        ui.radio_value(&mut self.doc.paint_target, PaintTarget::Color, "Color");
                        ui.radio_value(&mut self.doc.paint_target, PaintTarget::Mask, "Mask");
                    });
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(layer_readout(&self.doc));
                    ui.weak(format!(
                        "cache {}",
                        file_size_label(self.composite.bytes() as u64)
                    ));
                });
                ui.separator();
                self.lapse_card(ui, &t);
                ui.collapsing("Animation", |ui| {
                    ui.label("Layers are frames, bottom first.");
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.anim_assist, "Assist").changed() {
                            self.anim_playing = false;
                            self.texture = None; // rebuild the view (frame vs composite)
                            self.composite.invalidate();
                            self.ghost_dirty = true;
                            self.status = if self.anim_assist {
                                "Animation Assist on: one frame at a time.".to_string()
                            } else {
                                "Animation Assist off.".to_string()
                            };
                        }
                        let playing = self.anim_playing;
                        if ui.button(if playing { "Stop" } else { "Play" }).clicked() {
                            if playing {
                                self.anim_playing = false;
                                self.status = "Playback stopped.".to_string();
                            } else if self.doc.layers.len() < 2 {
                                self.status = "Need at least 2 layers to play.".to_string();
                            } else {
                                self.anim_assist = true;
                                self.anim_playing = true;
                                self.anim_fwd = true;
                                self.anim_last_t = ui.ctx().input(|i| i.time);
                                self.texture = None;
                                self.composite.invalidate();
                                self.ghost_dirty = true;
                                self.status = "Playing.".to_string();
                            }
                        }
                    });
                    if self.anim_assist && !self.doc.layers.is_empty() {
                        ui.horizontal(|ui| {
                            if ui.button("|<").clicked() {
                                let n = self.doc.layers.len();
                                self.doc.active = (self.doc.active + n - 1) % n;
                                self.doc.mark_full_dirty();
                            }
                            ui.label(format!(
                                "Frame {}/{}",
                                self.doc.active + 1,
                                self.doc.layers.len()
                            ));
                            if ui.button(">|").clicked() {
                                let n = self.doc.layers.len();
                                self.doc.active = (self.doc.active + 1) % n;
                                self.doc.mark_full_dirty();
                            }
                            if ui.button("Duplicate frame").clicked() {
                                let active = self.doc.active;
                                self.doc.duplicate_layer(active);
                                self.ghost_dirty = true;
                                self.status = format!("Frame {} duplicated.", self.doc.active + 1);
                            }
                        });
                        stepped_slider(
                            ui,
                            &mut self.anim_fps,
                            MIN_ANIM_FPS..=MAX_ANIM_FPS,
                            1.0,
                            "FPS",
                        );
                        ui.horizontal(|ui| {
                            ui.label("Loop:");
                            for m in LoopMode::ALL {
                                ui.radio_value(&mut self.anim_loop, m, m.name());
                            }
                        });
                        // Per-frame hold: the stepper edits the current frame only.
                        let mut hold = self
                            .doc
                            .layers
                            .get(self.doc.active)
                            .map(|l| l.hold)
                            .unwrap_or(MIN_ANIM_HOLD)
                            .clamp(MIN_ANIM_HOLD, MAX_ANIM_HOLD);
                        stepped_slider(
                            ui,
                            &mut hold,
                            MIN_ANIM_HOLD..=MAX_ANIM_HOLD,
                            1.0,
                            "Frame hold",
                        );
                        if let Some(layer) = self.doc.layers.get_mut(self.doc.active) {
                            if layer.hold != hold {
                                layer.hold = hold;
                            }
                        }
                        if ui.checkbox(&mut self.onion, "Onion skin").changed() {
                            self.ghost_dirty = true;
                        }
                        if self.onion {
                            let before = (self.onion_prev, self.onion_next);
                            stepped_slider(
                                ui,
                                &mut self.onion_prev,
                                0.05..=0.8,
                                0.05,
                                "Prev ghost",
                            );
                            stepped_slider(
                                ui,
                                &mut self.onion_next,
                                0.05..=0.8,
                                0.05,
                                "Next ghost",
                            );
                            if (self.onion_prev, self.onion_next) != before {
                                self.ghost_dirty = true;
                            }
                        }
                        // One row: the two GIF actions are siblings and
                        // wrapping them keeps them from stacking.
                        ui.horizontal(|ui| {
                            self.gif_export_ui(ui);
                            self.gif_import_ui(ui);
                        });
                    }
                });
                ui.separator();
                ui.collapsing("Transform layer", |ui| {
                    // Buttons compose into one session that always
                    // re-applies from the pristine base (see tform_apply):
                    // mashing them never compounds resampling blur.
                    tform_refresh(&self.doc, &mut self.tform_press);
                    let blocked = self.text_paint_blocked();
                    let mut scale_pct = (self.tform_press.scale * 100.0).round();
                    let mut angle = self.tform_press.degrees;
                    let mut ended = false;
                    if !blocked {
                        let size = ui.add(
                            egui::Slider::new(
                                &mut scale_pct,
                                TFORM_SCALE_MIN * 100.0..=TFORM_SCALE_MAX * 100.0,
                            )
                            .step_by(1.0)
                            .suffix("%")
                            .text("Size"),
                        );
                        if size.changed() {
                            tform_apply(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::SetScale(scale_pct),
                                true,
                            );
                        }
                        if size.drag_stopped() || (size.changed() && !size.dragged()) {
                            self.tform_press.end_gesture();
                            ended = true;
                        }
                        let turn = ui.add(
                            egui::Slider::new(&mut angle, -180.0..=180.0)
                                .step_by(1.0)
                                .suffix(" deg")
                                .text("Angle"),
                        );
                        if turn.changed() {
                            tform_apply(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::SetRotation(angle),
                                true,
                            );
                        }
                        if turn.drag_stopped() || (turn.changed() && !turn.dragged()) {
                            self.tform_press.end_gesture();
                            ended = true;
                        }
                    }
                    if ended {
                        self.status = tform_status(&self.tform_press);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Scale +").clicked() && !self.text_paint_blocked() {
                            tform_button(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::Scale(1.25),
                            );
                        }
                        if ui.button("Scale −").clicked() && !self.text_paint_blocked() {
                            tform_button(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::Scale(1.0 / 1.25),
                            );
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .button(if self.has_symbols {
                                "⟲ −15°"
                            } else {
                                "−15°"
                            })
                            .clicked()
                            && !self.text_paint_blocked()
                        {
                            tform_button(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::Rotate(-15.0),
                            );
                        }
                        if ui
                            .button(if self.has_symbols {
                                "⟳ +15°"
                            } else {
                                "+15°"
                            })
                            .clicked()
                            && !self.text_paint_blocked()
                        {
                            tform_button(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::Rotate(15.0),
                            );
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Flip H").clicked() && !self.text_paint_blocked() {
                            self.status = match tform_button(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::FlipH,
                            ) {
                                XformApply::Restored => {
                                    "Layer flip reset (back to the session start).".to_string()
                                }
                                _ => "Layer flipped horizontally.".to_string(),
                            };
                        }
                        if ui.button("Flip V").clicked() && !self.text_paint_blocked() {
                            self.status = match tform_button(
                                &mut self.doc,
                                &mut self.tform_press,
                                TformOp::FlipV,
                            ) {
                                XformApply::Restored => {
                                    "Layer flip reset (back to the session start).".to_string()
                                }
                                _ => "Layer flipped vertically.".to_string(),
                            };
                        }
                        if ui
                            .button("Reset")
                            .on_hover_text(
                                "Back to the layer as it was when this transform session began",
                            )
                            .clicked()
                            && !self.text_paint_blocked()
                        {
                            let _ =
                                tform_button(&mut self.doc, &mut self.tform_press, TformOp::Reset);
                            self.status = "Layer transform reset.".to_string();
                        }
                    });
                    // M48 transform handles: mode-specific on-canvas
                    // manipulation (live previews while dragging, one
                    // undo step per gesture).
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Handles:");
                        let mut mode = self.xform.mode;
                        for m in XformMode::all() {
                            ui.radio_value(&mut mode, m, m.label());
                        }
                        if mode != self.xform.mode {
                            self.xform.arm(mode, &self.doc);
                            self.status = xform_mode_status(mode).to_string();
                        }
                    });
                    if self.xform.mode == XformMode::Mesh {
                        let (mut cols, mut rows) = (self.xform.mesh_cols, self.xform.mesh_rows);
                        stepped_slider(
                            ui,
                            &mut cols,
                            XFORM_MESH_MIN..=XFORM_MESH_MAX,
                            1.0,
                            "Cells H",
                        );
                        stepped_slider(
                            ui,
                            &mut rows,
                            XFORM_MESH_MIN..=XFORM_MESH_MAX,
                            1.0,
                            "Cells V",
                        );
                        if (cols, rows) != (self.xform.mesh_cols, self.xform.mesh_rows) {
                            self.xform.set_mesh_cells(cols, rows, &self.doc);
                            self.status = format!("Mesh lattice {cols}x{rows} cells.");
                        }
                    }
                    if self.xform.mode == XformMode::Puppet {
                        let mut expand = self.xform.expand;
                        ui.add(
                            egui::Slider::new(&mut expand, 0.0..=XFORM_EXPAND_MAX)
                                .text("Expand mesh")
                                .suffix(" px"),
                        );
                        if (expand - self.xform.expand).abs() > 0.01 {
                            self.xform.expand = expand;
                            self.xform.arm(XformMode::Puppet, &self.doc);
                            self.status =
                                format!("Puppet mesh expanded by {expand:.0} px; pins reset.");
                        }
                        ui.horizontal_wrapped(|ui| {
                            let mut pin_tool = self.xform.pin_tool;
                            ui.label("Click pin:");
                            egui::ComboBox::from_id_salt("pin_tool")
                                .width(112.0)
                                .selected_text(pin_tool.label())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut pin_tool,
                                        PinTool::Select,
                                        PinTool::Select.label(),
                                    );
                                    ui.selectable_value(
                                        &mut pin_tool,
                                        PinTool::Delete,
                                        PinTool::Delete.label(),
                                    );
                                });
                            if pin_tool != self.xform.pin_tool {
                                self.xform.pin_tool = pin_tool;
                                self.status = format!("Puppet click: {}.", pin_tool.label());
                            }
                            if pin_tool == PinTool::Select {
                                let mut select = self.xform.pin_select;
                                ui.label("Select:");
                                egui::ComboBox::from_id_salt("pin_select")
                                    .width(150.0)
                                    .selected_text(select.label())
                                    .show_ui(ui, |ui| {
                                        for mode in [
                                            PinSelect::Replace,
                                            PinSelect::Add,
                                            PinSelect::Subtract,
                                            PinSelect::Toggle,
                                        ] {
                                            ui.selectable_value(&mut select, mode, mode.label());
                                        }
                                    });
                                if select != self.xform.pin_select {
                                    self.xform.pin_select = select;
                                    self.status = format!("Puppet selection: {}.", select.label());
                                }
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("Pins: {}", self.xform.pins.len()));
                            if !self.xform.selected_pins.is_empty() {
                                ui.label(format!("Selected: {}", self.xform.selected_pins.len()));
                            }
                            ui.label(
                                "Click to add; Shift-click or drag a box to select; \
                                         drag a selected pin to move the group.",
                            );
                            if ui.button("Clear pins").clicked() {
                                self.xform.clear_pins(&mut self.doc);
                                self.status = "Puppet pins reset to the frame corners.".to_string();
                            }
                        });
                    }
                    if self.xform.mode != XformMode::Off && ui.button("Reset transform").clicked() {
                        self.xform.reset_transform(&mut self.doc);
                        self.status = "Transform reset.".to_string();
                    }
                });
                ui.separator();
                ui.collapsing("Symmetry", |ui| {
                    for m in SymmetryMode::ALL {
                        if ui
                            .radio_value(&mut self.doc.symmetry, m, m.name())
                            .changed()
                        {
                            self.sym_move = false;
                            self.sym_grab = false;
                            self.status = format!("Symmetry: {}.", m.name());
                        }
                    }
                    if self.doc.symmetry != SymmetryMode::Off {
                        if ui
                            .checkbox(&mut self.sym_move, "Move center (drag it on canvas)")
                            .changed()
                        {
                            self.sym_grab = false;
                            self.status = if self.sym_move {
                                "Drag the mirror center; painting pauses while armed.".to_string()
                            } else {
                                "Mirror-center move off.".to_string()
                            };
                        }
                        if ui.button("Reset center").clicked() {
                            self.doc.reset_symmetry_center();
                            self.status = "Mirror center reset to canvas middle.".to_string();
                        }
                    }
                    if self.doc.symmetry == SymmetryMode::Kaleidoscope {
                        stepped_slider(
                            ui,
                            &mut self.doc.symmetry_segments,
                            2..=16,
                            1.0,
                            "Segments",
                        );
                    }
                });
                ui.separator();
                ui.collapsing("Filters", |ui| {
                    ui.label(
                        "Active layer. Sliders preview live; Apply keeps. Selections mask filters.",
                    );
                    // Tone previews live from a snapshot (one undo step); Apply
                    // keeps the preview, or applies once when nothing is pending.
                    let tone_before = (self.filt_brightness, self.filt_contrast);
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_brightness,
                        -100..=100,
                        1.0,
                        "Brightness",
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_contrast,
                        0.0..=200.0,
                        1.0,
                        "Contrast %",
                    );
                    if (self.filt_brightness, self.filt_contrast) != tone_before {
                        let (b, c) = (self.filt_brightness, self.filt_contrast);
                        self.filter_scrub(0, move |doc, layer| {
                            doc.adjust_tone(layer, (b as f32 * 2.55).round() as i32, c / 100.0);
                        });
                        self.status = "Previewing tone — Apply to keep.".to_string();
                    }
                    if ui.button("Apply tone").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 0) {
                            let (b, c) = (self.filt_brightness, self.filt_contrast);
                            self.doc.checkpoint();
                            self.doc.adjust_tone(
                                self.doc.active,
                                (b as f32 * 2.55).round() as i32,
                                c / 100.0,
                            );
                        } else {
                            // Throttled-stale preview (or a fresh one): stamp the
                            // latest values over the scrub snapshot — same undo step.
                            let (b, c) = (self.filt_brightness, self.filt_contrast);
                            self.restamp_preview(move |doc, layer| {
                                doc.adjust_tone(layer, (b as f32 * 2.55).round() as i32, c / 100.0);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "Tone applied.".to_string();
                        self.capture_timelapse();
                    }
                    let blur_before = self.filt_blur;
                    stepped_slider_label_end(ui, &mut self.filt_blur, 0..=25, 1.0, "Blur");
                    if self.filt_blur != blur_before {
                        let r = self.filt_blur.max(0) as u32;
                        self.filter_scrub(1, move |doc, layer| {
                            doc.blur_layer(layer, r);
                        });
                        self.status = "Previewing blur — Apply to keep.".to_string();
                    }
                    if ui.button("Apply blur").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 1) {
                            self.doc.checkpoint();
                            self.doc
                                .blur_layer(self.doc.active, self.filt_blur.max(0) as u32);
                        } else {
                            let r = self.filt_blur.max(0) as u32;
                            self.restamp_preview(move |doc, layer| {
                                doc.blur_layer(layer, r);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "Blur applied.".to_string();
                        self.capture_timelapse();
                    }
                    let sharp_before = self.filt_sharpen;
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_sharpen,
                        0.0..=200.0,
                        1.0,
                        "Sharpen %",
                    );
                    if self.filt_sharpen != sharp_before {
                        let s = self.filt_sharpen / 100.0;
                        self.filter_scrub(2, move |doc, layer| {
                            doc.sharpen_layer(layer, s);
                        });
                        self.status = "Previewing sharpen — Apply to keep.".to_string();
                    }
                    if ui.button("Apply sharpen").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 2) {
                            self.doc.checkpoint();
                            self.doc
                                .sharpen_layer(self.doc.active, self.filt_sharpen / 100.0);
                        } else {
                            let s = self.filt_sharpen / 100.0;
                            self.restamp_preview(move |doc, layer| {
                                doc.sharpen_layer(layer, s);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "Sharpen applied.".to_string();
                        self.capture_timelapse();
                    }
                    // M36 Levels: black/white points + mid gamma, same live
                    // preview + Apply rhythm as tone (preview kind 3).
                    let levels_before = (
                        self.filt_levels_shadow,
                        self.filt_levels_gamma,
                        self.filt_levels_high,
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_levels_shadow,
                        0..=255,
                        1.0,
                        "Shadow",
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_levels_gamma,
                        10.0..=400.0,
                        1.0,
                        "Gamma %",
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_levels_high,
                        0..=255,
                        1.0,
                        "Highlight",
                    );
                    if (
                        self.filt_levels_shadow,
                        self.filt_levels_gamma,
                        self.filt_levels_high,
                    ) != levels_before
                    {
                        let (s, g, h) = (
                            self.filt_levels_shadow,
                            self.filt_levels_gamma / 100.0,
                            self.filt_levels_high,
                        );
                        let lut = levels_lut(s, g, h);
                        self.filter_scrub(3, move |doc, layer| {
                            doc.apply_lut(layer, &lut);
                        });
                        self.status = "Previewing levels — Apply to keep.".to_string();
                    }
                    if ui.button("Apply levels").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 3) {
                            let lut = levels_lut(
                                self.filt_levels_shadow,
                                self.filt_levels_gamma / 100.0,
                                self.filt_levels_high,
                            );
                            self.doc.checkpoint();
                            self.doc.apply_lut(self.doc.active, &lut);
                        } else {
                            let lut = levels_lut(
                                self.filt_levels_shadow,
                                self.filt_levels_gamma / 100.0,
                                self.filt_levels_high,
                            );
                            self.restamp_preview(move |doc, layer| {
                                doc.apply_lut(layer, &lut);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "Levels applied.".to_string();
                        self.capture_timelapse();
                    }
                    // M36 Curves (parametric shadows/mids/highlights through a
                    // pinned-endpoint LUT; preview kind 4).
                    let curves_before = (
                        self.filt_curve_shadow,
                        self.filt_curve_mid,
                        self.filt_curve_high,
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_curve_shadow,
                        -100..=100,
                        1.0,
                        "Shadows",
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_curve_mid,
                        -100..=100,
                        1.0,
                        "Midtones",
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_curve_high,
                        -100..=100,
                        1.0,
                        "Highlights",
                    );
                    if (
                        self.filt_curve_shadow,
                        self.filt_curve_mid,
                        self.filt_curve_high,
                    ) != curves_before
                    {
                        let (s, m, h) = (
                            self.filt_curve_shadow,
                            self.filt_curve_mid,
                            self.filt_curve_high,
                        );
                        let lut = curves_lut(s, m, h);
                        self.filter_scrub(4, move |doc, layer| {
                            doc.apply_lut(layer, &lut);
                        });
                        self.status = "Previewing curves — Apply to keep.".to_string();
                    }
                    if ui.button("Apply curves").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 4) {
                            let lut = curves_lut(
                                self.filt_curve_shadow,
                                self.filt_curve_mid,
                                self.filt_curve_high,
                            );
                            self.doc.checkpoint();
                            self.doc.apply_lut(self.doc.active, &lut);
                        } else {
                            let lut = curves_lut(
                                self.filt_curve_shadow,
                                self.filt_curve_mid,
                                self.filt_curve_high,
                            );
                            self.restamp_preview(move |doc, layer| {
                                doc.apply_lut(layer, &lut);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "Curves applied.".to_string();
                        self.capture_timelapse();
                    }
                    // M37 HSL: hue spin + saturation scale + lightness shift
                    // (preview kind 5). Grays only answer to lightness.
                    let hsl_before = (self.filt_hue, self.filt_sat, self.filt_light);
                    stepped_slider_label_end(ui, &mut self.filt_hue, -180.0..=180.0, 1.0, "Hue°");
                    stepped_slider_label_end(ui, &mut self.filt_sat, 0.0..=400.0, 1.0, "Sat %");
                    stepped_slider_label_end(ui, &mut self.filt_light, -100..=100, 1.0, "Light");
                    if (self.filt_hue, self.filt_sat, self.filt_light) != hsl_before {
                        let (h, s, l) = (
                            self.filt_hue,
                            self.filt_sat / 100.0,
                            self.filt_light as f32 / 100.0,
                        );
                        self.filter_scrub(5, move |doc, layer| {
                            doc.adjust_hsl(layer, h, s, l);
                        });
                        self.status = "Previewing HSL — Apply to keep.".to_string();
                    }
                    if ui.button("Apply HSL").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 5) {
                            self.doc.checkpoint();
                            let active = self.doc.active;
                            self.doc.adjust_hsl(
                                active,
                                self.filt_hue,
                                self.filt_sat / 100.0,
                                self.filt_light as f32 / 100.0,
                            );
                        } else {
                            let (h, s, l) = (
                                self.filt_hue,
                                self.filt_sat / 100.0,
                                self.filt_light as f32 / 100.0,
                            );
                            self.restamp_preview(move |doc, layer| {
                                doc.adjust_hsl(layer, h, s, l);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "HSL applied.".to_string();
                        self.capture_timelapse();
                    }
                    // M37 Color balance: − toward cyan/magenta/yellow, + toward
                    // red/green/blue (preview kind 6).
                    let bal_before = (self.filt_bal_r, self.filt_bal_g, self.filt_bal_b);
                    stepped_slider_label_end(ui, &mut self.filt_bal_r, -100..=100, 1.0, "Red−Cyan");
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_bal_g,
                        -100..=100,
                        1.0,
                        "Green−Magenta",
                    );
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_bal_b,
                        -100..=100,
                        1.0,
                        "Blue−Yellow",
                    );
                    if (self.filt_bal_r, self.filt_bal_g, self.filt_bal_b) != bal_before {
                        let (r, g, b) = (
                            (self.filt_bal_r as f32 * 2.55).round() as i32,
                            (self.filt_bal_g as f32 * 2.55).round() as i32,
                            (self.filt_bal_b as f32 * 2.55).round() as i32,
                        );
                        self.filter_scrub(6, move |doc, layer| {
                            doc.color_balance(layer, r, g, b);
                        });
                        self.status = "Previewing balance — Apply to keep.".to_string();
                    }
                    if ui.button("Apply balance").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 6) {
                            let (r, g, b) = (
                                (self.filt_bal_r as f32 * 2.55).round() as i32,
                                (self.filt_bal_g as f32 * 2.55).round() as i32,
                                (self.filt_bal_b as f32 * 2.55).round() as i32,
                            );
                            self.doc.checkpoint();
                            self.doc.color_balance(self.doc.active, r, g, b);
                        } else {
                            let (r, g, b) = (
                                (self.filt_bal_r as f32 * 2.55).round() as i32,
                                (self.filt_bal_g as f32 * 2.55).round() as i32,
                                (self.filt_bal_b as f32 * 2.55).round() as i32,
                            );
                            self.restamp_preview(move |doc, layer| {
                                doc.color_balance(layer, r, g, b);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "Balance applied.".to_string();
                        self.capture_timelapse();
                    }
                    // M37 Recolor (colorize) toward the current paint color at
                    // strength (preview kind 7). White stays white by definition.
                    ui.label(format!(
                        "Recolor target: #{:02X}{:02X}{:02X} (paint color).",
                        self.color[0], self.color[1], self.color[2]
                    ));
                    let recolor_before = self.filt_recolor_strength;
                    stepped_slider_label_end(
                        ui,
                        &mut self.filt_recolor_strength,
                        0.0..=100.0,
                        1.0,
                        "Strength %",
                    );
                    if self.filt_recolor_strength != recolor_before {
                        let (target, s) = (self.color, self.filt_recolor_strength / 100.0);
                        self.filter_scrub(7, move |doc, layer| {
                            doc.colorize(layer, target, s);
                        });
                        self.status = "Previewing recolor — Apply to keep.".to_string();
                    }
                    if ui.button("Apply recolor").clicked() && !self.text_paint_blocked() {
                        if self.filter_preview.as_ref().is_none_or(|fp| fp.kind != 7) {
                            self.doc.checkpoint();
                            let active = self.doc.active;
                            self.doc.colorize(
                                active,
                                self.color,
                                self.filt_recolor_strength / 100.0,
                            );
                        } else {
                            let (target, s) = (self.color, self.filt_recolor_strength / 100.0);
                            self.restamp_preview(move |doc, layer| {
                                doc.colorize(layer, target, s);
                            });
                        }
                        self.filter_preview = None;
                        self.status = "Recolor applied.".to_string();
                        self.capture_timelapse();
                    }
                });
            });
        });
    }
}
