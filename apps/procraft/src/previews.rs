use crate::*;

/// Live ColorDrop preview: the pre-fill layer snapshot plus the drag
/// geometry, so each tolerance update restores and re-fills from the same
/// seed (one undo step for the whole gesture). Kept alive after the
/// gesture so the Tolerance/Grow sliders stay live: every update re-fills
/// from this snapshot, which anchors growth to the fill's original
/// (inner) boundary instead of stacking a new band on its grown outer
/// edge.
pub(crate) struct FillPreview {
    pub(crate) layer: usize,
    pub(crate) seed: (u32, u32),
    pub(crate) pixels: Raster,
    pub(crate) color: Color,
    pub(crate) opacity: f32,
    pub(crate) start_x: f32,
    pub(crate) start_tol: u8,
    pub(crate) applied: u8,
    pub(crate) live_hash: u64,
    /// Document context the fill was computed under. A change (selection,
    /// symmetry, active layer) finalizes the preview rather than replaying
    /// the fill into a different context.
    pub(crate) selection: Option<(u32, u32, u32, u32)>,
    pub(crate) mask_hash: Option<u64>,
    pub(crate) symmetry: SymmetryMode,
    pub(crate) segments: u8,
    pub(crate) sym_center: Option<(f32, f32)>,
}

/// Restore the pre-fill snapshot and re-run the fill at `tolerance`/`grow`.
/// Always re-applies from the original boundary, so a slider scrub both
/// expands and shrinks the result (and a re-tap never compounds growth).
pub(crate) fn fill_from_baseline(
    doc: &mut Document,
    fp: &mut FillPreview,
    tolerance: u8,
    grow: u8,
) {
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
pub(crate) fn fill_context_matches(doc: &Document, fp: &FillPreview) -> bool {
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
pub(crate) struct FilterPreview {
    /// 0 = tone, 1 = blur, 2 = sharpen, 3 = levels, 4 = curves,
    /// 5 = HSL, 6 = balance, 7 = recolor.
    pub(crate) kind: u8,
    pub(crate) layer: usize,
    pub(crate) pixels: Raster,
    pub(crate) last: std::time::Instant,
    pub(crate) stale: bool,
    /// Fingerprint of the layer right after our last apply (or of the
    /// snapshot on a fresh preview). A tick finding anything else means
    /// another edit touched the layer meanwhile — restoring the stale
    /// snapshot would wipe that work, so the preview restarts instead.
    pub(crate) live_hash: u64,
}

/// Cheap non-crypto fingerprint of layer pixels (samples every ~1KB):
/// detects "the layer changed underneath us" for preview guards. A miss
/// only ever restarts a preview, never destroys work.
pub(crate) fn pixels_hash(pixels: &[u8]) -> u64 {
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

/// Texture size for the preview strip: one texel per device pixel, so the
/// thumbnail is copied to the screen 1:1 instead of being resampled.
pub(crate) fn preview_size(scale: f32) -> (u32, u32) {
    let s = if scale.is_finite() {
        scale.max(1.0)
    } else {
        1.0
    };
    (
        (PREVIEW_W * s).round().max(1.0) as u32,
        (PREVIEW_H * s).round().max(1.0) as u32,
    )
}

/// The preset as it should be previewed: same edge, sized to fit the strip.
pub(crate) fn preview_brush(brush: &Brush, strip_h: f32) -> Brush {
    let mut b = Brush {
        eraser: false,
        ..brush.clone()
    };
    b.size = b.size.min(strip_h * PREVIEW_SIZE_FRAC).max(1.0);
    b
}

pub(crate) fn brush_preview_path(w: f32, h: f32) -> Vec<(f32, f32)> {
    // Fractions of the strip, so the wave keeps its shape at any preview
    // size (144x36 reproduces the original 8..136, mid 18, amplitude 8).
    let (x0, x1) = (w * (8.0 / 144.0), w * (136.0 / 144.0));
    let (mid, amp) = (h * 0.5, h * (8.0 / 36.0));
    let step = (w / 72.0).max(1.0);
    let mut points = Vec::new();
    let mut x = x0;
    while x <= x1 {
        let t = (x - x0) / (x1 - x0);
        let y = mid + amp * (t * std::f32::consts::PI * 2.0).sin();
        points.push((x, y));
        x += step;
    }
    points
}

/// Cumulative distance (canvas px) along `points`, starting at 0.
pub(crate) fn preview_path_progress(points: &[(f32, f32)]) -> Vec<f32> {
    let mut progress = Vec::with_capacity(points.len());
    progress.push(0.0);
    let mut total = 0.0;
    for pair in points.windows(2) {
        let (dx, dy) = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
        total += (dx * dx + dy * dy).sqrt();
        progress.push(total);
    }
    progress
}

pub(crate) fn paint_preview_stroke(doc: &mut Document, brush: &Brush, ink: Color) {
    let points = brush_preview_path(doc.w as f32, doc.h as f32);
    let progress = preview_path_progress(&points);
    let total = progress.last().copied().unwrap_or(0.0);
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
        let (l, lo) = doc.stroke_to_dyn(brush, ink, last, from_dyn, to, to_dyn, left);
        last = l;
        left = lo;
    }
    doc.end_stroke();
}

/// Dispatch a palette file by extension (shared by the desktop Import arm
/// and the iOS picker resolve path): `.pcpalette`/`.json` or GIMP `.gpl`.
pub(crate) fn load_palette_file(path: &std::path::Path) -> Result<Palette, String> {
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

pub(crate) struct LoadedBrushes {
    pub(crate) brushes: Vec<BrushPreset>,
    pub(crate) warnings: Vec<String>,
    pub(crate) skipped: usize,
}

/// Dispatch a brush file by extension into presets plus any importer
/// warnings (shared by the desktop Import arm and the iOS picker path).
pub(crate) fn load_brush_file_report(path: &std::path::Path) -> Result<LoadedBrushes, String> {
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

pub(crate) fn load_brush_file(path: &std::path::Path) -> Result<Vec<BrushPreset>, String> {
    load_brush_file_report(path).map(|loaded| loaded.brushes)
}

pub(crate) fn brush_import_status(
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
