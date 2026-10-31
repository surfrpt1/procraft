use crate::*;

impl ProCraftApp {
    /// Top safe inset for the mobile shell: the status bar/notch strip the
    /// canvas must not paint under. Read per frame (rotations change it);
    /// `PROCRAFT_SAFE_TOP` stands in for a device on host captures.
    pub(crate) fn status_inset_top(&mut self) -> f32 {
        if let Some(px) = forced_safe_top() {
            return px;
        }
        #[cfg(target_os = "ios")]
        {
            let (top, _) = rfd::ios_safe_area_insets().unwrap_or((0, 0));
            clamp_status_inset(top)
        }
        #[cfg(target_os = "android")]
        {
            if self.android_inset_top < 0.0 {
                self.android_inset_top = rfd::android_status_bar_height()
                    .map(clamp_status_inset)
                    .unwrap_or(0.0);
            }
            self.android_inset_top.max(0.0)
        }
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            0.0
        }
    }

    /// Fitted dialog presentation on the mobile shell; `None` on desktop.
    pub(crate) fn mobile_modal_frame(&mut self, ctx: &egui::Context) -> Option<MobileModal> {
        if !mobile_shell() {
            return None;
        }
        let vp = ctx.viewport_rect();
        let safe_top = self.status_inset_top();
        let safe_bottom = safe_bottom(true);
        let class = effective_layout(
            vp.width(),
            cfg!(any(target_os = "ios", target_os = "android")),
        );
        let t = design::tokens(self.theme, class);
        let fit = modal_fit(vp.size(), safe_top, safe_bottom, true)?;
        let anchor = mobile_modal_anchor(
            vp,
            safe_top,
            safe_bottom,
            class,
            mobile_island_height(t.control_h),
            t.gap,
            !quick_controls(self.tool).is_empty(),
        );
        let frame = egui::Frame::NONE
            .fill(t.surface_1)
            .stroke(egui::Stroke::new(1.0, t.stroke))
            .corner_radius(egui::CornerRadius::same(t.radius_card))
            .shadow(egui::epaint::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 0,
                color: egui::Color32::from_black_alpha(90),
            })
            .inner_margin(egui::Margin::same(12));
        Some(MobileModal { fit, anchor, frame })
    }

    /// Dim layer under a mobile sheet. Tapping it dismisses; it swallows
    /// canvas taps while a sheet is up. No-op on the desktop shell.
    pub(crate) fn mobile_scrim(&mut self, ctx: &egui::Context, id: &str) -> bool {
        if !mobile_shell() {
            return false;
        }
        let vp = ctx.viewport_rect();
        let mut clicked = false;
        egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Middle)
            .fixed_pos(vp.min)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(vp.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(110));
                clicked = resp.clicked();
            });
        clicked
    }

    /// Mobile chrome: floating clusters over a full-screen canvas, with the
    /// tools/layers sidebars presented as slide-over drawers. Every control is
    /// the same one the docked shell uses; only the presentation differs.
    pub(crate) fn mobile_chrome(&mut self, ctx: &egui::Context, class: BarLayout) {
        let t = design::tokens(self.theme, class);
        let vp = ctx.viewport_rect();
        let pad = 12.0;
        let safe_top = self.status_inset_top();
        let safe_bottom = safe_bottom(true);
        let phone = class == BarLayout::Narrow;
        // Floating islands sit slightly translucent over the canvas and use
        // fully rounded ends so they read as capsules, not panels.
        let island_fill =
            |c: egui::Color32| egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 242);
        let island_frame = || {
            egui::Frame::NONE
                .fill(island_fill(t.surface_2))
                .stroke(egui::Stroke::new(1.0, t.stroke))
                .corner_radius(egui::CornerRadius::same(
                    ((t.control_h + 12.0) * 0.5).round() as u8,
                ))
                .inner_margin(egui::Margin::from(egui::vec2(6.0, 6.0)))
        };
        let drawer_frame = || {
            egui::Frame::NONE
                .fill(t.surface_1)
                .stroke(egui::Stroke::new(1.0, t.stroke))
                .corner_radius(egui::CornerRadius::same(t.radius_card))
                .inner_margin(egui::Margin::from(egui::vec2(6.0, 6.0)))
        };
        let island_h = mobile_island_height(t.control_h);
        let strip_h = island_h;
        let hit = quick_hit_height(class);
        // The bar folds to a pocket pill like the phone strip, so the tool's
        // options can get out of the way without leaving the canvas.
        let capsule_visible = !quick_controls(self.tool).is_empty();
        let capsule_offset = if phone {
            phone_capsule_offset(safe_bottom, pad, strip_h, t.gap)
        } else {
            -(safe_bottom + pad)
        };

        if capsule_visible && !phone {
            // Tablet options bar: the phone's pocket-strip rules (grip drag,
            // three docks, fold to a pill) around the per-tool controls.
            // The control row scrolls horizontally with the bar hidden —
            // swipe, not a desktop scrollbar.
            let expanded = !self.strip_collapsed;
            let strip_w = if expanded {
                mobile_capsule_width(vp.width())
            } else {
                strip_mini_width(vp.width(), pad)
            };
            let left = strip_left_now(self.strip_dock, self.strip_offset, vp.width(), pad, strip_w);
            egui::Area::new(egui::Id::new("mobile-options"))
                .order(egui::Order::Middle)
                .anchor(egui::Align2::LEFT_BOTTOM, [left, capsule_offset])
                .show(ctx, |ui| {
                    ui.set_width(strip_w);
                    ui.spacing_mut().interact_size.y = hit;
                    island_frame().show(ui, |ui| {
                        ui.set_width((strip_w - 12.0).max(1.0));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(t.gap, t.gap);
                            let grip = strip_grip(ui, &t, hit);
                            self.strip_drag_ui(&grip, vp.width(), pad, strip_w);
                            if expanded {
                                let scroll_w =
                                    (strip_w - 12.0 - STRIP_GRIP_W - hit - t.gap).max(120.0);
                                egui::ScrollArea::horizontal()
                                    .id_salt("mobile-options-scroll")
                                    .max_width(scroll_w)
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                    )
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| self.quick_controls_ui(ui, hit, &t));
                                    });
                                if design::icon_button(ui, &t, design::Icon::ChevronDown, hit)
                                    .on_hover_text("Hide options")
                                    .clicked()
                                {
                                    self.strip_collapsed = true;
                                    self.strip_offset = 0.0;
                                }
                            } else {
                                if self.toggle_pill(
                                    ui,
                                    strip_tool_face(self.tool, self.has_symbols),
                                    true,
                                ) {
                                    self.strip_collapsed = false;
                                    self.strip_offset = 0.0;
                                }
                                if design::icon_button(ui, &t, design::Icon::Undo, hit)
                                    .on_hover_text("Undo")
                                    .clicked()
                                    && self.doc.undo()
                                {
                                    self.status = "Undone.".to_string();
                                }
                                if design::icon_button(ui, &t, design::Icon::ChevronUp, hit)
                                    .on_hover_text("Show options")
                                    .clicked()
                                {
                                    self.strip_collapsed = false;
                                    self.strip_offset = 0.0;
                                }
                            }
                        });
                    });
                });
        }

        // Slide-over motion: the panel keeps a `drawer_shown` copy for the
        // slide-out, and the scrim fades on the same progress.
        let drawer_open = self.drawer != MobileDrawer::None;
        if drawer_open {
            self.drawer_shown = self.drawer;
        }
        let draw_slide = ctx.animate_value_with_time(
            egui::Id::new("mobile-drawer-slide"),
            if drawer_open { 1.0 } else { 0.0 },
            0.18,
        );
        let shown = if drawer_open || draw_slide > 0.0 {
            self.drawer_shown
        } else {
            MobileDrawer::None
        };

        if shown != MobileDrawer::None {
            egui::Area::new(egui::Id::new("mobile-scrim"))
                .order(egui::Order::Middle)
                .fixed_pos(vp.min)
                .interactable(drawer_open)
                .show(ctx, |ui| {
                    let (rect, resp) = ui.allocate_exact_size(vp.size(), egui::Sense::click());
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        egui::Color32::from_black_alpha((110.0 * draw_slide) as u8),
                    );
                    if drawer_open && resp.clicked() {
                        self.drawer = MobileDrawer::None;
                        self.show_tools = false;
                        self.show_layers = false;
                    }
                });
        }

        if !phone {
            // The File popup hangs off a top island, so it must stop before
            // the options capsule (or the bottom inset when there is none).
            let menu_floor = vp.bottom()
                - safe_bottom
                - pad
                - if capsule_visible {
                    island_h + t.gap
                } else {
                    0.0
                };
            let menu_room = menu_popup_room(safe_top + pad + island_h + t.gap, menu_floor);
            egui::Area::new(egui::Id::new("mobile-top-left"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::LEFT_TOP, [pad, safe_top + pad])
                .show(ctx, |ui| {
                    island_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(t.gap, t.gap);
                            if self.pill(ui, "Gallery") {
                                self.gallery_open = !self.gallery_open;
                            }
                            egui::containers::menu::MenuButton::new("File")
                                .config(
                                    egui::containers::menu::MenuConfig::new().close_behavior(
                                        egui::PopupCloseBehavior::CloseOnClickOutside,
                                    ),
                                )
                                .ui(ui, |ui| {
                                    ui.spacing_mut().interact_size.y = t.control_h;
                                    self.file_menu_ui(ui, class, &t, Some(menu_room));
                                });
                            if self.pill(ui, "Reference") {
                                self.ref_open = !self.ref_open;
                            }
                            if self.pill(ui, "Clear") {
                                self.clear_layer();
                            }
                        });
                    });
                });

            egui::Area::new(egui::Id::new("mobile-top-right"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::RIGHT_TOP, [-pad, safe_top + pad])
                .show(ctx, |ui| {
                    island_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(t.gap, t.gap);
                            if design::icon_button(ui, &t, design::Icon::Undo, t.control_h)
                                .on_hover_text("Undo")
                                .clicked()
                                && self.doc.undo()
                            {
                                self.status = "Undone.".to_string();
                            }
                            if design::icon_button(ui, &t, design::Icon::Redo, t.control_h)
                                .on_hover_text("Redo")
                                .clicked()
                                && self.doc.redo()
                            {
                                self.status = "Redone.".to_string();
                            }
                            if self.pill(ui, "Tools") {
                                self.drawer = self.drawer.toggled(MobileDrawer::Tools);
                                self.show_tools = self.drawer == MobileDrawer::Tools;
                                if self.drawer == MobileDrawer::Tools {
                                    self.show_layers = false;
                                }
                            }
                            if self.pill(ui, "Layers") {
                                self.drawer = self.drawer.toggled(MobileDrawer::Layers);
                                self.show_layers = self.drawer == MobileDrawer::Layers;
                                if self.drawer == MobileDrawer::Layers {
                                    self.show_tools = false;
                                }
                            }
                            self.color_dot(ui, &t);
                            if design::theme_toggle(ui, &t, &mut self.theme) {
                                self.status = format!("{} theme.", self.theme.name());
                            }
                            egui::containers::menu::MenuButton::new("View")
                                .config(
                                    egui::containers::menu::MenuConfig::new().close_behavior(
                                        egui::PopupCloseBehavior::CloseOnClickOutside,
                                    ),
                                )
                                .ui(ui, |ui| {
                                    ui.spacing_mut().interact_size.y = t.control_h;
                                    self.view_controls(ui, true);
                                });
                        });
                    });
                });
        }

        if phone {
            // Phones trade the two top islands for one thumb-reachable strip.
            // Expanded it is edge-to-edge and scrolls; minimised it becomes a
            // pocket pill that keeps the essentials and can be dragged to
            // either side of the thumb zone (Apple's floating-bar behaviour).
            let expanded = !self.strip_collapsed;
            let strip_w = if expanded {
                vp.width() - 2.0 * pad
            } else {
                strip_mini_width(vp.width(), pad)
            };
            let left = strip_left_now(self.strip_dock, self.strip_offset, vp.width(), pad, strip_w);
            // A File popup opened from the strip gets the room between the
            // safe top and the strip, so it never slides under the bar.
            let strip_floor = vp.bottom() - safe_bottom - pad - strip_h - t.gap;
            let menu_room = menu_popup_room(safe_top + pad, strip_floor);
            egui::Area::new(egui::Id::new("mobile-strip"))
                .order(egui::Order::Foreground)
                .anchor(
                    egui::Align2::LEFT_BOTTOM,
                    [left, phone_strip_offset(safe_bottom, pad)],
                )
                .show(ctx, |ui| {
                    ui.set_width(strip_w);
                    ui.spacing_mut().interact_size.y = hit;
                    // A widget covering the whole island keeps touches on its
                    // gaps from reaching the canvas; buttons added after it
                    // still win inside their own rects.
                    let block =
                        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(strip_w, strip_h));
                    ui.interact(
                        block,
                        egui::Id::new("mobile-strip-block"),
                        egui::Sense::click_and_drag(),
                    );
                    island_frame().show(ui, |ui| {
                        ui.set_width((strip_w - 12.0).max(1.0));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(t.gap, t.gap);
                            if !expanded {
                                let grip = strip_grip(ui, &t, hit);
                                self.strip_drag_ui(&grip, vp.width(), pad, strip_w);
                                if self.toggle_pill(
                                    ui,
                                    strip_tool_face(self.tool, self.has_symbols),
                                    true,
                                ) {
                                    self.strip_collapsed = false;
                                }
                                if design::icon_button(ui, &t, design::Icon::Undo, hit)
                                    .on_hover_text("Undo")
                                    .clicked()
                                    && self.doc.undo()
                                {
                                    self.status = "Undone.".to_string();
                                }
                                if design::icon_button(ui, &t, design::Icon::ChevronUp, hit)
                                    .on_hover_text("Show tools")
                                    .clicked()
                                {
                                    self.strip_collapsed = false;
                                    self.strip_offset = 0.0;
                                }
                            } else {
                                let scroll_w = (strip_w - 12.0 - hit - t.gap).max(120.0);
                                egui::ScrollArea::horizontal()
                                    .id_salt("mobile-strip-scroll")
                                    .max_width(scroll_w)
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                    )
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing = egui::vec2(t.gap, t.gap);
                                            for tool in PHONE_STRIP_TOOLS {
                                                let face =
                                                    strip_tool_face(tool, self.has_symbols);
                                                if self.toggle_pill(ui, face, self.tool == tool) {
                                                    self.choose_tool(tool);
                                                }
                                            }
                                            design::readout_label(
                                                ui,
                                                &t,
                                                &format!("{:.0}", self.brush.size),
                                                &format!("{:.0}", BRUSH_SIZE_MAX),
                                            );
                                            ui.separator();
                                            if self.pill(ui, "Tools") {
                                                self.drawer =
                                                    self.drawer.toggled(MobileDrawer::Tools);
                                                self.show_tools =
                                                    self.drawer == MobileDrawer::Tools;
                                                if self.drawer == MobileDrawer::Tools {
                                                    self.show_layers = false;
                                                }
                                            }
                                            if self.pill(ui, "Layers") {
                                                self.drawer =
                                                    self.drawer.toggled(MobileDrawer::Layers);
                                                self.show_layers =
                                                    self.drawer == MobileDrawer::Layers;
                                                if self.drawer == MobileDrawer::Layers {
                                                    self.show_tools = false;
                                                }
                                            }
                                            self.color_dot(ui, &t);
                                            ui.separator();
                                            if design::icon_button(ui, &t, design::Icon::Undo, hit)
                                                .on_hover_text("Undo")
                                                .clicked()
                                                && self.doc.undo()
                                            {
                                                self.status = "Undone.".to_string();
                                            }
                                            if design::icon_button(ui, &t, design::Icon::Redo, hit)
                                                .on_hover_text("Redo")
                                                .clicked()
                                                && self.doc.redo()
                                            {
                                                self.status = "Redone.".to_string();
                                            }
                                            if self.pill(ui, "Gallery") {
                                                self.gallery_open = !self.gallery_open;
                                            }
                                            egui::containers::menu::MenuButton::new("File")
                                                .config(
                                                    egui::containers::menu::MenuConfig::new()
                                                        .close_behavior(
                                                            egui::PopupCloseBehavior::CloseOnClickOutside,
                                                        ),
                                                )
                                                .ui(ui, |ui| {
                                                    ui.spacing_mut().interact_size.y = t.control_h;
                                                    self.file_menu_ui(
                                                        ui,
                                                        class,
                                                        &t,
                                                        Some(menu_room),
                                                    );
                                                });
                                            if self.pill(ui, "Reference") {
                                                self.ref_open = !self.ref_open;
                                            }
                                            egui::containers::menu::MenuButton::new("More")
                                                .config(
                                                    egui::containers::menu::MenuConfig::new()
                                                        .close_behavior(
                                                            egui::PopupCloseBehavior::CloseOnClickOutside,
                                                        ),
                                                )
                                                .ui(ui, |ui| {
                                                    ui.spacing_mut().interact_size.y = t.control_h;
                                                    let armed =
                                                        self.xform.mode != XformMode::Off;
                                                    if ui
                                                        .button(if armed {
                                                            "Finish transform"
                                                        } else {
                                                            "Transform"
                                                        })
                                                        .clicked()
                                                    {
                                                        if armed {
                                                            // Disarm keeps the
                                                            // applied warp and
                                                            // hides the handles.
                                                            self.xform.arm(
                                                                XformMode::Off,
                                                                &self.doc,
                                                            );
                                                            self.status =
                                                                "Transform finished.".to_string();
                                                        } else {
                                                            self.xform.arm(
                                                                XformMode::Free,
                                                                &self.doc,
                                                            );
                                                            self.status = xform_mode_status(
                                                                XformMode::Free,
                                                            )
                                                            .to_string();
                                                        }
                                                        ui.close();
                                                    }
                                                    if ui.button("Clear layer").clicked() {
                                                        self.clear_layer();
                                                        ui.close();
                                                    }
                                                    if ui
                                                        .button(format!(
                                                            "Theme: {}",
                                                            self.theme.name()
                                                        ))
                                                        .clicked()
                                                    {
                                                        self.theme =
                                                            design::next_theme(self.theme);
                                                        self.status = format!(
                                                            "{} theme.",
                                                            self.theme.name()
                                                        );
                                                        ui.close();
                                                    }
                                                    ui.separator();
                                                    self.view_controls(ui, true);
                                                });
                                        });
                                    });
                                if design::icon_button(ui, &t, design::Icon::ChevronDown, hit)
                                    .on_hover_text("Hide tools")
                                    .clicked()
                                {
                                    self.strip_collapsed = true;
                                    self.strip_offset = 0.0;
                                }
                            }
                        });
                    });
                });
        }

        if shown != MobileDrawer::None {
            let tools = shown == MobileDrawer::Tools;
            let w = if phone {
                phone_drawer_width(vp.width())
            } else {
                mobile_drawer_width(vp.width())
            };
            let (top, h) = if phone {
                let room = PhoneRoom {
                    safe_top,
                    safe_bottom,
                    pad,
                    strip_h,
                    island_h,
                    gap: t.gap,
                };
                phone_drawer_span(vp.bottom(), room)
            } else {
                mobile_drawer_span(
                    vp.bottom(),
                    safe_top,
                    safe_bottom,
                    pad,
                    island_h,
                    t.gap,
                    capsule_visible,
                )
            };
            let slide = drawer_slide_distance(draw_slide, w, pad);
            let pos = if tools {
                egui::pos2(vp.left() + pad - slide, top)
            } else {
                egui::pos2(vp.right() - pad - w + slide, top)
            };
            egui::Area::new(egui::Id::new(if tools {
                "mobile-drawer-tools"
            } else {
                "mobile-drawer-layers"
            }))
            // One tier below popups: pressing a button in the drawer raises
            // the area to the top of its own order, which would otherwise put
            // the drawer over a menu opened from it. Menus are Foreground.
            .order(egui::Order::Middle)
            // Areas are clamped into the viewport by default, which pins the
            // closed drawer to the edge and kills the slide. Let it sit off
            // screen; the viewport clip still trims it.
            .constrain(false)
            .fixed_pos(pos)
            .interactable(drawer_open)
            .show(ctx, |ui| {
                // The panel covers its own gaps (margins, card spacing) so a
                // drag or tap between cards can never reach the canvas.
                let block = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(w, h));
                ui.interact(
                    block,
                    egui::Id::new("mobile-drawer-block"),
                    egui::Sense::click_and_drag(),
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(w, h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        drawer_frame().show(ui, |ui| {
                            ui.set_width(w - 2.0 * 6.0);
                            if tools {
                                self.tools_panel_body(ui, h < 400.0);
                            } else {
                                self.layers_panel_body(ui);
                            }
                        });
                    },
                );
            });
            // The panel bodies own their close arrows; honor them.
            if drawer_open && ((tools && !self.show_tools) || (!tools && !self.show_layers)) {
                self.drawer = MobileDrawer::None;
            }
        }

        // Status toast: the mobile shell has no status row, so the newest
        // message floats above the options capsule and fades out. It never
        // senses input.
        let now = ctx.input(|i| i.time);
        if self.status != self.status_seen {
            self.status_seen = self.status.clone();
            self.status_at = now;
        }
        let age = (now - self.status_at) as f32;
        let alpha = toast_alpha(age);
        if !self.status.is_empty() && alpha > 0.0 {
            let offset = toast_offset(capsule_offset, island_h, t.gap, capsule_visible);
            egui::Area::new(egui::Id::new("mobile-toast"))
                // Above the drawers (same layer would let them cover the toast).
                .order(egui::Order::Tooltip)
                .interactable(false)
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, offset])
                .show(ctx, |ui| {
                    ui.set_opacity(alpha);
                    ui.set_max_width((vp.width() - 2.0 * pad).min(520.0));
                    island_frame().show(ui, |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&self.status).small()).truncate(),
                        );
                    });
                });
            // Keep the fade running without a 60 fps spin through the hold.
            let wait = if age < TOAST_HOLD {
                TOAST_HOLD - age
            } else {
                0.016
            };
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(wait.max(0.016)));
        }
    }

    pub(crate) fn tool_options_bar(&mut self, ui: &mut egui::Ui, layout: BarLayout) {
        let hit = quick_hit_height(layout);
        let t = design::tokens(self.theme, layout);
        if layout != BarLayout::Wide {
            ui.spacing_mut().interact_size.y = hit;
        }
        if quick_scrolls(layout) {
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| self.quick_controls_ui(ui, hit, &t));
            });
        } else {
            ui.horizontal_wrapped(|ui| self.quick_controls_ui(ui, hit, &t));
        }
    }

    pub(crate) fn quick_controls_ui(&mut self, ui: &mut egui::Ui, hit: f32, t: &design::Tokens) {
        ui.strong(tool_label(self.tool, self.has_symbols));
        ui.separator();
        if quick_controls(self.tool).is_empty() {
            let hint = match self.tool {
                Tool::Hand => "Drag to pan · pinch to zoom",
                Tool::Move => "Drag to move layer content",
                _ => "",
            };
            ui.label(hint);
            return;
        }
        for control in quick_controls(self.tool) {
            self.quick_control_ui(ui, *control, hit, t);
        }
    }

    pub(crate) fn quick_control_ui(
        &mut self,
        ui: &mut egui::Ui,
        control: QuickControl,
        hit: f32,
        t: &design::Tokens,
    ) {
        let has_selection = self.doc.selection.is_some() || self.doc.selection_mask.is_some();
        match control {
            QuickControl::Size => {
                if self.tool == Tool::Liquify {
                    quick_slider(ui, &mut self.liquify_size, 8.0..=512.0, 1.0, "Size", t);
                } else {
                    quick_slider(
                        ui,
                        &mut self.brush.size,
                        BRUSH_SIZE_MIN..=BRUSH_SIZE_MAX,
                        1.0,
                        "Size",
                        t,
                    );
                }
            }
            QuickControl::Opacity => {
                quick_slider(ui, &mut self.brush.opacity, 0.05..=1.0, 0.05, "Opacity", t)
            }
            QuickControl::Softness => {
                quick_slider(ui, &mut self.brush.softness, 0.0..=1.0, 0.05, "Softness", t)
            }
            QuickControl::Spacing => {
                quick_slider(ui, &mut self.brush.spacing, 0.05..=1.0, 0.05, "Spacing", t)
            }
            QuickControl::Stabilizer => quick_slider(
                ui,
                &mut self.brush.stabilizer,
                0.0..=1.0,
                0.05,
                "Stabilizer",
                t,
            ),
            QuickControl::Strength => {
                if self.tool == Tool::Smudge {
                    quick_slider(
                        ui,
                        &mut self.smudge_strength,
                        0.0..=1.0,
                        0.05,
                        "Strength",
                        t,
                    );
                } else {
                    quick_slider(
                        ui,
                        &mut self.liquify_strength,
                        0.05..=1.0,
                        0.05,
                        "Strength",
                        t,
                    );
                }
            }
            QuickControl::Tolerance => {
                let before = self.fill_tolerance;
                quick_slider(ui, &mut self.fill_tolerance, 0..=255, 1.0, "Tolerance", t);
                if self.fill_tolerance != before && self.fill_preview.is_some() {
                    self.reapply_live_fill();
                }
            }
            QuickControl::Grow => {
                let before = self.fill_grow;
                quick_slider(ui, &mut self.fill_grow, 0..=16, 1.0, "Grow", t);
                if self.fill_grow != before && self.fill_preview.is_some() {
                    self.reapply_live_fill();
                }
            }
            QuickControl::PickerPx => {
                quick_slider(ui, &mut self.picker_size, 140.0..=420.0, 5.0, "Picker", t)
            }
            QuickControl::Feather => {
                quick_slider(ui, &mut self.sel_feather, 0..=32, 1.0, "Feather", t);
                if ui
                    .add_enabled(has_selection, egui::Button::new("Apply"))
                    .clicked()
                {
                    if self.doc.feather_selection(self.sel_feather) {
                        self.status =
                            format!("Feathered ({} px).", self.doc.selection_pixel_count());
                    } else {
                        self.status = "No selection to feather.".to_string();
                    }
                    self.sel_edges_dirty = true;
                }
            }
            QuickControl::SelGrow => {
                quick_slider(ui, &mut self.sel_grow, -32..=32, 1.0, "Grow", t);
                if ui
                    .add_enabled(has_selection, egui::Button::new("Apply"))
                    .clicked()
                {
                    if self.doc.grow_selection(self.sel_grow) {
                        self.status = format!("Grown ({} px).", self.doc.selection_pixel_count());
                    } else {
                        self.status = "No selection to grow.".to_string();
                    }
                    self.sel_edges_dirty = true;
                }
            }
            QuickControl::OpaqueThreshold => {
                quick_slider(ui, &mut self.sel_opaque_t, 0..=255, 1.0, "Opaque >", t);
                if ui
                    .add_enabled(has_selection, egui::Button::new("Select"))
                    .clicked()
                {
                    if self.doc.select_opaque(self.sel_opaque_t) {
                        self.status =
                            format!("Opaque selected ({} px).", self.doc.selection_pixel_count());
                    } else {
                        self.status = "Layer is empty.".to_string();
                    }
                    self.sel_edges_dirty = true;
                }
            }
            QuickControl::OutlinePx => {
                quick_slider(ui, &mut self.sel_stroke_w, 1..=16, 1.0, "Outline", t);
                if ui
                    .add_enabled(
                        has_selection && !self.text_paint_blocked(),
                        egui::Button::new("Paint"),
                    )
                    .clicked()
                {
                    self.doc.checkpoint();
                    let brush = self.base_brush();
                    let color = self.current_color();
                    if self.doc.stroke_selection(&brush, color, self.sel_stroke_w) {
                        self.status = "Outlined.".to_string();
                    } else {
                        self.status = "No selection to outline.".to_string();
                    }
                }
            }
            QuickControl::Invert => {
                if ui
                    .add_enabled(has_selection, egui::Button::new("Invert"))
                    .clicked()
                {
                    if self.doc.invert_selection() {
                        self.status =
                            format!("Inverted ({} px).", self.doc.selection_pixel_count());
                    } else {
                        self.status = "No selection to invert.".to_string();
                    }
                    self.sel_edges_dirty = true;
                }
            }
            QuickControl::ClearSelection => {
                if ui
                    .add_enabled(has_selection, egui::Button::new("Clear"))
                    .clicked()
                {
                    self.doc.clear_selection();
                    self.lasso_poly.clear();
                    self.sel_edges_dirty = true;
                    self.status = "Selection cleared.".to_string();
                }
            }
            QuickControl::SmoothSelection => {
                if ui
                    .add_enabled(has_selection, egui::Button::new("Smooth"))
                    .clicked()
                {
                    if self.doc.smooth_selection() {
                        self.status =
                            format!("Smoothed ({} px).", self.doc.selection_pixel_count());
                    } else {
                        self.status = "No selection to smooth.".to_string();
                    }
                    self.sel_edges_dirty = true;
                }
            }
            QuickControl::ShapeKind => {
                egui::ComboBox::from_id_salt("quick_shape")
                    .selected_text(self.shape_kind.name())
                    .show_ui(ui, |ui| {
                        if hit > 20.0 {
                            ui.spacing_mut().interact_size.y = hit;
                        }
                        for kind in ShapeKind::ALL {
                            ui.selectable_value(&mut self.shape_kind, kind, kind.name());
                        }
                    });
            }
            QuickControl::ShapeFill => {
                if self.shape_kind.fillable() {
                    ui.checkbox(&mut self.shape_fill, "Fill");
                }
            }
            QuickControl::ShapeSides => {
                if self.shape_kind == ShapeKind::Polygon {
                    quick_slider(ui, &mut self.shape_sides, 3..=12, 1.0, "Sides", t);
                }
            }
            QuickControl::CropApply => match self.crop_rect {
                Some((x0, y0, x1, y1)) => {
                    if ui.button("Apply crop").clicked() {
                        match self.doc.crop(x0, y0, x1, y1) {
                            Ok(()) => {
                                let (w, h) = (self.doc.w, self.doc.h);
                                self.after_canvas_change();
                                self.status = format!("Cropped to {w}x{h} (history cleared).");
                            }
                            Err(e) => self.status = format!("Crop failed: {e}"),
                        }
                    }
                }
                None => {
                    ui.label("Drag a rect on the canvas.");
                }
            },
            QuickControl::CropCancel => {
                if self.crop_rect.is_some() && ui.button("Cancel crop").clicked() {
                    self.crop_rect = None;
                    self.status = "Crop cancelled.".to_string();
                }
            }
            QuickControl::TextSize => quick_slider(
                ui,
                &mut self.text_size,
                MIN_TEXT_PX..=128.0,
                1.0,
                "Size px",
                t,
            ),
            QuickControl::TextLeading => quick_slider(
                ui,
                &mut self.text_leading,
                -MAX_TEXT_SPACING..=MAX_TEXT_SPACING,
                1.0,
                "Leading",
                t,
            ),
            QuickControl::TextTracking => quick_slider(
                ui,
                &mut self.text_tracking,
                -MAX_TEXT_SPACING..=MAX_TEXT_SPACING,
                1.0,
                "Tracking",
                t,
            ),
            QuickControl::TextOutline => {
                ui.checkbox(&mut self.text_outline, "Outline");
            }
            QuickControl::EditText => {
                if ui.button("Edit text").clicked() {
                    self.text_open = true;
                }
            }
        }
    }
}
