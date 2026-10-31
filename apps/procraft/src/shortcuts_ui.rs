use crate::*;

impl ProCraftApp {
    /// M49: run this frame's key events through the shortcut table.
    /// Capture mode owns every key; a focused text field keeps plain
    /// typing (spec call #6) — the table must not steal letters.
    pub(crate) fn dispatch_shortcuts(&mut self, ctx: &egui::Context) {
        if self.capture.is_some() {
            return;
        }
        if ctx.text_edit_focused() {
            return;
        }
        while let Some(action) = take_shortcut_action(&self.shortcuts, ctx) {
            self.apply_action(action);
        }
    }

    /// Space held = the Hand tool (pan), release returns to the tool that
    /// was active on press. Runs after the shortcut table (an explicit
    /// Space binding wins), and is blocked while typing, while capture is
    /// armed, or mid-stroke — a press never hijacks a live gesture. If
    /// another tool gets picked while held, that choice wins on release.
    pub(crate) fn handle_space_hand(&mut self, ctx: &egui::Context) {
        let usable = self.capture.is_none() && !ctx.text_edit_focused();
        let (pressed, down, pointer_down) = ctx.input(|i| {
            let pressed = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Space,
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            });
            (pressed, i.key_down(egui::Key::Space), i.pointer.any_down())
        });
        let before = self.space_hand;
        let after = space_hand_step(self.tool, before, pressed, down, pointer_down, usable);
        if before.is_some() || after.is_some() {
            // Swallow Space key events while engaged (and on the frame it
            // ends) so a focused widget can't also react to the key.
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key {
                            key: egui::Key::Space,
                            ..
                        }
                    )
                })
            });
        }
        match (before, after) {
            (None, Some(_)) => self.tool = Tool::Hand,
            (Some(prev), None) if self.tool == Tool::Hand => self.tool = prev,
            _ => {}
        }
        self.space_hand = after;
    }

    /// M49: perform one shortcut action — the same effect and status
    /// strings the toolbar buttons and the old hardwired keys used.
    pub(crate) fn apply_action(&mut self, action: ActionId) {
        match action {
            ActionId::Tool(t) => {
                let tool = tool_from_id(t);
                if tool != Tool::Fill {
                    self.fill_preview = None;
                }
                self.tool = tool;
            }
            ActionId::Undo => {
                if self.doc.undo() {
                    self.ghost_dirty = true;
                    self.status = "Undone.".to_string();
                }
            }
            ActionId::Redo | ActionId::RedoAlt => {
                if self.doc.redo() {
                    self.ghost_dirty = true;
                    self.status = "Redone.".to_string();
                }
            }
        }
    }

    /// M49: while capture is armed, the first resolvable key event picks
    /// the chord (or cancels / clears). Runs before the dispatcher so the
    /// editor never triggers the action it is editing.
    pub(crate) fn handle_capture(&mut self, ctx: &egui::Context) {
        if self.capture.is_some() && !self.show_shortcuts {
            // Closing the editor disarms capture — keys must never stay
            // hijacked by an invisible window.
            self.capture = None;
        }
        let Some(action) = self.capture else {
            return;
        };
        let ev = ctx.input_mut(|i| {
            let pos = i
                .events
                .iter()
                .position(|ev| resolve_capture_event(ev) != CaptureOutcome::Ignore)?;
            Some(i.events.remove(pos))
        });
        let Some(ev) = ev else {
            return;
        };
        match resolve_capture_event(&ev) {
            CaptureOutcome::Cancel => {
                self.capture = None;
                self.status = "Shortcut capture cancelled.".to_string();
            }
            CaptureOutcome::Clear => {
                self.shortcuts.clear_binding(action);
                self.capture = None;
                self.status = format!("Unbound {}.", self.action_label(action));
            }
            CaptureOutcome::Bind(chord) => match self.shortcuts.set_binding(action, chord) {
                Ok(()) => {
                    self.capture = None;
                    self.status =
                        format!("Bound {} to {}", self.action_label(action), chord.format());
                }
                Err(owner) => {
                    // Capture stays armed: the next key can be the fix.
                    self.status =
                        format!("{} is used by {}", chord.format(), self.action_label(owner));
                }
            },
            CaptureOutcome::Ignore => {}
        }
    }

    /// M49: display name for an action row (tools reuse the toolbar's
    /// symbol-aware labels).
    pub(crate) fn action_label(&self, a: ActionId) -> String {
        match a {
            ActionId::Tool(t) => tool_label(tool_from_id(t), self.has_symbols).to_string(),
            other => other.label(),
        }
    }

    /// M49: toolbar hover hint ("Brush — B", or "— unbound").
    pub(crate) fn tool_hint(&self, id: ToolId) -> String {
        let binding = match self.shortcuts.get(ActionId::Tool(id)) {
            Some(c) => c.format(),
            None => "unbound".to_string(),
        };
        format!(
            "{} — {}",
            tool_label(tool_from_id(id), self.has_symbols),
            binding
        )
    }

    /// M49: the Shortcuts editor — one row per action, the drag-modifier
    /// row with its direction picker, and Reset all. Clicking a binding
    /// arms capture for that row.
    pub(crate) fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_shortcuts;
        let sheet = self.mobile_modal_frame(ctx);
        let scrim = self.mobile_scrim(ctx, "mobile-scrim-shortcuts");
        fitted_modal(
            egui::Window::new("Shortcuts")
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .resizable(false)
                .collapsible(false),
            sheet.as_ref(),
        )
        .show(ctx, |ui| {
            for note in [&self.save_note, &self.settings_note].into_iter().flatten() {
                let t = design::tokens(
                    self.theme,
                    effective_layout(
                        ctx.viewport_rect().width(),
                        cfg!(any(target_os = "ios", target_os = "android")),
                    ),
                );
                ui.colored_label(t.danger, note);
            }
            if self.save_note.is_some() || self.settings_note.is_some() {
                ui.separator();
            }
            ui.label("Click a binding, then press the keys you want.");
            // Touch screens clamp the list to the fitted dialog height.
            let list_h = sheet
                .as_ref()
                .map_or(420.0, |s| (s.fit.y - 200.0).clamp(160.0, 420.0));
            egui::ScrollArea::vertical()
                .max_height(list_h)
                .show(ui, |ui| {
                    for a in ActionId::ALL {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [150.0, 18.0],
                                egui::Label::new(self.action_label(a)).truncate(),
                            );
                            let capturing = self.capture == Some(a);
                            let text = if capturing {
                                "Press keys… (Esc cancels, Del clears)".to_string()
                            } else {
                                self.shortcuts
                                    .get(a)
                                    .map(|c| c.format())
                                    .unwrap_or_else(|| "—".to_string())
                            };
                            if ui.button(text).clicked() {
                                self.capture = Some(a);
                            }
                            let clear = if self.has_symbols { "✕" } else { "x" };
                            if self.shortcuts.get(a).is_some() && ui.small_button(clear).clicked() {
                                self.shortcuts.clear_binding(a);
                                if self.capture == Some(a) {
                                    self.capture = None;
                                }
                            }
                        });
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.add_sized(
                    [150.0, 18.0],
                    egui::Label::new("Drag on canvas — Brush size"),
                );
                let cur = self.shortcuts.drag_get(DragActionId::BrushSize);
                let mut ctrl = cur.ctrl;
                let mut shift = cur.shift;
                ui.checkbox(&mut ctrl, "Ctrl");
                ui.checkbox(&mut shift, "Shift");
                // Alt is deliberately absent: it drives the pick loupe.
                let next = ModSet {
                    ctrl,
                    shift,
                    alt: false,
                };
                if !next.is_empty() && next != cur {
                    self.shortcuts.set_drag(DragActionId::BrushSize, next);
                }
            });
            ui.horizontal(|ui| {
                ui.add_sized([150.0, 18.0], egui::Label::new("Drag direction"));
                let mut mode = self.shortcuts.size_drag_mode();
                egui::ComboBox::from_id_salt("size_drag_mode")
                    .selected_text(size_drag_mode_label(mode))
                    .show_ui(ui, |ui| {
                        for m in [
                            SizeDragMode::Horizontal,
                            SizeDragMode::Vertical,
                            SizeDragMode::Distance,
                        ] {
                            ui.selectable_value(&mut mode, m, size_drag_mode_label(m));
                        }
                    });
                if mode != self.shortcuts.size_drag_mode() {
                    self.shortcuts.set_size_drag_mode(mode);
                }
            });
            ui.separator();
            if ui.button("Reset all").clicked() {
                self.shortcuts.reset_all();
                self.capture = None;
                self.status = "Shortcuts reset to defaults.".to_string();
            }
        });
        self.show_shortcuts = open && !scrim;
    }
}
