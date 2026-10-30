//! ProCraft by surfrpt1 (https://github.com/surfrpt1).
//! Original clean-room code, MIT licensed.
//!
//! ProCraft milestone 9: brush library (custom brushes + .pcbrush import/export),
//! plus all earlier features. Original code.

use eframe::egui::{self, Color32, ColorImage, Sense, TextureHandle, TextureOptions, Vec2};
// iOS has no rfd backend: same call shape, every dialog declined.
#[cfg(target_os = "android")]
use procraft_engine::ext_matches;
use procraft_engine::{
    analogous, check_font, clamp_frame, classify_touch_tap, complementary, curves_lut,
    export_gif_ext, export_timelapse, export_tip_png, file_size_label, gallery_thumb,
    import_brush_file_report, import_gif_document, import_gpl_file, levels_lut, next_frame,
    onion_ghost, open_brush_library, open_palette, open_psd, palette_from_rgba, pan_offset,
    prev_frame, rasterize_frame, rasterize_frame_region, save_brush_library, save_palette,
    save_psd, shift_wheel_remap, size_drag_factor, starter_palette, step_playback_visible, triadic,
    zoom_drag_factor, ActionId, AssistLine, BlendMode, Brush, BrushPreset, ChordKey, Color,
    DabDynamics, Document, DragActionId, GuideKind, Guides, KeyChord, LoopMode, ModSet,
    PaintTarget, Palette, PaletteSet, Raster, Settings, ShapePoly, Shortcuts, SizeDragMode,
    SymmetryMode, TapAction, TextBlock, TextStyle, ToolId, LAPSE_SIDE, MAX_ANIM_FPS, MAX_ANIM_HOLD,
    MAX_CANVAS_SIDE, MAX_FONT_BYTES, MAX_IMAGE_COLORS, MAX_LAPSE_FRAMES, MAX_LIBRARY_BRUSHES,
    MAX_OUTLINE_PX, MAX_PALETTES, MAX_PALETTE_COLORS, MAX_TEXT_SPACING, MIN_ANIM_FPS,
    MIN_ANIM_HOLD, MIN_TEXT_PX, SETTINGS_VERSION, TILE,
};
#[cfg(any(target_os = "ios", target_os = "android"))]
use rfd_shim as rfd;

mod canvas;
mod design;
mod settings;

/// Every extension `import_brush_file` can dispatch.
const BRUSH_IMPORT_EXTS: &[&str] = &[
    "pcbrush", "json", "abr", "brush", "brushset", "kpp", "bundle", "sut", "gbr", "png", "jpg",
    "jpeg", "webp", "bmp", "gif", "tif", "tiff",
];

/// Any brush file that can supply a dual tip.
const TIP_IMPORT_EXTS: &[&str] = BRUSH_IMPORT_EXTS;

/// Brush size range shared by the quick slider, the panel slider and drag
/// gestures. The upper bound matches the engine's validation limit.
const BRUSH_SIZE_MIN: f32 = 1.0;
const BRUSH_SIZE_MAX: f32 = 400.0;

/// Android: accepted extensions per open kind (mirrors the desktop filters).
#[cfg(target_os = "android")]
fn pending_open_exts(kind: PendingOpen) -> &'static [&'static str] {
    match kind {
        PendingOpen::Image | PendingOpen::Reference | PendingOpen::ImagePalette => {
            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"]
        }
        PendingOpen::Project => &["procraft"],
        PendingOpen::Psd => &["psd"],
        PendingOpen::Font => &["ttf", "otf"],
        PendingOpen::Brushes => BRUSH_IMPORT_EXTS,
        PendingOpen::Palette => &["pcpalette", "json", "gpl"],
        PendingOpen::DualTip => TIP_IMPORT_EXTS,
        PendingOpen::Gif => &["gif"],
    }
}

/// Android: importables for `kind` plus a note when the dir is unreadable.
/// Returns `(files, note)`: `note` is `Some` only when listing failed.
#[cfg(target_os = "android")]
fn android_import_list(kind: PendingOpen) -> (Vec<std::path::PathBuf>, Option<String>) {
    let exts = pending_open_exts(kind);
    let dir = match rfd::android_external_files() {
        Ok(d) => d,
        Err(reason) => return (Vec::new(), Some(format!("Files unavailable: {reason}."))),
    };
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file() && ext_matches(p.extension().and_then(|e| e.to_str()), exts)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    (out, None)
}

/// Latest Apple Pencil / touch pressure (0.0..=1.0) seen in this frame's
/// input events, if the backend forwarded any. egui 0.36 exposes pressure
/// only on `Event::Touch.force` — desktop mice always yield `None`, and
/// tilt is not exposed at all (documented M14 limitation).
fn latest_touch_force(ctx: &egui::Context) -> Option<f32> {
    ctx.input(|i| {
        let mut force = None;
        for e in &i.events {
            if let egui::Event::Touch {
                force: f, phase, ..
            } = e
            {
                if *phase == egui::TouchPhase::End || *phase == egui::TouchPhase::Cancel {
                    continue;
                }
                if let Some(v) = f {
                    force = Some(v.clamp(0.0, 1.0));
                }
            }
        }
        force
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Brush,
    Eraser,
    Smudge,
    Select,
    Lasso,
    Fill,
    Picker,
    Move,
    Hand,
    Liquify,
    Crop,
    Shapes,
    Text,
}

/// Tool radio label: icon glyphs on symbol-font platforms, plain words
/// where the glyphs would be tofu (iOS). Only glyphs the bundled fonts
/// cover (`○`) are unconditional; everything else has a word fallback.
fn tool_label(tool: Tool, symbols: bool) -> &'static str {
    match tool {
        Tool::Brush => {
            if symbols {
                "✏ Brush"
            } else {
                "Brush"
            }
        }
        Tool::Eraser => {
            if symbols {
                "⌫ Eraser"
            } else {
                "Eraser"
            }
        }
        Tool::Smudge => {
            if symbols {
                "💧 Smudge"
            } else {
                "Smudge"
            }
        }
        Tool::Select => {
            if symbols {
                "⬚ Select"
            } else {
                "Select"
            }
        }
        Tool::Lasso => {
            if symbols {
                "◌ Lasso"
            } else {
                "Lasso"
            }
        }
        Tool::Fill => {
            if symbols {
                "◧ Fill"
            } else {
                "Fill"
            }
        }
        Tool::Picker => {
            if symbols {
                "◉ Pick"
            } else {
                "Pick"
            }
        }
        Tool::Move => {
            if symbols {
                "↔ Move"
            } else {
                "Move"
            }
        }
        Tool::Hand => {
            if symbols {
                "✋ Hand"
            } else {
                "Hand"
            }
        }
        Tool::Liquify => {
            if symbols {
                "➰ Liquify"
            } else {
                "Liquify"
            }
        }
        Tool::Crop => {
            if symbols {
                "◫ Crop"
            } else {
                "Crop"
            }
        }
        Tool::Shapes => "○ Shapes",
        Tool::Text => "T Text",
    }
}

/// Compact glyph for the phone strip's paint-tool segment. The strip only
/// has room for icons if the drawers are to stay visible.
fn tool_glyph(tool: Tool) -> &'static str {
    match tool {
        Tool::Brush => "✏",
        Tool::Eraser => "⌫",
        Tool::Smudge => "💧",
        _ => "•",
    }
}

/// Phone-strip face for a paint tool: the glyph where the bundled fonts
/// cover it, the plain name where they do not.
fn strip_tool_face(tool: Tool, has_symbols: bool) -> &'static str {
    if has_symbols {
        return tool_glyph(tool);
    }
    match tool {
        Tool::Brush => "Brush",
        Tool::Eraser => "Eraser",
        _ => "Smudge",
    }
}

/// One control in the context-sensitive bar under the top bar. The bar is
/// driven by this list so every tool's quick options live in one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum QuickControl {
    Size,
    Opacity,
    Softness,
    Spacing,
    Stabilizer,
    Strength,
    Tolerance,
    Grow,
    PickerPx,
    Feather,
    SelGrow,
    OpaqueThreshold,
    OutlinePx,
    Invert,
    ClearSelection,
    SmoothSelection,
    ShapeKind,
    ShapeFill,
    ShapeSides,
    CropApply,
    CropCancel,
    TextSize,
    TextLeading,
    TextTracking,
    TextOutline,
    EditText,
}

fn quick_controls(tool: Tool) -> &'static [QuickControl] {
    use QuickControl::*;
    match tool {
        Tool::Brush | Tool::Eraser => &[Size, Opacity, Softness, Spacing, Stabilizer],
        Tool::Smudge => &[Size, Strength],
        Tool::Select | Tool::Lasso => &[
            Feather,
            SelGrow,
            OpaqueThreshold,
            OutlinePx,
            Invert,
            ClearSelection,
            SmoothSelection,
        ],
        Tool::Fill => &[Tolerance, Grow],
        Tool::Picker => &[PickerPx],
        Tool::Liquify => &[Size, Strength],
        Tool::Shapes => &[ShapeKind, ShapeFill, ShapeSides],
        Tool::Crop => &[CropApply, CropCancel],
        Tool::Text => &[TextSize, TextLeading, TextTracking, TextOutline, EditText],
        Tool::Move | Tool::Hand => &[],
    }
}

/// Finger-sized hit height for the quick bar, from the same token table the
/// rest of the chrome uses.
fn quick_hit_height(layout: BarLayout) -> f32 {
    design::tokens(Theme::Dark, layout).control_h
}

/// Phones and tablets get one horizontally scrollable row (vertical space
/// is canvas); desktop wraps inline.
fn quick_scrolls(layout: BarLayout) -> bool {
    layout != BarLayout::Wide
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeKind {
    Line,
    Rect,
    Ellipse,
    Triangle,
    Diamond,
    Star,
    Arrow,
    Polygon,
}

impl ShapeKind {
    const ALL: [ShapeKind; 8] = [
        ShapeKind::Line,
        ShapeKind::Rect,
        ShapeKind::Ellipse,
        ShapeKind::Triangle,
        ShapeKind::Diamond,
        ShapeKind::Star,
        ShapeKind::Arrow,
        ShapeKind::Polygon,
    ];

    fn name(self) -> &'static str {
        match self {
            ShapeKind::Line => "Line",
            ShapeKind::Rect => "Rect",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Triangle => "Triangle",
            ShapeKind::Diamond => "Diamond",
            ShapeKind::Star => "Star",
            ShapeKind::Arrow => "Arrow",
            ShapeKind::Polygon => "Polygon",
        }
    }

    /// Closed shapes honor the Fill toggle (arrow/line are strokes only).
    fn fillable(self) -> bool {
        !matches!(self, ShapeKind::Line | ShapeKind::Arrow)
    }
}

/// Live ColorDrop preview: the pre-fill layer snapshot plus the drag
/// geometry, so each tolerance update restores and re-fills from the same
/// seed (one undo step for the whole gesture). Kept alive after the
/// gesture so the Tolerance/Grow sliders stay live: every update re-fills
/// from this snapshot, which anchors growth to the fill's original
/// (inner) boundary instead of stacking a new band on its grown outer
/// edge.
struct FillPreview {
    layer: usize,
    seed: (u32, u32),
    pixels: Raster,
    color: Color,
    opacity: f32,
    start_x: f32,
    start_tol: u8,
    applied: u8,
    live_hash: u64,
    /// Document context the fill was computed under. A change (selection,
    /// symmetry, active layer) finalizes the preview rather than replaying
    /// the fill into a different context.
    selection: Option<(u32, u32, u32, u32)>,
    mask_hash: Option<u64>,
    symmetry: SymmetryMode,
    segments: u8,
    sym_center: Option<(f32, f32)>,
}

/// Restore the pre-fill snapshot and re-run the fill at `tolerance`/`grow`.
/// Always re-applies from the original boundary, so a slider scrub both
/// expands and shrinks the result (and a re-tap never compounds growth).
fn fill_from_baseline(doc: &mut Document, fp: &mut FillPreview, tolerance: u8, grow: u8) {
    if let Some(layer) = doc.layers.get_mut(fp.layer) {
        layer.pixels = fp.pixels.clone();
    }
    let brush = Brush {
        opacity: fp.opacity,
        ..Default::default()
    };
    doc.flood_fill(&brush, fp.color, fp.seed.0, fp.seed.1, tolerance, grow);
    fp.applied = tolerance;
    if let Some(layer) = doc.layers.get(fp.layer) {
        fp.live_hash = layer.pixels.fingerprint();
    }
}

/// Whether the document still matches the context the fill was made in.
fn fill_context_matches(doc: &Document, fp: &FillPreview) -> bool {
    doc.active == fp.layer
        && doc.selection == fp.selection
        && doc.selection_mask.as_deref().map(pixels_hash) == fp.mask_hash
        && doc.symmetry == fp.symmetry
        && doc.symmetry_segments == fp.segments
        && doc.symmetry_center == fp.sym_center
}

/// Live filter preview baseline: scrubbing a filter slider restores these
/// pixels then re-applies the slider value, so a whole preview gesture
/// costs one undo step instead of compounding per change. Re-applies are
/// throttled (see `PREVIEW_MIN_DT`): skipped ticks flag `stale`, and
/// Apply restamps the latest values so nothing is lost.
struct FilterPreview {
    /// 0 = tone, 1 = blur, 2 = sharpen, 3 = levels, 4 = curves,
    /// 5 = HSL, 6 = balance, 7 = recolor.
    kind: u8,
    layer: usize,
    pixels: Raster,
    last: std::time::Instant,
    stale: bool,
    /// Fingerprint of the layer right after our last apply (or of the
    /// snapshot on a fresh preview). A tick finding anything else means
    /// another edit touched the layer meanwhile — restoring the stale
    /// snapshot would wipe that work, so the preview restarts instead.
    live_hash: u64,
}

/// Cheap non-crypto fingerprint of layer pixels (samples every ~1KB):
/// detects "the layer changed underneath us" for preview guards. A miss
/// only ever restarts a preview, never destroys work.
fn pixels_hash(pixels: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    h = h
        .wrapping_mul(0x100000001b3)
        .wrapping_add(pixels.len() as u64);
    let step = (pixels.len() / 4096).max(1);
    for (i, &b) in pixels.iter().enumerate().step_by(step) {
        h ^= (b as u64).wrapping_add(i as u64);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Minimum time between live preview re-applies while a slider is
/// scrubbed. Full-canvas filters cost 15–60 ms at 1280x800 plus a
/// re-flatten and texture upload per tick; without a floor, every slider
/// tick queues one and the UI visibly hangs during the drag.
const PREVIEW_MIN_DT: std::time::Duration = std::time::Duration::from_millis(50);

/// M41 Time-lapse: minimum time between staged snapshots (with the
/// frame cap, this bounds a session to ~180 small thumbs, not gigabytes).
const LAPSE_MIN_DT: std::time::Duration = std::time::Duration::from_secs(10);

/// M35 Selection upgrades: saved snapshot (canvas dims, rect, mask).
type SavedSelection = ((u32, u32), Option<(u32, u32, u32, u32)>, Option<Vec<u8>>);

/// In-progress multi-finger tap (undo/redo): tracks the widest finger
/// count, start time/center, and whether it moved (a pan/pinch is not
/// a tap). Fires on clean release; see the top of `App::ui`.
struct TapTrack {
    n: usize,
    t0: f64,
    c0: egui::Pos2,
    moved: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Theme {
    Dark,
    Light,
    Paper,
}

/// Mobile open: what to load once the picker resolves (iOS document
/// picker) or the in-app browser picks (Android external files).
#[cfg(any(target_os = "ios", target_os = "android"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingOpen {
    Image,
    Project,
    Psd,
    Brushes,
    Palette,
    ImagePalette,
    Gif,
    Reference,
    Font,
    DualTip,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Paper => "paper",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
            Theme::Paper => "Paper",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            "paper" => Some(Theme::Paper),
            _ => None,
        }
    }
}

/// Append `.ext` when the chosen path has none (rfd doesn't force it).
fn ensure_extension(mut path: std::path::PathBuf, ext: &str) -> std::path::PathBuf {
    if path.extension().is_none() {
        path.set_extension(ext);
    }
    path
}

/// M33 Gallery: how many recent projects to remember (newest first).
const MAX_RECENT: usize = 10;
/// M33 Gallery: thumbnail longest side in px.
const GALLERY_THUMB_SIDE: u32 = 96;
/// Gallery new-canvas presets: (label, width, height). Every side stays
/// within [`MAX_CANVAS_SIDE`]; the 8K entry is the square export target.
const CANVAS_PRESETS: [(&str, u32, u32); 3] = [
    ("1080p", 1920, 1080),
    ("4K", 4096, 4096),
    ("8K", 8192, 8192),
];

/// M33 Gallery: record a project path as most-recent (dedup, truncate).
fn push_recent(recent: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    recent.retain(|p| p != path);
    recent.insert(0, path.to_string());
    recent.truncate(MAX_RECENT);
}

/// M49: cooldown before retrying a failed settings write (seconds).
const SAVE_RETRY_SECS: f64 = 2.0;

/// One row of the Layers panel, stack order: a group header (with its
/// nesting depth for indentation and how many layers it holds) or a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerPanelRow {
    Group {
        id: u32,
        depth: usize,
        members: usize,
    },
    Layer {
        index: usize,
        depth: usize,
    },
}

/// Flatten the layer stack into panel rows. Headers sit above their topmost
/// member; groups with no layers anywhere (fresh, or emptied by moving or
/// deleting members) still get a row so they never vanish, root-level ones
/// at the top. Collapsed groups hide their subtree but keep their header.
fn layer_panel_rows(doc: &Document) -> Vec<LayerPanelRow> {
    let chains: Vec<Vec<u32>> = (0..doc.layers.len())
        .map(|i| {
            let mut up = Vec::new();
            let mut seen = Vec::new();
            let mut g = doc.layers[i].group;
            while let Some(id) = g {
                if seen.contains(&id) {
                    break;
                }
                seen.push(id);
                let Some(gr) = doc.group(id) else {
                    break;
                };
                up.push(id);
                g = gr.parent;
            }
            up.reverse();
            up
        })
        .collect();
    let mut live: Vec<u32> = Vec::new();
    for chain in &chains {
        for id in chain {
            if !live.contains(id) {
                live.push(*id);
            }
        }
    }
    let members = |id: u32| chains.iter().filter(|c| c.contains(&id)).count();
    let collapsed = |id: u32| doc.group(id).is_some_and(|g| g.collapsed);
    let mut rows = Vec::new();
    let mut printed: Vec<u32> = Vec::new();
    push_layerless_groups(doc, None, 0, &live, &mut printed, &mut rows);
    for i in (0..doc.layers.len()).rev() {
        let chain = &chains[i];
        for (depth, id) in chain.iter().enumerate() {
            if printed.contains(id) || chain[..depth].iter().any(|a| collapsed(*a)) {
                continue;
            }
            printed.push(*id);
            rows.push(LayerPanelRow::Group {
                id: *id,
                depth,
                members: members(*id),
            });
            if !collapsed(*id) {
                push_layerless_groups(doc, Some(*id), depth + 1, &live, &mut printed, &mut rows);
            }
        }
        if chain.iter().any(|id| collapsed(*id)) {
            continue;
        }
        rows.push(LayerPanelRow::Layer {
            index: i,
            depth: chain.len(),
        });
    }
    rows
}

/// Append the layerless groups under `parent` (newest first), recursing into
/// their own layerless children and honoring collapse.
fn push_layerless_groups(
    doc: &Document,
    parent: Option<u32>,
    depth: usize,
    live: &[u32],
    printed: &mut Vec<u32>,
    rows: &mut Vec<LayerPanelRow>,
) {
    let mut children: Vec<u32> = doc
        .groups
        .iter()
        .filter(|g| g.parent == parent && !live.contains(&g.id) && !printed.contains(&g.id))
        .map(|g| g.id)
        .collect();
    children.sort_unstable_by(|a, b| b.cmp(a));
    for id in children {
        printed.push(id);
        let collapsed = doc.group(id).is_some_and(|g| g.collapsed);
        rows.push(LayerPanelRow::Group {
            id,
            depth,
            members: 0,
        });
        if !collapsed {
            push_layerless_groups(doc, Some(id), depth + 1, live, printed, rows);
        }
    }
}

/// Group-header button presses, applied after the row loop so the panel can
/// draw from `&self.doc` without holding a borrow while it mutates it.
#[derive(Default)]
struct GroupRowActions {
    collapse: Option<u32>,
    eye: Option<u32>,
    add: Option<u32>,
    ungroup: Option<u32>,
    opacity: Option<(u32, f32)>,
    rename: Option<(u32, String)>,
    /// `(group, up)` from the header menu.
    reorder: Option<(u32, bool)>,
}

/// What a batch of header presses changed: `dirty` means the canvas ghost
/// needs a rebuild, `status` is the line to show.
#[derive(Default)]
struct GroupActionReport {
    status: Option<String>,
    dirty: bool,
}

/// Apply the header presses collected during the row loop. Split out of the
/// draw so the wiring is testable without an egui context.
fn apply_group_actions(doc: &mut Document, acts: &GroupRowActions) -> GroupActionReport {
    let mut report = GroupActionReport::default();
    if let Some(gid) = acts.collapse {
        let cur = doc.group(gid).is_some_and(|g| g.collapsed);
        doc.set_group_collapsed(gid, !cur);
    }
    if let Some(gid) = acts.eye {
        let cur = doc.group(gid).is_none_or(|g| g.visible);
        doc.set_group_visible(gid, !cur);
        report.dirty = true;
    }
    if let Some((gid, up)) = acts.reorder {
        if doc.move_group(gid, up) {
            let name = doc.group(gid).map_or(String::new(), |g| g.name.clone());
            report.status = Some(format!("{name} moved {}.", if up { "up" } else { "down" }));
            report.dirty = true;
        }
    }
    if let Some(gid) = acts.add {
        let active = doc.active;
        doc.move_layer_into_group(active, Some(gid));
        report.dirty = true;
        report.status = Some(format!(
            "{} moved into the group.",
            doc.layers[doc.active].name
        ));
    }
    if let Some(gid) = acts.ungroup {
        doc.ungroup(gid);
        report.dirty = true;
        report.status = Some("Group disbanded (layers kept).".to_string());
    }
    if let Some((gid, op)) = acts.opacity {
        doc.set_group_opacity(gid, op);
        report.dirty = true;
    }
    if let Some((gid, name)) = acts.rename.as_ref() {
        doc.rename_group(*gid, name.clone());
    }
    report
}

/// Inline member count for a group header (`(3)`). Empty groups show an
/// "empty" marker instead, so they get no badge.
fn group_member_badge(members: usize) -> Option<String> {
    (members > 0).then(|| format!("({members})"))
}

/// Layers-panel selection after a row click. A plain click selects one
/// row; `toggle` (Ctrl/Cmd-click or long-press) adds or removes the row;
/// `extend` (Shift-click) takes the range from the active layer. The
/// selection never goes empty, so there is always an active row to act on.
fn next_layer_selection(
    selection: &[usize],
    active: usize,
    target: usize,
    toggle: bool,
    extend: bool,
) -> Vec<usize> {
    if extend {
        let (lo, hi) = (active.min(target), active.max(target));
        return (lo..=hi).collect();
    }
    if toggle {
        let mut next: Vec<usize> = selection.iter().copied().filter(|&i| i != target).collect();
        if next.len() == selection.len() {
            next.push(target);
        }
        next.sort_unstable();
        next.dedup();
        if next.is_empty() {
            next.push(target);
        }
        return next;
    }
    vec![target]
}

/// Consume a pending "bring this row into view" request when the row it
/// names is drawn. The request must fire once: the header redraws every
/// frame, and repeating the scroll would fight the user's own scrolling.
fn take_scroll_request(request: &mut Option<u32>, id: u32) -> bool {
    if *request == Some(id) {
        *request = None;
        true
    } else {
        false
    }
}

/// Status line after "New group": naming the parent explains why the new
/// group landed indented inside an existing one.
fn group_created_status(name: &str, parent: Option<&str>, targets: &str) -> String {
    match parent {
        Some(parent) => format!("{name} created inside {parent} with {targets}."),
        None => format!("{name} created with {targets}."),
    }
}

/// Layers-panel footer text: layer count, the active layer's name and the
/// document's allocated tile memory (painted tiles plus masks). Fresh
/// layers cost nothing until a tile is touched, so this starts at `0 B`
/// and grows by 256 KB per painted tile.
fn layer_readout(doc: &Document) -> String {
    format!(
        "{} layer(s), active: {} · {}",
        doc.layers.len(),
        doc.layers
            .get(doc.active)
            .map(|l| l.name.as_str())
            .unwrap_or("-"),
        file_size_label(doc.layer_alloc_bytes() as u64)
    )
}

/// Downscale the composite to a `side`-bounding box by sampling
/// `Document::sample` on a block grid (max 2x2 samples per output pixel).
/// Unlike `flatten` + `gallery_thumb`, this never materialises the full
/// canvas (268 MB at 8192²), so it stays cheap on the UI thread while
/// recording a time-lapse or rebuilding the Gallery. Aspect is preserved;
/// documents already within `side` keep their dimensions (no upscale).
/// Returns `(rgba, width, height)`; empty input yields an empty thumbnail.
fn sampled_thumb(doc: &Document, side: u32) -> (Vec<u8>, usize, usize) {
    let (w, h) = (doc.w, doc.h);
    if w == 0 || h == 0 || side == 0 {
        return (Vec::new(), 0, 0);
    }
    let longest = w.max(h);
    let (tw, th) = if longest <= side {
        (w, h)
    } else {
        let scale = side as f64 / longest as f64;
        (
            ((w as f64 * scale).round() as u32).max(1),
            ((h as f64 * scale).round() as u32).max(1),
        )
    };
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    for ty in 0..th {
        // Source rows under this output row: probe the first and last one,
        // so a block averages across its extent instead of one corner.
        let sy0 = ty * h / th;
        let sy1 = ((ty + 1) * h / th).saturating_sub(1).max(sy0);
        for tx in 0..tw {
            let sx0 = tx * w / tw;
            let sx1 = ((tx + 1) * w / tw).saturating_sub(1).max(sx0);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for &sy in &[sy0, sy1] {
                for &sx in &[sx0, sx1] {
                    if let Some(px) = doc.sample(sx, sy) {
                        r += px[0] as u32;
                        g += px[1] as u32;
                        b += px[2] as u32;
                        n += 1;
                    }
                }
            }
            if n == 0 {
                continue;
            }
            let d = ((ty * tw + tx) * 4) as usize;
            out[d] = ((r + n / 2) / n) as u8;
            out[d + 1] = ((g + n / 2) / n) as u8;
            out[d + 2] = ((b + n / 2) / n) as u8;
            out[d + 3] = 255;
        }
    }
    (out, tw as usize, th as usize)
}

/// M49: what makes two settings snapshots differ in the discrete sense.
/// Zoom is excluded — it coalesces through `should_flush_zoom` and gets
/// written by its own quiet-period path.
fn settings_differ(a: &Settings, b: &Settings) -> bool {
    a.version != b.version
        || a.theme != b.theme
        || a.show_tools != b.show_tools
        || a.show_layers != b.show_layers
        || a.recent != b.recent
        || a.shortcuts != b.shortcuts
        || a.strip_collapsed != b.strip_collapsed
        || a.strip_dock != b.strip_dock
        || a.time_lapse != b.time_lapse
}

/// M49: the persistable ChordKey for an egui key (None for keys the
/// shortcut table has no slot for: Escape, Enter, modifier keys, F13+).
fn chord_key_from_egui(k: egui::Key) -> Option<ChordKey> {
    Some(match k {
        egui::Key::A => ChordKey::A,
        egui::Key::B => ChordKey::B,
        egui::Key::C => ChordKey::C,
        egui::Key::D => ChordKey::D,
        egui::Key::E => ChordKey::E,
        egui::Key::F => ChordKey::F,
        egui::Key::G => ChordKey::G,
        egui::Key::H => ChordKey::H,
        egui::Key::I => ChordKey::I,
        egui::Key::J => ChordKey::J,
        egui::Key::K => ChordKey::K,
        egui::Key::L => ChordKey::L,
        egui::Key::M => ChordKey::M,
        egui::Key::N => ChordKey::N,
        egui::Key::O => ChordKey::O,
        egui::Key::P => ChordKey::P,
        egui::Key::Q => ChordKey::Q,
        egui::Key::R => ChordKey::R,
        egui::Key::S => ChordKey::S,
        egui::Key::T => ChordKey::T,
        egui::Key::U => ChordKey::U,
        egui::Key::V => ChordKey::V,
        egui::Key::W => ChordKey::W,
        egui::Key::X => ChordKey::X,
        egui::Key::Y => ChordKey::Y,
        egui::Key::Z => ChordKey::Z,
        egui::Key::Num0 => ChordKey::Num0,
        egui::Key::Num1 => ChordKey::Num1,
        egui::Key::Num2 => ChordKey::Num2,
        egui::Key::Num3 => ChordKey::Num3,
        egui::Key::Num4 => ChordKey::Num4,
        egui::Key::Num5 => ChordKey::Num5,
        egui::Key::Num6 => ChordKey::Num6,
        egui::Key::Num7 => ChordKey::Num7,
        egui::Key::Num8 => ChordKey::Num8,
        egui::Key::Num9 => ChordKey::Num9,
        egui::Key::F1 => ChordKey::F1,
        egui::Key::F2 => ChordKey::F2,
        egui::Key::F3 => ChordKey::F3,
        egui::Key::F4 => ChordKey::F4,
        egui::Key::F5 => ChordKey::F5,
        egui::Key::F6 => ChordKey::F6,
        egui::Key::F7 => ChordKey::F7,
        egui::Key::F8 => ChordKey::F8,
        egui::Key::F9 => ChordKey::F9,
        egui::Key::F10 => ChordKey::F10,
        egui::Key::F11 => ChordKey::F11,
        egui::Key::F12 => ChordKey::F12,
        egui::Key::ArrowUp => ChordKey::ArrowUp,
        egui::Key::ArrowDown => ChordKey::ArrowDown,
        egui::Key::ArrowLeft => ChordKey::ArrowLeft,
        egui::Key::ArrowRight => ChordKey::ArrowRight,
        egui::Key::Home => ChordKey::Home,
        egui::Key::End => ChordKey::End,
        egui::Key::PageUp => ChordKey::PageUp,
        egui::Key::PageDown => ChordKey::PageDown,
        egui::Key::Insert => ChordKey::Insert,
        egui::Key::Space => ChordKey::Space,
        egui::Key::Tab => ChordKey::Tab,
        egui::Key::Comma => ChordKey::Comma,
        egui::Key::Period => ChordKey::Period,
        egui::Key::Slash => ChordKey::Slash,
        egui::Key::Semicolon => ChordKey::Semicolon,
        egui::Key::Quote => ChordKey::Quote,
        egui::Key::Minus => ChordKey::Minus,
        egui::Key::Equals => ChordKey::Equals,
        egui::Key::OpenBracket => ChordKey::OpenBracket,
        egui::Key::CloseBracket => ChordKey::CloseBracket,
        egui::Key::Backslash => ChordKey::Backslash,
        egui::Key::Backtick => ChordKey::Backtick,
        _ => return None,
    })
}

/// M49: this event as a chord, when it is a key press (repeats included,
/// matching the old `consume_key` handling).
fn chord_from_event(ev: &egui::Event) -> Option<KeyChord> {
    let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = ev
    else {
        return None;
    };
    Some(KeyChord::new(
        chord_key_from_egui(*key)?,
        modifiers.ctrl,
        modifiers.shift,
        modifiers.alt,
    ))
}

/// M49: consume the first key event whose exact chord the table owns and
/// return that action (the event leaves the queue so nothing below
/// re-reads it). The `while` loop at the call site terminates: every hit
/// removes one event.
fn take_shortcut_action(shortcuts: &Shortcuts, ctx: &egui::Context) -> Option<ActionId> {
    ctx.input_mut(|i| {
        let pos = i.events.iter().position(|ev| {
            chord_from_event(ev)
                .and_then(|c| shortcuts.owner_of(c))
                .is_some()
        })?;
        let ev = i.events.remove(pos);
        shortcuts.owner_of(chord_from_event(&ev)?)
    })
}

/// Toolbar order, shared by the tools panel list and its mobile grid.
const TOOL_ORDER: [Tool; 13] = [
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

/// M49: shortcut id for an app tool (mirror of `tool_from_id`).
fn tool_id_for(tool: Tool) -> ToolId {
    match tool {
        Tool::Brush => ToolId::Brush,
        Tool::Eraser => ToolId::Eraser,
        Tool::Smudge => ToolId::Smudge,
        Tool::Select => ToolId::Select,
        Tool::Lasso => ToolId::Lasso,
        Tool::Fill => ToolId::Fill,
        Tool::Picker => ToolId::Picker,
        Tool::Move => ToolId::Move,
        Tool::Hand => ToolId::Hand,
        Tool::Liquify => ToolId::Liquify,
        Tool::Crop => ToolId::Crop,
        Tool::Shapes => ToolId::Shapes,
        Tool::Text => ToolId::Text,
    }
}

/// M49: the app's Tool enum for a shortcut ToolId (toolbar order).
fn tool_from_id(id: ToolId) -> Tool {
    match id {
        ToolId::Brush => Tool::Brush,
        ToolId::Eraser => Tool::Eraser,
        ToolId::Smudge => Tool::Smudge,
        ToolId::Select => Tool::Select,
        ToolId::Lasso => Tool::Lasso,
        ToolId::Fill => Tool::Fill,
        ToolId::Picker => Tool::Picker,
        ToolId::Move => Tool::Move,
        ToolId::Hand => Tool::Hand,
        ToolId::Liquify => Tool::Liquify,
        ToolId::Crop => Tool::Crop,
        ToolId::Shapes => Tool::Shapes,
        ToolId::Text => Tool::Text,
    }
}

/// M49: how one key event resolves while capture mode is armed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CaptureOutcome {
    /// Escape: leave capture without changing anything.
    Cancel,
    /// Delete/Backspace: unbind the captured action.
    Clear,
    /// A mappable key press: bind it to the captured action.
    Bind(KeyChord),
    /// Anything else (Enter, modifier keys, releases): keep waiting.
    Ignore,
}

/// M49: resolve a key event during capture. Escape and Delete/Backspace
/// are checked before chord mapping (both are unmappable anyway); only
/// pressed events ever resolve.
fn resolve_capture_event(ev: &egui::Event) -> CaptureOutcome {
    let egui::Event::Key {
        key, pressed: true, ..
    } = ev
    else {
        return CaptureOutcome::Ignore;
    };
    match key {
        egui::Key::Escape => CaptureOutcome::Cancel,
        egui::Key::Delete | egui::Key::Backspace => CaptureOutcome::Clear,
        _ => match chord_from_event(ev) {
            Some(c) => CaptureOutcome::Bind(c),
            None => CaptureOutcome::Ignore,
        },
    }
}

/// M49: brush size after a size-drag of `travel` px along the configured
/// direction, applied to the gesture's base size (never per-frame
/// deltas, so the value can't drift), clamped to the size slider's range
/// (1..=400).
fn drag_size(base: f32, travel: f32) -> f32 {
    (base * size_drag_factor(travel)).clamp(BRUSH_SIZE_MIN, BRUSH_SIZE_MAX)
}

/// M49: the brush-size drag's travel in px for `mode`, measured from the
/// gesture's start point `anchor` to the current pointer `pos`.
/// Horizontal and vertical are signed (right/up grows); distance is the
/// straight-line length, so any move grows the brush.
fn size_drag_travel(mode: SizeDragMode, anchor: egui::Pos2, pos: egui::Pos2) -> f32 {
    match mode {
        SizeDragMode::Horizontal => pos.x - anchor.x,
        SizeDragMode::Vertical => anchor.y - pos.y,
        SizeDragMode::Distance => (pos - anchor).length(),
    }
}

/// M49: human label for the size-drag direction picker.
fn size_drag_mode_label(mode: SizeDragMode) -> &'static str {
    match mode {
        SizeDragMode::Horizontal => "Left–right",
        SizeDragMode::Vertical => "Up–down",
        SizeDragMode::Distance => "Distance from start",
    }
}

/// Space-held transient-Hand transition. `cur` is the active tool,
/// `state` the held hand's stored tool (None = not engaged); `pressed` a
/// fresh Space press this frame, `down` Space held, `pointer_down` any
/// pointer button down, `usable` = no typing/capture context. Returns
/// the next state: Some(prev) while engaged, None otherwise.
fn space_hand_step(
    cur: Tool,
    state: Option<Tool>,
    pressed: bool,
    down: bool,
    pointer_down: bool,
    usable: bool,
) -> Option<Tool> {
    match state {
        Some(prev) => (down && usable).then_some(prev),
        None => (pressed && usable && !pointer_down).then_some(cur),
    }
}

fn page_presets() -> Vec<(&'static str, [u8; 3])> {
    vec![
        ("White", [255, 255, 255]),
        ("Black", [0, 0, 0]),
        ("Gray", [128, 128, 128]),
        ("Warm", [240, 230, 210]),
        ("Blue", [52, 120, 200]),
    ]
}

/// M49: brush-size drag gesture state — the size at press plus the press
/// point; travel from `anchor` along the configured direction (see
/// [`SizeDragMode`]) drives the size from `base`, and the cursor ring
/// stays pinned at the anchor while the gesture runs.
struct SizeDrag {
    base: f32,
    anchor: egui::Pos2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrushSlot {
    Builtin(usize),
    Custom(usize),
}

fn slot_is_active(selected: Option<BrushSlot>, candidate: BrushSlot) -> bool {
    selected == Some(candidate)
}

fn slot_after_remove(selected: Option<BrushSlot>, removed: usize) -> Option<BrushSlot> {
    match selected {
        Some(BrushSlot::Custom(index)) if index == removed => None,
        Some(BrushSlot::Custom(index)) if index > removed => Some(BrushSlot::Custom(index - 1)),
        other => other,
    }
}

struct ProCraftApp {
    doc: Document,
    brush: Brush,
    color: [u8; 3],
    tool: Tool,
    theme: Theme,
    theme_applied: Option<(Theme, BarLayout)>,
    /// True when the system symbol font loaded (desktop Windows): UI
    /// strings may use icon glyphs. iPad has no such font, so the same
    /// strings fall back to plain words (see `tool_label`).
    has_symbols: bool,
    page_custom: [u8; 3],
    picker_size: f32,
    texture: Option<TextureHandle>,
    /// Composite tiles behind the canvas texture: a dab recomposites and
    /// uploads only the tiles under its dirty rect (see `refresh_texture`).
    composite: canvas::CompositeCache,
    /// Viewport-resolution canvas (spec §4): the visible rect composited
    /// into a screen-sized texture instead of one canvas-sized one. Unused
    /// unless `canvas::viewport_active` says so.
    view_tex: canvas::ViewTex,
    painting: bool,
    last: Option<(f32, f32)>,
    last_target: Option<(f32, f32)>,
    last_dyn: DabDynamics,
    last_t: f64,
    leftover: f32,
    // Stabilizer state: exponentially smoothed pointer + pressure. Reset
    // on every stroke start so strokes never inherit the last line's tail.
    smooth_pt: Option<(f32, f32)>,
    smooth_pressure: f32,
    // Midpoint-smoothed stroke path: rounds the corners between sparse
    // input samples so fast strokes ink as curves, not chords.
    stroke_curve: StrokeCurve,
    // Segments of the live brush stroke, replayed on release when the
    // brush has a start/end taper.
    stroke_record: RecordedStroke,
    sel_anchor: Option<(f32, f32)>,
    sel_current: Option<(f32, f32)>,
    /// Drag-crop state: the live drag plus the committed rect (inclusive
    /// canvas px) waiting for Apply. Re-drag replaces, Esc cancels.
    crop_anchor: Option<(f32, f32)>,
    crop_current: Option<(f32, f32)>,
    crop_rect: Option<(u32, u32, u32, u32)>,
    /// Canvas-size panel: wanted dimensions plus where the old canvas sits
    /// (-1 left/top, 0 center, 1 right/bottom per axis). `canvas_sync`
    /// tracks the doc dims so the fields re-adopt the real size after an
    /// open/crop/apply without clobbering typing mid-edit.
    canvas_w: u32,
    canvas_h: u32,
    canvas_sync: (u32, u32),
    canvas_anchor: (i32, i32),
    lasso_points: Vec<(f32, f32)>,
    lasso_poly: Vec<(f32, f32)>,
    /// Cached mask-edge points for marching ants, rebuilt when
    /// `sel_edges_dirty` (set at every selection-mutating site).
    sel_edges: Vec<(u32, u32)>,
    sel_edges_dirty: bool,
    /// Canvas pan offset in screen px (two-finger drag on touch; the
    /// desktop scrolls the view with the wheel instead).
    view_offset: egui::Vec2,
    /// Canvas rotation in radians (two-finger twist on touch, slider on
    /// desktop). View-only like zoom/pan: paint math stays axis-aligned.
    view_angle: f32,
    /// A stroke applied at least one dab (guards pinch-cancel revert).
    stroke_painted: bool,
    /// Multi-finger tap in progress (two = undo, three = redo on release).
    tap_track: Option<TapTrack>,
    /// Autosave recovery file (v8 project snapshot every 120 s) plus the
    /// last write time and whether to offer it at startup.
    recovery_path: std::path::PathBuf,
    last_autosave: f64,
    recovery_offer: bool,
    fill_tolerance: u8,
    fill_grow: u8,
    fill_preview: Option<FillPreview>,
    smudge_strength: f32,
    filter_preview: Option<FilterPreview>,
    sel_feather: u32,
    sel_grow: i32,
    sel_opaque_t: u8,
    sel_stroke_w: u32,
    filt_brightness: i32,
    filt_contrast: f32,
    filt_blur: i32,
    filt_sharpen: f32,
    /// M36 Adjustments I: Levels black/white points + mid gamma (%).
    filt_levels_shadow: u8,
    filt_levels_gamma: f32,
    filt_levels_high: u8,
    /// M36 Curves (parametric): shadow/mid/highlight lift (+) or crush (-).
    filt_curve_shadow: i32,
    filt_curve_mid: i32,
    filt_curve_high: i32,
    /// M37 Adjustments II: HSL, color balance, recolor strength (the
    /// recolor target is the current paint color).
    filt_hue: f32,
    filt_sat: f32,
    filt_light: i32,
    filt_bal_r: i32,
    filt_bal_g: i32,
    filt_bal_b: i32,
    filt_recolor_strength: f32,
    renaming: Option<usize>,
    renaming_text: String,
    group_renaming: Option<u32>,
    /// Group header the panel should bring into view, set by "New group".
    /// Consumed by the matching row on the next draw.
    group_scroll: Option<u32>,
    /// Multi-selection for the Layers panel, as layer indexes. Always
    /// contains the active layer, and resets when the layer count changes
    /// (add, duplicate, merge, delete, undo) since indexes are not ids.
    layer_selection: Vec<usize>,
    /// `doc.layers.len()` seen on the last panel draw: detects the count
    /// changes that invalidate `layer_selection`.
    layer_count_marker: usize,
    group_name_edit: String,
    preset_textures: Vec<TextureHandle>,
    view_mask: bool,
    /// The mask viewer owns the canvas texture slot (it holds mask pixels,
    /// or was cleared for one), so leaving the view must drop it once
    /// instead of patching composite tiles over it.
    mask_shown: bool,
    show_tools: bool,
    show_layers: bool,
    drawer: MobileDrawer,
    /// Last drawer actually shown: `drawer` drops to `None` the moment it
    /// closes, this keeps the panel around for its slide-out.
    drawer_shown: MobileDrawer,
    /// Phone strip: the pocket pill instead of the full bar, which third of
    /// the bottom edge it docks to, and the live drag offset while it is
    /// being moved (screen px from the docked position).
    strip_collapsed: bool,
    strip_dock: StripDock,
    strip_offset: f32,
    /// One-shot request from the mobile Color dot: after the Tools drawer
    /// opens, scroll its body to the Color card.
    mobile_scroll_color: bool,
    /// Status text the toast has already announced, and when it changed.
    status_seen: String,
    status_at: f64,
    narrow_setup_done: bool,
    show_about: bool,
    #[cfg(target_os = "ios")]
    pending_open: Option<PendingOpen>,
    /// Android in-app browser request (external files; no system picker).
    #[cfg(target_os = "android")]
    android_browse: Option<PendingOpen>,
    /// Cached status-bar height in px (-1 = unread; JNI runs once).
    #[cfg(target_os = "android")]
    android_inset_top: f32,
    preview_theme: Option<Theme>,
    custom_brushes: Vec<BrushPreset>,
    brush_slot: Option<BrushSlot>,
    new_brush_name: String,
    palettes: PaletteSet,
    pal_renaming: bool,
    pal_name_edit: String,
    guides: Guides,
    vp_move: bool,
    vp_grab: Option<usize>,
    sym_move: bool,
    sym_grab: bool,
    /// M48 transform handles: on-canvas corner pins / warp lattice.
    xform: XformState,
    /// Transform-layer button session (Scale/rotate/flip without
    /// compounding resampling).
    tform_press: TformPress,
    assist: bool,
    assist_start: Option<(f32, f32)>,
    assist_lock: Option<AssistLine>,
    // Animation Assist: layers are frames (bottom-to-top), the active
    // layer is the current frame. Ghost handles are half-res overlays,
    // rebuilt when the neighbor pair changes or `ghost_dirty` is set by
    // undo/redo and layer edits (painting only touches the current frame,
    // so strokes never invalidate ghosts).
    anim_assist: bool,
    anim_playing: bool,
    anim_fps: u32,
    anim_loop: LoopMode,
    /// JPEG export quality 1-100 (M28 slider; TIFF is lossless, no dial).
    jpeg_quality: u8,
    anim_fwd: bool,
    anim_last_t: f64,
    anim_shown: Option<usize>,
    onion: bool,
    /// Onion ghost strengths per side (red = previous, blue = next).
    onion_prev: f32,
    onion_next: f32,
    /// M41 Time-lapse: record flag, staged snapshots (bytes, w, h),
    /// and the last stage time (min-interval gate).
    lapse_on: bool,
    lapse_frames: Vec<(Vec<u8>, u32, u32)>,
    lapse_last: std::time::Instant,
    ghost_prev: Option<TextureHandle>,
    ghost_next: Option<TextureHandle>,
    ghost_key: (Option<usize>, Option<usize>, u32, u32),
    ghost_dirty: bool,
    move_from: Option<(f32, f32)>,
    move_acc: (f32, f32),
    /// Ctrl+middle-drag zoom anchor (screen pos of the last tick).
    mmb_last: Option<egui::Pos2>,
    /// M40 Liquify: drag anchor plus brush size/strength.
    liquify_from: Option<(f32, f32)>,
    liquify_size: f32,
    liquify_strength: f32,
    shape_anchor: Option<(f32, f32)>,
    shape_current: Option<(f32, f32)>,
    shape_kind: ShapeKind,
    shape_fill: bool,
    shape_sides: u8,
    text_anchor: Option<(f32, f32)>,
    text_open: bool,
    text_body: String,
    text_size: f32,
    /// M38 Typography: imported face (session-only bytes + name),
    /// spacing, and outline. Render bakes through `TextStyle`.
    text_font_name: Option<String>,
    text_font_bytes: Option<Vec<u8>>,
    text_leading: f32,
    text_tracking: f32,
    text_outline: bool,
    text_outline_w: u32,
    text_outline_rgb: [u8; 3],
    /// Text-layer edit session: the layer index plus the block identity
    /// (text + anchor) captured when the dialog opened. Done compares
    /// against these so an edit landing on a layer that changed
    /// underneath (deleted, reordered, undone) cancels instead of
    /// corrupting; `None` means the dialog creates a new layer.
    editing_text: Option<usize>,
    editing_loaded: Option<(String, f32, f32)>,
    zoom: f32,
    status: String,
    /// M33 Gallery: recent project paths (newest first, max 10),
    /// the browser window flag, cached per-path thumbnails, the key
    /// they were built from, and the New-canvas size fields.
    recent: Vec<String>,
    gallery_open: bool,
    gallery_thumbs: Vec<Option<TextureHandle>>,
    gallery_keys: Vec<String>,
    gallery_new_w: u32,
    gallery_new_h: u32,
    /// M34 Reference companion: pinned image alongside the canvas
    /// (session-only, not saved in the project). Texture rebuilds lazily.
    ref_open: bool,
    ref_image: Option<(Vec<u8>, u32, u32)>,
    ref_tex: Option<TextureHandle>,
    ref_scale: f32,
    /// M35 Selection upgrades: saved snapshot ((w,h), rect, mask) plus
    /// the nudge steppers. Session-only; Load refuses size mismatches.
    saved_sel: Option<SavedSelection>,
    sel_nudge_x: i32,
    sel_nudge_y: i32,
    /// M49 settings: resolved settings dir (None = session-only), the
    /// last-saved snapshot (change detection), the shortcuts table, and
    /// the zoom write coalescer (dirty flag + last-change time).
    settings_dir: Option<std::path::PathBuf>,
    settings_snapshot: Settings,
    shortcuts: Shortcuts,
    zoom_dirty: bool,
    zoom_changed_at: f64,
    /// M49 UI: shortcuts window flag, live chord-capture target, a
    /// load/corrupt note for the settings window, and the Shift+drag
    /// brush-size gesture in flight.
    show_shortcuts: bool,
    capture: Option<ActionId>,
    settings_note: Option<String>,
    size_drag: Option<SizeDrag>,
    /// Space held = transient Hand: the tool to restore on release.
    space_hand: Option<Tool>,
    /// M49: a failed-save note (shown with the settings note) and the
    /// time of the last failed write attempt (retry cooldown).
    save_note: Option<String>,
    save_failed_at: Option<f64>,
    /// M49: set when this run seeded settings.json; the marker is written
    /// on the first frame (eframe's CreationContext storage is read-only).
    seed_marker_pending: bool,
}

impl ProCraftApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
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

    /// M49: current fields as a Settings value (the shape written to
    /// settings.json).
    fn settings_snapshot_from_fields(&self) -> Settings {
        Settings {
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
        }
    }

    /// M49: adopt a loaded file. Unknown theme ids keep the current
    /// theme; a non-finite/positive zoom keeps the current zoom (legacy
    /// keys' semantics); shortcut-file warnings become the settings note.
    fn apply_settings(&mut self, s: &Settings) {
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
    fn save_settings_file(&self, s: &Settings) -> Result<(), String> {
        match &self.settings_dir {
            Some(dir) => crate::settings::save_now(dir, s),
            None => Ok(()),
        }
    }

    /// M49: once-per-frame settings sync. Discrete edits (theme, panels,
    /// recents, shortcuts) save immediately; zoom writes coalesce and
    /// only land once `should_flush_zoom` says the pointer went quiet.
    /// A failed write leaves the snapshot un-advanced (the edit stays
    /// pending) and is retried after a short cooldown, so a transient
    /// disk error never silently drops a change.
    fn settings_sync(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let cur = self.settings_snapshot_from_fields();
        if cur.zoom != self.settings_snapshot.zoom {
            self.zoom_dirty = true;
            self.zoom_changed_at = now;
            self.settings_snapshot.zoom = cur.zoom;
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
                        self.zoom_dirty = false; // snapshot refreshed wholesale
                        self.save_note = None;
                        self.save_failed_at = None;
                    }
                    Err(err) => {
                        self.save_failed_at = Some(now);
                        self.note_save_failure(err);
                    }
                }
            }
        } else if crate::settings::should_flush_zoom(self.zoom_dirty, self.zoom_changed_at, now)
            && retry_ok
        {
            match self.save_settings_file(&cur) {
                Ok(()) => {
                    self.settings_snapshot = cur;
                    self.zoom_dirty = false;
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
    fn note_save_failure(&mut self, err: String) {
        let note = format!("Couldn't save settings ({err}); will retry.");
        if self.save_note.is_none() {
            self.status = note.clone();
        }
        self.save_note = Some(note);
    }

    /// M49: flush immediately when anything is pending (clean exit and
    /// iOS backgrounding — no timing gate here). A failure keeps the
    /// edit pending so the next frame re-attempts it.
    fn flush_settings_now(&mut self) {
        let cur = self.settings_snapshot_from_fields();
        if settings_differ(&cur, &self.settings_snapshot) || self.zoom_dirty {
            match self.save_settings_file(&cur) {
                Ok(()) => {
                    self.settings_snapshot = cur;
                    self.zoom_dirty = false;
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

    /// M49: run this frame's key events through the shortcut table.
    /// Capture mode owns every key; a focused text field keeps plain
    /// typing (spec call #6) — the table must not steal letters.
    fn dispatch_shortcuts(&mut self, ctx: &egui::Context) {
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
    fn handle_space_hand(&mut self, ctx: &egui::Context) {
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
    fn apply_action(&mut self, action: ActionId) {
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
    fn handle_capture(&mut self, ctx: &egui::Context) {
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
    fn action_label(&self, a: ActionId) -> String {
        match a {
            ActionId::Tool(t) => tool_label(tool_from_id(t), self.has_symbols).to_string(),
            other => other.label(),
        }
    }

    /// M49: toolbar hover hint ("Brush — B", or "— unbound").
    fn tool_hint(&self, id: ToolId) -> String {
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
    fn shortcuts_window(&mut self, ctx: &egui::Context) {
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

    fn base_brush(&self) -> Brush {
        Brush {
            eraser: self.tool == Tool::Eraser,
            ..self.brush.clone()
        }
    }

    fn current_color(&self) -> Color {
        Color::rgb(self.color[0], self.color[1], self.color[2])
    }

    /// Re-apply the last fill after a Tolerance/Grow slider move. Drops the
    /// session (leaving the canvas untouched) if another edit or a context
    /// change made the snapshot stale.
    fn reapply_live_fill(&mut self) {
        let Some(mut fp) = self.fill_preview.take() else {
            return;
        };
        let layer_ok = self
            .doc
            .layers
            .get(fp.layer)
            .is_some_and(|l| l.pixels.fingerprint() == fp.live_hash);
        if !layer_ok || !fill_context_matches(&self.doc, &fp) {
            return;
        }
        fill_from_baseline(&mut self.doc, &mut fp, self.fill_tolerance, self.fill_grow);
        self.status = format!(
            "Fill preview — tolerance {}, grow {}.",
            fp.applied, self.fill_grow
        );
        self.fill_preview = Some(fp);
    }

    /// Rebuild the cached mask-edge points for marching ants. Rect-only
    /// selections contribute their perimeter; masks contribute pixels on
    /// the nonzero boundary (feather fringe included, so the crawl shows
    /// the true extent). Runs once per selection change, not per frame.
    fn rebuild_sel_edges(&mut self) {
        self.sel_edges.clear();
        let (w, h) = (self.doc.w, self.doc.h);
        if w == 0 || h == 0 {
            return;
        }
        if let Some(mask) = self.doc.selection_mask.as_deref() {
            if mask.len() != (w * h) as usize {
                return;
            }
            let at = |x: u32, y: u32| mask[(y * w + x) as usize];
            for y in 0..h {
                for x in 0..w {
                    if at(x, y) == 0 {
                        continue;
                    }
                    let edge = x == 0
                        || y == 0
                        || x + 1 == w
                        || y + 1 == h
                        || at(x - 1, y) == 0
                        || at(x + 1, y) == 0
                        || at(x, y - 1) == 0
                        || at(x, y + 1) == 0;
                    if edge {
                        self.sel_edges.push((x, y));
                    }
                }
            }
        } else if let Some((x0, y0, x1, y1)) = self.doc.selection {
            let (x1, y1) = (x1.min(w - 1), y1.min(h - 1));
            for x in x0..=x1 {
                self.sel_edges.push((x, y0));
                if y1 != y0 {
                    self.sel_edges.push((x, y1));
                }
            }
            for y in y0 + 1..y1 {
                self.sel_edges.push((x0, y));
                if x1 != x0 {
                    self.sel_edges.push((x1, y));
                }
            }
        }
    }

    /// Whether the viewport-resolution fallback (spec §4) drives the canvas
    /// this frame. Mask view and Animation Assist opt out: they keep their
    /// own full uploads.
    fn viewport_canvas(&self, ctx: &egui::Context) -> bool {
        !self.view_mask
            && !self.anim_assist
            && canvas::viewport_active(
                viewport_forced(),
                cfg!(any(target_os = "android", target_os = "ios")),
                self.doc.w.max(self.doc.h),
                ctx.input(|i| i.max_texture_side),
            )
    }

    /// Upload only what changed since the last frame (no-op when clean).
    /// Composite view uploads one `set_partial` per tile under the dirty
    /// rect; pan, zoom and rotation never recomposite.
    fn refresh_texture(&mut self, ctx: &egui::Context) {
        // "View" can be checked on a layer with no mask. If that view would
        // also suppress the viewport fallback this document needs, drop it
        // for the frame rather than ask for a canvas texture this GPU cannot
        // hold.
        let has_mask = self
            .doc
            .layers
            .get(self.doc.active)
            .is_some_and(|l| l.mask.is_some());
        if canvas::maskless_view_blocks_fallback(
            self.view_mask,
            has_mask,
            self.anim_assist,
            canvas::viewport_active(
                viewport_forced(),
                cfg!(any(target_os = "android", target_os = "ios")),
                self.doc.w.max(self.doc.h),
                ctx.input(|i| i.max_texture_side),
            ),
        ) {
            self.view_mask = false;
            let msg = "This layer has no mask.";
            if self.status != msg {
                self.status = msg.to_string();
            }
        }
        // Viewport-resolution fallback (spec §4): the draw path composites
        // the visible rect, so there is nothing to upload here. Drop the
        // full-canvas texture — at 8K it is 268 MB of VRAM this path never
        // samples — but keep the composite cache: it is exactly what the
        // viewport reads, the one deliberate exception to "texture dropped
        // ⇒ cache invalidated".
        if self.viewport_canvas(ctx) {
            self.texture = None;
            return;
        }
        // Target A from here on: the viewport texture is dead weight, and a
        // dropped handle forces a full pass if the fallback returns.
        if self.view_tex.texture().is_some() {
            self.view_tex.clear();
        }
        // Mask viewer: grayscale of the active layer's mask. The decision
        // comes first because `take_dirty` is destructive: with the view on
        // and no mask on the layer, the dirty rect must reach the composite
        // path below (which patches tiles) instead of being drained here and
        // leaving the composite to flatten the whole canvas.
        match mask_action(self.view_mask, has_mask, self.mask_shown) {
            // The canvas texture holds mask pixels that no longer apply:
            // drop it once so the composite path starts from a clean
            // `texture == None` instead of patching tiles over mask pixels.
            MaskAction::Drop => {
                self.mask_shown = false;
                self.texture = None;
                self.composite.invalidate();
                // Fall through: the composite / assist path rebuilds.
            }
            // Nothing mask-specific this frame: the dirty rect is *not*
            // drained — the composite / assist path owns it.
            MaskAction::Idle => {
                if self.view_mask {
                    let msg = "This layer has no mask.";
                    if self.status != msg {
                        self.status = msg.to_string();
                    }
                }
            }
            // The layer has a mask and the view is on: build or patch the
            // mask texture. Draining the dirty rect means a mask dab re-reads
            // and re-uploads only that rect; the full canvas is built once
            // per texture (open, resize, view toggle), never per frame.
            MaskAction::Show => {
                let dirty = self.doc.take_dirty();
                // The texture is canvas-sized, so a GPU that cannot hold one
                // — or a document above the 8K cap this GPU-backed path was
                // sized for — would reject the upload in the backend: say so
                // instead.
                if !canvas::canvas_texture_fits(
                    self.doc.w.max(self.doc.h),
                    ctx.input(|i| i.max_texture_side),
                ) {
                    let msg = "Mask view needs a canvas texture this GPU cannot allocate.";
                    if self.status != msg {
                        self.status = msg.to_string();
                    }
                    self.texture = None;
                    self.mask_shown = true;
                    self.composite.invalidate();
                    return;
                }
                // The canvas texture is not the composite here, so no composite
                // tile can be trusted when the view returns.
                self.composite.invalidate();
                if self.texture.is_none() {
                    if let Some(mv) = self.doc.mask_view() {
                        let img = ColorImage::from_rgba_unmultiplied(
                            [self.doc.w as usize, self.doc.h as usize],
                            &mv,
                        );
                        self.texture =
                            Some(ctx.load_texture("canvas", img, TextureOptions::LINEAR));
                        self.mask_shown = true;
                        return;
                    }
                    // No mask to build from after all (mid-state): the slot
                    // does not hold mask pixels, so leaving the view must
                    // not drop the composite texture a second time.
                    self.mask_shown = false;
                } else {
                    match dirty {
                        Some((x0, y0, x1, y1)) => match self.doc.mask_view_region(x0, y0, x1, y1) {
                            Some(bytes) => {
                                let img = ColorImage::from_rgba_unmultiplied(
                                    [(x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize],
                                    &bytes,
                                );
                                if let Some(handle) = &mut self.texture {
                                    handle.set_partial(
                                        [x0 as usize, y0 as usize],
                                        img,
                                        TextureOptions::LINEAR,
                                    );
                                }
                                self.mask_shown = true;
                                return;
                            }
                            // Mask gone: fall through and rebuild from the
                            // composite instead of leaving a stale texture.
                            None => {
                                self.texture = None;
                                self.mask_shown = false;
                            }
                        },
                        // Nothing changed: the upload is already correct.
                        None => {
                            self.mask_shown = true;
                            return;
                        }
                    }
                }
            }
        }
        // Animation Assist: the canvas shows the current frame only
        // (active layer over the page), not the full composite.
        if self.anim_assist && !self.doc.layers.is_empty() {
            // The frame texture is canvas-sized too: a GPU that cannot hold
            // one — or a document above the 8K cap — would reject the upload
            // in the backend (a legacy 16384² document asks for ~1 GiB), so
            // say so instead of building it.
            if !canvas::canvas_texture_fits(
                self.doc.w.max(self.doc.h),
                ctx.input(|i| i.max_texture_side),
            ) {
                let msg = "Animation Assist needs a canvas texture this GPU cannot allocate.";
                if self.status != msg {
                    self.status = msg.to_string();
                }
                self.texture = None;
                self.anim_shown = None;
                self.composite.invalidate();
                return;
            }
            // Frame view is not the composite (dab rects are consumed
            // here, never by the tile cache): drop the tiles.
            self.composite.invalidate();
            let active = self.doc.active.min(self.doc.layers.len() - 1);
            if self.texture.is_none() || self.anim_shown != Some(active) {
                let bytes = rasterize_frame(&self.doc, active);
                let img = ColorImage::from_rgba_unmultiplied(
                    [self.doc.w as usize, self.doc.h as usize],
                    &bytes,
                );
                self.texture = Some(ctx.load_texture("canvas", img, TextureOptions::LINEAR));
                self.anim_shown = Some(active);
                return;
            }
            if let Some((x0, y0, x1, y1)) = self.doc.take_dirty() {
                let bytes = rasterize_frame_region(&self.doc, active, x0, y0, x1, y1);
                let w = (x1 - x0 + 1) as usize;
                let h = ((y1 - y0 + 1) as usize).max(bytes.len() / 4 / w.max(1));
                let img = ColorImage::from_rgba_unmultiplied([w, h], &bytes);
                if let Some(handle) = &mut self.texture {
                    handle.set_partial([x0 as usize, y0 as usize], img, TextureOptions::LINEAR);
                }
            }
            return;
        }
        self.anim_shown = None;
        // Full upload: new/changed texture. `full_image` interleaves the
        // tiles row by row and crops the last column and row, so the dense
        // buffer is exactly `w * h * 4` and the texture stays canvas-sized.
        if self.texture.is_none() {
            let plan = self.doc.stack_plan();
            let img = canvas::full_image(&self.doc, &plan, &mut self.composite);
            let image = ColorImage::from_rgba_unmultiplied(
                [self.doc.w as usize, self.doc.h as usize],
                &img,
            );
            self.texture = Some(ctx.load_texture("canvas", image, TextureOptions::LINEAR));
            // The upload already covers the whole canvas.
            self.doc.take_dirty();
            return;
        }
        if let Some((x0, y0, x1, y1)) = self.doc.take_dirty() {
            // One plan per pass: it snapshots the revision, and the next
            // dab bumps it (a retained plan would be stale).
            let plan = self.doc.stack_plan();
            let (x0, y0) = (
                x0.min(self.doc.w.saturating_sub(1)),
                y0.min(self.doc.h.saturating_sub(1)),
            );
            let (x1, y1) = (
                x1.min(self.doc.w.saturating_sub(1)),
                y1.min(self.doc.h.saturating_sub(1)),
            );
            for ty in y0 / TILE..=y1 / TILE {
                for tx in x0 / TILE..=x1 / TILE {
                    // A cached hit would be the pre-edit composite of the
                    // very tile this dab touched, so drop it first.
                    self.composite.evict(tx, ty);
                    let tile = self.composite.tile(&self.doc, &plan, tx, ty);
                    let (rgba, w, h) = canvas::tile_image(&self.doc, tile, tx, ty);
                    if w == 0 || h == 0 {
                        continue;
                    }
                    let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                    if let Some(handle) = &mut self.texture {
                        handle.set_partial(
                            [(tx * TILE) as usize, (ty * TILE) as usize],
                            image,
                            TextureOptions::LINEAR,
                        );
                    }
                }
            }
        }
    }

    /// Rebuild half-res onion ghost textures when the neighbor pair, the
    /// per-side strengths, or layer content (`ghost_dirty`) changed.
    /// No-op otherwise.
    fn refresh_ghosts(&mut self, ctx: &egui::Context) {
        let active = clamp_frame(&self.doc, self.doc.active).unwrap_or(0);
        let bits = |v: f32| (v.clamp(0.05, 0.8) * 1000.0).round() as u32;
        let pair = if self.anim_assist && self.onion && !self.doc.layers.is_empty() {
            (
                prev_frame(&self.doc, active),
                next_frame(&self.doc, active),
                bits(self.onion_prev),
                bits(self.onion_next),
            )
        } else {
            (None, None, bits(self.onion_prev), bits(self.onion_next))
        };
        if !self.ghost_dirty && pair == self.ghost_key {
            return;
        }
        self.ghost_dirty = false;
        self.ghost_key = pair;
        let (w, h) = (self.doc.w as usize, self.doc.h as usize);
        let load = |ctx: &egui::Context,
                    name: &str,
                    idx: Option<usize>,
                    amount: f32|
         -> Option<TextureHandle> {
            let full = onion_ghost(&self.doc, idx?, amount);
            Some(ctx.load_texture(name, downsample_half(&full, w, h), TextureOptions::LINEAR))
        };
        self.ghost_prev = load(ctx, "ghost-prev", pair.0, self.onion_prev);
        self.ghost_next = load(ctx, "ghost-next", pair.1, self.onion_next);
    }

    fn set_page_color(&mut self, rgb: [u8; 3]) {
        self.doc.background = [rgb[0], rgb[1], rgb[2], 255];
        self.doc.mark_full_dirty();
    }

    /// Mobile: load a resolved file. Mirrors the desktop open arms (kept
    /// inline there so desktop behavior never changes).
    #[cfg(any(target_os = "ios", target_os = "android"))]
    fn open_picked(&mut self, kind: PendingOpen, path: std::path::PathBuf) {
        match kind {
            PendingOpen::Image => match self.doc.load_background(&path) {
                Ok(()) => {
                    self.texture = None;
                    self.composite.invalidate();
                    self.lasso_points.clear();
                    self.lasso_poly.clear();
                    self.status = format!("Opened {}", path.display());
                }
                Err(e) => self.status = format!("Open failed: {e}"),
            },
            PendingOpen::Project => match Document::open_project(&path) {
                Ok(doc) => {
                    let s = path.to_string_lossy().to_string();
                    self.adopt_project(doc, &s, "Project opened");
                }
                Err(e) => self.status = format!("Project open failed: {e}"),
            },
            PendingOpen::Psd => match open_psd(&path) {
                Ok(doc) => {
                    self.doc = doc;
                    self.clear_recovery();
                    self.texture = None;
                    self.composite.invalidate();
                    self.lasso_points.clear();
                    self.lasso_poly.clear();
                    self.status = format!("PSD opened {} (masks do not import)", path.display());
                }
                Err(e) => self.status = format!("PSD open failed: {e}"),
            },
            PendingOpen::Brushes => match load_brush_file_report(&path) {
                Ok(libs) => self.status = self.absorb_brushes(libs, &path),
                Err(e) => self.status = format!("Brush import failed: {e}"),
            },
            PendingOpen::Palette => match load_palette_file(&path) {
                Ok(pal) => self.status = self.absorb_colors(pal, &format!("{}", path.display())),
                Err(e) => self.status = format!("Palette import failed: {e}"),
            },
            PendingOpen::ImagePalette => {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Image")
                    .to_string();
                let from = format!("{}", path.display());
                match image::open(&path).map(|i| i.to_rgba8()) {
                    Ok(img) => {
                        let (w, h) = (img.width(), img.height());
                        self.status = self.image_palette_status(&stem, img.as_raw(), w, h, &from);
                    }
                    Err(e) => self.status = format!("Image open failed: {e}"),
                }
            }
            PendingOpen::Gif => match std::fs::read(&path)
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
            },
            // M34 Reference: pin the image beside the canvas (not a layer).
            PendingOpen::Reference => match image::open(&path).map(|i| i.to_rgba8()) {
                Ok(img) => {
                    let (w, h) = (img.width(), img.height());
                    self.set_reference(img.into_raw(), w, h, &format!("{}", path.display()));
                }
                Err(e) => self.status = format!("Reference load failed: {e}"),
            },
            // M38 Typography: validate an imported face, keep its bytes.
            PendingOpen::Font => match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("font")
                        .to_string();
                    self.status = self.absorb_font(&bytes, &name);
                }
                Err(e) => self.status = format!("Font import failed: {e}"),
            },
            // M39 Brush depth: take the first stamp tip in the file.
            PendingOpen::DualTip => {
                self.status = self.absorb_dual_tip(&path);
            }
        }
    }

    /// Honest brush previews: each preset paints a wavy sample stroke through
    /// the real engine into a thumbnail texture (dotted chalk reads dotted).
    /// Ink follows the theme so cards stay crisp on dark and paper alike.
    /// Built-ins come first, then the user's custom brushes in order.
    fn build_preset_previews(&mut self, ctx: &egui::Context, ink: Color) {
        self.preset_textures.clear();
        let brushes: Vec<Brush> = procraft_engine::presets()
            .into_iter()
            .map(|(_, b)| b)
            .chain(self.custom_brushes.iter().map(|p| p.brush.clone()))
            .collect();
        for (idx, preset) in brushes.into_iter().enumerate() {
            let mut doc = Document::new(144, 36);
            let brush = Brush {
                eraser: false,
                ..preset
            };
            paint_preview_stroke(&mut doc, &brush, ink);
            let img =
                ColorImage::from_rgba_unmultiplied([144, 36], &doc.layers[0].pixels.to_dense());
            self.preset_textures.push(ctx.load_texture(
                format!("brush-{idx}"),
                img,
                TextureOptions::LINEAR,
            ));
        }
    }

    /// Total preview cards currently needed (built-ins + customs).
    fn preview_count(&self) -> usize {
        procraft_engine::presets().len() + self.custom_brushes.len()
    }
}

/// What the mask-view branch must do this frame.
#[derive(Debug, PartialEq)]
enum MaskAction {
    /// Nothing mask-specific: the composite / assist path owns the frame.
    Idle,
    /// The layer has a mask and the view is on: build or patch the mask texture.
    Show,
    /// A mask texture is on screen but no longer applies: drop it once.
    Drop,
}

/// The pure decision behind the mask-view branch of `refresh_texture`.
///
/// Only the mask view itself can drain the dirty rect: on a maskless layer
/// the composite path must still see it (a full-canvas flatten per dab
/// otherwise). A texture that was showing mask pixels is dropped exactly
/// once, whichever way the view leaves that state.
fn mask_action(view_mask: bool, has_mask: bool, mask_shown: bool) -> MaskAction {
    match (view_mask, has_mask) {
        (true, true) => MaskAction::Show,
        (false, true) => MaskAction::Idle,
        _ if mask_shown => MaskAction::Drop,
        _ => MaskAction::Idle,
    }
}

fn brush_preview_path() -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    let mut x = 8.0;
    while x <= 136.0 {
        let y = 18.0 + 8.0 * ((x - 8.0) / 128.0 * std::f32::consts::PI * 2.0).sin();
        points.push((x, y));
        x += 2.0;
    }
    points
}

fn preview_path_progress(points: &[(f32, f32)]) -> Vec<f32> {
    let mut progress = Vec::with_capacity(points.len());
    progress.push(0.0);
    let mut total = 0.0;
    for pair in points.windows(2) {
        let (dx, dy) = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
        total += (dx * dx + dy * dy).sqrt();
        progress.push(total);
    }
    if total > 0.0 {
        for value in &mut progress {
            *value /= total;
        }
    }
    progress
}

fn paint_preview_stroke(doc: &mut Document, brush: &Brush, ink: Color) {
    let points = brush_preview_path();
    let progress = preview_path_progress(&points);
    let mut last = None;
    let mut left = 0.0;
    for (idx, &to) in points.iter().enumerate() {
        let angle = if idx > 0 {
            let from = points[idx - 1];
            (to.1 - from.1).atan2(to.0 - from.0)
        } else {
            let next = points[1];
            (next.1 - to.1).atan2(next.0 - to.0)
        };
        let from_u = if idx == 0 { 0.0 } else { progress[idx - 1] };
        let from_dyn = DabDynamics {
            angle,
            taper: brush.taper.factor_at(from_u),
            ..Default::default()
        };
        let to_dyn = DabDynamics {
            angle,
            taper: brush.taper.factor_at(progress[idx]),
            ..Default::default()
        };
        let (l, lo) = doc.stroke_to_dyn(brush, ink, last, from_dyn, to, to_dyn, left);
        last = l;
        left = lo;
    }
}

/// Dispatch a palette file by extension (shared by the desktop Import arm
/// and the iOS picker resolve path): `.pcpalette`/`.json` or GIMP `.gpl`.
fn load_palette_file(path: &std::path::Path) -> Result<Palette, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "gpl" {
        import_gpl_file(path)
    } else {
        open_palette(path)
    }
}
struct LoadedBrushes {
    brushes: Vec<BrushPreset>,
    warnings: Vec<String>,
    skipped: usize,
}

/// Dispatch a brush file by extension into presets plus any importer
/// warnings (shared by the desktop Import arm and the iOS picker path).
fn load_brush_file_report(path: &std::path::Path) -> Result<LoadedBrushes, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "pcbrush" || ext == "json" {
        open_brush_library(path).map(|libs| LoadedBrushes {
            brushes: libs
                .into_iter()
                .map(|p| BrushPreset {
                    name: p.name,
                    brush: p.brush,
                })
                .collect(),
            warnings: Vec::new(),
            skipped: 0,
        })
    } else {
        import_brush_file_report(path).map(|outcome| LoadedBrushes {
            brushes: outcome
                .brushes
                .into_iter()
                .map(|b| BrushPreset {
                    name: b.name,
                    brush: b.brush,
                })
                .collect(),
            warnings: outcome.warnings,
            skipped: outcome.skipped,
        })
    }
}

fn load_brush_file(path: &std::path::Path) -> Result<Vec<BrushPreset>, String> {
    load_brush_file_report(path).map(|loaded| loaded.brushes)
}

fn brush_import_status(
    added: usize,
    skipped: usize,
    warnings: &[String],
    path: &std::path::Path,
) -> String {
    let mut status = format!("Imported {added} brushes from {}.", path.display());
    if skipped > 0 {
        status.push_str(&format!(" Skipped {skipped} presets."));
    }
    if let Some(first) = warnings.first() {
        status.push(' ');
        status.push_str(first);
        if warnings.len() > 1 {
            status.push_str(&format!(" (+{} more warnings)", warnings.len() - 1));
        }
    }
    status
}

impl ProCraftApp {
    /// Design tokens for the app's current theme and device class.
    fn tokens(&self, ui: &egui::Ui) -> design::Tokens {
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
    fn pill(&self, ui: &mut egui::Ui, label: &str) -> bool {
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
    fn color_dot(&mut self, ui: &mut egui::Ui, t: &design::Tokens) {
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
    fn toggle_pill(&self, ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
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
    fn strip_drag_ui(&mut self, grip: &egui::Response, viewport_w: f32, pad: f32, width: f32) {
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
    /// tool radio list in the Tools panel.
    fn choose_tool(&mut self, tool: Tool) {
        if self.tool == Tool::Fill && tool != Tool::Fill {
            self.fill_preview = None;
        }
        self.tool = tool;
    }

    /// Clear the active layer as one undoable step. Shared by the desktop
    /// top bar and the mobile actions cluster.
    fn clear_layer(&mut self) {
        if self.text_paint_blocked() {
            return;
        }
        self.doc.checkpoint();
        if let Some(layer) = self.doc.active_layer_mut() {
            layer.clear();
        }
        self.doc.mark_full_dirty();
    }

    /// Zoom/rotate/About cluster shared by the wide bar (inline) and the
    /// narrow/tablet bar (inside a View menu). `in_menu` dismisses the popup
    /// after momentary actions, matching the File menu convention; inline
    /// clicks must not close anything.
    fn view_controls(&mut self, ui: &mut egui::Ui, in_menu: bool) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("−").clicked() {
                self.zoom = (self.zoom / 1.25).clamp(0.1, 8.0);
            }
            ui.label(format!("{}%", (self.zoom * 100.0).round() as u32));
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
    fn tools_panel_body(&mut self, ui: &mut egui::Ui, compact: bool) {
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
                                        egui::Image::new((tex.id(), Vec2::new(96.0, 24.0)))
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
                                                egui::Image::new((tex.id(), Vec2::new(96.0, 24.0)))
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
    fn layer_action_row(&mut self, ui: &mut egui::Ui) {
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
    fn next_group_name(&self) -> String {
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
    fn group_header_row(
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

    fn layers_panel_body(&mut self, ui: &mut egui::Ui) {
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
                                let resp = ui
                                    .selectable_label(selected, &layer.name)
                                    .on_hover_text(
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
                                        self.status =
                                            format!("{} layers selected.", self.layer_selection.len());
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

    /// File menu items shared by the top bar and the mobile cluster.
    fn file_menu_ui(
        &mut self,
        ui: &mut egui::Ui,
        layout: BarLayout,
        bar_t: &design::Tokens,
        max_h: Option<f32>,
    ) {
        if layout == BarLayout::Tablet {
            ui.spacing_mut().interact_size.y = bar_t.control_h;
        }
        // A landscape phone cannot show Save/Open/PSD in one popup: cap it
        // above the bar and scroll instead of clipping the last rows.
        let out = egui::ScrollArea::vertical()
            .id_salt("file-menu-scroll")
            .max_height(max_h.unwrap_or(f32::INFINITY))
            .show(ui, |ui| {
                if ui.button("Save PNG").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("PNG", &["png"])
                        .save_file()
                    {
                        // The dialog may return an extensionless path (always
                        // on iOS); the encoder needs the suffix to pick PNG.
                        let path = ensure_extension(path, "png");
                        match self.doc.save_png(&path) {
                            Ok(()) => {
                                self.status = format!("Saved {}", path.display());
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Save failed: {e}"),
                        }
                    }
                }
                stepped_slider(ui, &mut self.jpeg_quality, 1..=100, 1.0, "JPEG q");
                if ui.button("Save JPEG").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("JPEG", &["jpg", "jpeg"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "jpg");
                        match self.doc.save_jpeg(&path, self.jpeg_quality) {
                            Ok(()) => {
                                self.status = format!(
                                    "Saved {} (quality {})",
                                    path.display(),
                                    self.jpeg_quality
                                );
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Save failed: {e}"),
                        }
                    }
                }
                if ui.button("Save TIFF").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TIFF", &["tif", "tiff"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "tiff");
                        match self.doc.save_tiff(&path) {
                            Ok(()) => {
                                self.status = format!("Saved {}", path.display());
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Save failed: {e}"),
                        }
                    }
                }
                if ui.button("Open image").clicked() {
                    ui.close();
                    #[cfg(not(target_os = "ios"))]
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter(
                            "Images",
                            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
                        )
                        .pick_file()
                    {
                        match self.doc.load_background(&path) {
                            Ok(()) => {
                                self.texture = None;
                                self.composite.invalidate();
                                self.lasso_points.clear();
                                self.lasso_poly.clear();
                                self.status = format!("Opened {}", path.display());
                            }
                            Err(e) => self.status = format!("Open failed: {e}"),
                        }
                    }
                    // iOS: presents the picker; open_picked() loads on resolve.
                    #[cfg(target_os = "ios")]
                    {
                        let _ = rfd::FileDialog::new()
                            .add_filter(
                                "Images",
                                &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
                            )
                            .pick_file();
                        self.pending_open = Some(PendingOpen::Image);
                        self.status = "Pick an image…".to_string();
                    }
                    #[cfg(target_os = "android")]
                    {
                        self.android_browse = Some(PendingOpen::Image);
                    }
                }
                ui.separator();
                if ui.button("Save project").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ProCraft project", &["procraft"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "procraft");
                        match self.doc.save_project(&path) {
                            Ok(()) => {
                                let s = path.to_string_lossy().to_string();
                                push_recent(&mut self.recent, &s);
                                self.gallery_keys.clear();
                                self.status = format!("Project saved {}", path.display());
                                self.clear_recovery();
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("Project save failed: {e}"),
                        }
                    }
                }
                if ui.button("Open project").clicked() {
                    ui.close();
                    #[cfg(not(target_os = "ios"))]
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ProCraft project", &["procraft"])
                        .pick_file()
                    {
                        match Document::open_project(&path) {
                            Ok(doc) => {
                                let s = path.to_string_lossy().to_string();
                                self.adopt_project(doc, &s, "Project opened");
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
                ui.separator();
                if ui.button("PSD export").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Photoshop document", &["psd"])
                        .save_file()
                    {
                        let path = ensure_extension(path, "psd");
                        match save_psd(&path, &self.doc) {
                            Ok(()) => {
                                self.status = format!(
                                    "PSD saved {} (masks do not round-trip)",
                                    path.display()
                                );
                                #[cfg(target_os = "ios")]
                                rfd::ios_share_file(&path);
                                #[cfg(target_os = "android")]
                                rfd::android_scan_file(&path);
                            }
                            Err(e) => self.status = format!("PSD save failed: {e}"),
                        }
                    }
                }
                if ui.button("Open PSD").clicked() {
                    ui.close();
                    #[cfg(not(target_os = "ios"))]
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Photoshop document", &["psd"])
                        .pick_file()
                    {
                        match open_psd(&path) {
                            Ok(doc) => {
                                self.doc = doc;
                                self.clear_recovery();
                                self.texture = None;
                                self.composite.invalidate();
                                self.lasso_points.clear();
                                self.lasso_poly.clear();
                                self.status =
                                    format!("PSD opened {} (masks do not import)", path.display());
                            }
                            Err(e) => self.status = format!("PSD open failed: {e}"),
                        }
                    }
                    #[cfg(target_os = "ios")]
                    {
                        let _ = rfd::FileDialog::new()
                            .add_filter("Photoshop document", &["psd"])
                            .pick_file();
                        self.pending_open = Some(PendingOpen::Psd);
                        self.status = "Pick a PSD…".to_string();
                    }
                    #[cfg(target_os = "android")]
                    {
                        self.android_browse = Some(PendingOpen::Psd);
                    }
                }
            });
        let fade = menu_fade_height(
            out.content_size.y,
            out.inner_rect.height(),
            out.state.offset.y,
        );
        if fade > 0.0 {
            fade_bottom_edge(ui, out.inner_rect, ui.visuals().window_fill(), fade);
        }
    }

    /// Top safe inset for the mobile shell: the status bar/notch strip the
    /// canvas must not paint under. Read per frame (rotations change it);
    /// `PROCRAFT_SAFE_TOP` stands in for a device on host captures.
    fn status_inset_top(&mut self) -> f32 {
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
    fn mobile_modal_frame(&mut self, ctx: &egui::Context) -> Option<MobileModal> {
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
    fn mobile_scrim(&mut self, ctx: &egui::Context, id: &str) -> bool {
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
    fn mobile_chrome(&mut self, ctx: &egui::Context, class: BarLayout) {
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

    fn tool_options_bar(&mut self, ui: &mut egui::Ui, layout: BarLayout) {
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

    fn quick_controls_ui(&mut self, ui: &mut egui::Ui, hit: f32, t: &design::Tokens) {
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

    fn quick_control_ui(
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

    fn choose_brush(&mut self, slot: BrushSlot, brush: &Brush) {
        self.brush = brush.clone();
        self.tool = Tool::Brush;
        self.brush_slot = Some(slot);
    }

    fn brush_slot_active(&self, slot: BrushSlot) -> bool {
        slot_is_active(self.brush_slot, slot)
    }

    fn remove_custom_brush(&mut self, index: usize) -> Option<String> {
        if index >= self.custom_brushes.len() {
            return None;
        }
        let name = self.custom_brushes.remove(index).name;
        self.brush_slot = slot_after_remove(self.brush_slot, index);
        Some(name)
    }

    /// Unique display name for a new custom brush: "Foo", "Foo (2)", ...
    fn unique_brush_name(&self, want: &str) -> String {
        let want = want.trim();
        if self.custom_brushes.iter().all(|p| p.name != want) {
            return want.to_string();
        }
        for n in 2..=MAX_LIBRARY_BRUSHES + 2 {
            let cand = format!("{want} ({n})");
            if self.custom_brushes.iter().all(|p| p.name != cand) {
                return cand;
            }
        }
        want.to_string()
    }

    /// Append imported presets with dedup names (shared by desktop Import
    /// and the iOS picker resolve path). Returns the status message.
    fn absorb_brushes(&mut self, loaded: LoadedBrushes, path: &std::path::Path) -> String {
        let LoadedBrushes {
            brushes,
            mut warnings,
            mut skipped,
        } = loaded;
        let mut added = 0;
        let mut capped = 0;
        for p in brushes {
            if self.custom_brushes.len() >= MAX_LIBRARY_BRUSHES {
                capped += 1;
                continue;
            }
            let name = self.unique_brush_name(&p.name);
            self.custom_brushes.push(BrushPreset {
                name,
                brush: p.brush,
            });
            added += 1;
        }
        if capped > 0 {
            skipped += capped;
            warnings.push(format!("Library is full; {capped} brushes were not added."));
        }
        brush_import_status(added, skipped, &warnings, path)
    }

    /// Append an imported palette's swatches to the active palette,
    /// skipping exact duplicates (shared by desktop Import, image/canvas
    /// extraction and the iOS picker resolve path).
    fn absorb_colors(&mut self, pal: Palette, from: &str) -> String {
        let mut added = 0;
        if let Some(active) = self.palettes.active_mut() {
            for c in pal.colors {
                if active.colors.len() >= MAX_PALETTE_COLORS {
                    break;
                }
                if !active.colors.contains(&c) {
                    active.colors.push(c);
                    added += 1;
                }
            }
        }
        format!("Imported {added} swatches from {from}.")
    }

    /// Extract a palette from RGBA pixels and absorb it (shared by the
    /// image-file and canvas arms on both platforms).
    fn image_palette_status(
        &mut self,
        name: &str,
        pixels: &[u8],
        w: u32,
        h: u32,
        from: &str,
    ) -> String {
        match palette_from_rgba(name, pixels, w, h, MAX_IMAGE_COLORS) {
            Ok(pal) => self.absorb_colors(pal, from),
            Err(e) => format!("Palette from image failed: {e}"),
        }
    }

    /// Scrub a filter slider with a live preview: checkpoint + snapshot on
    /// first touch, then restore-and-reapply on change, so the whole
    /// gesture is one undo step and values never compound. `kind` selects
    /// the baseline slot (see [`FilterPreview`]); a changed layer (or
    /// mismatched pixels after undo/layer edits) restarts it. Re-applies
    /// closer than [`PREVIEW_MIN_DT`] are skipped and flag the preview
    /// `stale` — Apply restamps the latest values (see `restamp_preview`).
    /// A preview whose layer changed underneath (paint stroke, undo, layer
    /// op, new/opened doc…) never restores its stale snapshot — that wipe
    /// is exactly "my paint disappeared" — it restarts on current pixels.
    fn filter_scrub(&mut self, kind: u8, apply: impl Fn(&mut Document, usize)) {
        if self.text_paint_blocked() {
            return;
        }
        let layer = self.doc.active;
        // Stale-snapshot guard: the live layer must still match what our
        // last apply left behind (or the snapshot on a fresh preview).
        let expired = match &self.filter_preview {
            Some(fp) => match self.doc.layers.get(fp.layer) {
                Some(l) => l.pixels.fingerprint() != fp.live_hash,
                None => true,
            },
            None => false,
        };
        if expired {
            self.filter_preview = None;
        }
        let fresh = match &self.filter_preview {
            Some(fp) => {
                fp.kind != kind
                    || fp.layer != layer
                    || self
                        .doc
                        .layers
                        .get(layer)
                        .is_none_or(|l| l.pixels.len() != fp.pixels.len())
            }
            None => true,
        };
        if fresh {
            self.doc.checkpoint();
            match self.doc.layers.get(layer) {
                Some(l) => {
                    let live_hash = l.pixels.fingerprint();
                    self.filter_preview = Some(FilterPreview {
                        kind,
                        layer,
                        pixels: l.pixels.clone(),
                        last: std::time::Instant::now() - PREVIEW_MIN_DT,
                        stale: false,
                        live_hash,
                    });
                }
                None => {
                    self.filter_preview = None;
                    return;
                }
            }
        }
        if let Some(mut fp) = self.filter_preview.take() {
            if self.doc.layers.get(fp.layer).is_none() {
                return;
            }
            // Throttle: the layer keeps the last preview; Apply catches up.
            if !fresh && fp.last.elapsed() < PREVIEW_MIN_DT {
                fp.stale = true;
                self.filter_preview = Some(fp);
                return;
            }
            if let Some(l) = self.doc.layers.get_mut(fp.layer) {
                l.pixels.clone_from(&fp.pixels);
            }
            apply(&mut self.doc, fp.layer);
            if let Some(l) = self.doc.layers.get(fp.layer) {
                fp.live_hash = l.pixels.fingerprint();
            }
            fp.last = std::time::Instant::now();
            fp.stale = false;
            self.filter_preview = Some(fp);
        }
    }

    /// Re-apply current slider values over the scrub snapshot (for a
    /// throttled-stale preview at Apply time, or to refresh a current
    /// one). The scrub's checkpoint still covers undo — no new step.
    /// If the layer moved on underneath (see `filter_scrub`), the stale
    /// preview is dropped and the values apply fresh on the active layer
    /// (new undo step) — the intervening work is always kept.
    fn restamp_preview(&mut self, apply: impl Fn(&mut Document, usize)) {
        if self.text_paint_blocked() {
            return;
        }
        let expired = match &self.filter_preview {
            Some(fp) => match self.doc.layers.get(fp.layer) {
                Some(l) => l.pixels.fingerprint() != fp.live_hash,
                None => true,
            },
            None => return,
        };
        if expired {
            self.filter_preview = None;
            self.doc.checkpoint();
            let layer = self.doc.active;
            if self.doc.layers.get(layer).is_some() {
                apply(&mut self.doc, layer);
            }
            return;
        }
        if let Some(mut fp) = self.filter_preview.take() {
            if self.doc.layers.get(fp.layer).is_some() {
                if let Some(l) = self.doc.layers.get_mut(fp.layer) {
                    l.pixels.clone_from(&fp.pixels);
                }
                apply(&mut self.doc, fp.layer);
                if let Some(l) = self.doc.layers.get(fp.layer) {
                    fp.live_hash = l.pixels.fingerprint();
                }
            }
            self.filter_preview = Some(fp);
        }
    }

    /// Time-lapse card above the Animation group: the recording switch sits
    /// one tap from the Layers panel instead of three taps deep inside the
    /// group (plan 12). Recording is on by default.
    fn lapse_card(&mut self, ui: &mut egui::Ui, t: &design::Tokens) {
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
    fn export_lapse(&mut self) {
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
        }
    }

    /// Export the visible layers as a GIF.
    fn gif_export_ui(&mut self, ui: &mut egui::Ui) {
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
        }
    }

    /// Import a GIF as the document (one layer per frame).
    fn gif_import_ui(&mut self, ui: &mut egui::Ui) {
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
    fn capture_timelapse(&mut self) {
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
    fn adopt_gif(&mut self, doc: Document, fps: u32) {
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

    /// Best-effort delete of the autosave recovery file (after an explicit
    /// save, load, or clean exit — anything that makes it stale).
    fn clear_recovery(&self) {
        let _ = std::fs::remove_file(&self.recovery_path);
    }

    /// Draw the eyedropper loupe for canvas point `(cx, cy)` (already
    /// clamped): magnified composite ring parked above the pointer (below
    /// near the top edge, shifted inside near the sides) with the
    /// would-be picked color read out.
    fn paint_picker_loupe(
        &self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        pos: egui::Pos2,
        cx: f32,
        cy: f32,
    ) {
        const SIDE: u32 = 11;
        const CELL: f32 = 12.0;
        let size = SIDE as f32 * CELL;
        let (cells, ci, cj) = loupe_cells(&self.doc, cx as u32, cy as u32, SIDE);
        let center = cells[(cj * SIDE + ci) as usize];
        let screen = ctx.viewport_rect();
        let mut at = egui::Pos2::new(pos.x + 24.0, pos.y - size - 48.0);
        if at.y < screen.min.y + 8.0 {
            at.y = pos.y + 28.0;
        }
        if at.x + size + 12.0 > screen.max.x - 8.0 {
            at.x = screen.max.x - 8.0 - size - 12.0;
        }
        if at.x < screen.min.x + 8.0 {
            at.x = screen.min.x + 8.0;
        }
        paint_loupe(
            painter,
            &cells,
            SIDE,
            (ci, cj),
            at,
            CELL,
            [center[0], center[1], center[2]],
        );
    }

    /// Reset view-side state after the canvas changes size (crop, canvas
    /// resize): the texture rebuilds at the new dims, ghosts and mask
    /// edges recompute, transient drags die, and the size panel re-adopts
    /// the real dimensions.
    fn after_canvas_change(&mut self) {
        self.texture = None;
        self.composite.invalidate();
        self.ghost_dirty = true;
        self.sel_edges_dirty = true;
        self.lasso_poly.clear();
        self.crop_rect = None;
        self.crop_anchor = None;
        self.crop_current = None;
        self.move_from = None;
        self.liquify_from = None;
        // M48 pins are canvas-sized state: a new/resized canvas
        // invalidates them (they were seeded at the old dimensions).
        self.xform = XformState::off();
        // Same for the Transform-layer button session base.
        self.tform_press = TformPress::new();
        self.canvas_sync = (self.doc.w, self.doc.h);
        self.canvas_w = self.doc.w;
        self.canvas_h = self.doc.h;
    }

    /// M33 Gallery: adopt an opened/saved/duplicated project — one place
    /// for the reset + recent + status so every entry path behaves alike.
    fn adopt_project(&mut self, doc: Document, path: &str, verb: &str) {
        self.doc = doc;
        self.clear_recovery();
        push_recent(&mut self.recent, path);
        self.after_canvas_change();
        self.gallery_keys.clear();
        self.status = format!("{verb} {path}");
    }

    /// M33 Gallery: start a blank canvas at `w` x `h` (clamped to the
    /// engine cap). No recent entry until it is saved somewhere.
    fn new_canvas(&mut self, w: u32, h: u32) {
        let w = w.clamp(1, MAX_CANVAS_SIDE);
        let h = h.clamp(1, MAX_CANVAS_SIDE);
        self.doc = Document::new(w, h);
        self.clear_recovery();
        self.after_canvas_change();
        self.status = format!("New canvas {w} x {h}. Time-lapse records under Animation.");
    }

    /// M33 Gallery: rebuild cached thumbnails when the recent list
    /// changed. Each path is opened, flattened, and downscaled; missing
    /// or corrupt files yield `None` (placeholder row, kept so the user
    /// can Remove it). Runs on demand, not per frame.
    fn refresh_gallery_thumbs(&mut self, ctx: &egui::Context) {
        if self.gallery_keys == self.recent {
            return;
        }
        self.gallery_thumbs.clear();
        for (idx, path) in self.recent.iter().enumerate() {
            let tex = Document::open_project(std::path::Path::new(path))
                .ok()
                .and_then(|doc| {
                    let (bytes, tw, th) = sampled_thumb(&doc, GALLERY_THUMB_SIDE);
                    if bytes.is_empty() {
                        return None;
                    }
                    let img = ColorImage::from_rgba_unmultiplied([tw, th], &bytes);
                    Some(ctx.load_texture(format!("gallery-{idx}"), img, TextureOptions::LINEAR))
                });
            self.gallery_thumbs.push(tex);
        }
        self.gallery_keys = self.recent.clone();
    }

    /// M33 Gallery: copy `recent[idx]` to a sibling ` copyN.procraft`
    /// file and adopt it. Errors surface in the status line.
    fn duplicate_recent(&mut self, idx: usize) {
        let Some(src) = self.recent.get(idx).cloned() else {
            return;
        };
        let src_path = std::path::PathBuf::from(&src);
        let doc = match Document::open_project(&src_path) {
            Ok(d) => d,
            Err(e) => {
                self.status = format!("Duplicate failed to open {src}: {e}");
                return;
            }
        };
        let stem = src_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("artwork");
        // Strip an older " copyN" tail so re-duplicates don't stack names.
        let base = stem.rsplit_once(" copy").map(|(b, _)| b).unwrap_or(stem);
        let mut dest = src_path.clone();
        let mut n = 1u32;
        loop {
            let name = if n == 1 {
                format!("{base} copy.procraft")
            } else {
                format!("{base} copy{n}.procraft")
            };
            dest.set_file_name(&name);
            if !dest.exists() {
                break;
            }
            n += 1;
            if n > 999 {
                self.status = "Duplicate failed: too many copies.".to_string();
                return;
            }
        }
        match doc.save_project(&dest) {
            Ok(()) => {
                let s = dest.to_string_lossy().to_string();
                self.adopt_project(doc, &s, "Duplicated");
            }
            Err(e) => self.status = format!("Duplicate save failed: {e}"),
        }
    }

    /// M34 Reference: pin RGBA bytes as the companion image (session-only;
    /// never touches the canvas or undo). Oversized images are downscaled
    /// through the Gallery helper so a phone photo can't OOM the session.
    fn set_reference(&mut self, pixels: Vec<u8>, w: u32, h: u32, from: &str) {
        if pixels.is_empty() || w == 0 || h == 0 {
            self.status = format!("Reference load failed: empty image ({from}).");
            return;
        }
        const MAX_SIDE: u32 = 1024;
        let (bytes, tw, th) = if w.max(h) > MAX_SIDE {
            gallery_thumb(&pixels, w, h, MAX_SIDE)
        } else {
            (pixels, w, h)
        };
        if bytes.is_empty() {
            self.status = format!("Reference load failed: bad pixels ({from}).");
            return;
        }
        self.ref_image = Some((bytes, tw, th));
        self.ref_tex = None;
        self.ref_open = true;
        self.status = format!("Reference pinned ({from}).");
    }

    /// M34 Reference: upload the companion texture on demand (load/replace
    /// only, never per frame).
    fn refresh_ref_tex(&mut self, ctx: &egui::Context) {
        if self.ref_tex.is_none() {
            if let Some((bytes, w, h)) = &self.ref_image {
                let img = ColorImage::from_rgba_unmultiplied([*w as usize, *h as usize], bytes);
                self.ref_tex = Some(ctx.load_texture("reference", img, TextureOptions::LINEAR));
            }
        }
    }

    /// M38 Typography: validate imported TTF/OTF bytes and keep them as
    /// the session face (shared by desktop Import and the iOS resolve).
    fn absorb_font(&mut self, bytes: &[u8], name: &str) -> String {
        if bytes.len() > MAX_FONT_BYTES {
            return "Font too large (max 2 MB).".to_string();
        }
        match check_font(bytes) {
            Ok(()) => {
                self.text_font_name = Some(name.to_string());
                self.text_font_bytes = Some(bytes.to_vec());
                format!("Font {name} ready.")
            }
            Err(e) => format!("Font {name} failed: {e}"),
        }
    }

    /// Shared guard for pixel-writing entry points: a live text layer's
    /// pixels are engine-owned (repainted from the block on every edit), so
    /// writers refuse and point at Rasterize instead of silently fighting
    /// the next re-render (or losing the edit).
    fn text_paint_blocked(&mut self) -> bool {
        if self
            .doc
            .layers
            .get(self.doc.active)
            .is_some_and(|l| l.is_text())
        {
            self.status = "Text layer — tap Rasterize (Layers) to paint.".to_string();
            true
        } else {
            false
        }
    }

    /// M39 Brush depth: take the first stamp tip in a brush file as the
    /// current brush's dual tip (shared by desktop Import and iOS resolve).
    fn absorb_dual_tip(&mut self, path: &std::path::Path) -> String {
        match load_brush_file(path) {
            Ok(libs) => match libs.into_iter().find_map(|p| p.brush.tip) {
                Some(tip) => {
                    let (w, h) = (tip.w, tip.h);
                    self.brush.dual_tip = Some(tip);
                    format!("Dual tip set ({w}x{h}).")
                }
                None => "No stamp tip in that file.".to_string(),
            },
            Err(e) => format!("Dual import failed: {e}"),
        }
    }

    /// M38 Typography: the current dialog settings as an engine style.
    fn text_style(&self) -> TextStyle {
        TextStyle {
            font_bytes: self.text_font_bytes.clone(),
            leading: self.text_leading,
            tracking: self.text_tracking,
            outline: self.text_outline.then(|| {
                (
                    Color::rgb(
                        self.text_outline_rgb[0],
                        self.text_outline_rgb[1],
                        self.text_outline_rgb[2],
                    ),
                    self.text_outline_w,
                )
            }),
        }
    }
}

/// Screen position of a vanishing-point handle: the true point when it is
/// on-canvas, otherwise clamped to the canvas edge (with a margin) so
/// off-canvas VPs — like the 2-point defaults — stay visible and grabbable.
/// Returns the position plus whether it was clamped.
fn vp_handle_pos(rect: egui::Rect, z: f32, vx: f32, vy: f32) -> (egui::Pos2, bool) {
    let p = egui::Pos2::new(rect.min.x + vx * z, rect.min.y + vy * z);
    if rect.contains(p) {
        return (p, false);
    }
    // Clamp into the rect with a finger-sized margin; degenerate tiny
    // rects fall back to the center instead of panicking in `clamp`.
    let m = 14.0;
    let (lo_x, hi_x) = (rect.min.x + m, rect.max.x - m);
    let (lo_y, hi_y) = (rect.min.y + m, rect.max.y - m);
    let cx = if lo_x <= hi_x {
        p.x.clamp(lo_x, hi_x)
    } else {
        (rect.min.x + rect.max.x) * 0.5
    };
    let cy = if lo_y <= hi_y {
        p.y.clamp(lo_y, hi_y)
    } else {
        (rect.min.y + rect.max.y) * 0.5
    };
    (egui::Pos2::new(cx, cy), true)
}

/// Slider with −/+ step buttons flush around it: one compact control
/// (`[-]bar[+] value name`) instead of widgets spread across the row.
/// `stepped_slider` puts the name first; `stepped_slider_label_end` (the
/// Filters convention) leaves value + name trailing. `step` is in value
/// units, endpoints clamp. Rows wrap on narrow screens; the step buttons
/// are 28x20.
const SLIDER_W: f32 = 100.0;

fn stepped_slider<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    step: f64,
    text: &str,
) {
    stepped_slider_inner(ui, value, range, step, text, false);
}

fn stepped_slider_label_end<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    step: f64,
    text: &str,
) {
    stepped_slider_inner(ui, value, range, step, text, true);
}

fn stepped_slider_inner<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    step: f64,
    text: &str,
    label_end: bool,
) {
    let lo = range.start().to_f64();
    let hi = range.end().to_f64();
    // Trailing readout: value first, then the name; a % in the name
    // fuses onto the value ("100%") instead of floating on its own.
    let v = value.to_f64();
    let num = if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    };
    ui.horizontal_wrapped(|ui| {
        if !label_end {
            ui.label(text);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            if ui.add_sized([28.0, 20.0], egui::Button::new("−")).clicked() {
                *value = Num::from_f64((value.to_f64() - step).clamp(lo, hi));
            }
            ui.add_sized(
                egui::Vec2::new(SLIDER_W, 20.0),
                egui::Slider::new(value, range).show_value(false),
            );
            if ui.add_sized([28.0, 20.0], egui::Button::new("+")).clicked() {
                *value = Num::from_f64((value.to_f64() + step).clamp(lo, hi));
            }
        });
        if text.contains('%') {
            ui.label(format!("{num}%"));
            let name = text.replace('%', "");
            let name = name.trim();
            if !name.is_empty() {
                ui.label(name);
            }
        } else {
            ui.label(num);
            if label_end {
                ui.label(text);
            }
        }
    });
}

/// Horizontal-bar slider: "Name value [−][rail][+]" in one run, sized for
/// the quick bar. The rail is the thick B2 primitive; the numeric type stays
/// generic so callers keep their `i32`/`f32` fields.
fn quick_slider<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    step: f64,
    text: &str,
    t: &design::Tokens,
) {
    let lo = range.start().to_f64() as f32;
    let hi = range.end().to_f64() as f32;
    let mut v = value.to_f64() as f32;
    // Double-click resets to the low end, matching the old stock slider's
    // "no default" behaviour without inventing a per-control default.
    let default = lo;
    design::thick_slider(ui, t, &mut v, lo..=hi, step, text, default);
    *value = Num::from_f64(v as f64);
}

/// Swatch side and gutter in logical px. The gutter is 4.0 (not 3.0): at
/// 125% Windows scaling 4 pt becomes 5 physical px, so every cell lands on a
/// whole pixel instead of a half one.
const SWATCH: f32 = 24.0;
const SWATCH_GAP: f32 = 4.0;

/// Swatch grid at full height (never an inner scroller — the panel's own
/// scroll handles overflow, and separate scrollers with no explicit ids
/// share one scroll offset). Wrap width is pinned to 8 swatches per row
/// so Starter always lands 3x8 at any panel width.
fn swatch_grid(ui: &mut egui::Ui, mut add_contents: impl FnMut(&mut egui::Ui)) {
    ui.scope(|ui| {
        ui.set_max_width(8.0 * (SWATCH + SWATCH_GAP));
        add_contents(ui);
    });
}

/// One clickable swatch: applies the color. Returns the response for
/// right-click-to-remove handling by the caller.
///
/// Painted by hand rather than as a `Button`: the control radius is 8–12 pt
/// on touch layouts, which turns a 24 pt swatch into a soft blob. Here the
/// corner is a fixed 4 pt and the rect is snapped to the physical pixel grid,
/// so the colour block stays crisp at any DPI.
fn swatch_button(ui: &mut egui::Ui, rgb: [u8; 3], selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SWATCH), Sense::click());
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let r = egui::Rect::from_min_max(
        egui::pos2(snap(rect.min.x), snap(rect.min.y)),
        egui::pos2(snap(rect.max.x), snap(rect.max.y)),
    );
    let corner = egui::CornerRadius::same(4);
    // The theme's accent marks hover/selection; the neutral stroke keeps
    // pale swatches visible on pale surfaces.
    let ring = if selected || response.hovered() {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().widgets.inactive.bg_stroke.color
    };
    let painter = ui.painter();
    painter.rect_filled(r, corner, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    painter.rect_stroke(
        r,
        corner,
        egui::Stroke::new(if selected { 2.0 } else { 1.0 }, ring),
        egui::StrokeKind::Inside,
    );
    response
}

/// Box-downsample straight RGBA to half resolution for cheap onion-ghost
/// overlays (the canvas rect stretches them back; ghosts are soft anyway).
fn downsample_half(full: &[u8], w: usize, h: usize) -> ColorImage {
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    let mut px = vec![0u8; hw * hh * 4];
    for y in 0..hh {
        for x in 0..hw {
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                    if sx < w && sy < h {
                        let o = (sy * w + sx) * 4;
                        if o + 3 < full.len() {
                            for c in 0..4 {
                                acc[c] += full[o + c] as u32;
                            }
                            n += 1;
                        }
                    }
                }
            }
            let o = (y * hw + x) * 4;
            for c in 0..4 {
                px[o + c] = acc[c].checked_div(n).unwrap_or(0) as u8;
            }
        }
    }
    ColorImage::from_rgba_unmultiplied([hw, hh], &px)
}

/// Rotate a screen point about a center (view rotation).
fn rot_pt(p: egui::Pos2, c: egui::Pos2, ang: f32) -> egui::Pos2 {
    let (s, co) = ang.sin_cos();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    egui::Pos2::new(c.x + dx * co - dy * s, c.y + dx * s + dy * co)
}

/// Inverse of [`rot_pt`] (screen back to the unrotated view).
fn unrot_pt(p: egui::Pos2, c: egui::Pos2, ang: f32) -> egui::Pos2 {
    rot_pt(p, c, -ang)
}

/// Workspace dot grid spacing in canvas px. Steps snap to powers of two so
/// the dots land on the same points as the zoom crosses an octave.
const BOARD_DOT_STEP: f32 = 32.0;
const BOARD_DOT_SCREEN_MIN: f32 = 24.0;
const BOARD_DOT_SCREEN_MAX: f32 = 48.0;
/// Breathing room the fit zoom leaves around the page so the board stays
/// visible instead of the sheet pinning itself to the viewport edge.
const BOARD_FIT_MARGIN: f32 = 28.0;

fn board_fit_zoom(w: f32, h: f32, avail: egui::Vec2) -> f32 {
    let (w, h) = (w.max(1.0), h.max(1.0));
    ((avail.x - 2.0 * BOARD_FIT_MARGIN) / w)
        .min((avail.y - 2.0 * BOARD_FIT_MARGIN) / h)
        .clamp(0.05, 8.0)
}

/// Offset that centres content smaller than the viewport, leaving the board
/// visible on both sides (the page floats in the middle instead of pinning
/// to the top-left corner). Zero when the content overflows, so a zoomed-in
/// page keeps the viewport origin.
fn center_offset(content: egui::Vec2, avail: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        ((avail.x - content.x) * 0.5).max(0.0),
        ((avail.y - content.y) * 0.5).max(0.0),
    )
}

fn board_dot_step(zoom: f32) -> f32 {
    let zoom = zoom.max(0.05);
    let mut step = BOARD_DOT_STEP;
    while step * zoom < BOARD_DOT_SCREEN_MIN {
        step *= 2.0;
    }
    while step * zoom > BOARD_DOT_SCREEN_MAX {
        step *= 0.5;
    }
    step
}

/// Inclusive index range of grid lines (`i * step`) inside `lo..=hi`.
fn board_dot_span(step: f32, lo: f32, hi: f32) -> (i32, i32) {
    ((lo / step).ceil() as i32, (hi / step).floor() as i32)
}

/// Canvas-fixed dots on the workspace board. They pan and rotate with the
/// page and are skipped wherever the page covers them, so the artwork and
/// its texture stay untouched.
fn paint_board_dots(
    painter: &egui::Painter,
    view: egui::Rect,
    page: egui::Rect,
    zoom: f32,
    ang: f32,
    color: egui::Color32,
) {
    if !view.is_positive() || !page.is_positive() {
        return;
    }
    let step = board_dot_step(zoom);
    let center = page.center();
    let to_canvas = |p: egui::Pos2| {
        let q = unrot_pt(p, center, ang);
        ((q.x - page.min.x) / zoom, (q.y - page.min.y) / zoom)
    };
    let mut x0 = f32::INFINITY;
    let mut y0 = f32::INFINITY;
    let mut x1 = f32::NEG_INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for p in [
        view.left_top(),
        view.right_top(),
        view.right_bottom(),
        view.left_bottom(),
    ] {
        let (x, y) = to_canvas(p);
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    let (i0, i1) = board_dot_span(step, x0, x1);
    let (j0, j1) = board_dot_span(step, y0, y1);
    if i1 < i0 || j1 < j0 {
        return;
    }
    let (pw, ph) = (page.width() / zoom, page.height() / zoom);
    for i in i0..=i1 {
        let x = i as f32 * step;
        for j in j0..=j1 {
            let y = j as f32 * step;
            if x >= 0.0 && x <= pw && y >= 0.0 && y <= ph {
                continue;
            }
            let p = rot_pt(
                egui::pos2(page.min.x + x * zoom, page.min.y + y * zoom),
                center,
                ang,
            );
            if view.contains(p) {
                painter.circle_filled(p, 1.15, color);
            }
        }
    }
}

/// Brush cursor ring radius in screen px: nominal dab diameter scaled by
/// zoom, floored so 1px brushes stay visible. Hover has no pressure, so
/// this is the base size; dynamics spread real dabs around it.
fn brush_ring_radius(size: f32, zoom: f32) -> f32 {
    (size * zoom * 0.5).max(2.0)
}

/// Size for the cursor ring, from the size the tool actually uses.
/// `None` for tools without a size ring.
fn tool_ring_size(tool: Tool, brush_size: f32, liquify_size: f32) -> Option<f32> {
    match tool {
        Tool::Brush | Tool::Eraser | Tool::Smudge => Some(brush_size),
        Tool::Liquify => Some(liquify_size),
        _ => None,
    }
}

/// Split one pointer segment into warp pushes no longer than `max_step`,
/// so a fast drag warps the whole segment in this frame. One capped step
/// per frame could never catch up with the cursor: the warp trailed the
/// pointer and the unwarped tail was dropped on release. The 64-push
/// ceiling keeps a pathological one-frame jump from stalling the frame.
fn liquify_pushes(
    from: (f32, f32),
    to: (f32, f32),
    max_step: f32,
) -> Vec<((f32, f32), (f32, f32))> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return Vec::new();
    }
    let steps = (len / max_step.max(1.0)).ceil().clamp(1.0, 64.0);
    let mut pushes = Vec::with_capacity(steps as usize);
    for i in 1..=steps as u32 {
        let t = i as f32 / steps;
        let t0 = (i - 1) as f32 / steps;
        let (sx, sy) = (dx * (t - t0), dy * (t - t0));
        pushes.push(((from.0 + dx * t, from.1 + dy * t), (sx, sy)));
    }
    pushes
}

/// Top-bar width era. Below [`WIDE_BAR_W`] the zoom/rotate/theme/About
/// group folds into a View menu (phone portrait starves the inline
/// stepper to a character-wide column); mobile at/above it is Tablet
/// (same fold, finger-sized controls); desktop stays unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BarLayout {
    Wide,
    Narrow,
    Tablet,
}

const WIDE_BAR_W: f32 = 600.0;

fn bar_layout(width: f32, mobile: bool) -> BarLayout {
    if width < WIDE_BAR_W {
        BarLayout::Narrow
    } else if mobile {
        BarLayout::Tablet
    } else {
        BarLayout::Wide
    }
}

/// A dev-only device-class override so a desktop build can be screenshotted
/// in the touch layouts. `bar_layout` stays pure; everything that asks for a
/// class goes through [`resolve_layout`] instead.
fn forced_layout() -> Option<BarLayout> {
    match std::env::var("PROCRAFT_FORCE_LAYOUT").ok()?.as_str() {
        "wide" => Some(BarLayout::Wide),
        "tablet" => Some(BarLayout::Tablet),
        "narrow" => Some(BarLayout::Narrow),
        _ => None,
    }
}

fn resolve_layout(forced: Option<BarLayout>, width: f32, mobile: bool) -> BarLayout {
    forced.unwrap_or_else(|| bar_layout(width, mobile))
}

fn effective_layout(width: f32, mobile: bool) -> BarLayout {
    resolve_layout(forced_layout(), width, mobile)
}

/// Presentation shell: desktop keeps the docked chrome, mobile floats it over
/// a full-screen canvas. Native mobile is the only automatic source; the env
/// override exists so host builds can be screenshotted in the touch shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shell {
    Desktop,
    Mobile,
}

fn forced_shell() -> Option<bool> {
    match std::env::var("PROCRAFT_FORCE_SHELL").ok()?.as_str() {
        "mobile" => Some(true),
        "desktop" => Some(false),
        _ => None,
    }
}

fn resolve_shell(forced: Option<bool>, native_mobile: bool) -> Shell {
    if forced.unwrap_or(native_mobile) {
        Shell::Mobile
    } else {
        Shell::Desktop
    }
}

fn mobile_shell() -> bool {
    resolve_shell(
        forced_shell(),
        cfg!(any(target_os = "ios", target_os = "android")),
    ) == Shell::Mobile
}

/// Which slide-over is open in the mobile shell. Session-only: the desktop
/// dock keeps owning the persisted `show_tools`/`show_layers` flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MobileDrawer {
    None,
    Tools,
    Layers,
}

impl MobileDrawer {
    /// Tapping the open drawer's trigger closes it; any other trigger swaps.
    fn toggled(self, other: MobileDrawer) -> MobileDrawer {
        if self == other {
            MobileDrawer::None
        } else {
            other
        }
    }
}

/// Horizontal anchor of the compact phone strip. The full bar is always
/// edge-to-edge; the pocket pill snaps to one of the bottom thirds so it can
/// live where the thumb reaches without covering the canvas middle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StripDock {
    Left,
    Center,
    Right,
}

impl StripDock {
    fn as_str(self) -> &'static str {
        match self {
            StripDock::Left => "left",
            StripDock::Center => "center",
            StripDock::Right => "right",
        }
    }

    fn from_str(s: &str) -> StripDock {
        match s {
            "left" => StripDock::Left,
            "right" => StripDock::Right,
            _ => StripDock::Center,
        }
    }
}

/// Drawer width on touch screens: wide enough for the two-column cards,
/// narrow enough to leave a visible strip of canvas and the scrim.
fn mobile_drawer_width(viewport_w: f32) -> f32 {
    (viewport_w * 0.88).clamp(240.0, 360.0)
}

/// Phones give the drawer slightly more room: there is no top-left island
/// to peek around.
fn phone_drawer_width(viewport_w: f32) -> f32 {
    (viewport_w * 0.92).clamp(240.0, 360.0)
}

/// Paint tools that stay one tap away in the phone strip.
const PHONE_STRIP_TOOLS: [Tool; 3] = [Tool::Brush, Tool::Eraser, Tool::Smudge];

/// Columns for the tools panel: the mobile drawer packs two, and three when
/// the drawer is shot (landscape) so every tool fits without scrolling; the
/// desktop dock is always a list.
fn tool_grid_columns(width: f32, compact: bool, mobile: bool) -> usize {
    if !mobile || width < 300.0 {
        1
    } else if compact {
        3
    } else {
        2
    }
}

/// Bottom safe inset in points. iOS reports the home indicator; Android
/// hosts keep 0 until the platform bridge exposes one.
fn safe_bottom(mobile: bool) -> f32 {
    if !mobile {
        return 0.0;
    }
    #[cfg(target_os = "ios")]
    {
        let (_, bottom) = rfd::ios_safe_area_insets().unwrap_or((0, 0));
        clamp_status_inset(bottom)
    }
    #[cfg(not(target_os = "ios"))]
    {
        0.0
    }
}

/// `PROCRAFT_SAFE_TOP` (points, clamped like a real inset) so host builds can
/// be captured with the same status-bar strip a device reserves.
fn forced_safe_top() -> Option<f32> {
    safe_inset_override(std::env::var("PROCRAFT_SAFE_TOP").ok().as_deref())
}

fn safe_inset_override(value: Option<&str>) -> Option<f32> {
    value?.trim().parse::<u32>().ok().map(clamp_status_inset)
}

/// Y offset (negative, for a bottom anchor) of the phone strip.
fn phone_strip_offset(safe_bottom: f32, pad: f32) -> f32 {
    -(safe_bottom + pad)
}

/// Y offset (negative, for a bottom anchor) of the options capsule: above
/// the strip and its gap on phones, directly above the inset elsewhere.
fn phone_capsule_offset(safe_bottom: f32, pad: f32, strip_h: f32, gap: f32) -> f32 {
    -(safe_bottom + pad + strip_h + gap)
}

/// Preferred width of a minimised bar (the phone's pocket pill and the
/// tablet's folded options capsule): grip, active tool, expand.
const PHONE_MINI_STRIP_W: f32 = 260.0;

/// Width of the drag grip: a dot column with a finger's slack on both sides.
const STRIP_GRIP_W: f32 = 24.0;

fn strip_mini_width(viewport_w: f32, pad: f32) -> f32 {
    (viewport_w - 2.0 * pad).clamp(180.0, PHONE_MINI_STRIP_W)
}

/// Widest left edge that still keeps a bar on screen.
fn strip_max_left(viewport_w: f32, pad: f32, width: f32) -> f32 {
    (viewport_w - pad - width).max(pad)
}

/// Left edge of a bar for a docked position.
fn strip_left(dock: StripDock, viewport_w: f32, pad: f32, width: f32) -> f32 {
    let max_left = strip_max_left(viewport_w, pad, width);
    match dock {
        StripDock::Left => pad,
        StripDock::Center => ((viewport_w - width) * 0.5).clamp(pad, max_left),
        StripDock::Right => max_left,
    }
}

/// Keep a dragged bar fully on screen.
fn strip_left_clamped(left: f32, viewport_w: f32, pad: f32, width: f32) -> f32 {
    left.clamp(pad, strip_max_left(viewport_w, pad, width))
}

/// Where a bar sits this frame: its dock plus any in-flight drag, clamped.
fn strip_left_now(dock: StripDock, offset: f32, viewport_w: f32, pad: f32, width: f32) -> f32 {
    strip_left_clamped(
        strip_left(dock, viewport_w, pad, width) + offset,
        viewport_w,
        pad,
        width,
    )
}

/// Nearest dock for a bar whose left edge is `left`: compare the bar's
/// centre against the three docked centres, so a wide bar still snaps to
/// its nearest side instead of always landing in the middle.
fn strip_dock_for(left: f32, viewport_w: f32, pad: f32, width: f32) -> StripDock {
    let c = left + width * 0.5;
    let mut best = (StripDock::Center, f32::INFINITY);
    for dock in [StripDock::Left, StripDock::Center, StripDock::Right] {
        let d = (strip_left(dock, viewport_w, pad, width) + width * 0.5 - c).abs();
        if d < best.1 {
            best = (dock, d);
        }
    }
    best.0
}

/// Drag handle for the pocket bars: two columns of painted dots. Painted,
/// not typeset, so it survives the iOS font stack like the other icons.
fn strip_grip(ui: &mut egui::Ui, t: &design::Tokens, h: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(STRIP_GRIP_W, h), egui::Sense::click_and_drag());
    for row in 0..3 {
        for col in 0..2 {
            let p = rect.center() + egui::vec2((col as f32 - 0.5) * 5.0, (row as f32 - 1.0) * 6.0);
            ui.painter().circle_filled(p, 1.5, t.text_lo);
        }
    }
    response
}

/// Vertical room available on a phone: safe insets, edge padding, the strip
/// and the floating-island height used by the capsule.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PhoneRoom {
    safe_top: f32,
    safe_bottom: f32,
    pad: f32,
    strip_h: f32,
    island_h: f32,
    gap: f32,
}

/// Below this span the phone drawer drops its strip gap and runs to the
/// bottom inset, so landscape keeps a usable panel height.
const DRAWER_TIGHT_SPAN: f32 = 320.0;

/// Drawer window `(top, height)` on phones: no top island, bottom clamped
/// above the strip unless the drawer is too short to fit there.
fn phone_drawer_span(viewport_bottom: f32, room: PhoneRoom) -> (f32, f32) {
    let top = room.safe_top + room.pad;
    let above_strip = viewport_bottom - room.safe_bottom - room.pad - room.strip_h - room.gap;
    // A short (landscape) drawer keeps the strip's room only when that still
    // leaves a usable panel; otherwise it runs to the bottom inset and covers
    // the strip while open — a slide-over is an overlay, not a dock.
    let full = viewport_bottom - room.safe_bottom - room.pad;
    let bottom = if above_strip - top >= DRAWER_TIGHT_SPAN {
        above_strip
    } else {
        full
    };
    (top, (bottom - top).max(200.0))
}

/// Options capsule width: nearly the whole canvas on phones, capped so it
/// stays a floating island on wide tablets.
fn mobile_capsule_width(viewport_w: f32) -> f32 {
    (viewport_w * 0.94).min(760.0)
}

/// Height of a floating cluster or capsule: one control row plus the
/// frame's 6 pt inner margin on both sides.
fn mobile_island_height(control_h: f32) -> f32 {
    control_h + 12.0
}

/// Drawer window `(top, height)`: starts below the floating clusters and
/// ends above the options capsule, so no drawer control hides under the
/// islands.
fn mobile_drawer_span(
    viewport_bottom: f32,
    safe_top: f32,
    safe_bottom: f32,
    pad: f32,
    island_h: f32,
    gap: f32,
    has_capsule: bool,
) -> (f32, f32) {
    let top = safe_top + pad + island_h + gap;
    let bottom =
        viewport_bottom - safe_bottom - pad - if has_capsule { island_h + gap } else { 0.0 };
    (top, (bottom - top).max(200.0))
}

/// Mobile edge padding (Android status-bar px / iOS safe-area points)
/// clamped so a wild value can never eat the screen. Host builds reach it
/// through `PROCRAFT_SAFE_TOP` and tests.
fn clamp_status_inset(px: u32) -> f32 {
    (px as f32).clamp(0.0, 200.0)
}

/// Status toast timing: long enough to read one line, short enough that it
/// is gone before the next stroke.
const TOAST_HOLD: f32 = 2.5;
const TOAST_FADE: f32 = 0.4;

/// Toast opacity `age` seconds after the status changed.
fn toast_alpha(age: f32) -> f32 {
    if age < 0.0 {
        return 0.0;
    }
    if age <= TOAST_HOLD {
        return 1.0;
    }
    ((TOAST_HOLD + TOAST_FADE - age) / TOAST_FADE).clamp(0.0, 1.0)
}

/// Y offset of the toast: stacked above the options capsule, or in the
/// capsule's place when the active tool has no options to show.
fn toast_offset(capsule_offset: f32, island_h: f32, gap: f32, has_capsule: bool) -> f32 {
    capsule_offset - if has_capsule { island_h + gap } else { 0.0 }
}

/// How far a drawer still has to travel: fully past the screen edge at 0,
/// flush with it at 1.
fn drawer_slide_distance(progress: f32, width: f32, pad: f32) -> f32 {
    (1.0 - progress.clamp(0.0, 1.0)) * (width + 2.0 * pad)
}

/// Mobile dialogs: at most 92% of the viewport in either axis, and short
/// enough that centring it still clears the status-bar strip. Desktop
/// windows keep their own sizing.
/// How a mobile sheet is presented: fitted size, the anchor that centres it
/// in the canvas area above the bottom chrome, and the drawer card frame.
struct MobileModal {
    fit: egui::Vec2,
    anchor: egui::Vec2,
    frame: egui::Frame,
}

fn modal_fit(
    viewport: egui::Vec2,
    safe_top: f32,
    safe_bottom: f32,
    mobile: bool,
) -> Option<egui::Vec2> {
    if !mobile {
        return None;
    }
    let insets = 2.0 * (safe_top.max(0.0) + safe_bottom.max(0.0) + 12.0);
    Some(egui::vec2(
        (viewport.x * 0.92).max(240.0),
        (viewport.y * 0.92).min(viewport.y - insets).max(200.0),
    ))
}

/// Vertical anchor offset (from the viewport centre) that centres a sheet in
/// the free canvas area: below the status inset and top islands, above the
/// phone strip or the tablet options capsule. Falls back to dead centre when
/// that area is too short to be a fair target.
fn mobile_modal_anchor(
    viewport: egui::Rect,
    safe_top: f32,
    safe_bottom: f32,
    class: BarLayout,
    island_h: f32,
    gap: f32,
    has_capsule: bool,
) -> egui::Vec2 {
    let pad = 12.0;
    let (free_top, free_bottom) = if class == BarLayout::Narrow {
        (
            safe_top + pad,
            viewport.bottom() - safe_bottom - pad - island_h - gap,
        )
    } else {
        (
            safe_top + pad + island_h + gap,
            viewport.bottom() - safe_bottom - pad - if has_capsule { island_h + gap } else { 0.0 },
        )
    };
    if free_bottom - free_top < 240.0 {
        return egui::Vec2::ZERO;
    }
    egui::vec2(0.0, (free_top + free_bottom) * 0.5 - viewport.center().y)
}

/// Apply the mobile sheet treatment to a dialog; desktop windows keep their
/// own size, position and frame.
fn fitted_modal<'a>(win: egui::Window<'a>, sheet: Option<&MobileModal>) -> egui::Window<'a> {
    match sheet {
        Some(s) => win
            .max_width(s.fit.x)
            .anchor(egui::Align2::CENTER_CENTER, s.anchor)
            .frame(s.frame),
        None => win,
    }
}

/// Mobile dialog body: scrolls within the fitted height so content never
/// clips below the fold. Desktop bodies stay unwrapped.
fn modal_scroll(ui: &mut egui::Ui, sheet: Option<&MobileModal>, body: impl FnOnce(&mut egui::Ui)) {
    let Some(s) = sheet else {
        return body(ui);
    };
    egui::ScrollArea::vertical()
        .max_height((s.fit.y - 56.0).max(160.0))
        .show(ui, body);
}

/// Cap for the File menu popup's scroll area: touch shells only, so on a
/// landscape phone the tail of the menu scrolls into reach instead of
/// running off the bottom edge.
/// Room for the File popup body between the bar it hangs from and the
/// opposite edge. Derived from bar geometry, never from the popup's own
/// rect: a cap that moved with the popup made it grow every frame.
fn menu_popup_room(from: f32, to: f32) -> f32 {
    (to - from - 16.0).max(200.0)
}

/// Fade height for a scrollable popup body: nothing when the list already
/// ends in view, otherwise a short gradient so the last row reads as
/// continuing rather than clipped.
fn menu_fade_height(content_h: f32, viewport_h: f32, offset_y: f32) -> f32 {
    let below = content_h - viewport_h - offset_y;
    if below <= 1.0 {
        0.0
    } else {
        MENU_FADE_H.min(viewport_h * 0.25)
    }
}

const MENU_FADE_H: f32 = 24.0;

/// Paint `rect`'s bottom `height` points as a gradient into `fill`, over the
/// rows that are only half in view.
fn fade_bottom_edge(ui: &egui::Ui, rect: egui::Rect, fill: egui::Color32, height: f32) {
    let clear = egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 0);
    let (left, right) = (rect.left() + 1.0, rect.right() - 1.0);
    let top = rect.bottom() - height;
    let mut mesh = egui::epaint::Mesh::default();
    for (pos, color) in [
        (egui::pos2(left, top), clear),
        (egui::pos2(right, top), clear),
        (egui::pos2(right, rect.bottom()), fill),
        (egui::pos2(left, rect.bottom()), fill),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(egui::Shape::mesh(mesh));
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum XformMode {
    Off,
    Distort,
    Skew,
    Perspective,
    Free,
    Mesh,
    Puppet,
}

impl XformMode {
    fn all() -> [XformMode; 7] {
        [
            XformMode::Off,
            XformMode::Distort,
            XformMode::Skew,
            XformMode::Perspective,
            XformMode::Free,
            XformMode::Mesh,
            XformMode::Puppet,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            XformMode::Off => "Off",
            XformMode::Distort => "Distort",
            XformMode::Skew => "Skew",
            XformMode::Perspective => "Perspective",
            XformMode::Free => "Free",
            XformMode::Mesh => "Mesh",
            XformMode::Puppet => "Puppet",
        }
    }

    /// Corner-pin modes share one representation: four frame pins that
    /// the engine samples through a triangulated mesh (plus Puppet's
    /// extra interior pins and Free's move/rotate gestures).
    fn is_pins(self) -> bool {
        matches!(
            self,
            XformMode::Distort
                | XformMode::Skew
                | XformMode::Perspective
                | XformMode::Free
                | XformMode::Puppet
        )
    }
}

fn xform_mode_status(mode: XformMode) -> &'static str {
    match mode {
        XformMode::Off => "Transform handles off — painting resumes.",
        XformMode::Distort => "Distort armed — drag a corner pin to bend the drawing.",
        XformMode::Skew => "Skew armed — drag a corner to slide that edge; the drag axis locks.",
        XformMode::Perspective => "Perspective armed — drag a corner; its partner mirrors.",
        XformMode::Free => {
            "Free transform armed — drag inside to move, a corner to scale, the top handle to rotate."
        }
        XformMode::Mesh => "Mesh lattice armed — drag a lattice point to warp.",
        XformMode::Puppet => {
            "Puppet warp armed — click to add, Shift-click or box-select to select, \
             drag pins to move, Alt/right-click to delete."
        }
    }
}

/// Mesh lattice size bounds (interior cells per axis; points = cells + 1).
const XFORM_MESH_MIN: u32 = 1;
const XFORM_MESH_MAX: u32 = 8;
const XFORM_MESH_DEFAULT: u32 = 2;

/// Maximum extra margin around a Puppet frame for thin-line warps.
const XFORM_EXPAND_MAX: f32 = 128.0;

/// Puppet warp pin budget (frame pins included).
const XFORM_PIN_MAX: usize = 24;

/// Minimum half-extent (canvas px) a Free-mode scale keeps so the box
/// never collapses to a line.
const XFORM_MIN_EXTENT: f32 = 2.0;

/// Corner order for every pin mode: [TL, TR, BR, BL], with each corner's
/// unit signs about the box center (mirrored pairs flip one sign).
const XFORM_CORNER_SIGNS: [(f32, f32); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];

fn xform_anchors(w: f32, h: f32) -> [(f32, f32); 4] {
    [
        (0.0, 0.0),
        (w - 1.0, 0.0),
        (w - 1.0, h - 1.0),
        (0.0, h - 1.0),
    ]
}

/// Canvas anchors not already covered by a frame pin (a content frame
/// that touches the canvas edge sits on the anchor itself). Skipping
/// duplicates keeps the Delaunay mesh free of zero-area triangles while
/// still pinning the whole canvas.
fn xform_extra_anchors(pins: &[(f32, f32)], w: f32, h: f32) -> Vec<(f32, f32)> {
    xform_anchors(w, h)
        .into_iter()
        .filter(|a| {
            !pins
                .iter()
                .any(|p| (p.0 - a.0).abs() < 0.5 && (p.1 - a.1).abs() < 0.5)
        })
        .collect()
}

/// Transform frame: the active layer's opaque bbox padded a little and
/// clamped to the canvas, so handles wrap the drawing instead of the
/// whole canvas (empty layers fall back to the full canvas).
fn xform_bounds(doc: &Document, w: f32, h: f32, margin: f32) -> (f32, f32, f32, f32) {
    let pad = margin;
    match doc.content_bounds(doc.active) {
        Some((x0, y0, x1, y1)) => (
            (x0 - pad).clamp(0.0, w - 1.0),
            (y0 - pad).clamp(0.0, h - 1.0),
            (x1 + pad).clamp(0.0, w - 1.0),
            (y1 + pad).clamp(0.0, h - 1.0),
        ),
        None => (0.0, 0.0, w - 1.0, h - 1.0),
    }
}

fn xform_bounds_corners(b: (f32, f32, f32, f32)) -> [(f32, f32); 4] {
    [(b.0, b.1), (b.2, b.1), (b.2, b.3), (b.0, b.3)]
}

fn xform_unit(v: (f32, f32)) -> (f32, f32) {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len < 1e-4 {
        (1.0, 0.0)
    } else {
        (v.0 / len, v.1 / len)
    }
}

fn xform_points_center(points: &[(f32, f32)]) -> (f32, f32) {
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let (sx, sy) = points
        .iter()
        .fold((0.0f32, 0.0f32), |(ax, ay), &(x, y)| (ax + x, ay + y));
    (sx / points.len() as f32, sy / points.len() as f32)
}

/// Lock a drag to its dominant axis (Shift while moving).
fn constrain_axis(dx: f32, dy: f32) -> (f32, f32) {
    if dx.abs() >= dy.abs() {
        (dx, 0.0)
    } else {
        (0.0, dy)
    }
}

/// Quantize an angle in degrees to 15° steps (Shift while rotating).
fn snap_angle(deg: f32) -> f32 {
    const STEP: f32 = 15.0;
    (deg / STEP).round() * STEP
}

/// Half extents of the four-corner frame: (half width, half height).
fn frame_extents(points: &[(f32, f32)]) -> (f32, f32) {
    if points.len() != 4 {
        return (1.0, 1.0);
    }
    let side =
        |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).hypot(b.1 - a.1) * 0.5).max(XFORM_MIN_EXTENT);
    (side(points[0], points[1]), side(points[0], points[3]))
}

/// Lattice lines for one axis: the canvas edges, the bounds edges, and
/// `cells` evenly spread lines across the bounds. The bounds line shares
/// its row, so the outside cells only ever move transparent paint (the
/// bounds wrap the content); near-duplicates collapse.
fn xform_axis_lines(a: f32, b: f32, cells: u32, max: f32) -> Vec<f32> {
    let cells = cells.clamp(XFORM_MESH_MIN, XFORM_MESH_MAX);
    let mut v = vec![0.0f32];
    if a > 0.5 {
        v.push(a);
    }
    for i in 1..cells {
        v.push(a + (b - a) * i as f32 / cells as f32);
    }
    if b < max - 0.5 {
        v.push(b);
    }
    v.push(max);
    v.sort_by(f32::total_cmp);
    v.dedup_by(|x, y| (*x - *y).abs() < 0.5);
    v
}

/// Bounds-hugging warp lattice plus its total cell counts for the engine
/// call. Point count is `(cols+1) * (rows+1)` over the returned grid.
fn xform_bounds_grid(
    b: (f32, f32, f32, f32),
    w: f32,
    h: f32,
    cols: u32,
    rows: u32,
) -> (Vec<(f32, f32)>, u32, u32) {
    let xs = xform_axis_lines(b.0, b.2, cols, w - 1.0);
    let ys = xform_axis_lines(b.1, b.3, rows, h - 1.0);
    let mut grid = Vec::with_capacity(xs.len() * ys.len());
    for &y in &ys {
        for &x in &xs {
            grid.push((x, y));
        }
    }
    (grid, xs.len() as u32 - 1, ys.len() as u32 - 1)
}

/// Bowyer-Watson Delaunay triangulation (f64 predicates; the point
/// counts here are tiny). Returns index triples into `points`; degenerate
/// (zero-area) triangles are dropped. Empty below three points.
fn xform_delaunay(points: &[(f32, f32)]) -> Vec<[u32; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(n + 3);
    for &(x, y) in points {
        let (x, y) = (x as f64, y as f64);
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
        pts.push((x, y));
    }
    let d = ((maxx - minx).max(maxy - miny).max(1.0)) * 20.0;
    let cx = (minx + maxx) * 0.5;
    let cy = (miny + maxy) * 0.5;
    pts.push((cx - d, cy - d));
    pts.push((cx + d, cy - d));
    pts.push((cx, cy + d));
    let mut tris: Vec<[usize; 3]> = vec![[n, n + 1, n + 2]];
    for i in 0..n {
        let p = pts[i];
        let mut edges: Vec<(usize, usize)> = Vec::new();
        let mut keep: Vec<[usize; 3]> = Vec::new();
        for t in &tris {
            if xform_in_circumcircle(p, pts[t[0]], pts[t[1]], pts[t[2]]) {
                for e in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    if let Some(pos) = edges.iter().position(|&x| x == (e.1, e.0)) {
                        edges.swap_remove(pos);
                    } else {
                        edges.push(e);
                    }
                }
            } else {
                keep.push(*t);
            }
        }
        for (a, b) in edges {
            keep.push([a, b, i]);
        }
        tris = keep;
    }
    tris.retain(|t| {
        t.iter().all(|&i| i < n)
            && t[0] != t[1]
            && t[1] != t[2]
            && t[0] != t[2]
            && xform_tri_area(pts[t[0]], pts[t[1]], pts[t[2]]).abs() > 1e-6
    });
    tris.into_iter()
        .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
        .collect()
}

fn xform_in_circumcircle(p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    let (ax, ay) = (a.0 - p.0, a.1 - p.1);
    let (bx, by) = (b.0 - p.0, b.1 - p.1);
    let (cx, cy) = (c.0 - p.0, c.1 - p.1);
    let det = (ax * ax + ay * ay) * (bx * cy - cx * by) - (bx * bx + by * by) * (ax * cy - cx * ay)
        + (cx * cx + cy * cy) * (ax * by - bx * ay);
    det > 0.0
}

fn xform_tri_area(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)
}

/// A mesh is usable while every triangle keeps a real area (a collapsed
/// triangle has no sane inverse map) and all indices resolve.
fn xform_pins_ok(points: &[(f32, f32)], tris: &[[u32; 3]]) -> bool {
    !tris.is_empty()
        && tris.iter().all(|t| {
            t.iter().all(|&i| (i as usize) < points.len())
                && t[0] != t[1]
                && t[1] != t[2]
                && t[0] != t[2]
        })
        && tris.iter().all(|t| {
            let (a, b, c) = (
                points[t[0] as usize],
                points[t[1] as usize],
                points[t[2] as usize],
            );
            ((b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)).abs() > 1e-3
        })
}

/// Gaussian elimination with partial pivoting (f64 for stability).
/// `None` when a pivot collapses below tolerance: the pin configuration
/// is degenerate (overlapping pins) and no sane transform exists.
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    if a.len() != n || a.iter().any(|row| row.len() != n) {
        return None;
    }
    for col in 0..n {
        let piv = (col..n).max_by(|&r1, &r2| a[r1][col].abs().total_cmp(&a[r2][col].abs()))?;
        if a[piv][col].abs() < 1e-9 {
            return None;
        }
        if piv != col {
            a.swap(piv, col);
            b.swap(piv, col);
        }
        let d = a[col][col];
        let pivot_row = a[col].clone();
        for r in col + 1..n {
            let f = a[r][col] / d;
            for (c, pv) in pivot_row.iter().enumerate() {
                a[r][c] -= f * pv;
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0f64; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for c in r + 1..n {
            s -= a[r][c] * x[c];
        }
        x[r] = s / a[r][r];
    }
    Some(x)
}

/// Inverse solve for the warp lattice. Weights mirror
/// `Document::warp_buffer` exactly (gx = x/w * cols, clamped); `pins`
/// and `from` are row-major `(cols+1) * (rows+1)` grids. `None` on
/// degenerate (collapsed) pins or on a malformed grid.
fn pull_grid(
    pins: &[(f32, f32)],
    from: &[(f32, f32)],
    w: f32,
    h: f32,
    cols: u32,
    rows: u32,
) -> Option<Vec<(f32, f32)>> {
    let n = ((cols + 1) * (rows + 1)) as usize;
    if w < 2.0 || h < 2.0 || cols < 1 || rows < 1 || pins.len() != n || from.len() != n {
        return None;
    }
    let stride = cols as usize + 1;
    let mut a = vec![vec![0.0f64; n]; n];
    for (row, &(px, py)) in pins.iter().enumerate() {
        let gx = (px / w * cols as f32).clamp(0.0, cols as f32 - 1e-6) as f64;
        let gy = (py / h * rows as f32).clamp(0.0, rows as f32 - 1e-6) as f64;
        let i = gx.floor() as usize;
        let j = gy.floor() as usize;
        let (fu, fv) = (gx - i as f64, gy - j as f64);
        let (iu, iv) = (1.0 - fu, 1.0 - fv);
        a[row][j * stride + i] = iu * iv;
        a[row][j * stride + i + 1] = fu * iv;
        a[row][(j + 1) * stride + i] = iu * fv;
        a[row][(j + 1) * stride + i + 1] = fu * fv;
    }
    let bx = solve_linear(a.clone(), from.iter().map(|p| p.0 as f64).collect())?;
    let by = solve_linear(a, from.iter().map(|p| p.1 as f64).collect())?;
    Some(
        bx.into_iter()
            .zip(by)
            .map(|(x, y)| (x as f32, y as f32))
            .collect(),
    )
}

/// Result of a live transform preview tick (see [`xform_preview`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum XformApply {
    /// Handles off (or nothing to map): nothing was written.
    NoOp,
    /// The layer now shows the transform previewed from the session base.
    Applied,
    /// Pins sit back on the base positions: pristine pixels were restored.
    Restored,
    /// The pins collapsed (solve failed): the last good preview stays.
    Skipped,
}

/// Live-preview session base: the active layer's pixels and mask as they
/// were when the transform session started, plus the pin positions they
/// correspond to and the document revision at capture. Every preview
/// recomputes from this base, so dragging shows live feedback and
/// repeated back-and-forth gestures never compound resampling blur or
/// permanently lose content pushed off-canvas. Dropped by re-arming,
/// re-captured whenever the document or layer changes behind it.
struct XformBase {
    pixels: Raster,
    mask: Option<Raster>,
    layer: usize,
    pins: Vec<(f32, f32)>,
    /// Canvas anchors the base topology was built with (pins already
    /// sitting on an anchor are not repeated).
    anchors: Vec<(f32, f32)>,
    /// Triangle topology the base pins belong to (pin modes); empty for
    /// the lattice path.
    tris: Vec<[u32; 3]>,
    rev: u64,
}

/// Recompute the active layer from the session base with the pins where
/// they are now: the one path that writes transform pixels. Captures the
/// base lazily on the first write (while the pixels still correspond to
/// the gesture-start pins), restores pristine pixels when the pins land
/// back on the base, and re-captures instead of warping a stale base
/// after external edits (painting, undo, filters) or a layer switch.
fn xform_preview(doc: &mut Document, x: &mut XformState) -> XformApply {
    if x.mode == XformMode::Off || x.points().is_empty() {
        return XformApply::NoOp;
    }
    let active = doc.active;
    // The pins only describe the pixels they were armed against. When
    // undo, a filter, a resize or a layer switch changes those pixels
    // behind the session's back, re-arm around the current content
    // instead of warping the new pixels through stale pin geometry
    // (which can map the whole layer off-canvas and look like lost paint).
    let rearm = match x.base.as_ref() {
        Some(b) => b.layer != active || b.rev != doc.revision(),
        None => x.session_rev != doc.revision() || x.session_layer != active,
    };
    if rearm {
        let mode = x.mode;
        x.arm(mode, doc);
        return XformApply::NoOp;
    }
    if x.base.is_none() {
        let pins = if x.grab.is_some() {
            x.grab_from.clone()
        } else {
            x.points().to_vec()
        };
        let pins = if pins.len() == x.points().len() {
            pins
        } else {
            x.points().to_vec()
        };
        let layer = &doc.layers[active];
        x.base = Some(XformBase {
            pixels: layer.pixels.clone(),
            mask: layer.mask.clone(),
            layer: active,
            pins,
            anchors: x.mesh_anchors.clone(),
            tris: x.tris.clone(),
            rev: doc.revision(),
        });
    }
    let base = x.base.as_ref().expect("base captured above");
    let base_pins = base.pins.clone();
    let base_anchors = base.anchors.clone();
    let base_tris = base.tris.clone();
    if x.points() == base_pins.as_slice() {
        // Pins back on the base: hand back the pristine pixels (also the
        // cancel path), nothing to resample.
        {
            let b = x.base.as_ref().expect("base captured above");
            let layer = &mut doc.layers[active];
            layer.pixels.clone_from(&b.pixels);
            layer.mask.clone_from(&b.mask);
        }
        doc.mark_full_dirty();
        if let Some(b) = x.base.as_mut() {
            b.rev = doc.revision();
        }
        return XformApply::Restored;
    }
    let (w, h) = (doc.w as f32, doc.h as f32);
    enum Warp {
        Mesh(Vec<(f32, f32)>, u32, u32),
        Pins(Vec<(f32, f32)>, Vec<(f32, f32)>, Vec<[u32; 3]>),
    }
    let warp = match x.mode {
        XformMode::Mesh => match pull_grid(&x.grid, &base_pins, w, h, x.grid_cols, x.grid_rows) {
            Some(g) => Warp::Mesh(g, x.grid_cols, x.grid_rows),
            None => return XformApply::Skipped,
        },
        XformMode::Off => return XformApply::NoOp,
        _ => {
            let src: Vec<(f32, f32)> = base_pins
                .iter()
                .copied()
                .chain(base_anchors.iter().copied())
                .collect();
            let dst: Vec<(f32, f32)> = x
                .pins
                .iter()
                .copied()
                .chain(base_anchors.iter().copied())
                .collect();
            if !xform_pins_ok(&dst, &base_tris) {
                return XformApply::Skipped;
            }
            Warp::Pins(src, dst, base_tris.clone())
        }
    };
    // One undo step per gesture, pushed before its first pixel write
    // (tap-grabs that never move never checkpoint).
    if !x.checkpointed {
        doc.checkpoint();
        x.checkpointed = true;
    }
    // Swap the pristine base in, then let the engine warp it: the result
    // always comes from the base, never from a previous preview.
    {
        let b = x.base.as_ref().expect("base captured above");
        let layer = &mut doc.layers[active];
        layer.pixels.clone_from(&b.pixels);
        layer.mask.clone_from(&b.mask);
    }
    match warp {
        Warp::Mesh(g, cols, rows) => doc.warp_layer(active, cols, rows, &g),
        Warp::Pins(src, dst, tris) => doc.warp_layer_tris(active, &src, &dst, &tris),
    }
    if let Some(b) = x.base.as_mut() {
        b.rev = doc.revision();
    }
    XformApply::Applied
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grab {
    Pin(usize),
    /// Move every currently selected Puppet pin together.
    Selected,
    All,
    Rotate,
}

/// CSP-style "Click pin" tool property: clicking a Puppet pin either
/// selects it or deletes it outright.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PinTool {
    Select,
    Delete,
}

impl PinTool {
    fn label(self) -> &'static str {
        match self {
            Self::Select => "Select pin",
            Self::Delete => "Delete pin",
        }
    }
}

/// What a click on a pin does when it is already selected (or not).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PinSelect {
    Replace,
    Add,
    Subtract,
    Toggle,
}

impl PinSelect {
    fn label(self) -> &'static str {
        match self {
            Self::Replace => "Replace selection",
            Self::Add => "Add to selection",
            Self::Subtract => "Subtract from selection",
            Self::Toggle => "Toggle selection",
        }
    }
}

/// Armed transform state. Pins drag on the canvas; every tick previews
/// the transform live from the session base (see [`xform_preview`]), so
/// repeated gestures never compound resampling loss and release keeps
/// whatever the preview showed.
struct XformState {
    mode: XformMode,
    /// Frame pins [TL, TR, BR, BL] plus any Puppet pins the user adds.
    pins: Vec<(f32, f32)>,
    /// Canvas anchors appended to `pins` when the mesh was built.
    mesh_anchors: Vec<(f32, f32)>,
    /// Triangle topology over `pins` followed by `mesh_anchors`. Rebuilt
    /// only when pins are added/removed (Delaunay on base positions),
    /// never per frame.
    tris: Vec<[u32; 3]>,
    /// Content bounds captured when the mode was armed: the frame pins
    /// start on these corners and the mesh lattice hugs them.
    bounds: Option<(f32, f32, f32, f32)>,
    /// Mesh lattice points (canvas edges + bounds-bounded interior).
    grid: Vec<(f32, f32)>,
    /// Extra margin (canvas px) around the content bounds for thin
    /// strokes that need more reference space, as in CSP's mesh expand.
    expand: f32,
    /// Interior cell counts picked from the panel (1..=8 per axis).
    mesh_cols: u32,
    mesh_rows: u32,
    /// Total lattice cell counts for the current grid (includes the
    /// canvas-edge cells outside the content bounds).
    grid_cols: u32,
    grid_rows: u32,
    grab: Option<Grab>,
    /// Pointer position (canvas coords) where the current grab began:
    /// drags move the pin by (pointer now - grab_at), so a long-press
    /// with no movement never writes a phantom position.
    grab_at: (f32, f32),
    /// Points as they were when the current grab started; a release
    /// applies only when they differ (drag-away-and-back included),
    /// cancel restores them, and the preview maps pins back onto them so
    /// content keeps following the pin across gestures.
    grab_from: Vec<(f32, f32)>,
    /// Skew drag axis for the current gesture: `Some(true)` slides a row
    /// horizontally, `Some(false)` slides a column vertically.
    skew_lock: Option<bool>,
    /// Most recently added/selected Puppet pin (the orange ring pin).
    active_pin: Option<usize>,
    /// Puppet pins selected for a group move. Indices match `pins`.
    selected_pins: Vec<usize>,
    /// CSP tool-property state for Puppet pin clicks.
    pin_tool: PinTool,
    pin_select: PinSelect,
    /// Live marquee in canvas coordinates (start, current) while the
    /// pointer drags from outside the mesh.
    marquee: Option<((f32, f32), (f32, f32))>,
    /// Live-preview session base (captured lazily; see [`xform_preview`]).
    base: Option<XformBase>,
    /// Document revision and active layer when the session was armed.
    /// Used before the first base capture, when there is no base revision
    /// to compare against, so undo/layer changes still discard stale pins.
    session_rev: u64,
    session_layer: usize,
    /// Whether the current gesture already pushed its undo checkpoint.
    checkpointed: bool,
}

impl XformState {
    fn off() -> Self {
        Self {
            mode: XformMode::Off,
            pins: Vec::new(),
            mesh_anchors: Vec::new(),
            tris: Vec::new(),
            bounds: None,
            grid: Vec::new(),
            expand: 0.0,
            mesh_cols: XFORM_MESH_DEFAULT,
            mesh_rows: XFORM_MESH_DEFAULT,
            grid_cols: 1,
            grid_rows: 1,
            grab: None,
            grab_at: (0.0, 0.0),
            grab_from: Vec::new(),
            skew_lock: None,
            active_pin: None,
            selected_pins: Vec::new(),
            pin_tool: PinTool::Select,
            pin_select: PinSelect::Replace,
            marquee: None,
            base: None,
            session_rev: 0,
            session_layer: 0,
            checkpointed: false,
        }
    }

    fn points(&self) -> &[(f32, f32)] {
        match self.mode {
            XformMode::Mesh => &self.grid,
            XformMode::Off => &[],
            _ => &self.pins,
        }
    }

    fn points_mut(&mut self) -> &mut [(f32, f32)] {
        match self.mode {
            XformMode::Mesh => &mut self.grid,
            XformMode::Off => &mut [],
            _ => &mut self.pins,
        }
    }

    fn grabbed_pin(&self) -> Option<usize> {
        match self.grab {
            Some(Grab::Pin(i)) => Some(i),
            _ => None,
        }
    }

    fn pin_selected(&self, i: usize) -> bool {
        self.selected_pins.contains(&i)
    }

    fn toggle_pin_selection(&mut self, i: usize) {
        if let Some(pos) = self.selected_pins.iter().position(|&s| s == i) {
            self.selected_pins.remove(pos);
        } else {
            self.selected_pins.push(i);
            self.selected_pins.sort_unstable();
        }
    }

    /// Apply the panel's click-select rule. Returns whether the pin is
    /// selected afterwards; an unselected pin must not begin a drag.
    fn click_pin(&mut self, i: usize, shift: bool) -> bool {
        if i >= self.pins.len() {
            return false;
        }
        if shift {
            self.toggle_pin_selection(i);
        } else {
            match self.pin_select {
                PinSelect::Replace => {
                    self.selected_pins.clear();
                    self.selected_pins.push(i);
                }
                PinSelect::Add => {
                    if !self.pin_selected(i) {
                        self.selected_pins.push(i);
                        self.selected_pins.sort_unstable();
                    }
                }
                PinSelect::Subtract => {
                    self.selected_pins.retain(|&s| s != i);
                }
                PinSelect::Toggle => self.toggle_pin_selection(i),
            }
        }
        self.post_selection_active(i);
        self.pin_selected(i)
    }

    /// Make an unselected pin part of the drag selection for the modes
    /// where dragging implies selecting it. Toggle/Subtract wait for the
    /// click so a drag never silently changes those selections.
    fn prepare_pin_drag(&mut self, i: usize, shift: bool) {
        if self.mode != XformMode::Puppet || i >= self.pins.len() {
            return;
        }
        self.active_pin = Some(i);
        // Shift-click decides on release; pre-selecting here would make
        // the click toggle the pin straight back off.
        if shift {
            return;
        }
        if !self.pin_selected(i) && matches!(self.pin_select, PinSelect::Replace | PinSelect::Add) {
            if self.pin_select == PinSelect::Replace {
                self.selected_pins.clear();
            }
            self.selected_pins.push(i);
            self.selected_pins.sort_unstable();
        }
    }

    fn post_selection_active(&mut self, i: usize) {
        if self.pin_selected(i) {
            self.active_pin = Some(i);
        } else {
            self.active_pin = self.selected_pins.last().copied();
        }
    }

    /// Finish a marquee: Shift adds to the current selection; otherwise
    /// the panel's Select pins rule decides.
    fn marquee_select(&mut self, shift: bool) {
        let Some(((ax, ay), (bx, by))) = self.marquee.take() else {
            return;
        };
        let (x0, x1) = (ax.min(bx), ax.max(bx));
        let (y0, y1) = (ay.min(by), ay.max(by));
        let hits: Vec<usize> = self
            .pins
            .iter()
            .enumerate()
            .filter(|(_, &(x, y))| x >= x0 && x <= x1 && y >= y0 && y <= y1)
            .map(|(i, _)| i)
            .collect();
        if shift || self.pin_select == PinSelect::Add {
            for i in hits {
                if !self.pin_selected(i) {
                    self.selected_pins.push(i);
                }
            }
            self.selected_pins.sort_unstable();
        } else {
            match self.pin_select {
                PinSelect::Subtract => self.selected_pins.retain(|i| !hits.contains(i)),
                PinSelect::Toggle => {
                    for i in hits {
                        self.toggle_pin_selection(i);
                    }
                }
                PinSelect::Replace | PinSelect::Add => self.selected_pins = hits,
            }
        }
        self.active_pin = self.selected_pins.last().copied();
    }

    fn begin_grab(&mut self, i: usize, at: (f32, f32)) {
        self.grab = Some(Grab::Pin(i));
        self.grab_at = at;
        self.grab_from = self.points().to_vec();
        self.checkpointed = false;
        self.skew_lock = None;
        if self.mode == XformMode::Puppet {
            self.active_pin = Some(i);
        }
    }

    fn begin_selected(&mut self, at: (f32, f32)) {
        if self.selected_pins.is_empty() {
            return;
        }
        self.grab = Some(Grab::Selected);
        self.grab_at = at;
        self.grab_from = self.points().to_vec();
        self.checkpointed = false;
        self.skew_lock = None;
    }

    fn begin_all(&mut self, at: (f32, f32)) {
        self.grab = Some(Grab::All);
        self.grab_at = at;
        self.grab_from = self.points().to_vec();
        self.checkpointed = false;
        self.skew_lock = None;
    }

    fn begin_rotate(&mut self, at: (f32, f32)) {
        self.grab = Some(Grab::Rotate);
        self.grab_at = at;
        self.grab_from = self.points().to_vec();
        self.checkpointed = false;
        self.skew_lock = None;
    }

    /// Move the grabbed pin (or gesture) by the pointer's displacement
    /// since the grab started (never the absolute pointer position: a
    /// long-press with no movement must not relocate the pin), with the
    /// live modifier keys: Shift locks the aspect on a Free corner, snaps
    /// Free rotation to 15° steps and constrains free moves to one axis;
    /// Alt scales a Free corner about the frame center instead of the
    /// opposite corner.
    fn drag_by_mod(&mut self, dx: f32, dy: f32, shift: bool, alt: bool) {
        let Some(g) = self.grab else {
            return;
        };
        match self.mode {
            XformMode::Off => {}
            XformMode::Mesh => {
                if let Grab::Pin(i) = g {
                    let (dx, dy) = if shift {
                        constrain_axis(dx, dy)
                    } else {
                        (dx, dy)
                    };
                    if let (Some(from), Some(slot)) =
                        (self.grab_from.get(i).copied(), self.grid.get_mut(i))
                    {
                        *slot = (from.0 + dx, from.1 + dy);
                    }
                }
            }
            XformMode::Distort | XformMode::Puppet => match g {
                Grab::Pin(i) => {
                    let (dx, dy) = if shift {
                        constrain_axis(dx, dy)
                    } else {
                        (dx, dy)
                    };
                    if let (Some(from), Some(slot)) =
                        (self.grab_from.get(i).copied(), self.pins.get_mut(i))
                    {
                        *slot = (from.0 + dx, from.1 + dy);
                    }
                }
                Grab::Selected => {
                    for &i in &self.selected_pins {
                        if let (Some(from), Some(slot)) =
                            (self.grab_from.get(i).copied(), self.pins.get_mut(i))
                        {
                            *slot = (from.0 + dx, from.1 + dy);
                        }
                    }
                }
                _ => {}
            },
            XformMode::Free => match g {
                Grab::Pin(i) => self.drag_scale_mod(i, dx, dy, shift, alt),
                Grab::All => {
                    let (dx, dy) = if shift {
                        constrain_axis(dx, dy)
                    } else {
                        (dx, dy)
                    };
                    for (p, f) in self.pins.iter_mut().zip(self.grab_from.iter()) {
                        *p = (f.0 + dx, f.1 + dy);
                    }
                }
                Grab::Rotate => self.drag_rotate(dx, dy, shift),
                Grab::Selected => {}
            },
            XformMode::Skew => self.drag_skew(dx, dy),
            XformMode::Perspective => self.drag_perspective(dx, dy),
        }
    }

    /// Scale a Free corner about the opposite corner (or the frame center
    /// with `from_center`), optionally locking the frame's aspect ratio.
    fn drag_scale_mod(&mut self, i: usize, dx: f32, dy: f32, lock_aspect: bool, from_center: bool) {
        if self.grab_from.len() != 4 || i >= 4 {
            return;
        }
        let s = XFORM_CORNER_SIGNS;
        let (ox, oy, so) = if from_center {
            let (cx, cy) = xform_points_center(&self.grab_from);
            (cx, cy, (0.0, 0.0))
        } else {
            let o = (i + 2) % 4;
            (self.grab_from[o].0, self.grab_from[o].1, s[o])
        };
        let tl = self.grab_from[0];
        let u = xform_unit((self.grab_from[1].0 - tl.0, self.grab_from[1].1 - tl.1));
        let v = xform_unit((self.grab_from[3].0 - tl.0, self.grab_from[3].1 - tl.1));
        let (px, py) = (self.grab_at.0 + dx - ox, self.grab_at.1 + dy - oy);
        let du = s[i].0 - so.0;
        let dv = s[i].1 - so.1;
        let mut hu = ((px * u.0 + py * u.1) / du).abs().max(XFORM_MIN_EXTENT);
        let mut hv = ((px * v.0 + py * v.1) / dv).abs().max(XFORM_MIN_EXTENT);
        if lock_aspect {
            let (w0, h0) = frame_extents(&self.grab_from);
            let k = (hu / w0).max(hv / h0);
            hu = w0 * k;
            hv = h0 * k;
        }
        let (un, vn) = ((u.0 * hu, u.1 * hu), (v.0 * hv, v.1 * hv));
        for (k, slot) in self.pins.iter_mut().enumerate() {
            let duk = s[k].0 - so.0;
            let dvk = s[k].1 - so.1;
            *slot = (ox + duk * un.0 + dvk * vn.0, oy + duk * un.1 + dvk * vn.1);
        }
    }

    /// Rotate the Free frame about its center; `snap` quantizes the turn
    /// to 15° steps.
    fn drag_rotate(&mut self, dx: f32, dy: f32, snap: bool) {
        let (cx, cy) = xform_points_center(&self.grab_from);
        let a0 = (self.grab_at.1 - cy).atan2(self.grab_at.0 - cx);
        let a1 = (self.grab_at.1 + dy - cy).atan2(self.grab_at.0 + dx - cx);
        let mut delta = a1 - a0;
        if snap {
            delta = snap_angle(delta.to_degrees()).to_radians();
        }
        let (sin, cos) = delta.sin_cos();
        for (p, f) in self.pins.iter_mut().zip(self.grab_from.iter()) {
            let (px, py) = (f.0 - cx, f.1 - cy);
            *p = (cx + px * cos - py * sin, cy + px * sin + py * cos);
        }
    }

    fn drag_skew(&mut self, dx: f32, dy: f32) {
        let Some(Grab::Pin(i)) = self.grab else {
            return;
        };
        if self.grab_from.len() != 4 || i >= 4 {
            return;
        }
        if self.skew_lock.is_none() && dx.abs() + dy.abs() > 0.5 {
            self.skew_lock = Some(dx.abs() >= dy.abs());
        }
        let Some(horiz) = self.skew_lock else {
            return;
        };
        let pair = if horiz { i ^ 1 } else { i ^ 2 };
        let (dx, dy) = if horiz { (dx, 0.0) } else { (0.0, dy) };
        for k in [i, pair] {
            if let (Some(f), Some(slot)) = (self.grab_from.get(k).copied(), self.pins.get_mut(k)) {
                *slot = (f.0 + dx, f.1 + dy);
            }
        }
    }

    fn drag_perspective(&mut self, dx: f32, dy: f32) {
        let Some(Grab::Pin(i)) = self.grab else {
            return;
        };
        if self.grab_from.len() != 4 || i >= 4 {
            return;
        }
        let partner = i ^ 1;
        let (fi, fp) = (self.grab_from[i], self.grab_from[partner]);
        let half = ((fp.0 - fi.0).abs() * 0.5 - XFORM_MIN_EXTENT).max(0.0);
        let dx = dx.min(half);
        self.pins[i] = (fi.0 + dx, fi.1 + dy);
        self.pins[partner] = (fp.0 - dx, fp.1 + dy);
    }

    /// Drop any grab; the points snap back to where the grab started so
    /// Esc and gesture abort leave nothing pending for the next gesture.
    fn cancel_grab(&mut self) {
        if self.grab.is_some() && self.grab_from.len() == self.points().len() {
            let from = std::mem::take(&mut self.grab_from);
            self.points_mut().copy_from_slice(&from);
            self.grab_from = from;
        }
        self.grab = None;
        self.grab_from.clear();
    }

    /// Panel Reset: roll a live preview back to its session base, then
    /// re-seed the frame. Reset means start over, not bake the warp.
    fn reset_transform(&mut self, doc: &mut Document) {
        if let Some(b) = self.base.take() {
            if b.layer < doc.layers.len() && b.rev == doc.revision() {
                let layer = &mut doc.layers[b.layer];
                layer.pixels = b.pixels;
                layer.mask = b.mask;
                doc.mark_full_dirty();
            }
        }
        let mode = self.mode;
        self.arm(mode, doc);
    }

    fn rebuild_tris(&mut self, doc: &Document) {
        let (w, h) = (doc.w as f32, doc.h as f32);
        // A live session has one topology over its source pins; current
        // pins are destinations for that same topology. Adding or
        // removing a pin must keep the pristine base and recompute from
        // source positions, otherwise the removed pin's warp is baked and
        // can no longer snap back.
        let source = match self.base.as_ref() {
            Some(b) => b.pins.clone(),
            None => self.pins.clone(),
        };
        let anchors = xform_extra_anchors(&source, w, h);
        let mut all = source.clone();
        all.extend(anchors.iter().copied());
        let tris = xform_delaunay(&all);
        self.mesh_anchors = anchors.clone();
        self.tris = tris.clone();
        if let Some(b) = self.base.as_mut() {
            b.pins = source;
            b.anchors = anchors;
            b.tris = tris;
        }
    }

    /// Switch modes: restore any in-flight grab into the mode that owns
    /// the points, then seed the new mode from the active layer's content
    /// bounds. Off keeps the last positions (re-arming is cheap).
    /// Re-arming also resets the preview session: the next preview
    /// captures a fresh base from the current canvas.
    fn arm(&mut self, mode: XformMode, doc: &Document) {
        self.cancel_grab();
        self.mode = mode;
        self.base = None;
        self.session_rev = doc.revision();
        self.session_layer = doc.active;
        self.checkpointed = false;
        self.skew_lock = None;
        self.active_pin = None;
        self.selected_pins.clear();
        self.marquee = None;
        let (w, h) = (doc.w as f32, doc.h as f32);
        let b = xform_bounds(doc, w, h, 3.0 + self.expand);
        self.bounds = Some(b);
        match mode {
            XformMode::Off => {}
            XformMode::Mesh => {
                let (grid, cols, rows) = xform_bounds_grid(b, w, h, self.mesh_cols, self.mesh_rows);
                self.grid = grid;
                self.grid_cols = cols;
                self.grid_rows = rows;
            }
            _ => {
                self.pins = xform_bounds_corners(b).to_vec();
                self.rebuild_tris(doc);
            }
        }
    }

    fn set_mesh_cells(&mut self, cols: u32, rows: u32, doc: &Document) {
        let cols = cols.clamp(XFORM_MESH_MIN, XFORM_MESH_MAX);
        let rows = rows.clamp(XFORM_MESH_MIN, XFORM_MESH_MAX);
        if cols == self.mesh_cols && rows == self.mesh_rows {
            return;
        }
        self.mesh_cols = cols;
        self.mesh_rows = rows;
        if self.mode == XformMode::Mesh {
            self.arm(XformMode::Mesh, doc);
        }
    }

    fn mesh_point_active(&self, i: usize) -> bool {
        match (self.bounds, self.grid.get(i)) {
            (Some(b), Some(&(x, y))) => {
                x >= b.0 - 0.5 && x <= b.2 + 0.5 && y >= b.1 - 0.5 && y <= b.3 + 0.5
            }
            _ => false,
        }
    }

    fn add_pin(&mut self, at: (f32, f32), doc: &Document) -> bool {
        if self.mode != XformMode::Puppet || self.pins.len() >= XFORM_PIN_MAX {
            return false;
        }
        self.pins.push(at);
        // A live base needs the new pin's source position too, so
        // deleting it later can rebuild the remaining warp without
        // baking this pin's effect. It starts at its click point and
        // therefore does not move pixels on add.
        if let Some(b) = self.base.as_mut() {
            b.pins.push(at);
        }
        self.rebuild_tris(doc);
        // The new pin becomes the active one, mirroring CSP: its orange
        // ring marks the last pin the user placed.
        self.active_pin = Some(self.pins.len() - 1);
        self.selected_pins.clear();
        self.selected_pins.push(self.pins.len() - 1);
        true
    }

    fn remove_pin(&mut self, i: usize, doc: &mut Document) -> bool {
        if self.mode != XformMode::Puppet || self.pins.len() <= 3 || i >= self.pins.len() {
            return false;
        }
        self.pins.remove(i);
        if let Some(b) = self.base.as_mut() {
            if i < b.pins.len() {
                b.pins.remove(i);
            }
        }
        let mut selected = std::mem::take(&mut self.selected_pins);
        selected.retain(|&s| s != i);
        for s in &mut selected {
            if *s > i {
                *s -= 1;
            }
        }
        self.selected_pins = selected;
        self.active_pin = match self.active_pin {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
        if self.active_pin.is_none() {
            self.active_pin = self.selected_pins.last().copied();
        }
        self.rebuild_tris(doc);
        // The video's pin rule: deleting a pin removes its effect.
        // Recompute the remaining pins from the pristine base right away
        // instead of leaving the warp baked into the layer.
        if self.base.is_some() {
            self.checkpointed = false;
            let _ = xform_preview(doc, self);
        }
        true
    }

    fn remove_pin_near(&mut self, at: (f32, f32), radius: f32, doc: &mut Document) -> bool {
        if self.mode != XformMode::Puppet || self.pins.len() <= 3 {
            return false;
        }
        let mut best: Option<(usize, f32)> = None;
        for (i, &(x, y)) in self.pins.iter().enumerate() {
            let d = ((x - at.0).powi(2) + (y - at.1).powi(2)).sqrt();
            if d <= radius && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((i, _)) = best else {
            return false;
        };
        self.remove_pin(i, doc)
    }

    fn clear_pins(&mut self, doc: &mut Document) {
        if self.mode == XformMode::Puppet {
            // Clearing every pin removes every pin's effect: restore the
            // pristine session base, then reseed the frame pins.
            self.reset_transform(doc);
        }
    }

    fn point_in_frame(&self, at: (f32, f32)) -> bool {
        if self.pins.len() < 4 {
            return false;
        }
        let pts = &self.pins[..4];
        let mut inside = false;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let (xi, yi) = pts[i];
            let (xj, yj) = pts[j];
            if (yi > at.1) != (yj > at.1) && at.0 < (xj - xi) * (at.1 - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    fn rotate_handle(&self, zoom: f32) -> Option<(f32, f32)> {
        if self.mode != XformMode::Free || self.pins.len() < 4 {
            return None;
        }
        let (tl, tr) = (self.pins[0], self.pins[1]);
        let mid = ((tl.0 + tr.0) * 0.5, (tl.1 + tr.1) * 0.5);
        let dir = xform_unit((tr.0 - tl.0, tr.1 - tl.1));
        let mut n = (-dir.1, dir.0);
        let c = xform_points_center(&self.pins);
        if (mid.0 - c.0) * n.0 + (mid.1 - c.1) * n.1 < 0.0 {
            n = (-n.0, -n.1);
        }
        let off = 36.0 / zoom.max(0.05);
        Some((mid.0 + n.0 * off, mid.1 + n.1 * off))
    }
}

/// One Transform-layer panel press.
#[derive(Clone, Copy, PartialEq, Debug)]
enum TformOp {
    Scale(f32),
    /// Rotation in degrees (engine convention, matches `affine_layer`).
    Rotate(f32),
    /// Absolute session scale as a percentage (100 = untouched).
    SetScale(f32),
    /// Absolute session rotation in degrees.
    SetRotation(f32),
    FlipH,
    FlipV,
    Reset,
}

/// Session scale bounds, shared by the slider and the absolute setter.
const TFORM_SCALE_MIN: f32 = 0.25;
const TFORM_SCALE_MAX: f32 = 4.0;

/// Fold degrees into (-180, 180] so the angle slider never winds up.
fn wrap_degrees(deg: f32) -> f32 {
    let mut d = (deg + 180.0).rem_euclid(360.0) - 180.0;
    if d == -180.0 {
        d = 180.0;
    }
    d
}

/// Session base for the Transform-layer buttons: the layer's pixels and
/// mask as they were when the session's first press happened. Presses
/// compose here and re-apply from this base, so mashing Scale/rotate/flip
/// never compounds resampling blur (each press stays one undo step).
/// Re-captured whenever the document changes behind it (painting, undo,
/// pin drags) so sessions never fight each other.
struct TformBase {
    pixels: Raster,
    mask: Option<Raster>,
    layer: usize,
    rev: u64,
    ox: i32,
    oy: i32,
}

/// Transform-layer button session state (see [`TformBase`]).
struct TformPress {
    base: Option<TformBase>,
    /// Cumulative uniform scale about the content center.
    scale: f32,
    /// Cumulative rotation in degrees.
    degrees: f32,
    flip_h: bool,
    flip_v: bool,
    /// The current gesture already opened its undo step; live slider
    /// frames share it instead of piling one snapshot per frame.
    checkpointed: bool,
}

impl TformPress {
    fn new() -> Self {
        Self {
            base: None,
            scale: 1.0,
            degrees: 0.0,
            flip_h: false,
            flip_v: false,
            checkpointed: false,
        }
    }

    /// Close the current gesture so the next one opens a fresh undo step.
    fn end_gesture(&mut self) {
        self.checkpointed = false;
    }
}

/// True when the session base no longer matches the layer in front of it.
fn tform_base_stale(doc: &Document, t: &TformPress) -> bool {
    let active = doc.active;
    match t.base.as_ref() {
        None => true,
        Some(b) => {
            b.layer != active
                || b.rev != doc.revision()
                || b.pixels.len() != doc.layers[active].pixels.len()
        }
    }
}

/// Re-capture the session base (and reset the session transform) when the
/// document changed behind it, so panel readouts never describe a dead
/// session.
fn tform_refresh(doc: &Document, t: &mut TformPress) {
    if !tform_base_stale(doc, t) {
        return;
    }
    let layer = &doc.layers[doc.active];
    t.base = Some(TformBase {
        pixels: layer.pixels.clone(),
        mask: layer.mask.clone(),
        layer: doc.active,
        rev: doc.revision(),
        ox: layer.ox,
        oy: layer.oy,
    });
    t.scale = 1.0;
    t.degrees = 0.0;
    t.flip_h = false;
    t.flip_v = false;
    t.checkpointed = false;
}

/// Exact in-place horizontal mirror of a `w`x`h` buffer, `stride` bytes
/// per cell (4 for RGBA pixels, 1 for masks), about the canvas center.
fn flip_buf_h(buf: &mut [u8], w: usize, h: usize, stride: usize) {
    if w == 0 || buf.len() != w * h * stride {
        return;
    }
    for y in 0..h {
        for x in 0..w / 2 {
            for k in 0..stride {
                buf.swap(
                    y * w * stride + x * stride + k,
                    y * w * stride + (w - 1 - x) * stride + k,
                );
            }
        }
    }
}

/// Exact in-place vertical mirror about the canvas center.
fn flip_buf_v(buf: &mut [u8], w: usize, h: usize, stride: usize) {
    let row = w * stride;
    if w == 0 || buf.len() != row * h {
        return;
    }
    for y in 0..h / 2 {
        let a = y * row;
        let b = (h - 1 - y) * row;
        for k in 0..row {
            buf.swap(a + k, b + k);
        }
    }
}

/// Flip a raster in place through the dense bridge: the transform session's
/// exact pixel permutation works on whole buffers, not tiles.
fn flip_raster(r: &mut Raster, horizontal: bool) {
    let (w, h, bpp) = (r.w() as usize, r.h() as usize, r.bpp());
    let mut d = r.to_dense();
    if horizontal {
        flip_buf_h(&mut d, w, h, bpp);
    } else {
        flip_buf_v(&mut d, w, h, bpp);
    }
    r.replace_dense(&d);
}

/// Apply one Transform-layer press: compose it onto the session and
/// re-apply everything from the pristine base in a single pass. Flips
/// are exact pixel permutations; scale/rotation reuse
/// [`Document::affine_layer`], whose quarter turns stay lossless.
///
/// `live` marks continuous slider frames: the first one opens the undo
/// step, the rest ride on it, so a whole drag undoes in one go.
fn tform_apply(doc: &mut Document, t: &mut TformPress, op: TformOp, live: bool) -> XformApply {
    tform_refresh(doc, t);
    let active = doc.active;
    // Compose the press onto the session transform (T' = op o T). Flips
    // conjugate rotations (Fx R(a) Fx = R(-a)), hence the negations.
    match op {
        TformOp::Scale(f) => t.scale *= f,
        TformOp::Rotate(deg) => t.degrees += deg,
        TformOp::SetScale(pct) => t.scale = (pct * 0.01).clamp(TFORM_SCALE_MIN, TFORM_SCALE_MAX),
        TformOp::SetRotation(deg) => t.degrees = wrap_degrees(deg),
        TformOp::FlipH => {
            t.degrees = -t.degrees;
            t.flip_h = !t.flip_h;
        }
        TformOp::FlipV => {
            t.degrees = -t.degrees;
            t.flip_v = !t.flip_v;
        }
        TformOp::Reset => {
            t.scale = 1.0;
            t.degrees = 0.0;
            t.flip_h = false;
            t.flip_v = false;
        }
    }
    if (t.scale - 1.0).abs() < 1e-4 && t.degrees.abs() < 1e-2 && !t.flip_h && !t.flip_v {
        // Composed back to identity (e.g. plus 15 then minus 15): hand
        // back the pristine pixels.
        {
            let b = t.base.as_ref().expect("base captured above");
            let layer = &mut doc.layers[active];
            layer.pixels.clone_from(&b.pixels);
            layer.mask.clone_from(&b.mask);
            layer.ox = b.ox;
            layer.oy = b.oy;
        }
        doc.mark_full_dirty();
        if let Some(b) = t.base.as_mut() {
            b.rev = doc.revision();
        }
        return XformApply::Restored;
    }
    if !live || !t.checkpointed {
        doc.checkpoint();
        t.checkpointed = true;
    }
    // Swap the pristine base in (offsets included), apply the flips as
    // exact permutations, then the scale/rotation resample over it.
    {
        let b = t.base.as_ref().expect("base captured above");
        let layer = &mut doc.layers[active];
        layer.pixels.clone_from(&b.pixels);
        layer.mask.clone_from(&b.mask);
        layer.ox = b.ox;
        layer.oy = b.oy;
        if t.flip_h {
            flip_raster(&mut layer.pixels, true);
            if let Some(m) = layer.mask.as_mut() {
                flip_raster(m, true);
            }
            layer.ox = -layer.ox;
        }
        if t.flip_v {
            flip_raster(&mut layer.pixels, false);
            if let Some(m) = layer.mask.as_mut() {
                flip_raster(m, false);
            }
            layer.oy = -layer.oy;
        }
    }
    doc.affine_layer(active, t.scale, t.degrees);
    if let Some(b) = t.base.as_mut() {
        b.rev = doc.revision();
    }
    XformApply::Applied
}

/// One discrete press: its own undo step, gesture closed afterwards.
fn tform_button(doc: &mut Document, t: &mut TformPress, op: TformOp) -> XformApply {
    let result = tform_apply(doc, t, op, false);
    t.end_gesture();
    result
}

/// Status line describing the session transform, e.g. `125%, 15 deg`.
fn tform_status(t: &TformPress) -> String {
    let mut parts = vec![format!("{:.0}%", t.scale * 100.0)];
    if t.degrees.abs() >= 0.5 {
        parts.push(format!("{:.0} deg", t.degrees));
    }
    if t.flip_h {
        parts.push("flip H".to_string());
    }
    if t.flip_v {
        parts.push("flip V".to_string());
    }
    format!("Layer transform: {}.", parts.join(", "))
}

/// Per-point stabilizer coefficient for an `n`-point input trail: applying
/// it `n` times composes to the same per-frame convergence as one step of
/// `a` (the single-step coefficient), so trail stamping never changes the
/// stabilizer's feel. `n <= 1` returns `a` bit-for-bit.
fn stabilizer_substep(a: f32, n: usize) -> f32 {
    if n <= 1 || a <= 0.0 || a >= 1.0 {
        return a;
    }
    1.0 - (1.0 - a).powf(1.0 / n as f32)
}

const SPEED_NORM_PX_PER_SEC: f32 = 2000.0;

fn sample_pointer(
    smooth_pt: Option<(f32, f32)>,
    last_target: Option<(f32, f32)>,
    target: (f32, f32),
    a: f32,
    dt: f32,
) -> ((f32, f32), f32) {
    let smooth = match smooth_pt {
        Some((px, py)) => (
            px + (target.0 - px) * (1.0 - a),
            py + (target.1 - py) * (1.0 - a),
        ),
        None => target,
    };
    let speed = match last_target {
        Some((lx, ly)) => {
            let dist = ((target.0 - lx).powi(2) + (target.1 - ly).powi(2)).sqrt();
            (dist / dt.max(f32::EPSILON) / SPEED_NORM_PX_PER_SEC).clamp(0.0, 1.0)
        }
        None => 0.0,
    };
    (smooth, speed)
}

/// Quadratic piece to stamp: (start, control, end).
type StrokePiece = ((f32, f32), (f32, f32), (f32, f32));

/// Midpoint-smoothed stroke path (classic quadratic smoothing): stamps a
/// curve through the midpoints of consecutive input samples, each sample
/// as the control point, so sparse samples (60 Hz vsync cap, OS mouse
/// coalescing) still ink as curves instead of chords. The pen trails the
/// raw pointer by half a sample and never overshoots it.
struct StrokeCurve {
    /// Last emitted curve position; None until the stroke's first point.
    e: Option<(f32, f32)>,
    /// Last raw input point.
    b: (f32, f32),
}

impl StrokeCurve {
    fn new() -> Self {
        Self {
            e: None,
            b: (0.0, 0.0),
        }
    }

    fn reset(&mut self) {
        self.e = None;
    }

    /// Feed the next raw point; returns the quadratic piece (start,
    /// control, end) to stamp. None for the first point of a stroke
    /// (that one stamps a plain dab).
    fn push(&mut self, c: (f32, f32)) -> Option<StrokePiece> {
        let piece = self.e.map(|e| (e, self.b, stroke_mid(self.b, c)));
        self.e = Some(match piece {
            Some((_, _, m)) => m,
            None => c,
        });
        self.b = c;
        piece
    }

    /// Stroke end: the tail from the cursor to the last raw point (there
    /// is no later sample to curve through). Resets the cursor so the
    /// next stroke starts with a dab again.
    fn finish(&mut self) -> Option<((f32, f32), (f32, f32))> {
        let e = self.e.take()?;
        Some((e, self.b))
    }
}

fn stroke_mid(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

/// Point on the quadratic Bezier `p0 -> c -> p1` at `t` in `0..=1`.
fn stroke_quad_pt(p0: (f32, f32), c: (f32, f32), p1: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    (
        u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
        u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
    )
}

/// Linear sub-steps to stamp a curve piece of `len` px: one per ~4 px so
/// each stamped sub-step is visually straight; capped for giant jumps.
fn stroke_steps(len: f32) -> usize {
    (len / 4.0).ceil().clamp(1.0, 64.0) as usize
}

#[derive(Clone, Copy)]
struct RecordedSegment {
    from: Option<(f32, f32)>,
    from_dyn: DabDynamics,
    to: (f32, f32),
    to_dyn: DabDynamics,
}

#[derive(Default)]
struct RecordedStroke {
    segments: Vec<RecordedSegment>,
    total_len: f32,
}

impl RecordedStroke {
    fn reset(&mut self) {
        self.segments.clear();
        self.total_len = 0.0;
    }

    fn push(
        &mut self,
        from: Option<(f32, f32)>,
        from_dyn: DabDynamics,
        to: (f32, f32),
        to_dyn: DabDynamics,
    ) {
        if let Some((fx, fy)) = from {
            self.total_len += ((to.0 - fx).powi(2) + (to.1 - fy).powi(2)).sqrt();
        }
        self.segments.push(RecordedSegment {
            from,
            from_dyn,
            to,
            to_dyn,
        });
    }
}

/// Rewind the live stroke and redraw it with Procreate's start/end taper.
/// The checkpoint stays on the undo stack, so one undo still removes the
/// whole stroke.
fn taper_replay(doc: &mut Document, brush: &Brush, color: Color, stroke: &RecordedStroke) -> bool {
    if stroke.segments.is_empty() || stroke.total_len <= 0.0 {
        return false;
    }
    if brush.taper.start <= 0.0 && brush.taper.end <= 0.0 {
        return false;
    }
    if !doc.restore_last_checkpoint() {
        return false;
    }
    let mut travelled = 0.0;
    let mut leftover = 0.0;
    for segment in &stroke.segments {
        let leg = match segment.from {
            Some((fx, fy)) => ((segment.to.0 - fx).powi(2) + (segment.to.1 - fy).powi(2)).sqrt(),
            None => 0.0,
        };
        let from_u = (travelled / stroke.total_len).clamp(0.0, 1.0);
        travelled += leg;
        let to_u = (travelled / stroke.total_len).clamp(0.0, 1.0);
        let mut from_dyn = segment.from_dyn;
        from_dyn.taper = brush.taper.factor_at(from_u);
        let mut to_dyn = segment.to_dyn;
        to_dyn.taper = brush.taper.factor_at(to_u);
        let (_, lo) = doc.stroke_to_dyn(
            brush,
            color,
            segment.from,
            from_dyn,
            segment.to,
            to_dyn,
            leftover,
        );
        leftover = lo;
    }
    true
}

/// Flush the smoothed stroke's final tail (curve cursor -> last raw
/// sample) at pointer release. Only the painting branch owns this: the
/// smudge tool advances its own last-point chain and must never run a
/// brush flush from the shared curve state (that drew a stale brush
/// segment from the smudge position to an earlier brush stroke).
#[allow(clippy::too_many_arguments)]
fn flush_stroke_tail(
    doc: &mut Document,
    brush: &Brush,
    color: Color,
    curve: &mut StrokeCurve,
    last: &mut Option<(f32, f32)>,
    dyn_state: DabDynamics,
    leftover: &mut f32,
    stroke: &mut RecordedStroke,
) -> bool {
    let Some((from, to)) = curve.finish() else {
        return false;
    };
    if last.is_none() {
        return false;
    }
    let len = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
    let steps = stroke_steps(len);
    for k in 1..=steps {
        let t = k as f32 / steps as f32;
        let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        let (l, lo) = doc.stroke_to_dyn(brush, color, *last, dyn_state, p, dyn_state, *leftover);
        stroke.push(*last, dyn_state, p, dyn_state);
        *last = l;
        *leftover = lo;
    }
    true
}

/// Host captures and remote device testing force the viewport-resolution
/// canvas (spec §4) regardless of document size or GPU limits.
fn viewport_forced() -> bool {
    std::env::var("PROCRAFT_FORCE_VIEWPORT_CANVAS").is_ok_and(|v| v == "1")
}

/// Textured quad for a rotated canvas view: `corners` are TL, TR, BR, BL
/// with matching unit UVs, tinted (egui has no canvas transform, so the
/// texture stays axis-aligned and the quad does the rotating).
fn paint_textured_quad(
    painter: &egui::Painter,
    tex: egui::TextureId,
    corners: [egui::Pos2; 4],
    tint: Color32,
) {
    let uvs = [
        egui::Pos2::new(0.0, 0.0),
        egui::Pos2::new(1.0, 0.0),
        egui::Pos2::new(1.0, 1.0),
        egui::Pos2::new(0.0, 1.0),
    ];
    let mut mesh = egui::Mesh::with_texture(tex);
    for (pos, uv) in corners.iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *pos,
            uv,
            color: tint,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Preview polyline for the shape tool drag, in canvas coordinates:
/// closed families repeat their first point, line/arrow stay open. Traces
/// the same geometry the commit draws (engine vertex builders).
fn shape_preview_points(
    kind: ShapeKind,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    sides: u8,
) -> Vec<(f32, f32)> {
    match kind {
        ShapeKind::Line => vec![(ax, ay), (bx, by)],
        ShapeKind::Rect => Document::rect_vertices(ax, ay, bx, by),
        ShapeKind::Ellipse => Document::ellipse_vertices(ax, ay, bx, by),
        ShapeKind::Triangle => Document::poly_vertices(ShapePoly::Triangle, ax, ay, bx, by, sides),
        ShapeKind::Diamond => Document::poly_vertices(ShapePoly::Diamond, ax, ay, bx, by, sides),
        ShapeKind::Star => Document::poly_vertices(ShapePoly::Star, ax, ay, bx, by, sides),
        ShapeKind::Arrow => Document::arrow_path(ax, ay, bx, by),
        ShapeKind::Polygon => Document::poly_vertices(ShapePoly::Polygon, ax, ay, bx, by, sides),
    }
}

/// Marching-ants path: dark base polyline plus moving light dashes along
/// consecutive points, dash phase carried across segments.
fn paint_ants_path(painter: &egui::Painter, pts: &[egui::Pos2], time: f64) {
    let dark = Color32::BLACK;
    let light = Color32::WHITE;
    for w in pts.windows(2) {
        painter.line_segment([w[0], w[1]], (1.5, dark));
    }
    let dash = 7.0;
    let gap = 5.0;
    let period = dash + gap;
    let mut offset = (time * 28.0) as f32 % period;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = (b - a).length();
        let mut t = -offset;
        while t < len {
            let t0 = t.max(0.0);
            let t1 = (t + dash).min(len);
            if t1 > t0 {
                let dir = (b - a) / len.max(0.001);
                painter.line_segment([a + dir * t0, a + dir * t1], (1.5, light));
            }
            t += period;
        }
        offset = (offset + len) % period;
    }
}

/// Marching-ants quad (a rectangle in an unrotated view): dark base line
/// plus moving light dashes along the four corners in order.
fn paint_selection(painter: &egui::Painter, corners: [egui::Pos2; 4], time: f64) {
    let loop_pts = [corners[0], corners[1], corners[2], corners[3], corners[0]];
    paint_ants_path(painter, &loop_pts, time);
}

/// Eyedropper loupe cells: an odd-sided square of composited colors
/// around `(cx, cy)`, missing edge cells filled with the page background.
/// Returns the cells plus the cursor's cell position inside the grid.
fn loupe_cells(doc: &Document, cx: u32, cy: u32, side: u32) -> (Vec<[u8; 4]>, u32, u32) {
    let half = side / 2;
    let (w, h) = (doc.w, doc.h);
    let (rx0, ry0) = (cx.saturating_sub(half), cy.saturating_sub(half));
    let (rx1, ry1) = (
        (cx + half).min(w.saturating_sub(1)),
        (cy + half).min(h.saturating_sub(1)),
    );
    let region = doc.flatten_region(rx0, ry0, rx1, ry1);
    let bg = doc.background;
    let mut cells = Vec::with_capacity((side * side) as usize);
    for j in 0..side {
        for i in 0..side {
            let (x, y) = (rx0 + i, ry0 + j);
            if x <= rx1 && y <= ry1 {
                let o = (((y - ry0) * (rx1 - rx0 + 1) + (x - rx0)) * 4) as usize;
                cells.push([region[o], region[o + 1], region[o + 2], region[o + 3]]);
            } else {
                cells.push(bg);
            }
        }
    }
    (cells, cx - rx0, cy - ry0)
}

/// Magnifier ring for the eyedropper: `cells` render as squares at `cell`
/// px with the cursor cell outlined, plus a chip + hex readout. Screen
/// space (never rotated with the canvas); the caller parks it clear of
/// the pointer so fingers never cover the target.
fn paint_loupe(
    painter: &egui::Painter,
    cells: &[[u8; 4]],
    side: u32,
    center: (u32, u32),
    at: egui::Pos2,
    cell: f32,
    picked: [u8; 3],
) {
    let size = side as f32 * cell;
    let pad = 6.0;
    let row = 22.0;
    painter.rect_filled(
        egui::Rect::from_min_size(
            at - egui::Vec2::splat(pad),
            egui::Vec2::new(size + pad * 2.0, size + pad * 2.0 + row),
        ),
        8.0,
        Color32::from_black_alpha(220),
    );
    for (k, c) in cells.iter().enumerate() {
        let (i, j) = ((k as u32 % side) as f32, (k as u32 / side) as f32);
        painter.rect_filled(
            egui::Rect::from_min_size(
                at + egui::Vec2::new(i * cell, j * cell),
                egui::Vec2::splat(cell),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]),
        );
    }
    painter.rect_stroke(
        egui::Rect::from_min_size(
            at + egui::Vec2::new(center.0 as f32 * cell, center.1 as f32 * cell),
            egui::Vec2::splat(cell),
        ),
        0.0,
        (2.0, Color32::WHITE),
        egui::StrokeKind::Middle,
    );
    let chip = egui::Rect::from_min_size(
        at + egui::Vec2::new(0.0, size + pad),
        egui::Vec2::new(row - 6.0, row - 6.0),
    );
    painter.rect_filled(
        chip,
        3.0,
        Color32::from_rgb(picked[0], picked[1], picked[2]),
    );
    painter.text(
        at + egui::Vec2::new(row, size + pad + (row - 6.0) * 0.5),
        egui::Align2::LEFT_CENTER,
        format!("#{:02X}{:02X}{:02X}", picked[0], picked[1], picked[2]),
        egui::FontId::monospace(13.0),
        Color32::WHITE,
    );
}

/// Freehand path overlay: dark base plus light top, visible on any art.
fn paint_path(painter: &egui::Painter, pts: &[egui::Pos2], close: bool) {
    if pts.len() < 2 {
        return;
    }
    let mut segs: Vec<[egui::Pos2; 2]> = pts.windows(2).map(|w| [w[0], w[1]]).collect();
    if close {
        segs.push([pts[pts.len() - 1], pts[0]]);
    }
    for [a, b] in &segs {
        painter.line_segment([*a, *b], (3.0, Color32::BLACK));
    }
    for [a, b] in &segs {
        painter.line_segment([*a, *b], (1.5, Color32::WHITE));
    }
}

/// Marching ants along mask edges: dark dots everywhere, light dots on a
/// moving subset (classic two-tone crawl). `to_screen` maps canvas pixels.
fn paint_mask_edges(
    painter: &egui::Painter,
    edges: &[(u32, u32)],
    to_screen: impl Fn(u32, u32) -> egui::Pos2,
    time: f64,
) {
    let tick = (time * 28.0 / 3.0) as usize;
    for &(x, y) in edges.iter() {
        let p = to_screen(x, y);
        let r = egui::Rect::from_center_size(p, egui::Vec2::splat(2.5));
        painter.rect_filled(r, 0.0, Color32::BLACK);
    }
    for (i, &(x, y)) in edges.iter().enumerate() {
        if (i + tick) % 12 < 6 {
            let p = to_screen(x, y);
            let r = egui::Rect::from_center_size(p, egui::Vec2::splat(1.5));
            painter.rect_filled(r, 0.0, Color32::WHITE);
        }
    }
}

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
                self.status = "Brush size reverted.".to_string();
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
        // Autosave: silent v8 snapshot every 120 s (unconditional — a few
        // ms on typical canvases, and no change can slip through dirty
        // tracking). Failures surface once in the status line.
        {
            let now = ui.ctx().input(|i| i.time);
            if now - self.last_autosave >= 120.0 {
                self.last_autosave = now;
                if let Err(e) = self.doc.save_project(&self.recovery_path) {
                    self.status = format!("Autosave failed: {e}");
                }
            }
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
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(w * z, h * z), Sense::click_and_drag());
                // A page that fits floats centred on the board; a zoomed-in
                // page keeps the viewport origin so panning still works.
                let rect = rect.translate(center_offset(rect.size(), avail));
                // Pan applies to the drawn canvas and every overlay/mapping
                // below (sizes are translation-invariant).
                let rect = rect.translate(self.view_offset);
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
                        self.status = format!("Brush size {:.0}", self.brush.size);
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
                            self.liquify_from = Some(to_canvas(pos));
                        }
                    }
                    if resp.dragged() {
                        if let (Some(from), Some(pos)) = (self.liquify_from, pos) {
                            let to = to_canvas(pos);
                            // Warp the whole pointer segment every frame.
                            // Sub-steps cap each push at a quarter of the
                            // brush radius so fast strokes stay smooth
                            // instead of tearing holes into the layer.
                            let max_step = (self.liquify_size * 0.125).max(2.0);
                            let active = self.doc.active;
                            let (size, strength) = (self.liquify_size, self.liquify_strength);
                            for ((x, y), (dx, dy)) in liquify_pushes(from, to, max_step) {
                                self.doc.liquify(active, x, y, dx, dy, size, strength);
                                self.ghost_dirty = true;
                            }
                            self.liquify_from = Some(to);
                        }
                    }
                    if resp.drag_stopped() {
                        self.liquify_from = None;
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
                if self.painting && resp.dragged() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        // Stamp through every pointer position the backend
                        // recorded this frame (device rate), not just the
                        // frame-end hover point: fast strokes stop turning
                        // into straight chords between frames. Frames with
                        // no extra events keep the exact old behavior.
                        let mut pts: Vec<egui::Pos2> = ctx.input(|i| {
                            i.events
                                .iter()
                                .filter_map(|e| match e {
                                    egui::Event::PointerMoved(p) => Some(*p),
                                    _ => None,
                                })
                                .collect()
                        });
                        if pts.last() != Some(&pos) {
                            pts.push(pos);
                        }
                        let now = ctx.input(|i| i.time);
                        let frame_dt = (now - self.last_t).max(1.0 / 240.0) as f32;
                        self.last_t = now;
                        let dt = frame_dt / pts.len() as f32;
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
                        let a = stabilizer_substep(a, pts.len());
                        for pt in pts {
                            let (x, y) = to_canvas(pt);
                            // Clamp: off-page drags paint nothing instead of
                            // streaking the canvas edge (and poisoning the
                            // dirty rect, which used to quit the app).
                            let (x, y) = clamp_pt(x, y);
                            let target = (x, y);
                            let ((sx, sy), speed_norm) =
                                sample_pointer(self.smooth_pt, self.last_target, target, a, dt);
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
                            let to_dyn = DabDynamics {
                                pressure: self.smooth_pressure,
                                speed_norm,
                                angle,
                                taper: 1.0,
                            };
                            let color = self.current_color();
                            // Ink the smoothed path (see StrokeCurve): the
                            // stroke's first point lands a dab, every later
                            // point stamps its curved piece in short linear
                            // steps so sparse samples still draw curves.
                            match self.stroke_curve.push((sx, sy)) {
                                None => {
                                    let from = self.last;
                                    let (last, leftover) = self.doc.stroke_to_dyn(
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
                                }
                                Some((p0, ctrl, p1)) => {
                                    let piece =
                                        ((p1.0 - p0.0).powi(2) + (p1.1 - p0.1).powi(2)).sqrt();
                                    let steps = stroke_steps(piece);
                                    for k in 1..=steps {
                                        let t = k as f32 / steps as f32;
                                        let (px, py) = stroke_quad_pt(p0, ctrl, p1, t);
                                        let from = self.last;
                                        let (last, leftover) = self.doc.stroke_to_dyn(
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
                                    }
                                }
                            }
                            self.last_dyn = to_dyn;
                            self.stroke_painted = true;
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
                        if self.stroke_painted {
                            taper_replay(&mut self.doc, &brush, color, &self.stroke_record);
                        }
                        self.stroke_record.reset();
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
            });
        });

        if shell == Shell::Mobile {
            self.mobile_chrome(ui.ctx(), class);
        }
    }
}

/// Window icon: original layer-stack mark drawn in VectorCraft
/// (see `assets/icon-generator.py`, rendered to `assets/icon-48.png`).
fn app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../../../assets/icon-48.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

fn build_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1400.0, 900.0]);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    // Disk persistence for window rect, panel widths (by panel id), and
    // the UI state saved in `App::save` (theme, panels, zoom). Mobile
    // keeps it off (same as iOS: sandboxed, session-only).
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    let persistence_path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .map(|mut dir| {
            dir.push("procraft");
            let _ = std::fs::create_dir_all(&dir);
            dir.push("ui.json");
            dir
        });
    #[cfg(any(target_os = "ios", target_os = "android"))]
    let persistence_path = None;
    eframe::NativeOptions {
        viewport,
        persistence_path,
        ..Default::default()
    }
}

fn run_with(options: eframe::NativeOptions) {
    let _ = eframe::run_native(
        "ProCraft",
        options,
        Box::new(|cc| Ok(Box::new(ProCraftApp::new(cc)))),
    );
}

/// Desktop/iOS entry (the `[[bin]]` wrapper calls this).
pub fn run() {
    run_with(build_options());
}

/// Android entry: `NativeActivity` spawns this on its own thread (not
/// `main`); the `AndroidApp` handle is required for winit's event loop.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    if let Some(p) = app.internal_data_path() {
        crate::settings::set_android_data_dir(p);
    }
    let mut options = build_options();
    options.android_app = Some(app);
    run_with(options);
}

#[cfg(test)]
mod tests {
    use super::*;
    use procraft_engine::Taper;

    #[test]
    fn brush_and_tip_filters_cover_every_importable_type() {
        for ext in [
            "pcbrush", "json", "abr", "brush", "brushset", "kpp", "bundle", "sut", "gbr", "png",
            "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff",
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
        let (smooth, speed) = sample_pointer(Some((0.0, 0.0)), Some((0.0, 0.0)), target, a, dt);
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
    fn liquify_pushes_cover_the_whole_pointer_segment() {
        // Within the cap: one push, ending exactly at the pointer.
        let pushes = liquify_pushes((0.0, 0.0), (3.0, 4.0), 10.0);
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0], ((3.0, 4.0), (3.0, 4.0)));

        // A fast move is split so every push stays under the cap, the
        // pushes chain end-to-end, and the last anchor is the pointer —
        // nothing is left over for the release to drop.
        let pushes = liquify_pushes((0.0, 0.0), (120.0, 160.0), 10.0);
        assert_eq!(pushes.len(), 20);
        let mut at = (0.0, 0.0);
        let mut sum = (0.0, 0.0);
        for ((x, y), (dx, dy)) in &pushes {
            assert!(dx.hypot(*dy) <= 10.0 + 1e-4, "step {dx},{dy}");
            assert!(
                (at.0 + dx - x).abs() < 1e-4 && (at.1 + dy - y).abs() < 1e-4,
                "gap before {x},{y}"
            );
            at = (*x, *y);
            sum = (sum.0 + dx, sum.1 + dy);
        }
        assert_eq!(at, (120.0, 160.0));
        assert!((sum.0 - 120.0).abs() < 1e-3 && (sum.1 - 160.0).abs() < 1e-3);

        // A pathological one-frame jump stays bounded.
        assert_eq!(liquify_pushes((0.0, 0.0), (10_000.0, 0.0), 10.0).len(), 64);

        // Idle hand: nothing to push.
        assert!(liquify_pushes((7.0, 8.0), (7.0, 8.0), 5.0).is_empty());
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
        let phone_landscape =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(844.0, 390.0));
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

        let phone_portrait =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(390.0, 844.0));
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
            (mobile_modal_anchor(tablet, 24.0, 20.0, BarLayout::Tablet, 56.0, 8.0, true).y - 2.0)
                .abs()
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
        assert!(taper_replay(&mut doc, &brush, color, &record));
        let alpha = |x: u32| doc.layers[0].pixels.get(((6 * doc.w + x) * 4 + 3) as usize);
        assert!(alpha(6) > 200, "head alpha {}", alpha(6));
        assert!(alpha(46) < 40, "tail alpha {}", alpha(46));
        assert!(alpha(20) < alpha(6));
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
        let points = brush_preview_path();
        let progress = preview_path_progress(&points);
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
            let from_u = if idx == 0 { 0.0 } else { progress[idx - 1] };
            let from_dyn = DabDynamics {
                angle,
                taper: brush.taper.factor_at(from_u),
                ..Default::default()
            };
            let to_dyn = DabDynamics {
                angle,
                taper: brush.taper.factor_at(progress[idx]),
                ..Default::default()
            };
            let (l, lo) = doc.stroke_to_dyn(&brush, color, last, from_dyn, to, to_dyn, leftover);
            record.push(last, from_dyn, to, to_dyn);
            last = l;
            leftover = lo;
        }
        assert!(taper_replay(&mut doc, &brush, color, &record));
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
        assert_eq!(next_layer_selection(&[1, 2], 2, 5, true, false), vec![1, 2, 5]);
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
}
