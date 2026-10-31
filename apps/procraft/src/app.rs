use crate::*;

/// M35 Selection upgrades: saved snapshot (canvas dims, rect, mask).
pub(crate) type SavedSelection = ((u32, u32), Option<(u32, u32, u32, u32)>, Option<Vec<u8>>);

pub(crate) struct ProCraftApp {
    pub(crate) doc: Document,
    pub(crate) brush: Brush,
    /// Per-tool copies of size/opacity/softness: switching tools saves the
    /// live brush and restores the entering tool's own.
    pub(crate) tool_brushes: ToolBrushes,
    pub(crate) color: [u8; 3],
    pub(crate) tool: Tool,
    pub(crate) theme: Theme,
    pub(crate) theme_applied: Option<(Theme, BarLayout)>,
    /// True when the system symbol font loaded (desktop Windows): UI
    /// strings may use icon glyphs. iPad has no such font, so the same
    /// strings fall back to plain words (see `tool_label`).
    pub(crate) has_symbols: bool,
    pub(crate) page_custom: [u8; 3],
    pub(crate) picker_size: f32,
    pub(crate) texture: Option<TextureHandle>,
    /// Composite tiles behind the canvas texture: a dab recomposites and
    /// uploads only the tiles under its dirty rect (see `refresh_texture`).
    pub(crate) composite: canvas::CompositeCache,
    /// Viewport-resolution canvas (spec §4): the visible rect composited
    /// into a screen-sized texture instead of one canvas-sized one. Unused
    /// unless `canvas::viewport_active` says so.
    pub(crate) view_tex: canvas::ViewTex,
    pub(crate) painting: bool,
    pub(crate) last: Option<(f32, f32)>,
    pub(crate) last_target: Option<(f32, f32)>,
    pub(crate) last_dyn: DabDynamics,
    pub(crate) last_t: f64,
    pub(crate) leftover: f32,
    // Stabilizer state: exponentially smoothed pointer + pressure. Reset
    // on every stroke start so strokes never inherit the last line's tail.
    pub(crate) smooth_pt: Option<(f32, f32)>,
    pub(crate) smooth_pressure: f32,
    // Midpoint-smoothed stroke path: rounds the corners between sparse
    // input samples so fast strokes ink as curves, not chords.
    pub(crate) stroke_curve: StrokeCurve,
    // Segments of the live brush stroke, replayed on release when the
    // brush has a start/end taper.
    pub(crate) stroke_record: RecordedStroke,
    // Input samples that did not fit this frame's stamping budget. A flick
    // delivers a canvas-length of travel in one frame; stamping it whole
    // stalls the frame, so the tail carries over (see `STAMP_BUDGET`).
    pub(crate) pending_pts: Vec<egui::Pos2>,
    // Taper replay running across frames after a lift (see `TaperReplay`),
    // plus whether the lift still owes its timelapse frame.
    pub(crate) taper_job: Option<TaperReplay>,
    pub(crate) taper_capture: bool,
    pub(crate) sel_anchor: Option<(f32, f32)>,
    pub(crate) sel_current: Option<(f32, f32)>,
    /// Drag-crop state: the live drag plus the committed rect (inclusive
    /// canvas px) waiting for Apply. Re-drag replaces, Esc cancels.
    pub(crate) crop_anchor: Option<(f32, f32)>,
    pub(crate) crop_current: Option<(f32, f32)>,
    pub(crate) crop_rect: Option<(u32, u32, u32, u32)>,
    /// Canvas-size panel: wanted dimensions plus where the old canvas sits
    /// (-1 left/top, 0 center, 1 right/bottom per axis). `canvas_sync`
    /// tracks the doc dims so the fields re-adopt the real size after an
    /// open/crop/apply without clobbering typing mid-edit.
    pub(crate) canvas_w: u32,
    pub(crate) canvas_h: u32,
    pub(crate) canvas_sync: (u32, u32),
    pub(crate) canvas_anchor: (i32, i32),
    pub(crate) lasso_points: Vec<(f32, f32)>,
    pub(crate) lasso_poly: Vec<(f32, f32)>,
    /// Cached mask-edge points for marching ants, rebuilt when
    /// `sel_edges_dirty` (set at every selection-mutating site).
    pub(crate) sel_edges: Vec<(u32, u32)>,
    pub(crate) sel_edges_dirty: bool,
    /// Canvas pan offset in screen px (two-finger drag on touch; the
    /// desktop scrolls the view with the wheel instead).
    pub(crate) view_offset: egui::Vec2,
    /// Canvas rotation in radians (two-finger twist on touch, slider on
    /// desktop). View-only like zoom/pan: paint math stays axis-aligned.
    pub(crate) view_angle: f32,
    /// A stroke applied at least one dab (guards pinch-cancel revert).
    pub(crate) stroke_painted: bool,
    /// Multi-finger tap in progress (two = undo, three = redo on release).
    pub(crate) tap_track: Option<TapTrack>,
    /// Autosave recovery file (v8 project snapshot every 120 s) plus the
    /// last write time and whether to offer it at startup.
    pub(crate) recovery_path: std::path::PathBuf,
    pub(crate) last_autosave: f64,
    /// Autosave encode in flight: a worker deflates a copy-on-write
    /// document snapshot (tiles are shared `Arc`s, so the clone is cheap)
    /// and sends the bytes back for the UI thread to write.
    pub(crate) autosave_job: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
    pub(crate) recovery_offer: bool,
    pub(crate) fill_tolerance: u8,
    pub(crate) fill_grow: u8,
    pub(crate) fill_preview: Option<FillPreview>,
    pub(crate) smudge_strength: f32,
    pub(crate) filter_preview: Option<FilterPreview>,
    pub(crate) sel_feather: u32,
    pub(crate) sel_grow: i32,
    pub(crate) sel_opaque_t: u8,
    pub(crate) sel_stroke_w: u32,
    pub(crate) filt_brightness: i32,
    pub(crate) filt_contrast: f32,
    pub(crate) filt_blur: i32,
    pub(crate) filt_sharpen: f32,
    /// M36 Adjustments I: Levels black/white points + mid gamma (%).
    pub(crate) filt_levels_shadow: u8,
    pub(crate) filt_levels_gamma: f32,
    pub(crate) filt_levels_high: u8,
    /// M36 Curves (parametric): shadow/mid/highlight lift (+) or crush (-).
    pub(crate) filt_curve_shadow: i32,
    pub(crate) filt_curve_mid: i32,
    pub(crate) filt_curve_high: i32,
    /// M37 Adjustments II: HSL, color balance, recolor strength (the
    /// recolor target is the current paint color).
    pub(crate) filt_hue: f32,
    pub(crate) filt_sat: f32,
    pub(crate) filt_light: i32,
    pub(crate) filt_bal_r: i32,
    pub(crate) filt_bal_g: i32,
    pub(crate) filt_bal_b: i32,
    pub(crate) filt_recolor_strength: f32,
    pub(crate) renaming: Option<usize>,
    pub(crate) renaming_text: String,
    pub(crate) group_renaming: Option<u32>,
    /// Group header the panel should bring into view, set by "New group".
    /// Consumed by the matching row on the next draw.
    pub(crate) group_scroll: Option<u32>,
    /// Multi-selection for the Layers panel, as layer indexes. Always
    /// contains the active layer, and resets when the layer count changes
    /// (add, duplicate, merge, delete, undo) since indexes are not ids.
    pub(crate) layer_selection: Vec<usize>,
    /// `doc.layers.len()` seen on the last panel draw: detects the count
    /// changes that invalidate `layer_selection`.
    pub(crate) layer_count_marker: usize,
    pub(crate) group_name_edit: String,
    pub(crate) preset_textures: Vec<TextureHandle>,
    pub(crate) view_mask: bool,
    /// The mask viewer owns the canvas texture slot (it holds mask pixels,
    /// or was cleared for one), so leaving the view must drop it once
    /// instead of patching composite tiles over it.
    pub(crate) mask_shown: bool,
    pub(crate) show_tools: bool,
    pub(crate) show_layers: bool,
    pub(crate) drawer: MobileDrawer,
    /// Last drawer actually shown: `drawer` drops to `None` the moment it
    /// closes, this keeps the panel around for its slide-out.
    pub(crate) drawer_shown: MobileDrawer,
    /// Phone strip: the pocket pill instead of the full bar, which third of
    /// the bottom edge it docks to, and the live drag offset while it is
    /// being moved (screen px from the docked position).
    pub(crate) strip_collapsed: bool,
    pub(crate) strip_dock: StripDock,
    pub(crate) strip_offset: f32,
    /// One-shot request from the mobile Color dot: after the Tools drawer
    /// opens, scroll its body to the Color card.
    pub(crate) mobile_scroll_color: bool,
    /// Status text the toast has already announced, and when it changed.
    pub(crate) status_seen: String,
    pub(crate) status_at: f64,
    pub(crate) narrow_setup_done: bool,
    pub(crate) show_about: bool,
    #[cfg(target_os = "ios")]
    pub(crate) pending_open: Option<PendingOpen>,
    /// Android in-app browser request (external files; no system picker).
    #[cfg(target_os = "android")]
    pub(crate) android_browse: Option<PendingOpen>,
    /// Cached status-bar height in px (-1 = unread; JNI runs once).
    #[cfg(target_os = "android")]
    pub(crate) android_inset_top: f32,
    pub(crate) preview_theme: Option<Theme>,
    /// Display scale the preview textures were built at: a window dragged to
    /// another monitor needs them re-rendered or every thumbnail resamples.
    pub(crate) preview_scale: f32,
    pub(crate) custom_brushes: Vec<BrushPreset>,
    pub(crate) brush_slot: Option<BrushSlot>,
    pub(crate) new_brush_name: String,
    pub(crate) palettes: PaletteSet,
    pub(crate) pal_renaming: bool,
    pub(crate) pal_name_edit: String,
    pub(crate) guides: Guides,
    pub(crate) vp_move: bool,
    pub(crate) vp_grab: Option<usize>,
    pub(crate) sym_move: bool,
    pub(crate) sym_grab: bool,
    /// M48 transform handles: on-canvas corner pins / warp lattice.
    pub(crate) xform: XformState,
    /// Transform-layer button session (Scale/rotate/flip without
    /// compounding resampling).
    pub(crate) tform_press: TformPress,
    pub(crate) assist: bool,
    pub(crate) assist_start: Option<(f32, f32)>,
    pub(crate) assist_lock: Option<AssistLine>,
    // Animation Assist: layers are frames (bottom-to-top), the active
    // layer is the current frame. Ghost handles are half-res overlays,
    // rebuilt when the neighbor pair changes or `ghost_dirty` is set by
    // undo/redo and layer edits (painting only touches the current frame,
    // so strokes never invalidate ghosts).
    pub(crate) anim_assist: bool,
    pub(crate) anim_playing: bool,
    pub(crate) anim_fps: u32,
    pub(crate) anim_loop: LoopMode,
    /// JPEG export quality 1-100 (M28 slider; TIFF is lossless, no dial).
    pub(crate) jpeg_quality: u8,
    pub(crate) anim_fwd: bool,
    pub(crate) anim_last_t: f64,
    pub(crate) anim_shown: Option<usize>,
    pub(crate) onion: bool,
    /// Onion ghost strengths per side (red = previous, blue = next).
    pub(crate) onion_prev: f32,
    pub(crate) onion_next: f32,
    /// M41 Time-lapse: record flag, staged snapshots (bytes, w, h),
    /// and the last stage time (min-interval gate).
    pub(crate) lapse_on: bool,
    pub(crate) lapse_frames: Vec<(Vec<u8>, u32, u32)>,
    pub(crate) lapse_last: std::time::Instant,
    pub(crate) ghost_prev: Option<TextureHandle>,
    pub(crate) ghost_next: Option<TextureHandle>,
    pub(crate) ghost_key: (Option<usize>, Option<usize>, u32, u32),
    pub(crate) ghost_dirty: bool,
    pub(crate) move_from: Option<(f32, f32)>,
    pub(crate) move_acc: (f32, f32),
    /// Ctrl+middle-drag zoom anchor (screen pos of the last tick).
    pub(crate) mmb_last: Option<egui::Pos2>,
    /// M40 Liquify: drag anchor plus brush size/strength.
    pub(crate) liquify_from: Option<(f32, f32)>,
    pub(crate) liquify_size: f32,
    pub(crate) liquify_strength: f32,
    pub(crate) shape_anchor: Option<(f32, f32)>,
    pub(crate) shape_current: Option<(f32, f32)>,
    pub(crate) shape_kind: ShapeKind,
    pub(crate) shape_fill: bool,
    pub(crate) shape_sides: u8,
    pub(crate) text_anchor: Option<(f32, f32)>,
    pub(crate) text_open: bool,
    pub(crate) text_body: String,
    pub(crate) text_size: f32,
    /// M38 Typography: imported face (session-only bytes + name),
    /// spacing, and outline. Render bakes through `TextStyle`.
    pub(crate) text_font_name: Option<String>,
    pub(crate) text_font_bytes: Option<Vec<u8>>,
    pub(crate) text_leading: f32,
    pub(crate) text_tracking: f32,
    pub(crate) text_outline: bool,
    pub(crate) text_outline_w: u32,
    pub(crate) text_outline_rgb: [u8; 3],
    /// Text-layer edit session: the layer index plus the block identity
    /// (text + anchor) captured when the dialog opened. Done compares
    /// against these so an edit landing on a layer that changed
    /// underneath (deleted, reordered, undone) cancels instead of
    /// corrupting; `None` means the dialog creates a new layer.
    pub(crate) editing_text: Option<usize>,
    pub(crate) editing_loaded: Option<(String, f32, f32)>,
    pub(crate) zoom: f32,
    pub(crate) status: String,
    /// M33 Gallery: recent project paths (newest first, max 10),
    /// the browser window flag, cached per-path thumbnails, the key
    /// they were built from, and the New-canvas size fields.
    pub(crate) recent: Vec<String>,
    pub(crate) gallery_open: bool,
    pub(crate) gallery_thumbs: Vec<Option<TextureHandle>>,
    pub(crate) gallery_keys: Vec<String>,
    pub(crate) gallery_new_w: u32,
    pub(crate) gallery_new_h: u32,
    /// M34 Reference companion: pinned image alongside the canvas
    /// (session-only, not saved in the project). Texture rebuilds lazily.
    pub(crate) ref_open: bool,
    pub(crate) ref_image: Option<(Vec<u8>, u32, u32)>,
    pub(crate) ref_tex: Option<TextureHandle>,
    pub(crate) ref_scale: f32,
    /// M35 Selection upgrades: saved snapshot ((w,h), rect, mask) plus
    /// the nudge steppers. Session-only; Load refuses size mismatches.
    pub(crate) saved_sel: Option<SavedSelection>,
    pub(crate) sel_nudge_x: i32,
    pub(crate) sel_nudge_y: i32,
    /// M49 settings: resolved settings dir (None = session-only), the
    /// last-saved snapshot (change detection), the shortcuts table, and
    /// the zoom write coalescer (dirty flag + last-change time).
    pub(crate) settings_dir: Option<std::path::PathBuf>,
    pub(crate) settings_snapshot: Settings,
    pub(crate) shortcuts: Shortcuts,
    pub(crate) zoom_dirty: bool,
    pub(crate) zoom_changed_at: f64,
    /// M49 UI: shortcuts window flag, live chord-capture target, a
    /// load/corrupt note for the settings window, and the Shift+drag
    /// brush-size gesture in flight.
    pub(crate) show_shortcuts: bool,
    pub(crate) capture: Option<ActionId>,
    pub(crate) settings_note: Option<String>,
    pub(crate) size_drag: Option<SizeDrag>,
    /// Space held = transient Hand: the tool to restore on release.
    pub(crate) space_hand: Option<Tool>,
    /// M49: a failed-save note (shown with the settings note) and the
    /// time of the last failed write attempt (retry cooldown).
    pub(crate) save_note: Option<String>,
    pub(crate) save_failed_at: Option<f64>,
    /// M49: set when this run seeded settings.json; the marker is written
    /// on the first frame (eframe's CreationContext storage is read-only).
    pub(crate) seed_marker_pending: bool,
}

impl ProCraftApp {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Extra glyph coverage (tool icons) from a system symbol font; the
        // app works fine without it (UI strings check `has_symbols`).
        let mut fonts = egui::FontDefinitions::default();
        let has_symbols = std::fs::read("C:\\Windows\\Fonts\\seguisym.ttf")
            .map(|bytes| {
                fonts.font_data.insert(
                    "segui-symbol".to_owned(),
                    std::sync::Arc::new(egui::FontData::from_owned(bytes)),
                );
                if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                    list.push("segui-symbol".to_owned());
                }
            })
            .is_ok();
        cc.egui_ctx.set_fonts(fonts);
        // Autosave recovery lives next to the UI state (created eagerly
        // so a crash in the first two minutes still offers recovery only
        // if a snapshot actually landed).
        let mut dir = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        dir.push("procraft");
        let _ = std::fs::create_dir_all(&dir);
        dir.push("recovery.pcraft");
        let recovery_path = dir;
        // M49: settings.json lives in the same app dir (independent
        // lookup; no dir on a platform without APPDATA/HOME means
        // settings stay session-only).
        let settings_dir = crate::settings::settings_dir();
        let mut app = Self {
            doc: Document::new(1280, 800),
            brush: Brush::default(),
            tool_brushes: ToolBrushes::new(&Brush::default()),
            color: [30, 144, 255],
            tool: Tool::Brush,
            theme: Theme::Dark,
            theme_applied: None,
            has_symbols,
            page_custom: [255, 255, 255],
            picker_size: 165.0,
            texture: None,
            composite: canvas::CompositeCache::new(
                if cfg!(any(target_os = "android", target_os = "ios")) {
                    canvas::CACHE_MOBILE
                } else {
                    canvas::CACHE_DESKTOP
                },
            ),
            view_tex: canvas::ViewTex::new(),
            painting: false,
            last: None,
            last_target: None,
            last_dyn: DabDynamics::default(),
            last_t: 0.0,
            leftover: 0.0,
            smooth_pt: None,
            smooth_pressure: 1.0,
            stroke_curve: StrokeCurve::new(),
            stroke_record: RecordedStroke::default(),
            pending_pts: Vec::new(),
            taper_job: None,
            taper_capture: false,
            sel_anchor: None,
            sel_current: None,
            crop_anchor: None,
            crop_current: None,
            crop_rect: None,
            canvas_w: 1280,
            canvas_h: 800,
            canvas_sync: (1280, 800),
            canvas_anchor: (0, 0),
            lasso_points: Vec::new(),
            lasso_poly: Vec::new(),
            sel_edges: Vec::new(),
            sel_edges_dirty: true,
            view_offset: egui::Vec2::ZERO,
            view_angle: 0.0,
            stroke_painted: false,
            tap_track: None,
            recovery_path: recovery_path.clone(),
            last_autosave: 0.0,
            autosave_job: None,
            recovery_offer: std::path::Path::new(&recovery_path).exists(),
            fill_tolerance: 32,
            fill_grow: 2,
            fill_preview: None,
            smudge_strength: 0.5,
            filter_preview: None,
            sel_feather: 4,
            sel_grow: 4,
            sel_opaque_t: 8,
            sel_stroke_w: 2,
            filt_brightness: 0,
            filt_contrast: 100.0,
            filt_blur: 0,
            filt_sharpen: 0.0,
            filt_levels_shadow: 0,
            filt_levels_gamma: 100.0,
            filt_levels_high: 255,
            filt_curve_shadow: 0,
            filt_curve_mid: 0,
            filt_curve_high: 0,
            filt_hue: 0.0,
            filt_sat: 100.0,
            filt_light: 0,
            filt_bal_r: 0,
            filt_bal_g: 0,
            filt_bal_b: 0,
            filt_recolor_strength: 100.0,
            renaming: None,
            renaming_text: String::new(),
            group_renaming: None,
            group_scroll: None,
            layer_selection: Vec::new(),
            layer_count_marker: 0,
            group_name_edit: String::new(),
            preset_textures: Vec::new(),
            view_mask: false,
            mask_shown: false,
            show_tools: true,
            show_layers: true,
            drawer: MobileDrawer::None,
            drawer_shown: MobileDrawer::None,
            strip_collapsed: false,
            strip_dock: StripDock::Center,
            strip_offset: 0.0,
            mobile_scroll_color: false,
            status_seen: String::new(),
            status_at: 0.0,
            narrow_setup_done: false,
            show_about: false,
            #[cfg(target_os = "ios")]
            pending_open: None,
            #[cfg(target_os = "android")]
            android_browse: None,
            #[cfg(target_os = "android")]
            android_inset_top: -1.0,
            preview_theme: None,
            preview_scale: 1.0,
            custom_brushes: Vec::new(),
            brush_slot: None,
            new_brush_name: String::new(),
            palettes: PaletteSet::new(),
            pal_renaming: false,
            pal_name_edit: String::new(),
            guides: Guides::default(),
            vp_move: false,
            vp_grab: None,
            sym_move: false,
            sym_grab: false,
            xform: XformState::off(),
            tform_press: TformPress::new(),
            assist: false,
            assist_start: None,
            assist_lock: None,
            anim_assist: false,
            anim_playing: false,
            anim_fps: 12,
            anim_loop: LoopMode::Forward,
            jpeg_quality: 90,
            anim_fwd: true,
            anim_last_t: 0.0,
            anim_shown: None,
            onion: true,
            onion_prev: 0.3,
            onion_next: 0.3,
            lapse_on: true,
            lapse_frames: Vec::new(),
            lapse_last: std::time::Instant::now(),
            ghost_prev: None,
            ghost_next: None,
            ghost_key: (None, None, 0, 0),
            ghost_dirty: true,
            move_from: None,
            move_acc: (0.0, 0.0),
            mmb_last: None,
            liquify_from: None,
            liquify_size: 128.0,
            liquify_strength: 0.5,
            shape_anchor: None,
            shape_current: None,
            shape_kind: ShapeKind::Rect,
            shape_fill: false,
            shape_sides: 6,
            text_anchor: None,
            text_open: false,
            text_body: String::new(),
            text_size: 48.0,
            text_font_name: None,
            text_font_bytes: None,
            text_leading: 0.0,
            text_tracking: 0.0,
            text_outline: false,
            text_outline_w: 3,
            text_outline_rgb: [0, 0, 0],
            editing_text: None,
            editing_loaded: None,
            zoom: 1.0,
            status:
                "Drag on the canvas to paint. Ctrl+Z undoes, Ctrl+Y redoes, Esc clears selection."
                    .to_string(),
            recent: Vec::new(),
            gallery_open: false,
            gallery_thumbs: Vec::new(),
            gallery_keys: Vec::new(),
            gallery_new_w: 1280,
            gallery_new_h: 800,
            ref_open: false,
            ref_image: None,
            ref_tex: None,
            ref_scale: 1.0,
            saved_sel: None,
            sel_nudge_x: 0,
            sel_nudge_y: 0,
            settings_dir,
            settings_snapshot: Settings::default(),
            shortcuts: Shortcuts::default(),
            zoom_dirty: false,
            zoom_changed_at: 0.0,
            show_shortcuts: false,
            capture: None,
            settings_note: None,
            size_drag: None,
            space_hand: None,
            save_note: None,
            save_failed_at: None,
            seed_marker_pending: false,
        };
        // M49: settings.json is the source of truth. The legacy eframe
        // keys (theme/panels/zoom/recent) are read exactly once — the
        // first run after M49 — to seed the file; the marker then makes
        // a later missing file mean "fresh defaults", never a legacy
        // resurrection. Platforms with no settings dir stay session-only;
        // platforms without eframe storage (Android/iOS) skip only the
        // legacy migration — the file itself still loads and saves.
        if let Some(dir) = app.settings_dir.clone() {
            let marker_set = cc
                .storage
                .as_ref()
                .is_some_and(|s| s.get_string(crate::settings::MARKER_KEY).is_some());
            let file_exists = crate::settings::settings_path(&dir).is_file();
            match crate::settings::init_action(file_exists, marker_set) {
                crate::settings::Inits::LoadFile => match crate::settings::read(&dir) {
                    Ok(s) => app.apply_settings(&s),
                    Err(err) => {
                        let backup = crate::settings::quarantine(&dir);
                        let note = match backup {
                            Some(p) => format!(
                                "Settings were unreadable ({err}); backup kept as {}.",
                                p.display()
                            ),
                            None => {
                                format!("Settings were unreadable ({err}); defaults used.")
                            }
                        };
                        app.status = note.clone();
                        app.settings_note = Some(note);
                    }
                },
                crate::settings::Inits::SeedLegacy => {
                    // Legacy values live in eframe storage, which mobile
                    // never provides — nothing to migrate there; the
                    // first change still creates the file, and later
                    // launches take the LoadFile branch.
                    if let Some(storage) = cc.storage {
                        if let Some(name) = storage.get_string("theme") {
                            if let Some(theme) = Theme::from_str(&name) {
                                app.theme = theme;
                            }
                        }
                        if let Some(v) = storage.get_string("show_tools") {
                            app.show_tools = v == "1";
                        }
                        if let Some(v) = storage.get_string("show_layers") {
                            app.show_layers = v == "1";
                        }
                        if let Some(v) = storage.get_string("zoom") {
                            if let Ok(z) = v.parse::<f32>() {
                                if z.is_finite() && z > 0.0 {
                                    app.zoom = z;
                                }
                            }
                        }
                        // Legacy recents: newline-joined paths.
                        if let Some(v) = storage.get_string("recent") {
                            app.recent = v
                                .lines()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .take(MAX_RECENT)
                                .map(str::to_string)
                                .collect();
                        }
                        // Seed the file from the migrated values; only a
                        // landed write earns the marker (a failed write
                        // retries the seed next launch). eframe 0.36 hands
                        // `new` a read-only storage, so the marker itself
                        // is written on the first frame via `storage_mut`.
                        let snap = app.settings_snapshot_from_fields();
                        if crate::settings::save_now(&dir, &snap).is_ok() {
                            app.seed_marker_pending = true;
                        }
                    }
                }
                crate::settings::Inits::UseDefaults => {}
            }
        }
        // The snapshot always mirrors what the app actually holds, so
        // the first frame's sync sees no spurious changes.
        app.settings_snapshot = app.settings_snapshot_from_fields();
        app
    }
}
