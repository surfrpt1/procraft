use crate::*;

impl ProCraftApp {
    /// Design tokens for the app's current theme and device class.
    pub(crate) fn tokens(&self, ui: &egui::Ui) -> design::Tokens {
        design::tokens(
            self.theme,
            effective_layout(
                ui.ctx().viewport_rect().width(),
                cfg!(any(target_os = "ios", target_os = "android")),
            ),
        )
    }

    /// A plain action button in the B2 visual language. Menu triggers stay
    /// stock `MenuButton`s on purpose; anything that is just "click me" is a
    /// pill.
    pub(crate) fn pill(&self, ui: &mut egui::Ui, label: &str) -> bool {
        let t = self.tokens(ui);
        design::pill_button(
            ui,
            &t,
            design::PillSpec {
                icon: None,
                glyph: None,
                label: Some(label),
                selected: false,
                enabled: true,
            },
        )
        .clicked()
    }

    /// Current paint colour as a tappable dot. On mobile it doubles as the
    /// Tools entry point: it opens that drawer scrolled to the Color card.
    pub(crate) fn color_dot(&mut self, ui: &mut egui::Ui, t: &design::Tokens) {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(t.control_h, t.control_h), egui::Sense::click());
        let dot_r = t.control_h * 0.5 - 8.0;
        ui.painter().circle_filled(
            rect.center(),
            dot_r,
            egui::Color32::from_rgb(self.color[0], self.color[1], self.color[2]),
        );
        ui.painter()
            .circle_stroke(rect.center(), dot_r, egui::Stroke::new(1.0, t.stroke));
        if resp.on_hover_text("Color").clicked() {
            self.drawer = MobileDrawer::Tools;
            self.show_tools = true;
            self.show_layers = false;
            self.mobile_scroll_color = true;
        }
    }

    /// A pill that shows selection state (the phone strip's paint tools).
    pub(crate) fn toggle_pill(&self, ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
        let t = self.tokens(ui);
        design::pill_button(
            ui,
            &t,
            design::PillSpec {
                icon: None,
                glyph: None,
                label: Some(label),
                selected,
                enabled: true,
            },
        )
        .clicked()
    }

    /// Shared grip drag for the bottom bars: the bar follows the finger and
    /// release snaps it to the nearest of the three docks. `docked_left`
    /// recomputes from the current dock each frame, so a drag that spans a
    /// release (or a resize) never accumulates drift.
    pub(crate) fn strip_drag_ui(
        &mut self,
        grip: &egui::Response,
        viewport_w: f32,
        pad: f32,
        width: f32,
    ) {
        let docked_left = strip_left(self.strip_dock, viewport_w, pad, width);
        if grip.drag_started() {
            self.strip_offset = 0.0;
        }
        if grip.dragged() {
            self.strip_offset += grip.drag_delta().x;
            self.strip_offset = (docked_left + self.strip_offset)
                .clamp(pad, strip_max_left(viewport_w, pad, width))
                - docked_left;
            grip.ctx.request_repaint();
        }
        if grip.drag_stopped() {
            let dropped =
                strip_left_clamped(docked_left + self.strip_offset, viewport_w, pad, width);
            self.strip_dock = strip_dock_for(dropped, viewport_w, pad, width);
            self.strip_offset = 0.0;
        }
    }

    /// Tool switch that also retires a live fill preview, mirroring the
    /// tool radio list in the Tools panel. The outgoing paint tool keeps
    /// its own size/opacity/softness and the entering one gets its own back.
    pub(crate) fn choose_tool(&mut self, tool: Tool) {
        if self.tool == Tool::Fill && tool != Tool::Fill {
            self.fill_preview = None;
        }
        if tool != self.tool {
            self.tool_brushes.save(self.tool, &self.brush);
            self.tool = tool;
            self.tool_brushes.load(tool, &mut self.brush);
        }
    }

    /// Clear the active layer as one undoable step. Shared by the desktop
    /// top bar and the mobile actions cluster.
    pub(crate) fn clear_layer(&mut self) {
        if self.text_paint_blocked() {
            return;
        }
        self.doc.checkpoint();
        if let Some(layer) = self.doc.active_layer_mut() {
            layer.clear();
        }
        self.doc.mark_full_dirty();
    }
}
