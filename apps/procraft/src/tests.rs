use super::*;
use procraft_engine::Taper;

#[test]
fn desktop_save_cancel_leaves_the_status_alone() {
    // Desktop rfd returns None only when the user cancelled the panel;
    // that must not be reported as a failure.
    let mut status = "before".to_string();
    note_save_declined(&mut status, "PNG");
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    assert_eq!(status, "before");
    let _ = status;
}

/// Drive a taper replay to completion the way the frame loop does.
fn taper_replay_all(
    doc: &mut Document,
    brush: &Brush,
    color: Color,
    stroke: &RecordedStroke,
    rect: Option<(u32, u32, u32, u32)>,
) -> bool {
    let Some(mut job) = taper_begin(doc, brush, color, stroke, rect) else {
        return false;
    };
    loop {
        match taper_step(doc, &mut job, stroke) {
            TaperStep::More => {}
            TaperStep::Done => return true,
            TaperStep::Cancelled => return false,
        }
    }
}

#[test]
fn brush_and_tip_filters_cover_every_importable_type() {
    for ext in [
        "pcbrush", "json", "abr", "brush", "brushset", "kpp", "bundle", "sut", "gbr", "png", "jpg",
        "jpeg", "webp", "bmp", "gif", "tif", "tiff",
    ] {
        assert!(
            BRUSH_IMPORT_EXTS.contains(&ext),
            "{ext} missing from brush filter"
        );
    }
    for ext in ["kpp", "bundle", "gbr", "png", "gif", "tif", "tiff"] {
        assert!(
            TIP_IMPORT_EXTS.contains(&ext),
            "{ext} missing from tip filter"
        );
    }
}

#[test]
#[ignore]
fn corpus_brush_previews_do_not_panic() {
    let Ok(dir) = std::env::var("PROCRAFT_BRUSH_CORPUS") else {
        return;
    };
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            continue;
        }
        let Ok(loaded) = load_brush_file_report(&path) else {
            continue;
        };
        for preset in loaded.brushes {
            let brush = Brush {
                eraser: false,
                ..preset.brush
            };
            let mut doc = Document::new(144, 36);
            paint_preview_stroke(&mut doc, &brush, Color::rgb(30, 30, 30));
        }
    }
}

#[test]
fn brush_import_status_reports_skipped_and_first_warning() {
    let path = std::path::Path::new("brushes.bundle");
    let status = brush_import_status(
        2,
        1,
        &[
            "Skipped preset 'Bad': no sampled tip.".to_string(),
            "Library is full; 1 brushes were not added.".to_string(),
        ],
        path,
    );
    assert!(status.contains("Imported 2 brushes"), "{status}");
    assert!(status.contains("Skipped 1 presets"), "{status}");
    assert!(status.contains("Skipped preset 'Bad'"), "{status}");
    assert!(status.contains("1 more warnings"), "{status}");
}

fn key_ev(key: egui::Key, mods: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: mods,
    }
}

#[test]
fn chord_from_event_maps_letters_digits_and_fkeys() {
    let b = chord_from_event(&key_ev(egui::Key::B, egui::Modifiers::NONE)).unwrap();
    assert_eq!(b, KeyChord::new(ChordKey::B, false, false, false));
    let n0 = chord_from_event(&key_ev(egui::Key::Num0, egui::Modifiers::CTRL)).unwrap();
    assert_eq!(n0, KeyChord::new(ChordKey::Num0, true, false, false));
    let f5 = chord_from_event(&key_ev(egui::Key::F5, egui::Modifiers::NONE)).unwrap();
    assert_eq!(f5, KeyChord::new(ChordKey::F5, false, false, false));
    assert!(chord_from_event(&key_ev(egui::Key::Escape, egui::Modifiers::NONE)).is_none());
    assert!(chord_from_event(&key_ev(egui::Key::Enter, egui::Modifiers::NONE)).is_none());
}

#[test]
fn shape_preview_matches_kind() {
    // Line: the bare drag segment.
    assert_eq!(
        shape_preview_points(ShapeKind::Line, 1.0, 2.0, 30.0, 40.0, 6),
        vec![(1.0, 2.0), (30.0, 40.0)]
    );
    // Rect: closed loop through the drag corners (either drag
    // direction traces the same box).
    let rect = shape_preview_points(ShapeKind::Rect, 30.0, 40.0, 10.0, 5.0, 6);
    assert_eq!(rect.len(), 5);
    assert_eq!(rect.first(), rect.last());
    assert_eq!(rect[0], (30.0, 40.0));
    assert_eq!(rect[2], (10.0, 5.0));
    // Closed families trace one loop per kind (star 10 vertices, n-gon
    // sides + 1), each closing on its first point.
    for (kind, n) in [
        (ShapeKind::Triangle, 4),
        (ShapeKind::Diamond, 5),
        (ShapeKind::Star, 11),
        (ShapeKind::Polygon, 7),
    ] {
        let pts = shape_preview_points(kind, 0.0, 0.0, 20.0, 20.0, 6);
        assert_eq!(pts.len(), n, "{}", kind.name());
        assert_eq!(pts.first(), pts.last(), "{} closes", kind.name());
    }
    // Sides clamp mirrors the engine (1 behaves like a triangle).
    assert_eq!(
        shape_preview_points(ShapeKind::Polygon, 0.0, 0.0, 20.0, 20.0, 1).len(),
        4
    );
    // Ellipse: sampled loop whose endpoints coincide to sub-pixel.
    let ell = shape_preview_points(ShapeKind::Ellipse, 0.0, 0.0, 20.0, 20.0, 6);
    assert!(ell.len() >= 25);
    let (ef, el) = (ell.first().unwrap(), ell.last().unwrap());
    assert!((ef.0 - el.0).abs() < 1e-3 && (ef.1 - el.1).abs() < 1e-3);
    // Arrow: open shaft from the drag start, head wings backtracked.
    let arrow = shape_preview_points(ShapeKind::Arrow, 5.0, 5.0, 45.0, 5.0, 6);
    assert_eq!(arrow.len(), 5);
    assert_eq!(arrow[0], (5.0, 5.0));
    assert_eq!(arrow[1], (45.0, 5.0));
}

#[test]
fn exact_matching_sends_ctrl_shift_z_to_redoalt() {
    let s = Shortcuts::default();
    let ev = key_ev(egui::Key::Z, egui::Modifiers::CTRL | egui::Modifiers::SHIFT);
    let chord = chord_from_event(&ev).unwrap();
    assert_eq!(s.owner_of(chord), Some(ActionId::RedoAlt));
    let ev2 = key_ev(egui::Key::Z, egui::Modifiers::CTRL);
    assert_eq!(
        s.owner_of(chord_from_event(&ev2).unwrap()),
        Some(ActionId::Undo)
    );
}

#[test]
fn tool_id_round_trips_to_tool() {
    assert_eq!(tool_from_id(ToolId::Brush), Tool::Brush);
    assert_eq!(tool_from_id(ToolId::Text), Tool::Text);
    assert_eq!(tool_from_id(ToolId::Liquify), Tool::Liquify);
}

#[test]
fn capture_resolution_pins_esc_delete_and_bind() {
    let esc = key_ev(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(matches!(
        resolve_capture_event(&esc),
        CaptureOutcome::Cancel
    ));
    let del = key_ev(egui::Key::Delete, egui::Modifiers::NONE);
    assert!(matches!(resolve_capture_event(&del), CaptureOutcome::Clear));
    let back = key_ev(egui::Key::Backspace, egui::Modifiers::NONE);
    assert!(matches!(
        resolve_capture_event(&back),
        CaptureOutcome::Clear
    ));
    let b = key_ev(egui::Key::B, egui::Modifiers::NONE);
    assert!(matches!(resolve_capture_event(&b), CaptureOutcome::Bind(c)
        if c == KeyChord::new(ChordKey::B, false, false, false)));
    let enter = key_ev(egui::Key::Enter, egui::Modifiers::NONE);
    assert!(matches!(
        resolve_capture_event(&enter),
        CaptureOutcome::Ignore
    ));
    let release = egui::Event::Key {
        key: egui::Key::B,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    assert!(matches!(
        resolve_capture_event(&release),
        CaptureOutcome::Ignore
    ));
}

#[test]
fn drag_size_clamps_to_the_slider_range() {
    assert_eq!(drag_size(50.0, 0.0), 50.0);
    assert_eq!(drag_size(50.0, 120.0), 62.5);
    assert_eq!(drag_size(1.0, -120.0), 1.0); // floor
    assert_eq!(drag_size(200.0, 120.0), 250.0);
    assert_eq!(drag_size(300.0, 0.0), 300.0);
    assert_eq!(drag_size(400.0, 120.0), 400.0); // ceiling
}

#[test]
fn settings_differ_ignores_zoom_but_catches_discrete_changes() {
    let a = procraft_engine::Settings::default();
    let mut b = a.clone();
    b.zoom = 2.0;
    assert!(!settings_differ(&a, &b), "zoom is coalesced elsewhere");
    b.recent = vec!["x.procraft".into()];
    assert!(settings_differ(&a, &b));
    let mut c = a.clone();
    c.show_tools = !c.show_tools;
    assert!(settings_differ(&a, &c));
    let mut d = a.clone();
    d.shortcuts.bindings.insert("undo".into(), None);
    assert!(settings_differ(&a, &d));
    // Strip state persists immediately like the other discrete edits.
    let mut e = a.clone();
    e.strip_collapsed = true;
    assert!(settings_differ(&a, &e));
    let mut f = a.clone();
    f.strip_dock = "left".into();
    assert!(settings_differ(&a, &f));
    // Time-lapse is a discrete edit too, so turning it off survives a
    // relaunch instead of silently coming back on.
    let mut g = a.clone();
    g.time_lapse = false;
    assert!(settings_differ(&a, &g));
}

#[test]
fn stabilizer_substeps_compose_to_one_frame_step() {
    // Two half-steps must converge exactly like one frame-sized step:
    // the trail rewrite must not change the stabilizer's feel.
    let a = 0.3_f32;
    let s = stabilizer_substep(a, 2);
    let (s0, p) = (0.0_f32, 1.0_f32);
    let s1 = s0 + (p - s0) * s;
    let s2 = s1 + (p - s1) * s;
    assert!((s2 - (s0 + (p - s0) * a)).abs() < 1e-6);
    // A single point keeps today's coefficient bit-for-bit.
    assert_eq!(stabilizer_substep(a, 1), a);
    // Raw mode (no stabilizer) stays raw for any trail length.
    assert_eq!(stabilizer_substep(0.0, 7), 0.0);
}

#[test]
fn speed_follows_raw_pointer_not_stabilized_trail() {
    let dt = 1.0 / 120.0;
    let a = 0.2975;
    let target = (1000.0 * dt, 0.0);
    let (smooth, speed) = sample_pointer(Some((0.0, 0.0)), Some((0.0, 0.0)), target, a, dt, 1.0);
    assert!(
        smooth.0 < target.0 * 0.8,
        "stabilizer should lag the pointer"
    );
    assert!(
        (speed - 0.5).abs() < 0.01,
        "raw 1000 px/s should map to half speed, got {speed}"
    );
}

#[test]
fn speed_is_measured_on_screen_at_any_zoom() {
    // The same hand speed must give the same velocity dynamics whether
    // the canvas is shown 1:1 or 4x: canvas pixels per second scale
    // with zoom, screen pixels do not.
    let dt = 1.0 / 120.0;
    let (_, at_one) = sample_pointer(
        Some((0.0, 0.0)),
        Some((0.0, 0.0)),
        (1000.0 * dt, 0.0),
        0.0,
        dt,
        1.0,
    );
    let (_, at_four) = sample_pointer(
        Some((0.0, 0.0)),
        Some((0.0, 0.0)),
        (250.0 * dt, 0.0),
        0.0,
        dt,
        4.0,
    );
    assert!((at_one - 0.5).abs() < 0.01, "1:1 speed {at_one}");
    assert!((at_one - at_four).abs() < 1e-4, "zoom changed the speed");
}

#[test]
fn pixels_hash_sees_any_change() {
    // Empty and identical buffers hash equal (length included).
    assert_eq!(pixels_hash(&[]), pixels_hash(&[]));
    let a = vec![7u8; 4096];
    assert_eq!(pixels_hash(&a), pixels_hash(&a.clone()));
    // One dab-sized byte flip anywhere (head, middle, tail) flips it.
    for i in [0usize, 1000, 2048, 4095] {
        let mut b = a.clone();
        b[i] = b[i].wrapping_add(1);
        assert_ne!(pixels_hash(&a), pixels_hash(&b), "byte {i}");
    }
    // Length alone distinguishes.
    assert_ne!(pixels_hash(&a), pixels_hash(&a[..2048]));
}

#[test]
fn brush_slot_selection_distinguishes_equal_brushes_and_reindexes() {
    let selected = Some(BrushSlot::Custom(1));
    assert!(slot_is_active(selected, BrushSlot::Custom(1)));
    assert!(!slot_is_active(selected, BrushSlot::Custom(0)));
    assert_eq!(slot_after_remove(selected, 0), Some(BrushSlot::Custom(0)));
    assert_eq!(slot_after_remove(selected, 1), None);
    assert_eq!(
        slot_after_remove(Some(BrushSlot::Builtin(2)), 0),
        Some(BrushSlot::Builtin(2))
    );
}

#[test]
fn brush_ring_radius_tracks_size_and_zoom() {
    // 20px brush at zoom 1: 10px ring.
    assert_eq!(brush_ring_radius(20.0, 1.0), 10.0);
    // Zoom scales the ring with the canvas.
    assert_eq!(brush_ring_radius(20.0, 2.0), 20.0);
    // Tiny brushes stay visible (2px floor).
    assert_eq!(brush_ring_radius(1.0, 0.5), 2.0);
}

#[test]
fn tool_ring_size_uses_the_tools_own_size() {
    assert_eq!(tool_ring_size(Tool::Brush, 20.0, 128.0), Some(20.0));
    assert_eq!(tool_ring_size(Tool::Eraser, 20.0, 128.0), Some(20.0));
    assert_eq!(tool_ring_size(Tool::Smudge, 20.0, 128.0), Some(20.0));
    assert_eq!(tool_ring_size(Tool::Liquify, 20.0, 128.0), Some(128.0));
    assert_eq!(tool_ring_size(Tool::Fill, 20.0, 128.0), None);
}

#[test]
fn bar_layout_splits_at_phone_width() {
    assert_eq!(bar_layout(320.0, true), BarLayout::Narrow);
    assert_eq!(bar_layout(599.0, true), BarLayout::Narrow);
    assert_eq!(bar_layout(360.0, false), BarLayout::Narrow);
    assert_eq!(bar_layout(600.0, true), BarLayout::Tablet);
    assert_eq!(bar_layout(820.0, true), BarLayout::Tablet);
    assert_eq!(bar_layout(844.0, true), BarLayout::Tablet);
    assert_eq!(bar_layout(600.0, false), BarLayout::Wide);
    assert_eq!(bar_layout(1280.0, false), BarLayout::Wide);
}

#[test]
fn quick_bar_covers_the_active_tools_controls() {
    assert_eq!(
        quick_controls(Tool::Brush),
        &[
            QuickControl::Size,
            QuickControl::Opacity,
            QuickControl::Softness,
            QuickControl::Spacing,
            QuickControl::Stabilizer,
        ]
    );
    assert_eq!(quick_controls(Tool::Eraser), quick_controls(Tool::Brush));
    assert_eq!(
        quick_controls(Tool::Smudge),
        &[QuickControl::Size, QuickControl::Strength]
    );
    assert_eq!(
        quick_controls(Tool::Fill),
        &[QuickControl::Tolerance, QuickControl::Grow]
    );
    assert_eq!(quick_controls(Tool::Picker), &[QuickControl::PickerPx]);
    assert_eq!(
        quick_controls(Tool::Liquify),
        &[QuickControl::Size, QuickControl::Strength]
    );
    assert_eq!(
        quick_controls(Tool::Select),
        &[
            QuickControl::Feather,
            QuickControl::SelGrow,
            QuickControl::OpaqueThreshold,
            QuickControl::OutlinePx,
            QuickControl::Invert,
            QuickControl::ClearSelection,
            QuickControl::SmoothSelection,
        ]
    );
    assert_eq!(quick_controls(Tool::Lasso), quick_controls(Tool::Select));
    assert_eq!(
        quick_controls(Tool::Shapes),
        &[
            QuickControl::ShapeKind,
            QuickControl::ShapeFill,
            QuickControl::ShapeSides,
        ]
    );
    assert_eq!(
        quick_controls(Tool::Text),
        &[
            QuickControl::TextSize,
            QuickControl::TextLeading,
            QuickControl::TextTracking,
            QuickControl::TextOutline,
            QuickControl::EditText,
        ]
    );
    assert_eq!(
        quick_controls(Tool::Crop),
        &[QuickControl::CropApply, QuickControl::CropCancel]
    );
    assert!(quick_controls(Tool::Move).is_empty());
    assert!(quick_controls(Tool::Hand).is_empty());
}

#[test]
fn quick_bar_touch_geometry_is_finger_sized() {
    assert_eq!(quick_hit_height(BarLayout::Wide), 26.0);
    assert_eq!(quick_hit_height(BarLayout::Narrow), 44.0);
    assert_eq!(quick_hit_height(BarLayout::Tablet), 44.0);
    assert!(quick_scrolls(BarLayout::Narrow));
    assert!(!quick_scrolls(BarLayout::Wide));
}

#[test]
fn forced_layout_wins_over_the_width_rules() {
    assert_eq!(
        resolve_layout(Some(BarLayout::Tablet), 380.0, false),
        BarLayout::Tablet
    );
    assert_eq!(
        resolve_layout(Some(BarLayout::Narrow), 1400.0, false),
        BarLayout::Narrow
    );
    assert_eq!(resolve_layout(None, 380.0, false), BarLayout::Narrow);
    assert_eq!(resolve_layout(None, 844.0, true), BarLayout::Tablet);
}

#[test]
fn native_mobile_gets_the_mobile_shell() {
    assert_eq!(resolve_shell(None, true), Shell::Mobile);
    assert_eq!(resolve_shell(None, false), Shell::Desktop);
}

#[test]
fn forced_shell_overrides_the_native_default() {
    assert_eq!(resolve_shell(Some(true), false), Shell::Mobile);
    assert_eq!(resolve_shell(Some(false), true), Shell::Desktop);
}

#[test]
fn drawer_trigger_swaps_and_retaps_close() {
    use MobileDrawer::{Layers, None, Tools};
    assert_eq!(None.toggled(Tools), Tools);
    assert_eq!(Tools.toggled(Tools), None);
    assert_eq!(Tools.toggled(Layers), Layers);
    assert_eq!(Layers.toggled(Layers), None);
    assert_eq!(Layers.toggled(Tools), Tools);
}

#[test]
fn drawer_width_keeps_a_canvas_edge_visible() {
    assert!((mobile_drawer_width(390.0) - 343.2).abs() < 0.01);
    assert_eq!(mobile_drawer_width(844.0), 360.0);
    assert_eq!(mobile_drawer_width(1366.0), 360.0);
    assert_eq!(mobile_drawer_width(240.0), 240.0);
}

#[test]
fn capsule_width_nearly_fills_phones_and_caps_on_tablets() {
    assert!((mobile_capsule_width(390.0) - 366.6).abs() < 0.01);
    assert_eq!(mobile_capsule_width(1180.0), 760.0);
    assert_eq!(mobile_capsule_width(820.0), 760.0);
}

#[test]
fn islands_are_one_control_row_plus_frame_margin() {
    assert_eq!(mobile_island_height(44.0), 56.0);
    assert_eq!(mobile_island_height(36.0), 48.0);
}

#[test]
fn sheets_centre_between_the_islands_and_the_bottom_bar() {
    let phone_landscape = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(844.0, 390.0));
    let off = mobile_modal_anchor(
        phone_landscape,
        0.0,
        0.0,
        BarLayout::Narrow,
        56.0,
        8.0,
        true,
    );
    // Free area 12..314 centres at 163; the viewport centre is 195.
    assert!((off.y - (-32.0)).abs() < 0.01);

    let phone_portrait = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(390.0, 844.0));
    let off = mobile_modal_anchor(
        phone_portrait,
        47.0,
        34.0,
        BarLayout::Narrow,
        56.0,
        8.0,
        true,
    );
    assert!((off.y - (-25.5)).abs() < 0.01);

    let tablet = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1024.0, 768.0));
    assert!(
        (mobile_modal_anchor(tablet, 24.0, 20.0, BarLayout::Tablet, 56.0, 8.0, true).y - 2.0).abs()
            < 0.01
    );
    assert!(
        (mobile_modal_anchor(tablet, 24.0, 20.0, BarLayout::Tablet, 56.0, 8.0, false).y - 34.0)
            .abs()
            < 0.01
    );

    // Too little free height to be a fair target: stay dead centre.
    let short = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(844.0, 300.0));
    assert_eq!(
        mobile_modal_anchor(short, 0.0, 0.0, BarLayout::Narrow, 56.0, 8.0, true),
        egui::Vec2::ZERO
    );
}

#[test]
fn file_menu_room_stops_short_of_the_bar() {
    // Phone landscape: safe top 12 -> strip top 324 leaves ~296 for rows.
    assert_eq!(menu_popup_room(12.0, 324.0), 296.0);
    // Portrait has plenty of room, so no scrolling.
    assert_eq!(menu_popup_room(59.0, 734.0), 659.0);
    // Never smaller than a couple of rows, even if the popup opens low.
    assert_eq!(menu_popup_room(300.0, 390.0), 200.0);
}

#[test]
fn file_menu_fades_only_while_more_rows_are_below() {
    // Scrolled to the last row: no more content, so no fade.
    assert_eq!(menu_fade_height(489.0, 296.0, 193.0), 0.0);
    // A list that fits needs no gradient either.
    assert_eq!(menu_fade_height(200.0, 296.0, 0.0), 0.0);
    // Rows below the fold: short fade, capped at a quarter of the body.
    assert_eq!(menu_fade_height(489.0, 296.0, 0.0), 24.0);
    assert_eq!(menu_fade_height(489.0, 60.0, 0.0), 15.0);
}

#[test]
fn tool_grid_packs_columns_only_in_the_mobile_drawer() {
    assert_eq!(tool_grid_columns(326.0, false, true), 2);
    assert_eq!(tool_grid_columns(300.0, false, true), 2);
    assert_eq!(tool_grid_columns(299.0, false, true), 1);
    // A shot drawer (landscape) adds a third column so all tools fit.
    assert_eq!(tool_grid_columns(326.0, true, true), 3);
    // The desktop dock keeps its single-column list at any width.
    assert_eq!(tool_grid_columns(326.0, false, false), 1);
    assert_eq!(tool_grid_columns(600.0, true, false), 1);
}

#[test]
fn drawer_span_clears_clusters_and_options_capsule() {
    let (top, h) = mobile_drawer_span(820.0, 24.0, 20.0, 12.0, 56.0, 8.0, true);
    assert_eq!(top, 100.0);
    let bottom = 820.0 - 20.0 - 12.0 - 56.0 - 8.0;
    assert_eq!(h, bottom - top);
    let (_, taller) = mobile_drawer_span(820.0, 24.0, 20.0, 12.0, 56.0, 8.0, false);
    assert_eq!(taller - h, 64.0);
    let (_, floor) = mobile_drawer_span(300.0, 100.0, 0.0, 12.0, 56.0, 8.0, true);
    assert_eq!(floor, 200.0);
}

#[test]
fn phone_drawer_is_wider_and_keeps_the_strip_only_when_tall() {
    assert!((phone_drawer_width(390.0) - 358.8).abs() < 0.01);
    assert_eq!(phone_drawer_width(844.0), 360.0);
    assert_eq!(phone_drawer_width(240.0), 240.0);
    let room = PhoneRoom {
        safe_top: 24.0,
        safe_bottom: 20.0,
        pad: 12.0,
        strip_h: 56.0,
        island_h: 56.0,
        gap: 8.0,
    };
    // Portrait: the strip stays visible, so the drawer stops above it.
    let (top, h) = phone_drawer_span(844.0, room);
    assert_eq!(top, 36.0);
    assert_eq!(h, 844.0 - 20.0 - 12.0 - 56.0 - 8.0 - 36.0);
    // Landscape: too little room above the strip, so the drawer runs to
    // the bottom inset and covers the strip while it is open.
    let (_, tight) = phone_drawer_span(400.0, room);
    assert_eq!(tight, 400.0 - 20.0 - 12.0 - 36.0);
    assert_eq!(phone_drawer_span(200.0, room).1, 200.0);
}

#[test]
fn phone_strip_sits_above_the_home_indicator() {
    assert_eq!(safe_bottom(false), 0.0);
    assert_eq!(phone_strip_offset(0.0, 12.0), -12.0);
    assert_eq!(phone_strip_offset(34.0, 12.0), -46.0);
    assert_eq!(phone_capsule_offset(34.0, 12.0, 56.0, 8.0), -110.0);
    assert_eq!(phone_capsule_offset(0.0, 12.0, 56.0, 8.0), -76.0);
}

#[test]
fn phone_strip_offers_the_three_paint_tools() {
    assert_eq!(PHONE_STRIP_TOOLS, [Tool::Brush, Tool::Eraser, Tool::Smudge]);
    for tool in PHONE_STRIP_TOOLS {
        assert!(!quick_controls(tool).is_empty());
        assert_ne!(tool_glyph(tool), "");
    }
}

#[test]
fn pocket_strip_docks_to_three_thumb_positions() {
    for dock in [StripDock::Left, StripDock::Center, StripDock::Right] {
        assert_eq!(StripDock::from_str(dock.as_str()), dock);
    }
    assert_eq!(StripDock::from_str("somewhere"), StripDock::Center);

    let (vp, pad, w) = (390.0, 12.0, 260.0);
    assert_eq!(strip_left(StripDock::Left, vp, pad, w), 12.0);
    assert_eq!(strip_left(StripDock::Center, vp, pad, w), 65.0);
    assert_eq!(strip_left(StripDock::Right, vp, pad, w), 118.0);
    // A drag can never push the pill off either edge.
    assert_eq!(strip_left_clamped(-50.0, vp, pad, w), 12.0);
    assert_eq!(strip_left_clamped(500.0, vp, pad, w), 118.0);
    assert_eq!(strip_max_left(vp, pad, w), 118.0);

    // Release snaps to the nearest docked centre.
    assert_eq!(strip_dock_for(12.0, vp, pad, w), StripDock::Left);
    assert_eq!(strip_dock_for(65.0, vp, pad, w), StripDock::Center);
    assert_eq!(strip_dock_for(118.0, vp, pad, w), StripDock::Right);
    assert_eq!(strip_dock_for(70.0, vp, pad, w), StripDock::Center);
}

#[test]
fn pocket_bar_follows_a_drag_and_stays_on_screen() {
    // This frame's left edge = dock + in-flight offset, clamped.
    let (vp, pad, w) = (1024.0, 12.0, 760.0);
    assert_eq!(strip_left_now(StripDock::Center, 0.0, vp, pad, w), 132.0);
    assert_eq!(strip_left_now(StripDock::Center, 40.0, vp, pad, w), 172.0);
    // Off-screen drags pin to the edges instead of leaving a gap.
    assert_eq!(strip_left_now(StripDock::Left, -80.0, vp, pad, w), 12.0);
    assert_eq!(strip_left_now(StripDock::Right, 999.0, vp, pad, w), 252.0);
}

#[test]
fn pocket_strip_shrinks_to_fit_narrow_phones() {
    assert_eq!(strip_mini_width(390.0, 12.0), 260.0);
    assert_eq!(strip_mini_width(320.0, 12.0), 260.0);
    assert_eq!(strip_mini_width(280.0, 12.0), 256.0);
    assert_eq!(strip_mini_width(150.0, 12.0), 180.0);
    // Tablet viewports get the same pocket pill, never wider.
    assert_eq!(strip_mini_width(1024.0, 12.0), 260.0);
}

#[test]
fn board_dots_keep_a_steady_screen_spacing_across_zoom() {
    for zoom in [0.1_f32, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0] {
        let screen = board_dot_step(zoom) * zoom;
        assert!(
            (BOARD_DOT_SCREEN_MIN..=BOARD_DOT_SCREEN_MAX).contains(&screen),
            "zoom {zoom} gives {screen}pt dot spacing"
        );
    }
}

#[test]
fn canvas_origin_snaps_only_at_whole_device_pixel_zoom() {
    // 100 % on a 1x display: whole points.
    assert_eq!(
        snap_canvas_origin(egui::pos2(279.4, 100.6), 1.0, 1.0, 0.0),
        egui::pos2(279.0, 101.0)
    );
    // 200 % on a 1.5x display: 3 device px per canvas pixel, so the
    // origin still lands on a physical pixel (2/3 pt steps).
    assert_eq!(
        snap_canvas_origin(egui::pos2(100.1, 40.2), 2.0, 1.5, 0.0),
        egui::pos2(100.0, 40.0)
    );
    // Fractional zoom: no grid to land on, leave the pan untouched.
    let free = egui::pos2(10.3, 10.3);
    assert_eq!(snap_canvas_origin(free, 2.58, 1.0, 0.0), free);
    // Rotated views are mapped by angle, so snapping would misalign.
    assert_eq!(snap_canvas_origin(free, 1.0, 1.0, 0.4), free);
}

#[test]
fn board_dots_snap_to_a_power_of_two_canvas_grid() {
    // Alignment across zoom levels matters: doubling the step must land
    // on the same points, so every step is BOARD_DOT_STEP * 2^k.
    for zoom in [0.1_f32, 0.3, 0.5, 1.0, 2.0, 3.7, 8.0] {
        let ratio = board_dot_step(zoom) / BOARD_DOT_STEP;
        assert!(
            ratio > 0.0 && (ratio.log2().fract()).abs() < 1e-4,
            "zoom {zoom}"
        );
    }
    assert_eq!(board_dot_step(1.0), BOARD_DOT_STEP);
    // One octave of zoom either way keeps roughly the same density.
    assert_eq!(board_dot_step(0.5), BOARD_DOT_STEP * 2.0);
    assert_eq!(board_dot_step(2.0), BOARD_DOT_STEP * 0.5);
}

#[test]
fn board_dot_span_brackets_the_visible_range() {
    assert_eq!(board_dot_span(32.0, 0.0, 64.0), (0, 2));
    assert_eq!(board_dot_span(32.0, -10.0, 70.0), (0, 2));
    assert_eq!(board_dot_span(32.0, 5.0, 40.0), (1, 1));
    // Nothing to draw when the range falls between two grid lines.
    let (first, last) = board_dot_span(32.0, 5.0, 20.0);
    assert!(first > last);
}

#[test]
fn board_fit_leaves_a_margin_around_the_page() {
    let avail = egui::vec2(1024.0, 1366.0);
    let z = board_fit_zoom(1280.0, 800.0, avail);
    assert!(
        (1280.0 * z - (avail.x - 2.0 * BOARD_FIT_MARGIN)).abs() < 1e-3,
        "fitted width should fill the padded viewport"
    );
    assert!(800.0 * z <= avail.y - 2.0 * BOARD_FIT_MARGIN);
    assert!(1280.0 * z < avail.x, "board stays visible beside the page");

    // Never larger than the unpadded fit, and still clamped at both ends.
    let plain = (avail.x / 1280.0).min(avail.y / 800.0);
    assert!(board_fit_zoom(1280.0, 800.0, avail) <= plain);
    assert_eq!(board_fit_zoom(8.0, 8.0, avail), 8.0);
    assert_eq!(board_fit_zoom(1280.0, 800.0, egui::vec2(40.0, 40.0)), 0.05);
}

#[test]
fn board_centres_the_page_only_when_it_fits() {
    let avail = egui::vec2(1000.0, 800.0);
    assert_eq!(
        center_offset(egui::vec2(1000.0, 800.0), avail),
        egui::Vec2::ZERO
    );
    assert_eq!(
        center_offset(egui::vec2(600.0, 400.0), avail),
        egui::vec2(200.0, 200.0)
    );
    // Overflow clips instead of centring, so a zoomed-in page keeps its
    // origin at the viewport edge.
    assert_eq!(
        center_offset(egui::vec2(1600.0, 1200.0), avail),
        egui::Vec2::ZERO
    );
}

#[test]
fn page_sensor_covers_the_centred_and_panned_page() {
    let slot = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 500.0));
    let avail = egui::vec2(1000.0, 700.0);
    let page = canvas_page_rect(slot, avail, egui::Vec2::ZERO);
    assert_eq!(page.min, egui::pos2(100.0, 100.0));
    // The far corner of the visible page must accept a stroke start;
    // the old sensor stayed on the slot and ended at (800, 500).
    assert!(page.contains(egui::pos2(899.0, 599.0)));

    // Touch pan moves the sensor with the artwork, not the slot.
    let panned = canvas_page_rect(slot, avail, egui::vec2(30.0, -20.0));
    assert_eq!(panned.min, egui::pos2(130.0, 80.0));
    assert_eq!(panned.size(), page.size());

    // A page bigger than the board is not centred: only the pan moves it.
    let big = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 900.0));
    assert_eq!(
        canvas_page_rect(big, avail, egui::vec2(-50.0, 0.0)).min,
        egui::pos2(-50.0, 0.0)
    );
}

#[test]
fn status_inset_clamps_wild_values() {
    assert_eq!(clamp_status_inset(0), 0.0);
    assert_eq!(clamp_status_inset(72), 72.0);
    assert_eq!(clamp_status_inset(9999), 200.0);
}

#[test]
fn safe_top_override_parses_points_and_clamps() {
    assert_eq!(safe_inset_override(None), None);
    assert_eq!(safe_inset_override(Some("")), None);
    assert_eq!(safe_inset_override(Some("nope")), None);
    assert_eq!(safe_inset_override(Some(" 59 ")), Some(59.0));
    assert_eq!(safe_inset_override(Some("-4")), None);
    assert_eq!(safe_inset_override(Some("9999")), Some(200.0));
    assert_eq!(safe_inset_override(Some("0")), Some(0.0));
}

#[test]
fn toast_holds_then_fades_to_nothing() {
    assert_eq!(toast_alpha(-0.1), 0.0);
    assert_eq!(toast_alpha(0.0), 1.0);
    assert_eq!(toast_alpha(TOAST_HOLD), 1.0);
    assert!((toast_alpha(TOAST_HOLD + TOAST_FADE * 0.5) - 0.5).abs() < 1e-5);
    assert_eq!(toast_alpha(TOAST_HOLD + TOAST_FADE), 0.0);
    assert_eq!(toast_alpha(60.0), 0.0);
}

#[test]
fn toast_stacks_above_the_options_capsule() {
    let capsule = phone_capsule_offset(34.0, 12.0, 56.0, 8.0);
    assert_eq!(toast_offset(capsule, 56.0, 8.0, true), capsule - 64.0);
    assert_eq!(toast_offset(-24.0, 56.0, 8.0, false), -24.0);
}

#[test]
fn drawer_slides_a_full_width_off_screen() {
    assert_eq!(drawer_slide_distance(1.0, 358.8, 12.0), 0.0);
    assert!((drawer_slide_distance(0.0, 358.8, 12.0) - 382.8).abs() < 0.01);
    assert!((drawer_slide_distance(0.5, 358.8, 12.0) - 191.4).abs() < 0.01);
    // Out-of-range progress can never overshoot the edge.
    assert_eq!(drawer_slide_distance(2.0, 100.0, 10.0), 0.0);
    assert_eq!(drawer_slide_distance(-1.0, 100.0, 10.0), 120.0);
}

#[test]
fn mobile_modals_fit_the_viewport_and_clear_the_status_strip() {
    // Desktop dialogs keep their own size.
    assert_eq!(modal_fit(egui::vec2(1600.0, 900.0), 0.0, 0.0, false), None);

    let phone = modal_fit(egui::vec2(390.0, 844.0), 47.0, 0.0, true).unwrap();
    assert!((phone.x - 358.8).abs() < 0.01);
    // 92% alone would start 34 px down, under the 47 pt status bar.
    assert_eq!(phone.y, 844.0 - 2.0 * (47.0 + 12.0));

    let tablet = modal_fit(egui::vec2(1024.0, 768.0), 24.0, 0.0, true).unwrap();
    assert_eq!(tablet.y, 768.0 - 2.0 * (24.0 + 12.0));

    // No inset: the plain 92% cap, still a usable dialog in landscape.
    let land = modal_fit(egui::vec2(844.0, 390.0), 0.0, 0.0, true).unwrap();
    assert!((land.y - 390.0 * 0.92).abs() < 0.01);
    assert!(land.y >= 200.0);
}

#[test]
fn every_tool_has_a_unique_quick_control_row() {
    let tools = [
        Tool::Brush,
        Tool::Eraser,
        Tool::Smudge,
        Tool::Select,
        Tool::Lasso,
        Tool::Fill,
        Tool::Picker,
        Tool::Move,
        Tool::Hand,
        Tool::Liquify,
        Tool::Crop,
        Tool::Shapes,
        Tool::Text,
    ];
    for tool in tools {
        let mut seen: Vec<QuickControl> = Vec::new();
        for control in quick_controls(tool) {
            assert!(!seen.contains(control), "{tool:?} repeats {control:?}");
            seen.push(*control);
        }
    }
    // The touch strip can still name its tools when glyphs are missing.
    for tool in PHONE_STRIP_TOOLS {
        assert!(!strip_tool_face(tool, false).is_empty());
        assert_ne!(strip_tool_face(tool, true), strip_tool_face(tool, false));
    }
}

#[test]
fn xform_axis_lines_hug_bounds_and_dedup_canvas_edges() {
    assert_eq!(xform_axis_lines(0.0, 7.0, 2, 7.0), vec![0.0, 3.5, 7.0]);
    assert_eq!(
        xform_axis_lines(1.0, 14.0, 1, 15.0),
        vec![0.0, 1.0, 14.0, 15.0]
    );
    // Four interior cells between the bounds edges.
    let lines = xform_axis_lines(1.0, 14.0, 4, 15.0);
    assert_eq!(lines.len(), 7);
    assert_eq!(lines[1], 1.0);
    assert_eq!(lines[5], 14.0);
}

#[test]
fn xform_bounds_grid_spans_canvas_and_bounds() {
    let (g, cols, rows) = xform_bounds_grid((1.0, 1.0, 14.0, 14.0), 16.0, 16.0, 2, 2);
    assert_eq!((cols, rows), (4, 4));
    assert_eq!(g.len(), 25);
    assert_eq!(g[0], (0.0, 0.0));
    assert!(g.contains(&(1.0, 1.0)));
    assert!(g.contains(&(14.0, 14.0)));
    assert!(g.contains(&(15.0, 15.0)));
}

#[test]
fn xform_mesh_cells_rebuild_the_lattice() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Mesh, &doc);
    assert_eq!((s.grid_cols, s.grid_rows), (4, 4));
    s.set_mesh_cells(4, 3, &doc);
    assert_eq!((s.mesh_cols, s.mesh_rows), (4, 3));
    assert_eq!((s.grid_cols, s.grid_rows), (6, 5));
    assert_eq!(s.grid.len(), 7 * 6);
    // Out-of-range picks clamp instead of collapsing the lattice.
    s.set_mesh_cells(0, 99, &doc);
    assert_eq!((s.mesh_cols, s.mesh_rows), (1, 8));
}

#[test]
fn xform_mesh_only_drags_points_inside_the_bounds() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Mesh, &doc);
    let stride = s.grid_cols as usize + 1;
    assert!(!s.mesh_point_active(0));
    assert!(s.mesh_point_active(stride + 1));
    assert!(s.mesh_point_active(stride * (stride - 1) - 2));
    assert!(!s.mesh_point_active(stride * (stride - 1) - 1));
    assert!(!s.mesh_point_active(stride * (stride - 1)));
}

#[test]
fn xform_arm_seeds_pins_on_content_bounds() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Distort, &doc);
    assert_eq!(
        s.pins,
        vec![(1.0, 1.0), (14.0, 1.0), (14.0, 14.0), (1.0, 14.0)]
    );
    assert!(!s.tris.is_empty());
    assert_eq!(s.grab, None);
    assert_eq!(s.mode, XformMode::Distort);
    // Switching to Mesh re-seeds the bounds lattice and clears the grab.
    s.pins = vec![(5.0, 5.0)];
    s.grab = Some(Grab::Pin(2));
    s.arm(XformMode::Mesh, &doc);
    assert_eq!(s.grid.len(), 25);
    assert_eq!(s.grab, None);
    assert_eq!(s.mode, XformMode::Mesh);
    // Off keeps the last positions so re-arming starts where the
    // user left off; it still drops any grab.
    s.grab = Some(Grab::Pin(4));
    s.arm(XformMode::Off, &doc);
    assert_eq!(s.grid.len(), 25);
    assert_eq!(s.grab, None);
    assert_eq!(s.mode, XformMode::Off);
}

#[test]
fn xform_delaunay_covers_the_canvas_hull() {
    let pts = [
        (1.0, 1.0),
        (14.0, 1.0),
        (14.0, 14.0),
        (1.0, 14.0),
        (0.0, 0.0),
        (15.0, 0.0),
        (15.0, 15.0),
        (0.0, 15.0),
    ];
    let tris = xform_delaunay(&pts);
    assert!(!tris.is_empty());
    let mut area = 0.0f64;
    for t in &tris {
        assert!(t.iter().all(|&i| (i as usize) < pts.len()));
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        area += xform_tri_area(
            (a.0 as f64, a.1 as f64),
            (b.0 as f64, b.1 as f64),
            (c.0 as f64, c.1 as f64),
        )
        .abs();
    }
    assert!((area * 0.5 - 225.0).abs() < 1.0, "hull area {area}");
}

#[test]
fn xform_delaunay_tolerates_degenerate_points() {
    let pts = [(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0), (0.0, 0.0)];
    let tris = xform_delaunay(&pts);
    for t in &tris {
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        assert!(area.abs() > 1e-3);
    }
    assert!(xform_delaunay(&[(1.0, 1.0), (2.0, 2.0)]).is_empty());
}

#[test]
fn xform_pull_grid_identity_pins_keep_identity_grid() {
    let grid = [
        (0.0, 0.0),
        (4.0, 0.0),
        (8.0, 0.0),
        (0.0, 4.0),
        (4.0, 4.0),
        (8.0, 4.0),
        (0.0, 8.0),
        (4.0, 8.0),
        (8.0, 8.0),
    ];
    let solved = pull_grid(&grid, &grid, 8.0, 8.0, 2, 2).unwrap();
    for (s, i) in solved.iter().zip(grid.iter()) {
        assert!(
            (s.0 - i.0).abs() < 1e-3 && (s.1 - i.1).abs() < 1e-3,
            "{s:?} vs {i:?}"
        );
    }
}

#[test]
fn xform_pull_grid_carries_content_from_to_pin() {
    // Lattice center dragged from (4,4) to (6,4): the texel painted
    // at (4,4) must show at (6,4) after the solved warp.
    let mut doc = Document::new(8, 8);
    let o = ((4 * 8 + 4) * 4) as usize;
    doc.layers[0].pixels.set4(o, [255, 0, 0, 255]);
    let from = vec![
        (0.0, 0.0),
        (4.0, 0.0),
        (8.0, 0.0),
        (0.0, 4.0),
        (4.0, 4.0),
        (8.0, 4.0),
        (0.0, 8.0),
        (4.0, 8.0),
        (8.0, 8.0),
    ];
    let mut pins = from.clone();
    pins[4] = (6.0, 4.0);
    let grid = pull_grid(&pins, &from, 8.0, 8.0, 2, 2).expect("non-degenerate pins solve");
    doc.warp_layer(doc.active, 2, 2, &grid);
    let o = ((4 * 8 + 6) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get4(o), [255, 0, 0, 255]);
}

#[test]
fn xform_pull_grid_collapsed_pins_is_none() {
    let pins = vec![(4.0, 4.0); 9];
    assert!(pull_grid(&pins, &pins, 8.0, 8.0, 2, 2).is_none());
}

#[test]
fn xform_drag_moves_pin_by_pointer_delta() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Distort, &doc);
    s.begin_grab(0, (0.0, 0.0));
    s.drag_by_mod(2.5, 3.5, false, false);
    assert_eq!(s.pins[0], (2.5, 3.5));
    // Every tick measures from the grab start, not the last tick:
    // relative dragging never accumulates pointer jitter.
    s.drag_by_mod(1.0, 1.0, false, false);
    assert_eq!(s.pins[0], (1.0, 1.0));
}

#[test]
fn xform_hold_without_move_keeps_pins() {
    // Long-press fires drag_started with no movement (egui): the pin
    // must stay put so the release gate sees "not moved" and skips.
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Mesh, &doc);
    let before = s.grid.clone();
    let at = s.grid[4];
    s.begin_grab(4, at);
    s.drag_by_mod(0.0, 0.0, false, false);
    assert_eq!(s.grid, before);
    assert_eq!(s.grid.as_slice(), s.grab_from.as_slice());
}

#[test]
fn xform_cancel_restores_pins_from_grab() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Distort, &doc);
    let before = s.pins.clone();
    s.begin_grab(1, before[1]);
    s.drag_by_mod(5.0, 5.0, false, false);
    assert_ne!(s.pins, before);
    s.cancel_grab();
    assert_eq!(s.pins, before);
    assert_eq!(s.grab, None);
}

#[test]
fn xform_arm_restores_grab_before_switching_modes() {
    // Switching modes mid-grab restores the grabbed mode's points
    // first (into the mode that owns them), then seeds the new mode.
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Mesh, &doc);
    let at = s.grid[4];
    s.begin_grab(4, at);
    s.drag_by_mod(3.0, 0.0, false, false);
    assert_eq!(s.grid[4], (at.0 + 3.0, at.1));
    s.arm(XformMode::Distort, &doc);
    assert_eq!(s.grid[4], at);
    assert_eq!(
        s.pins,
        vec![(0.0, 0.0), (15.0, 0.0), (15.0, 15.0), (0.0, 15.0)]
    );
}

#[test]
fn xform_skew_locks_an_axis_and_slides_pairs() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Skew, &doc);
    let before = s.pins.clone();
    s.begin_grab(0, before[0]);
    s.drag_by_mod(4.0, 1.0, false, false);
    assert_eq!(s.skew_lock, Some(true));
    assert_eq!(s.pins[0], (before[0].0 + 4.0, before[0].1));
    assert_eq!(s.pins[1], (before[1].0 + 4.0, before[1].1));
    assert_eq!(s.pins[2], before[2]);
    assert_eq!(s.pins[3], before[3]);
    // A vertical second gesture slides the column instead.
    let mid = s.pins.clone();
    s.begin_grab(1, mid[1]);
    s.drag_by_mod(1.0, 5.0, false, false);
    assert_eq!(s.skew_lock, Some(false));
    assert_eq!(s.pins[1], (mid[1].0, mid[1].1 + 5.0));
    assert_eq!(s.pins[3], (mid[3].0, mid[3].1 + 5.0));
    assert_eq!(s.pins[0], mid[0]);
    assert_eq!(s.pins[2], mid[2]);
}

#[test]
fn xform_perspective_mirrors_the_partner_corner() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Perspective, &doc);
    let before = s.pins.clone();
    s.begin_grab(0, before[0]);
    s.drag_by_mod(3.0, 2.0, false, false);
    assert_eq!(s.pins[0], (before[0].0 + 3.0, before[0].1 + 2.0));
    assert_eq!(s.pins[1], (before[1].0 - 3.0, before[1].1 + 2.0));
    assert_eq!(s.pins[2], before[2]);
    assert_eq!(s.pins[3], before[3]);
    // Squeezing past the minimum width clamps instead of inverting.
    let at = s.pins[0];
    s.begin_grab(0, at);
    s.drag_by_mod(100.0, 0.0, false, false);
    assert!(s.pins[1].0 - s.pins[0].0 >= 4.0 - 1e-3);
}

#[test]
fn xform_free_move_translates_all_pins() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Free, &doc);
    let before = s.pins.clone();
    assert!(s.point_in_frame((7.0, 7.0)));
    assert!(!s.point_in_frame((0.2, 0.2)));
    s.begin_all((7.0, 7.0));
    s.drag_by_mod(-2.0, 3.0, false, false);
    for (p, f) in s.pins.iter().zip(before.iter()) {
        assert_eq!(*p, (f.0 - 2.0, f.1 + 3.0));
    }
}

#[test]
fn xform_free_scale_keeps_the_opposite_corner() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Free, &doc);
    let before = s.pins.clone();
    let at = before[0];
    s.begin_grab(0, at);
    s.drag_by_mod(-3.0, -3.0, false, false);
    // Corner 0 walks to the pointer; corner 2 anchors the gesture.
    assert_eq!(s.pins[0], (-3.0, -3.0));
    assert_eq!(s.pins[2], before[2]);
    assert_eq!(s.pins[1], (before[2].0, -3.0));
    assert_eq!(s.pins[3], (-3.0, before[2].1));
    // Dragging onto the anchor clamps to a 4px box, never a line.
    let at = s.pins[0];
    s.begin_grab(0, at);
    s.drag_by_mod(18.0, 18.0, false, false);
    assert_eq!(s.pins[1].0 - s.pins[0].0, 4.0);
    assert_eq!(s.pins[3].1 - s.pins[0].1, 4.0);
}

#[test]
fn xform_free_rotate_keeps_the_center() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Free, &doc);
    let before = s.pins.clone();
    let c = xform_points_center(&before);
    s.begin_rotate((c.0, c.1 - 10.0));
    s.drag_by_mod(10.0, 10.0, false, false);
    let after = xform_points_center(&s.pins);
    assert!((after.0 - c.0).abs() < 1e-3 && (after.1 - c.1).abs() < 1e-3);
    // Quarter turn: TL lands on TR's old spot, distance preserved.
    assert!((s.pins[0].0 - before[1].0).abs() < 1e-3);
    assert!((s.pins[0].1 - before[1].1).abs() < 1e-3);
    let d0 = (before[0].0 - c.0).hypot(before[0].1 - c.1);
    let d1 = (s.pins[0].0 - after.0).hypot(s.pins[0].1 - after.1);
    assert!((d0 - d1).abs() < 1e-3);
    assert!(s.rotate_handle(1.0).is_some());
    assert!(!s.point_in_frame(s.rotate_handle(1.0).unwrap()));
}

#[test]
fn xform_free_move_shift_locks_the_dominant_axis() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Free, &doc);
    let before = s.pins.clone();
    s.begin_all((7.0, 7.0));
    s.drag_by_mod(6.0, 2.0, true, false);
    for (p, f) in s.pins.iter().zip(before.iter()) {
        assert_eq!(*p, (f.0 + 6.0, f.1));
    }
    let mid = s.pins.clone();
    s.begin_all((7.0, 7.0));
    s.drag_by_mod(1.0, -4.0, true, false);
    for (p, f) in s.pins.iter().zip(mid.iter()) {
        assert_eq!(*p, (f.0, f.1 - 4.0));
    }
}

#[test]
fn xform_free_corner_shift_locks_the_aspect_ratio() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Free, &doc);
    let before = s.pins.clone();
    s.begin_grab(0, before[0]);
    s.drag_by_mod(-3.0, -6.0, true, false);
    let (w0, h0) = frame_extents(&before);
    let (w1, h1) = frame_extents(&s.pins);
    assert!(
        (w1 / h1 - w0 / h0).abs() < 1e-4,
        "aspect drifted: {} vs {w0}/{h0} -> {w1}/{h1}",
        w1 / h1
    );
    // The opposite corner still anchors the gesture.
    assert_eq!(s.pins[2], before[2]);
}

#[test]
fn xform_free_corner_alt_anchors_at_the_center() {
    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Free, &doc);
    let before = s.pins.clone();
    let c = xform_points_center(&before);
    s.begin_grab(0, before[0]);
    s.drag_by_mod(3.0, 3.0, false, true);
    // Growth is symmetric about the center: the opposite corner
    // travels as far as the dragged one, and the center never moves.
    assert!((s.pins[0].0 - (before[0].0 + 3.0)).abs() < 1e-4);
    assert!((s.pins[0].1 - (before[0].1 + 3.0)).abs() < 1e-4);
    assert!((s.pins[2].0 - (before[2].0 - 3.0)).abs() < 1e-4);
    assert!((s.pins[2].1 - (before[2].1 - 3.0)).abs() < 1e-4);
    let c1 = xform_points_center(&s.pins);
    assert!((c1.0 - c.0).abs() < 1e-4 && (c1.1 - c.1).abs() < 1e-4);
}

#[test]
fn xform_free_rotate_shift_snaps_to_fifteen_degrees() {
    assert_eq!(snap_angle(7.4), 0.0);
    assert_eq!(snap_angle(7.6), 15.0);
    assert_eq!(snap_angle(-7.0), 0.0);
    assert_eq!(snap_angle(-8.0), -15.0);
    assert_eq!(snap_angle(52.0), 45.0);

    let doc = Document::new(16, 16);
    let mut s = XformState::off();
    s.arm(XformMode::Free, &doc);
    let c = xform_points_center(&s.pins);
    // Pointer 10 px above the center, dragged 10 right / 3 down:
    // a 55° turn, snapped to 60°.
    s.begin_rotate((c.0, c.1 - 10.0));
    s.drag_by_mod(10.0, 3.0, true, false);
    let v = (s.pins[0].0 - c.0, s.pins[0].1 - c.1);
    let deg = v.1.atan2(v.0).to_degrees();
    assert!((deg - -75.0).abs() < 1e-3, "corner angle was {deg}");
    assert_eq!(constrain_axis(6.0, 2.0), (6.0, 0.0));
    assert_eq!(constrain_axis(1.0, -4.0), (0.0, -4.0));
}

#[test]
fn xform_puppet_adds_and_removes_pins() {
    let mut doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Puppet, &doc);
    assert_eq!(s.pins.len(), 4);
    assert!(s.add_pin((7.0, 7.0), &doc));
    assert_eq!(s.pins.len(), 5);
    assert!(s.tris.iter().any(|t| t.contains(&4)));
    assert!(s
        .tris
        .iter()
        .all(|t| t.iter().all(|&i| (i as usize) < s.pins.len() + 4)));
    assert!(s.base.is_none());
    assert!(s.remove_pin_near((7.1, 7.2), 2.0, &mut doc));
    assert_eq!(s.pins.len(), 4);
    assert!(!s.pins.contains(&(7.0, 7.0)));
    // The frame itself is never removed (a mesh needs its corners).
    s.pins.truncate(3);
    assert!(!s.remove_pin_near((1.0, 1.0), 100.0, &mut doc));
    assert_eq!(s.pins.len(), 3);
}

#[test]
fn xform_expand_mesh_widens_the_puppet_frame() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.expand = 10.0;
    s.arm(XformMode::Puppet, &doc);
    assert_eq!(s.bounds, Some((0.0, 0.0, 15.0, 15.0)));
    assert_eq!(
        s.pins,
        vec![(0.0, 0.0), (15.0, 0.0), (15.0, 15.0), (0.0, 15.0)]
    );
}

#[test]
fn xform_pin_click_modes_build_the_selection() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Puppet, &doc);
    s.add_pin((7.0, 7.0), &doc);
    s.add_pin((9.0, 9.0), &doc);
    // Adding a pin makes it the sole active/selected pin.
    assert_eq!(s.active_pin, Some(5));
    assert_eq!(s.selected_pins, vec![5]);

    assert!(s.click_pin(4, false));
    assert_eq!(s.selected_pins, vec![4]);
    assert_eq!(s.active_pin, Some(4));

    // Shift adds/removes without discarding the rest.
    s.click_pin(5, true);
    assert_eq!(s.selected_pins, vec![4, 5]);
    assert_eq!(s.active_pin, Some(5));
    s.click_pin(4, true);
    assert_eq!(s.selected_pins, vec![5]);

    s.pin_select = PinSelect::Add;
    s.click_pin(4, false);
    assert_eq!(s.selected_pins, vec![4, 5]);
    s.pin_select = PinSelect::Subtract;
    s.click_pin(5, false);
    assert_eq!(s.selected_pins, vec![4]);
    s.pin_select = PinSelect::Toggle;
    s.click_pin(4, false);
    assert!(s.selected_pins.is_empty());
    assert_eq!(s.active_pin, None);

    // Shift-click waits for release: a press must not pre-select and
    // then let the click toggle the pin straight back off.
    s.pin_select = PinSelect::Replace;
    s.selected_pins = vec![5];
    s.prepare_pin_drag(4, true);
    assert_eq!(s.selected_pins, vec![5]);
    assert_eq!(s.active_pin, Some(4));
}

#[test]
fn xform_selected_pins_drag_as_a_group() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Puppet, &doc);
    s.add_pin((7.0, 7.0), &doc);
    s.add_pin((9.0, 9.0), &doc);
    let frame = s.pins[..4].to_vec();
    s.pin_select = PinSelect::Add;
    s.click_pin(4, false);
    s.click_pin(5, false);
    assert_eq!(s.selected_pins, vec![4, 5]);

    s.prepare_pin_drag(4, false);
    s.begin_selected((0.0, 0.0));
    assert_eq!(s.grab, Some(Grab::Selected));
    s.drag_by_mod(2.0, -1.0, false, false);
    assert_eq!(s.pins[4], (9.0, 6.0));
    assert_eq!(s.pins[5], (11.0, 8.0));
    assert_eq!(s.pins[..4], frame[..]);
}

#[test]
fn xform_marquee_selects_pins_inside_the_box() {
    let doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Puppet, &doc);
    s.add_pin((6.0, 6.0), &doc);
    s.add_pin((10.0, 10.0), &doc);

    s.marquee = Some(((5.0, 5.0), (11.0, 11.0)));
    s.marquee_select(false);
    assert_eq!(s.selected_pins, vec![4, 5]);
    assert_eq!(s.active_pin, Some(5));

    // Shift extends an existing selection rather than replacing it.
    s.marquee = Some(((5.0, 5.0), (7.0, 7.0)));
    s.marquee_select(true);
    assert_eq!(s.selected_pins, vec![4, 5]);

    // A plain box replaces; Subtract removes the boxed pins.
    s.pin_select = PinSelect::Subtract;
    s.marquee = Some(((5.0, 5.0), (7.0, 7.0)));
    s.marquee_select(false);
    assert_eq!(s.selected_pins, vec![5]);
    s.pin_select = PinSelect::Replace;
    s.marquee = Some(((9.0, 9.0), (11.0, 11.0)));
    s.marquee_select(false);
    assert_eq!(s.selected_pins, vec![5]);
}

#[test]
fn xform_removing_a_pin_keeps_selection_indices_valid() {
    let mut doc = xform_test_doc16();
    let mut s = XformState::off();
    s.arm(XformMode::Puppet, &doc);
    s.add_pin((6.0, 6.0), &doc);
    s.add_pin((8.0, 8.0), &doc);
    s.add_pin((10.0, 10.0), &doc);
    s.selected_pins = vec![4, 5, 6];
    s.active_pin = Some(6);
    assert!(s.remove_pin(5, &mut doc));
    assert_eq!(s.pins.len(), 6);
    assert_eq!(s.selected_pins, vec![4, 5]);
    assert_eq!(s.active_pin, Some(5));
    assert_eq!(s.pins[5], (10.0, 10.0));
}

// --- Live transform preview session: drag previews from a base ---

/// 8x8 layer with a solid opaque 4x4 red square at (2..6, 2..6).
fn xform_test_doc() -> Document {
    let mut doc = Document::new(8, 8);
    let mut dense = vec![0u8; 8 * 8 * 4];
    for y in 2..6 {
        for x in 2..6 {
            let i = (y * 8 + x) * 4;
            dense[i] = 255;
            dense[i + 3] = 255;
        }
    }
    doc.layers[0].pixels = Raster::from_dense(8, 8, 4, &dense);
    doc
}

/// 16x16 layer with a solid opaque 8x8 red square at (4..12, 4..12):
/// content bounds (1, 1, 14, 14) after the 3px frame pad.
fn xform_test_doc16() -> Document {
    let mut doc = Document::new(16, 16);
    let mut dense = vec![0u8; 16 * 16 * 4];
    for y in 4..12 {
        for x in 4..12 {
            let i = (y * 16 + x) * 4;
            dense[i] = 255;
            dense[i + 3] = 255;
        }
    }
    doc.layers[0].pixels = Raster::from_dense(16, 16, 4, &dense);
    doc
}

/// 8x8 layer painted edge to edge: the frame pins land on the canvas
/// anchors, so the whole canvas is one rigid mesh.
fn xform_test_doc_full() -> Document {
    let mut doc = Document::new(8, 8);
    let mut dense = vec![0u8; 8 * 8 * 4];
    for p in dense.chunks_mut(4) {
        p[0] = 255;
        p[3] = 255;
    }
    doc.layers[0].pixels = Raster::from_dense(8, 8, 4, &dense);
    doc
}

fn xform_alpha(doc: &Document, x: usize, y: usize) -> u8 {
    doc.layers[0].pixels.get((y * 8 + x) * 4 + 3)
}

fn xform_alpha16(doc: &Document, layer: usize, x: usize, y: usize) -> u8 {
    doc.layers[layer].pixels.get((y * 16 + x) * 4 + 3)
}

fn xform_opaque16(doc: &Document, layer: usize) -> usize {
    doc.layers[layer]
        .pixels
        .to_dense()
        .chunks(4)
        .filter(|p| p[3] != 0)
        .count()
}

/// One gesture: grab pin 0, translate every pin by (dx, dy), preview,
/// then drop the grab the way a release does.
fn xform_translate(doc: &mut Document, x: &mut XformState, dx: f32, dy: f32) -> XformApply {
    x.begin_grab(0, (0.0, 0.0));
    for p in x.points_mut() {
        p.0 += dx;
        p.1 += dy;
    }
    let r = xform_preview(doc, x);
    x.grab = None;
    x.grab_from.clear();
    r
}

#[test]
fn xform_drag_previews_live_and_content_follows() {
    let mut doc = xform_test_doc();
    let before = doc.layers[0].pixels.clone();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    // Mid-drag tick: the pixels change on the same call, no release.
    assert_eq!(
        xform_translate(&mut doc, &mut x, 2.0, 0.0),
        XformApply::Applied
    );
    assert_ne!(doc.layers[0].pixels, before);
    // Content followed the pins: the square sits 2px to the right.
    assert_eq!(xform_alpha(&doc, 3, 3), 0);
    assert_eq!(xform_alpha(&doc, 4, 3), 255);
    assert_eq!(xform_alpha(&doc, 7, 3), 255);
}

#[test]
fn xform_session_does_not_compound() {
    let mut doc = xform_test_doc();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    // Two half-pixel gestures: every tick re-warps from the session
    // base, so the sum is one clean pixel — no double-resampled edges
    // (which would fade 255 -> ~127 -> ~64).
    assert_eq!(
        xform_translate(&mut doc, &mut x, 0.5, 0.0),
        XformApply::Applied
    );
    assert_eq!(
        xform_translate(&mut doc, &mut x, 0.5, 0.0),
        XformApply::Applied
    );
    assert_eq!(xform_alpha(&doc, 2, 3), 0);
    assert_eq!(xform_alpha(&doc, 3, 3), 255);
    assert_eq!(xform_alpha(&doc, 6, 3), 255);
    assert_eq!(xform_alpha(&doc, 7, 3), 0);
}

#[test]
fn xform_cancel_restores_pixels_from_base() {
    let mut doc = xform_test_doc();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    assert_eq!(
        xform_translate(&mut doc, &mut x, 1.0, 0.0),
        XformApply::Applied
    );
    let after_one = doc.layers[0].pixels.clone();
    // Gesture 2 runs, then aborts (Esc / pinch): pins snap back to
    // the gesture start and the pixels follow — bit-for-bit the
    // state before gesture 2.
    x.begin_grab(0, (0.0, 0.0));
    for p in x.points_mut() {
        p.0 += 4.0;
    }
    assert_eq!(xform_preview(&mut doc, &mut x), XformApply::Applied);
    assert_ne!(doc.layers[0].pixels, after_one);
    x.cancel_grab();
    assert_eq!(xform_preview(&mut doc, &mut x), XformApply::Applied);
    assert_eq!(doc.layers[0].pixels, after_one);
}

#[test]
fn xform_reset_rolls_back_the_live_preview() {
    let mut doc = xform_test_doc();
    let pristine = doc.layers[0].pixels.clone();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    assert_eq!(
        xform_translate(&mut doc, &mut x, 2.0, 0.0),
        XformApply::Applied
    );
    assert_ne!(doc.layers[0].pixels, pristine);
    x.reset_transform(&mut doc);
    assert_eq!(doc.layers[0].pixels, pristine);
    assert_eq!(x.pins, vec![(0.0, 0.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)]);
    assert!(x.base.is_none());
}

#[test]
fn xform_offcanvas_content_returns() {
    let mut doc = xform_test_doc_full();
    let pristine = doc.layers[0].pixels.clone();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    // Fling the full-bleed canvas far off-screen: everything clips.
    assert_eq!(
        xform_translate(&mut doc, &mut x, 40.0, 0.0),
        XformApply::Applied
    );
    assert_eq!(xform_alpha(&doc, 4, 3), 0);
    // Drag the pins straight back onto the base positions: the
    // content was never destroyed, so it returns bit-for-bit.
    assert_eq!(
        xform_translate(&mut doc, &mut x, -40.0, 0.0),
        XformApply::Restored
    );
    assert_eq!(doc.layers[0].pixels, pristine);
}

#[test]
fn xform_pin_preview_skips_degenerate_pins() {
    let mut doc = xform_test_doc();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    assert_eq!(
        xform_translate(&mut doc, &mut x, 1.0, 0.0),
        XformApply::Applied
    );
    let after_one = doc.layers[0].pixels.clone();
    for p in x.points_mut() {
        *p = (3.5, 3.5);
    }
    assert_eq!(xform_preview(&mut doc, &mut x), XformApply::Skipped);
    assert_eq!(doc.layers[0].pixels, after_one);
}

#[test]
fn xform_mesh_preview_translates_and_restores() {
    let mut doc = xform_test_doc();
    let pristine = doc.layers[0].pixels.clone();
    let mut x = XformState::off();
    x.arm(XformMode::Mesh, &doc);
    assert_eq!(
        xform_translate(&mut doc, &mut x, 2.0, 0.0),
        XformApply::Applied
    );
    assert_eq!(xform_alpha(&doc, 3, 3), 0);
    assert_eq!(xform_alpha(&doc, 6, 3), 255);
    assert_eq!(
        xform_translate(&mut doc, &mut x, -2.0, 0.0),
        XformApply::Restored
    );
    assert_eq!(doc.layers[0].pixels, pristine);
}

#[test]
fn xform_external_change_recaptures_base() {
    let mut doc = xform_test_doc();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    assert_eq!(
        xform_translate(&mut doc, &mut x, 0.5, 0.0),
        XformApply::Applied
    );
    // Something else edits the canvas behind the session's back.
    let i = ((8 + 1) * 4) as usize;
    doc.layers[0].pixels.set(i + 1, 200);
    doc.layers[0].pixels.set(i + 3, 255);
    doc.mark_full_dirty();
    // The first tick after an external edit re-arms instead of
    // warping the edited pixels through pin geometry that no longer
    // belongs to them, so the edit survives and nothing is erased.
    assert_eq!(
        xform_translate(&mut doc, &mut x, 0.5, 0.0),
        XformApply::NoOp
    );
    assert!(doc.layers[0]
        .pixels
        .to_dense()
        .chunks(4)
        .any(|p| p[1] >= 20));
    // The fresh session then warps coherently from the edited pixels.
    assert_eq!(
        xform_translate(&mut doc, &mut x, 0.5, 0.0),
        XformApply::Applied
    );
    assert!(doc.layers[0]
        .pixels
        .to_dense()
        .chunks(4)
        .any(|p| p[1] >= 20));
}

#[test]
fn xform_undo_then_drag_keeps_paint() {
    let mut doc = xform_test_doc_full();
    let pristine = doc.layers[0].pixels.clone();
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    // Fling the canvas away, then undo the gesture.
    assert_eq!(
        xform_translate(&mut doc, &mut x, 40.0, 0.0),
        XformApply::Applied
    );
    assert_eq!(xform_alpha(&doc, 4, 3), 0);
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, pristine);
    // The next drag must not map the restored pixels through the
    // stale off-canvas pin geometry.
    let r = xform_translate(&mut doc, &mut x, -40.0, 0.0);
    assert!(matches!(r, XformApply::NoOp | XformApply::Restored));
    assert_eq!(doc.layers[0].pixels, pristine);
}

#[test]
fn xform_layer_switch_rearms_instead_of_blanking() {
    let mut doc = xform_test_doc16();
    doc.add_layer("overlay".to_string());
    for y in 4..8 {
        for x in 0..4 {
            let i = (y * 16 + x) * 4;
            doc.layers[1].pixels.set(i + 1, 255);
            doc.layers[1].pixels.set(i + 3, 255);
        }
    }
    doc.active = 0;
    let mut x = XformState::off();
    x.arm(XformMode::Distort, &doc);
    let before = xform_opaque16(&doc, 1);
    assert_eq!(before, 16);
    // A different layer becomes active while the handles stay armed.
    doc.active = 1;
    let r = xform_translate(&mut doc, &mut x, 1.0, 0.0);
    assert_eq!(r, XformApply::NoOp);
    assert_eq!(xform_opaque16(&doc, 1), before);
    assert_eq!(xform_alpha16(&doc, 1, 0, 4), 255);
}

#[test]
fn xform_delete_pin_restores_its_effect() {
    let mut doc = xform_test_doc16();
    let pristine = doc.layers[0].pixels.clone();
    let mut x = XformState::off();
    x.arm(XformMode::Puppet, &doc);
    assert!(x.add_pin((8.0, 8.0), &doc));
    x.begin_grab(4, (8.0, 8.0));
    x.pins[4] = (11.0, 8.0);
    assert_eq!(xform_preview(&mut doc, &mut x), XformApply::Applied);
    x.grab = None;
    x.grab_from.clear();
    assert_ne!(doc.layers[0].pixels, pristine);
    // Deleting a pin removes its effect: the pristine base comes
    // back for the remaining pins instead of baking the warp.
    assert!(x.remove_pin_near((11.0, 8.0), 3.0, &mut doc));
    assert_eq!(doc.layers[0].pixels, pristine);
}

#[test]
fn xform_clear_pins_restores_original() {
    let mut doc = xform_test_doc16();
    let pristine = doc.layers[0].pixels.clone();
    let mut x = XformState::off();
    x.arm(XformMode::Puppet, &doc);
    assert!(x.add_pin((8.0, 8.0), &doc));
    x.begin_grab(4, (8.0, 8.0));
    x.pins[4] = (11.0, 8.0);
    assert_eq!(xform_preview(&mut doc, &mut x), XformApply::Applied);
    x.grab = None;
    x.grab_from.clear();
    assert_ne!(doc.layers[0].pixels, pristine);
    x.clear_pins(&mut doc);
    assert_eq!(doc.layers[0].pixels, pristine);
    assert_eq!(x.pins.len(), 4);
}

// --- Midpoint-smoothed stroke path (chord corners -> curves) ---

#[test]
fn stroke_curve_starts_with_dab_then_curves() {
    let mut c = StrokeCurve::new();
    assert!(c.push((1.0, 2.0)).is_none()); // first point: dab only
    let (p0, ctrl, p1) = c.push((5.0, 2.0)).unwrap();
    assert_eq!(p0, (1.0, 2.0));
    assert_eq!(ctrl, (1.0, 2.0)); // degenerate control -> line
    assert_eq!(p1, (3.0, 2.0)); // midpoint of the first pair
    let (p0, ctrl, p1) = c.push((5.0, 6.0)).unwrap();
    assert_eq!(p0, (3.0, 2.0));
    assert_eq!(ctrl, (5.0, 2.0));
    assert_eq!(p1, (5.0, 4.0));
}

#[test]
fn stroke_curve_rounds_corners_without_overshoot() {
    // A right-angle sample corner: the smoothed path must leave the
    // corner (cut inside the bend), never bulge past the samples.
    let mut c = StrokeCurve::new();
    assert!(c.push((0.0, 0.0)).is_none());
    assert!(c.push((10.0, 0.0)).is_some());
    let (p0, ctrl, p1) = c.push((10.0, 10.0)).unwrap();
    let m = stroke_quad_pt(p0, ctrl, p1, 0.5);
    let d = ((m.0 - 10.0).powi(2) + m.1.powi(2)).sqrt();
    assert!(d > 1.0, "curve should cut the corner (d={d})");
    assert!(m.0 <= 10.0 && m.1 >= 0.0); // stays inside the samples
}

#[test]
fn stroke_curve_finish_flushes_the_tail_once() {
    let mut c = StrokeCurve::new();
    assert!(c.push((0.0, 0.0)).is_none());
    assert!(c.push((8.0, 0.0)).is_some());
    assert_eq!(c.finish(), Some(((4.0, 0.0), (8.0, 0.0))));
    assert_eq!(c.finish(), None);
    // A new stroke starts fresh with a dab again.
    assert!(c.push((2.0, 2.0)).is_none());
}

#[test]
fn flush_stroke_tail_stamps_the_pending_tail_once() {
    let mut doc = Document::new(20, 6);
    let paint = Brush {
        size: 3.0,
        opacity: 1.0,
        softness: 0.0,
        ..Default::default()
    };
    doc.fill_rect(&paint, Color::rgb(255, 255, 255), 0, 0, 19, 5);
    let mut curve = StrokeCurve::new();
    assert!(curve.push((2.0, 3.0)).is_none());
    assert!(curve.push((10.0, 3.0)).is_some());
    let mut last = Some((6.0, 3.0));
    let mut leftover = 0.0;
    let mut record = RecordedStroke::default();
    assert!(flush_stroke_tail(
        &mut doc,
        &paint,
        Color::rgb(255, 0, 0),
        &mut curve,
        &mut last,
        DabDynamics::default(),
        &mut leftover,
        &mut record,
    ));
    let i = ((3 * doc.w + 9) * 4) as usize;
    assert_eq!(&doc.layers[0].pixels.to_dense()[i..i + 3], &[255, 0, 0]);
    // The tail is consumed exactly once.
    assert!(!flush_stroke_tail(
        &mut doc,
        &paint,
        Color::rgb(0, 255, 0),
        &mut curve,
        &mut last,
        DabDynamics::default(),
        &mut leftover,
        &mut record,
    ));
}

#[test]
fn flush_stroke_tail_without_a_pending_curve_is_a_noop() {
    let mut doc = Document::new(8, 8);
    let brush = Brush::default();
    let mut curve = StrokeCurve::new();
    let mut last = Some((4.0, 4.0));
    let mut leftover = 0.0;
    let mut record = RecordedStroke::default();
    let before = doc.layers[0].pixels.clone();
    assert!(!flush_stroke_tail(
        &mut doc,
        &brush,
        Color::rgb(255, 0, 0),
        &mut curve,
        &mut last,
        DabDynamics::default(),
        &mut leftover,
        &mut record,
    ));
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn taper_replay_rewinds_the_live_stroke_and_fades_the_tail() {
    let mut doc = Document::new(48, 12);
    let brush = Brush {
        size: 6.0,
        opacity: 1.0,
        softness: 0.0,
        spacing: 0.25,
        taper: Taper {
            end: 1.0,
            opacity: 1.0,
            ..Taper::NONE
        },
        ..Default::default()
    };
    let color = Color::rgb(255, 0, 0);
    let dyn_ = DabDynamics::default();
    doc.checkpoint();
    let mut record = RecordedStroke::default();
    let mut last = None;
    let mut leftover = 0.0;
    for x in [6.0, 14.0, 22.0, 30.0, 38.0, 46.0] {
        let (l, lo) = doc.stroke_to_dyn(&brush, color, last, dyn_, (x, 6.0), dyn_, leftover);
        record.push(last, dyn_, (x, 6.0), dyn_);
        last = l;
        leftover = lo;
    }
    let rect = doc
        .stroke_rect()
        .map(|(x0, y0, x1, y1)| (x0.saturating_sub(2), y0.saturating_sub(2), x1 + 2, y1 + 2));
    assert!(taper_replay_all(&mut doc, &brush, color, &record, rect));
    let alpha = |x: u32| doc.layers[0].pixels.get(((6 * doc.w + x) * 4 + 3) as usize);
    assert!(alpha(6) > 200, "head alpha {}", alpha(6));
    assert!(alpha(46) < 40, "tail alpha {}", alpha(46));
    assert!(alpha(20) < alpha(6));
}

#[test]
fn taper_replay_sliced_one_segment_per_frame_matches_one_pass() {
    let mut doc = Document::new(96, 20);
    let brush = Brush {
        size: 8.0,
        opacity: 1.0,
        softness: 0.0,
        spacing: 0.25,
        taper: Taper {
            start: 0.2,
            end: 0.4,
            opacity: 0.8,
            ..Taper::NONE
        },
        ..Default::default()
    };
    let color = Color::rgb(20, 120, 240);
    let dyn_ = DabDynamics::default();
    doc.checkpoint();
    let mut record = RecordedStroke::default();
    let mut last = None;
    let mut leftover = 0.0;
    for x in [8.0, 18.0, 28.0, 38.0, 48.0, 58.0, 68.0, 78.0, 88.0] {
        let (l, lo) = doc.stroke_to_dyn(&brush, color, last, dyn_, (x, 10.0), dyn_, leftover);
        record.push(last, dyn_, (x, 10.0), dyn_);
        last = l;
        leftover = lo;
    }
    let rect = doc
        .stroke_rect()
        .map(|(x0, y0, x1, y1)| (x0.saturating_sub(2), y0.saturating_sub(2), x1 + 2, y1 + 2));
    let mut whole = doc.clone();
    assert!(taper_replay_all(&mut whole, &brush, color, &record, rect));
    // The same replay with a zero budget stamps one segment per call,
    // the way the frame loop slices it.
    let mut sliced = doc.clone();
    let mut job = taper_begin(&mut sliced, &brush, color, &record, rect).unwrap();
    let mut calls = 0;
    loop {
        calls += 1;
        match taper_step_with(std::time::Duration::ZERO, &mut sliced, &mut job, &record) {
            TaperStep::More => {}
            TaperStep::Done => break,
            TaperStep::Cancelled => panic!("replay cancelled without an edit"),
        }
    }
    assert!(
        calls > 1,
        "zero budget stamped the whole replay in one call"
    );
    assert_eq!(whole.layers[0].pixels, sliced.layers[0].pixels);
}

#[test]
fn taper_replay_stops_when_the_document_moves_on() {
    let mut doc = Document::new(48, 12);
    let brush = Brush {
        size: 6.0,
        taper: Taper {
            end: 1.0,
            ..Taper::NONE
        },
        ..Default::default()
    };
    let color = Color::rgb(1, 2, 3);
    let dyn_ = DabDynamics::default();
    doc.checkpoint();
    let mut record = RecordedStroke::default();
    let mut last = None;
    let mut leftover = 0.0;
    for x in [6.0, 14.0, 22.0, 30.0] {
        let (l, lo) = doc.stroke_to_dyn(&brush, color, last, dyn_, (x, 6.0), dyn_, leftover);
        record.push(last, dyn_, (x, 6.0), dyn_);
        last = l;
        leftover = lo;
    }
    let rect = doc
        .stroke_rect()
        .map(|(x0, y0, x1, y1)| (x0.saturating_sub(2), y0.saturating_sub(2), x1 + 2, y1 + 2));
    let mut job = taper_begin(&mut doc, &brush, color, &record, rect).unwrap();
    // Something else paints between frames (a new stroke, a restore):
    // the replay must refuse to stamp over it.
    doc.dab(&brush, color, 4.0, 4.0);
    let before = doc.layers[0].pixels.clone();
    assert!(matches!(
        taper_step_with(std::time::Duration::ZERO, &mut doc, &mut job, &record),
        TaperStep::Cancelled
    ));
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn live_stroke_equals_the_release_replay_outside_the_tail() {
    // A pen lays ink only while it touches the paper: whatever the
    // release replay paints, the live stroke must already show.
    let mut doc = Document::new(72, 16);
    let brush = Brush {
        size: 6.0,
        opacity: 1.0,
        softness: 0.0,
        spacing: 0.25,
        taper: Taper {
            start: 0.25,
            end: 0.35,
            size: 0.7,
            shape: 1.0,
            ..Taper::NONE
        },
        ..Default::default()
    };
    let color = Color::rgb(30, 30, 30);
    doc.checkpoint();
    let mut record = RecordedStroke::default();
    let mut last: Option<(f32, f32)> = None;
    let mut leftover = 0.0;
    let mut travelled = 0.0;
    let mut last_taper = 1.0;
    for x in [8.0, 16.0, 24.0, 32.0, 40.0, 48.0, 56.0, 64.0] {
        let to = (x, 8.0);
        let leg = last.map_or(0.0, |(fx, fy)| {
            ((to.0 - fx).powi(2) + (to.1 - fy).powi(2)).sqrt()
        });
        let from_dyn = DabDynamics {
            taper: last_taper,
            ..Default::default()
        };
        let to_dyn = DabDynamics {
            taper: live_taper_at(&brush, travelled, leg),
            ..Default::default()
        };
        let (l, lo) = doc.stroke_to_dyn(&brush, color, last, from_dyn, to, to_dyn, leftover);
        record.push(last, from_dyn, to, to_dyn);
        last = l;
        leftover = lo;
        travelled += leg;
        last_taper = to_dyn.taper;
    }
    let live = doc.layers[0].pixels.clone();
    let (dw, dh) = (doc.w, doc.h);
    let alpha = |px: &Raster, x: u32, y: u32| px.get(((y * dw + x) * 4 + 3) as usize);
    let width = |px: &Raster, x: u32| (0..dh).filter(|&y| alpha(px, x, y) > 0).count();
    assert!(
        width(&live, 32) >= 5,
        "live stroke is {0} px wide at mid, not the 6 px brush",
        width(&live, 32)
    );
    let rect = doc
        .stroke_rect()
        .map(|(x0, y0, x1, y1)| (x0.saturating_sub(2), y0.saturating_sub(2), x1 + 2, y1 + 2));
    assert!(taper_replay_all(&mut doc, &brush, color, &record, rect));
    let replayed = doc.layers[0].pixels.clone();
    // Past the start ramp and before the end ramp the two agree byte
    // for byte: lifting adds nothing there.
    for x in 20..=44 {
        for y in 0..dh {
            assert_eq!(
                alpha(&live, x, y),
                alpha(&replayed, x, y),
                "lift changed pixel ({x}, {y})"
            );
        }
    }
    // The tail is the one thing the lift may change: it narrows.
    assert!(
        width(&replayed, 63) < width(&live, 63),
        "end taper did not narrow the tail: live {} replay {}",
        width(&live, 63),
        width(&replayed, 63)
    );
}

#[test]
fn screenshot_taper_replay_closes_both_tips() {
    let mut doc = Document::new(144, 36);
    let brush = Brush {
        size: 40.0,
        opacity: 1.0,
        softness: 0.0,
        spacing: 0.10,
        taper: Taper {
            start: 0.04,
            end: 0.05,
            size: 0.46,
            shape: 1.0,
            opacity: 0.0,
            pressure: 0.0,
        },
        ..Default::default()
    };
    let color = Color::rgb(20, 20, 20);
    let points = brush_preview_path(doc.w as f32, doc.h as f32);
    let progress = preview_path_progress(&points);
    let total = progress.last().copied().unwrap_or(0.0);
    doc.checkpoint();
    let mut record = RecordedStroke::default();
    let mut last = None;
    let mut leftover = 0.0;
    for (idx, &to) in points.iter().enumerate() {
        let angle = if idx > 0 {
            let from = points[idx - 1];
            (to.1 - from.1).atan2(to.0 - from.0)
        } else {
            let next = points[1];
            (next.1 - to.1).atan2(next.0 - to.0)
        };
        let from_dist = progress[idx.saturating_sub(1)];
        let from_dyn = DabDynamics {
            angle,
            taper: brush
                .taper
                .distance_factor(from_dist, total - from_dist, brush.size),
            ..Default::default()
        };
        let to_dist = progress[idx];
        let to_dyn = DabDynamics {
            angle,
            taper: brush
                .taper
                .distance_factor(to_dist, total - to_dist, brush.size),
            ..Default::default()
        };
        let (l, lo) = doc.stroke_to_dyn(&brush, color, last, from_dyn, to, to_dyn, leftover);
        record.push(last, from_dyn, to, to_dyn);
        last = l;
        leftover = lo;
    }
    let rect = doc
        .stroke_rect()
        .map(|(x0, y0, x1, y1)| (x0.saturating_sub(2), y0.saturating_sub(2), x1 + 2, y1 + 2));
    assert!(taper_replay_all(&mut doc, &brush, color, &record, rect));
    let alpha = |x: u32, y: u32| doc.layers[0].pixels.get(((y * doc.w + x) * 4 + 3) as usize);
    assert!(alpha(8, 18) > 0, "start tip alpha {}", alpha(8, 18));
    assert!(alpha(136, 18) > 0, "end tip alpha {}", alpha(136, 18));
}

#[test]
fn preview_stroke_renders_taper() {
    let mut doc = Document::new(144, 36);
    let brush = Brush {
        size: 6.0,
        opacity: 1.0,
        softness: 0.0,
        spacing: 0.25,
        taper: Taper {
            start: 0.2,
            end: 0.2,
            size: 1.0,
            ..Taper::NONE
        },
        ..Default::default()
    };
    paint_preview_stroke(&mut doc, &brush, Color::rgb(255, 0, 0));
    let alpha = |x: u32| {
        doc.layers[0]
            .pixels
            .get(((18 * doc.w + x) * 4 + 3) as usize)
    };
    assert!(alpha(72) > 100, "mid alpha {}", alpha(72));
    assert!(alpha(8) < alpha(72), "head alpha {}", alpha(8));
    assert!(alpha(136) < alpha(72), "tail alpha {}", alpha(136));
}

#[test]
fn preset_previews_fit_the_strip_at_display_scale() {
    // A preset paints at its own size (Airbrush is 64 px); on the strip
    // that cropped into a band that showed no edge at all. Every preview
    // must fit, and the texture is built at display scale so the panel
    // copies it 1:1.
    assert_eq!(preview_size(1.0), (96, 24));
    assert_eq!(preview_size(1.5), (144, 36));
    assert_eq!(preview_size(0.0), (96, 24));
    for scale in [1.0f32, 1.5, 2.0] {
        let (w, h) = preview_size(scale);
        for (name, preset) in procraft_engine::presets() {
            let brush = preview_brush(&preset, h as f32);
            assert!(
                brush.size <= h as f32 * PREVIEW_SIZE_FRAC + 0.01,
                "{name} preview size {} overflows the {h} px strip",
                brush.size
            );
            let mut doc = Document::new(w, h);
            paint_preview_stroke(&mut doc, &brush, Color::rgb(30, 30, 30));
            let alpha = |x: u32, y: u32| doc.layers[0].pixels.get(((y * w + x) * 4 + 3) as usize);
            assert!(
                (0..h).any(|y| (0..w).any(|x| alpha(x, y) > 0)),
                "{name} preview is blank"
            );
            for x in 0..w {
                assert_eq!(alpha(x, 0), 0, "{name} preview clipped at the top");
                assert_eq!(alpha(x, h - 1), 0, "{name} preview clipped at the bottom");
            }
        }
    }
}

#[test]
fn stroke_steps_densify_long_pieces() {
    assert_eq!(stroke_steps(0.0), 1);
    assert_eq!(stroke_steps(3.9), 1);
    assert_eq!(stroke_steps(4.1), 2);
    assert_eq!(stroke_steps(10.0), 3);
    assert_eq!(stroke_steps(1.0e6), 64);
}

// --- Transform-layer button session (Scale/rotate/flip) ---

/// 8x8 layer with an off-center 3x3 red square plus a lone marker
/// pixel at (6,6): every flip/rotate/scale visibly moves something.
fn tform_doc() -> Document {
    let mut doc = Document::new(8, 8);
    let mut dense = vec![0u8; 8 * 8 * 4];
    for y in 2..5 {
        for x in 2..5 {
            let i = (y * 8 + x) * 4;
            dense[i] = 255;
            dense[i + 3] = 255;
        }
    }
    let i = (6 * 8 + 6) * 4;
    dense[i] = 255;
    dense[i + 3] = 255;
    doc.layers[0].pixels = Raster::from_dense(8, 8, 4, &dense);
    doc
}

#[test]
fn tform_scale_presses_compose_without_compounding() {
    // Two Scale(+25%) presses must equal one 1.25^2 press: every
    // press re-applies from the base, never from the previous result.
    let mut a = tform_doc();
    let mut b = tform_doc();
    let mut ta = TformPress::new();
    let mut tb = TformPress::new();
    assert_eq!(
        tform_button(&mut a, &mut ta, TformOp::Scale(1.25)),
        XformApply::Applied
    );
    assert_eq!(
        tform_button(&mut a, &mut ta, TformOp::Scale(1.25)),
        XformApply::Applied
    );
    assert_eq!(
        tform_button(&mut b, &mut tb, TformOp::Scale(1.5625)),
        XformApply::Applied
    );
    assert_eq!(a.layers[0].pixels, b.layers[0].pixels);
}

#[test]
fn tform_rotate_pair_restores_pristine() {
    let mut doc = tform_doc();
    let pristine = doc.layers[0].pixels.clone();
    let mut t = TformPress::new();
    assert_eq!(
        tform_button(&mut doc, &mut t, TformOp::Rotate(-15.0)),
        XformApply::Applied
    );
    assert_ne!(doc.layers[0].pixels, pristine);
    assert_eq!(
        tform_button(&mut doc, &mut t, TformOp::Rotate(15.0)),
        XformApply::Restored
    );
    assert_eq!(doc.layers[0].pixels, pristine);
}

#[test]
fn tform_flip_pair_restores_pristine_and_permutes_exactly() {
    let mut doc = tform_doc();
    let pristine = doc.layers[0].pixels.clone();
    let mut t = TformPress::new();
    assert_eq!(
        tform_button(&mut doc, &mut t, TformOp::FlipH),
        XformApply::Applied
    );
    assert_ne!(doc.layers[0].pixels, pristine);
    // Exact permutation about the canvas center: the marker at (6,6)
    // lands on (1,6) with no resampling.
    assert_eq!(xform_alpha(&doc, 1, 6), 255);
    assert_eq!(xform_alpha(&doc, 6, 6), 0);
    assert_eq!(
        tform_button(&mut doc, &mut t, TformOp::FlipH),
        XformApply::Restored
    );
    assert_eq!(doc.layers[0].pixels, pristine);
}

#[test]
fn tform_external_change_recaptures_base() {
    let mut doc = tform_doc();
    let mut t = TformPress::new();
    assert_eq!(
        tform_button(&mut doc, &mut t, TformOp::Scale(1.25)),
        XformApply::Applied
    );
    // Something else edits the canvas behind the session's back.
    let i = ((8 + 1) * 4) as usize; // (1,1)
    doc.layers[0].pixels.set(i + 1, 200);
    doc.layers[0].pixels.set(i + 3, 255);
    doc.mark_full_dirty();
    assert_eq!(
        tform_button(&mut doc, &mut t, TformOp::Scale(1.25)),
        XformApply::Applied
    );
    // The edit was baked into the re-captured base and survives.
    assert!(doc.layers[0].pixels.to_dense().chunks(4).any(|p| p[1] > 0));
}

#[test]
fn tform_absolute_scale_sets_the_session_instead_of_compounding() {
    let mut a = tform_doc();
    let mut b = tform_doc();
    let mut ta = TformPress::new();
    let mut tb = TformPress::new();
    tform_button(&mut a, &mut ta, TformOp::SetScale(150.0));
    tform_button(&mut a, &mut ta, TformOp::SetScale(200.0));
    tform_button(&mut b, &mut tb, TformOp::SetScale(200.0));
    assert_eq!(ta.scale, 2.0);
    assert_eq!(a.layers[0].pixels, b.layers[0].pixels);
}

#[test]
fn tform_absolute_scale_and_angle_clamp_to_the_slider_range() {
    let mut doc = tform_doc();
    let mut t = TformPress::new();
    tform_button(&mut doc, &mut t, TformOp::SetScale(9999.0));
    assert_eq!(t.scale, TFORM_SCALE_MAX);
    tform_button(&mut doc, &mut t, TformOp::SetScale(1.0));
    assert_eq!(t.scale, TFORM_SCALE_MIN);
    assert!((wrap_degrees(370.0) - 10.0).abs() < 1e-4);
    assert!((wrap_degrees(-370.0) + 10.0).abs() < 1e-4);
    assert_eq!(wrap_degrees(180.0), 180.0);
    assert_eq!(wrap_degrees(-180.0), 180.0);
    assert!((wrap_degrees(540.0) - 180.0).abs() < 1e-4);
}

#[test]
fn tform_live_drag_frames_share_one_undo_step() {
    let mut doc = tform_doc();
    let pristine = doc.layers[0].pixels.clone();
    let mut t = TformPress::new();
    for pct in [102.0, 110.0, 125.0, 140.0] {
        assert_eq!(
            tform_apply(&mut doc, &mut t, TformOp::SetScale(pct), true),
            XformApply::Applied
        );
    }
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, pristine);
}

#[test]
fn tform_discrete_press_after_live_drag_opens_a_new_step() {
    let mut doc = tform_doc();
    let mut t = TformPress::new();
    tform_apply(&mut doc, &mut t, TformOp::SetScale(150.0), true);
    tform_apply(&mut doc, &mut t, TformOp::SetScale(160.0), true);
    let mid = doc.layers[0].pixels.clone();
    t.end_gesture();
    tform_button(&mut doc, &mut t, TformOp::SetRotation(20.0));
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, mid);
}

#[test]
fn tform_reset_returns_to_the_session_base() {
    let mut doc = tform_doc();
    let pristine = doc.layers[0].pixels.clone();
    let mut t = TformPress::new();
    tform_button(&mut doc, &mut t, TformOp::FlipH);
    tform_button(&mut doc, &mut t, TformOp::SetRotation(30.0));
    assert_eq!(
        tform_button(&mut doc, &mut t, TformOp::Reset),
        XformApply::Restored
    );
    assert_eq!(doc.layers[0].pixels, pristine);
    assert_eq!(
        (t.scale, t.degrees, t.flip_h, t.flip_v),
        (1.0, 0.0, false, false)
    );
}

#[test]
fn tform_refresh_resets_a_stale_session_for_the_panel_readout() {
    let mut doc = tform_doc();
    let mut t = TformPress::new();
    tform_button(&mut doc, &mut t, TformOp::SetScale(200.0));
    assert_eq!(t.scale, 2.0);
    // Painting behind the session's back invalidates it; the panel
    // must read 100% again, not a transform that no longer applies.
    doc.layers[0].pixels.set(0, 10);
    doc.mark_full_dirty();
    tform_refresh(&doc, &mut t);
    assert_eq!((t.scale, t.degrees), (1.0, 0.0));
    assert!(!t.flip_h && !t.flip_v && !t.checkpointed);
}

#[test]
fn tform_status_names_scale_angle_and_flips() {
    let mut t = TformPress::new();
    assert_eq!(tform_status(&t), "Layer transform: 100%.");
    t.scale = 1.25;
    t.degrees = 15.0;
    t.flip_h = true;
    assert_eq!(tform_status(&t), "Layer transform: 125%, 15 deg, flip H.");
}

#[test]
fn size_drag_travel_reads_the_selected_direction() {
    let a = egui::pos2(100.0, 100.0);
    // Horizontal: right grows, left shrinks (signed dx).
    assert_eq!(
        size_drag_travel(SizeDragMode::Horizontal, a, egui::pos2(220.0, 40.0)),
        120.0
    );
    assert_eq!(
        size_drag_travel(SizeDragMode::Horizontal, a, egui::pos2(0.0, 260.0)),
        -100.0
    );
    // Vertical: up grows, down shrinks (positive up-travel).
    assert_eq!(
        size_drag_travel(SizeDragMode::Vertical, a, egui::pos2(140.0, -20.0)),
        120.0
    );
    assert_eq!(
        size_drag_travel(SizeDragMode::Vertical, a, egui::pos2(100.0, 220.0)),
        -120.0
    );
    // Distance: straight-line travel from the anchor — any direction
    // grows, so the value is never negative.
    assert_eq!(
        size_drag_travel(SizeDragMode::Distance, a, egui::pos2(220.0, 100.0)),
        120.0
    );
    let diag = size_drag_travel(SizeDragMode::Distance, a, egui::pos2(136.0, 148.0));
    assert!((diag - 60.0).abs() < 1e-3, "3-4-5 travel was {diag}");
    assert!(size_drag_travel(SizeDragMode::Distance, a, egui::pos2(0.0, 0.0)) > 0.0);
}

#[test]
fn drag_size_scales_from_base_and_clamps() {
    assert!((drag_size(100.0, 120.0) - 125.0).abs() < 1e-3);
    assert!((drag_size(100.0, -120.0) - 80.0).abs() < 1e-3);
    assert_eq!(drag_size(2.0, -10_000.0), 1.0);
    assert_eq!(drag_size(150.0, 10_000.0), 400.0);
    // Zero travel = the untouched base size.
    assert_eq!(drag_size(50.0, 0.0), 50.0);
}

#[test]
fn space_hand_step_is_a_pure_transition() {
    let b = Tool::Brush;
    // A fresh Space press engages the hand...
    assert_eq!(space_hand_step(b, None, true, true, false, true), Some(b));
    // ...holding keeps it (key repeats carry no fresh press)...
    assert_eq!(
        space_hand_step(Tool::Hand, Some(b), false, true, false, true),
        Some(b)
    );
    // ...and release ends it.
    assert_eq!(
        space_hand_step(Tool::Hand, Some(b), false, false, false, true),
        None
    );
    // A pointer already down blocks engagement (no mid-stroke hijack).
    assert_eq!(space_hand_step(b, None, true, true, true, true), None);
    // Typing or an armed capture blocks it too.
    assert_eq!(space_hand_step(b, None, true, true, false, false), None);
    // Losing the context mid-hold releases the hand.
    assert_eq!(
        space_hand_step(Tool::Hand, Some(b), false, true, false, false),
        None
    );
}

#[test]
fn live_fill_reapplies_from_the_original_boundary() {
    // White field, a 4px light-gray band the fill can grow into, and a
    // black wall behind it. Grow must be measured from the original
    // (inner) boundary: scrub 8 -> 2 -> 4 and every step has to match a
    // direct fill at that Grow, instead of stacking bands outward.
    fn scene() -> Document {
        let brush = Brush::default();
        let mut doc = Document::new(14, 9);
        doc.fill_rect(&brush, Color::rgb(255, 255, 255), 0, 0, 13, 8);
        for x in 8..=11 {
            doc.fill_rect(&brush, Color::rgb(200, 200, 200), x, 0, x, 8);
        }
        doc.fill_rect(&brush, Color::rgb(0, 0, 0), 12, 0, 12, 8);
        doc
    }
    fn direct(grow: u8) -> Document {
        let mut doc = scene();
        doc.flood_fill(&Brush::default(), Color::rgb(0, 0, 255), 3, 4, 0, grow);
        doc
    }
    fn px(doc: &Document, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * doc.w + x) * 4) as usize;
        let r = &doc.layers[0].pixels;
        [r.get(i), r.get(i + 1), r.get(i + 2), r.get(i + 3)]
    }

    let mut live = scene();
    let mut fp = FillPreview {
        layer: 0,
        seed: (3, 4),
        pixels: live.layers[0].pixels.clone(),
        color: Color::rgb(0, 0, 255),
        opacity: 1.0,
        start_x: 0.0,
        start_tol: 0,
        applied: 0,
        live_hash: 0,
        selection: None,
        mask_hash: None,
        symmetry: SymmetryMode::Off,
        segments: 0,
        sym_center: None,
    };
    fill_from_baseline(&mut live, &mut fp, 0, 8);
    assert_eq!(px(&live, 11, 4), [0, 0, 255, 255]);
    fill_from_baseline(&mut live, &mut fp, 0, 2);
    assert_eq!(live.layers[0].pixels, direct(2).layers[0].pixels);
    assert_eq!(px(&live, 11, 4), [200, 200, 200, 255]);
    fill_from_baseline(&mut live, &mut fp, 0, 4);
    assert_eq!(live.layers[0].pixels, direct(4).layers[0].pixels);
}

#[test]
fn engine_cap_matches_the_8k_preset() {
    assert_eq!(MAX_CANVAS_SIDE, 8192);
    assert_eq!(CANVAS_PRESETS[2], ("8K", MAX_CANVAS_SIDE, MAX_CANVAS_SIDE));
}

#[test]
fn canvas_presets_cover_1080p_4k_and_8k() {
    assert_eq!(
        CANVAS_PRESETS,
        [
            ("1080p", 1920, 1080),
            ("4K", 4096, 4096),
            ("8K", 8192, 8192),
        ]
    );
    for (label, w, h) in CANVAS_PRESETS {
        assert!(
            w <= MAX_CANVAS_SIDE && h <= MAX_CANVAS_SIDE,
            "{label} ({w}x{h}) exceeds the engine cap"
        );
    }
    assert_eq!((8192u32.div_ceil(procraft_engine::TILE)).pow(2), 1024);
}

#[test]
fn layer_panel_rows_keep_empty_groups_visible() {
    let mut doc = Document::new(8, 8);
    let g = doc.new_group("Empty".to_string());
    let rows = layer_panel_rows(&doc);
    assert!(rows.contains(&LayerPanelRow::Group {
        id: g,
        depth: 0,
        members: 0
    }));
    // The layer list still follows, so the group row is not a dead end.
    assert_eq!(
        rows.last(),
        Some(&LayerPanelRow::Layer { index: 0, depth: 0 })
    );
}

#[test]
fn layer_panel_rows_indent_members_under_their_group() {
    let mut doc = Document::new(8, 8);
    doc.add_layer("top".to_string());
    let outer = doc.new_group("outer".to_string());
    doc.move_layer_into_group(1, Some(outer));
    let inner = doc.group_layer(1, "inner".to_string()).unwrap();
    assert_eq!(
        layer_panel_rows(&doc),
        vec![
            LayerPanelRow::Group {
                id: outer,
                depth: 0,
                members: 1
            },
            LayerPanelRow::Group {
                id: inner,
                depth: 1,
                members: 1
            },
            LayerPanelRow::Layer { index: 1, depth: 2 },
            LayerPanelRow::Layer { index: 0, depth: 0 },
        ]
    );
    // A collapsed group keeps its own header but hides the subtree.
    doc.set_group_collapsed(outer, true);
    assert_eq!(
        layer_panel_rows(&doc),
        vec![
            LayerPanelRow::Group {
                id: outer,
                depth: 0,
                members: 1
            },
            LayerPanelRow::Layer { index: 0, depth: 0 },
        ]
    );
}

#[test]
fn layer_panel_rows_show_empty_children_under_their_parent() {
    let mut doc = Document::new(8, 8);
    let parent = doc.new_group("parent".to_string());
    doc.move_layer_into_group(0, Some(parent));
    let empty = doc.new_group("empty".to_string());
    doc.groups
        .iter_mut()
        .find(|g| g.id == empty)
        .unwrap()
        .parent = Some(parent);
    let rows = layer_panel_rows(&doc);
    assert_eq!(
        rows,
        vec![
            LayerPanelRow::Group {
                id: parent,
                depth: 0,
                members: 1
            },
            LayerPanelRow::Group {
                id: empty,
                depth: 1,
                members: 0
            },
            LayerPanelRow::Layer { index: 0, depth: 1 },
        ]
    );
}

#[test]
fn layer_selection_click_shift_and_toggle() {
    // Plain click replaces the selection.
    assert_eq!(next_layer_selection(&[1, 2], 2, 5, false, false), vec![5]);
    // Toggle adds and removes, keeping the order stable.
    assert_eq!(
        next_layer_selection(&[1, 2], 2, 5, true, false),
        vec![1, 2, 5]
    );
    assert_eq!(
        next_layer_selection(&[1, 2, 5], 5, 2, true, false),
        vec![1, 5]
    );
    // Shift extends from the active layer in either direction.
    assert_eq!(next_layer_selection(&[5], 1, 3, false, true), vec![1, 2, 3]);
    assert_eq!(next_layer_selection(&[5], 5, 3, false, true), vec![3, 4, 5]);
    // The selection never goes empty: toggling off the last row keeps it.
    assert_eq!(next_layer_selection(&[4], 4, 4, true, false), vec![4]);
}

#[test]
fn group_member_badge_shows_the_count_inline() {
    assert_eq!(group_member_badge(0), None);
    assert_eq!(group_member_badge(1).as_deref(), Some("(1)"));
    assert_eq!(group_member_badge(3).as_deref(), Some("(3)"));
}

#[test]
fn group_created_status_names_the_parent_when_nested() {
    assert_eq!(
        group_created_status("Group 2", None, "Layer 3"),
        "Group 2 created with Layer 3."
    );
    assert_eq!(
        group_created_status("Group 2", Some("Group 1"), "Layer 3"),
        "Group 2 created inside Group 1 with Layer 3."
    );
}

#[test]
fn scroll_request_fires_once_for_its_group() {
    let mut request = Some(7);
    assert!(take_scroll_request(&mut request, 7));
    assert_eq!(request, None);
    // The header redraws every frame; repeating the request would fight
    // the user's own scrolling.
    assert!(!take_scroll_request(&mut request, 7));
}

#[test]
fn scroll_request_waits_for_its_group_row() {
    let mut request = Some(7);
    assert!(!take_scroll_request(&mut request, 3));
    assert_eq!(request, Some(7));
    assert!(!take_scroll_request(&mut None, 7));
}

#[test]
fn group_actions_apply_each_header_press() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    let g = doc.group_layer(0, "g".to_string()).expect("layer 0 groups");
    // Rename only changes the name.
    let report = apply_group_actions(
        &mut doc,
        &GroupRowActions {
            rename: Some((g, "Renamed".to_string())),
            ..Default::default()
        },
    );
    assert_eq!(doc.group(g).unwrap().name, "Renamed");
    assert!(!report.dirty);
    assert!(report.status.is_none());
    // The eye toggles visibility and dirties the view.
    let report = apply_group_actions(
        &mut doc,
        &GroupRowActions {
            eye: Some(g),
            ..Default::default()
        },
    );
    assert!(!doc.group(g).unwrap().visible);
    assert!(report.dirty);
    // Collapse toggles without dirtying the view.
    let report = apply_group_actions(
        &mut doc,
        &GroupRowActions {
            collapse: Some(g),
            ..Default::default()
        },
    );
    assert!(doc.group(g).unwrap().collapsed);
    assert!(!report.dirty);
    // Add pulls the active layer in and names it in the status.
    doc.active = 1;
    let report = apply_group_actions(
        &mut doc,
        &GroupRowActions {
            add: Some(g),
            ..Default::default()
        },
    );
    assert_eq!(doc.layers.iter().filter(|l| l.group == Some(g)).count(), 2);
    assert_eq!(report.status.as_deref(), Some("b moved into the group."));
    // Dispose of one member so the group still has a block to move.
    assert!(doc.move_layer_out_of_group(1));
    // Ungroup disbands and reports it.
    let report = apply_group_actions(
        &mut doc,
        &GroupRowActions {
            ungroup: Some(g),
            ..Default::default()
        },
    );
    assert!(doc.group(g).is_none());
    assert_eq!(
        report.status.as_deref(),
        Some("Group disbanded (layers kept).")
    );
    assert!(report.dirty);
}

#[test]
fn group_actions_move_the_group_and_report_it() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    let g = doc.group_layer(0, "g".to_string()).expect("layer 0 groups");
    let report = apply_group_actions(
        &mut doc,
        &GroupRowActions {
            reorder: Some((g, true)),
            ..Default::default()
        },
    );
    assert_eq!(doc.layers[1].group, Some(g));
    assert!(report.dirty);
    assert_eq!(report.status.as_deref(), Some("g moved up."));
    // Already at the top: the blocked move reports nothing.
    let report = apply_group_actions(
        &mut doc,
        &GroupRowActions {
            reorder: Some((g, true)),
            ..Default::default()
        },
    );
    assert!(!report.dirty);
    assert!(report.status.is_none());
}

#[test]
fn layer_readout_formats_the_document_allocation() {
    let mut doc = Document::new(64, 64);
    assert_eq!(layer_readout(&doc), "1 layer(s), active: Layer 1 · 0 B");
    // One painted cell materialises a whole 256² RGBA tile.
    doc.layers[0].pixels.set4(0, [1, 2, 3, 4]);
    assert_eq!(
        layer_readout(&doc),
        "1 layer(s), active: Layer 1 · 256.0 KB"
    );
}

#[test]
fn sampled_thumb_downscales_without_flattening() {
    // 600x300: a 2:1 canvas downscaled into a 96 px box is 96x48, and
    // sampling never allocates the full 600x300 composite.
    let (w, h) = (600u32, 300u32);
    let mut dense = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let d = ((y * w + x) * 4) as usize;
            let rgb = if x < w / 2 { [255, 0, 0] } else { [0, 0, 255] };
            dense[d..d + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    let mut doc = Document::new(w, h);
    doc.layers[0].pixels = Raster::from_dense(w, h, 4, &dense);
    let (bytes, tw, th) = sampled_thumb(&doc, GALLERY_THUMB_SIDE);
    assert_eq!((tw, th), (96, 48));
    assert_eq!(bytes.len(), tw * th * 4);
    assert_eq!(&bytes[..4], &[255, 0, 0, 255]);
    let last = (tw - 1) * 4;
    assert_eq!(&bytes[last..last + 4], &[0, 0, 255, 255]);
    assert!(bytes.as_chunks::<4>().0.iter().all(|px| px[3] == 255));
}

#[test]
fn mask_action_shows_only_for_a_masked_layer_in_view() {
    assert_eq!(mask_action(true, true, false), MaskAction::Show);
    // Already showing a mask: the Show arm stays reachable, so a dab
    // patches instead of rebuilding.
    assert_eq!(mask_action(true, true, true), MaskAction::Show);
}

#[test]
fn mask_action_leaves_the_mask_texture_when_only_the_view_turns_off() {
    // The checkbox marks the whole canvas dirty, so the composite path
    // repaints every tile over the mask pixels: no drop is needed, and
    // the mask texture is still valid if the view comes back.
    assert_eq!(mask_action(false, true, true), MaskAction::Idle);
}

#[test]
fn mask_action_drops_a_shown_mask_when_the_layer_loses_it() {
    // "View" on, switched to a maskless layer.
    assert_eq!(mask_action(true, false, true), MaskAction::Drop);
    // "Clear mask" also turns the view off; the texture still holds
    // mask pixels, so it must be dropped rather than patched over.
    assert_eq!(mask_action(false, false, true), MaskAction::Drop);
}

#[test]
fn mask_action_is_idle_when_there_is_no_mask_to_show() {
    // The crux: the dirty rect must be left for the composite path.
    assert_eq!(mask_action(true, false, false), MaskAction::Idle);
    assert_eq!(mask_action(false, false, false), MaskAction::Idle);
    // View off with a mask: the mask texture was already dropped (or
    // never built), so nothing to do.
    assert_eq!(mask_action(false, true, false), MaskAction::Idle);
}
