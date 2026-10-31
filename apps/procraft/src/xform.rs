use crate::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum XformMode {
    Off,
    Distort,
    Skew,
    Perspective,
    Free,
    Mesh,
    Puppet,
}

impl XformMode {
    pub(crate) fn all() -> [XformMode; 7] {
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

    pub(crate) fn label(self) -> &'static str {
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
    pub(crate) fn is_pins(self) -> bool {
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

pub(crate) fn xform_mode_status(mode: XformMode) -> &'static str {
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
pub(crate) const XFORM_MESH_MIN: u32 = 1;

pub(crate) const XFORM_MESH_MAX: u32 = 8;

pub(crate) const XFORM_MESH_DEFAULT: u32 = 2;

/// Maximum extra margin around a Puppet frame for thin-line warps.
pub(crate) const XFORM_EXPAND_MAX: f32 = 128.0;

/// Puppet warp pin budget (frame pins included).
pub(crate) const XFORM_PIN_MAX: usize = 24;

/// Minimum half-extent (canvas px) a Free-mode scale keeps so the box
/// never collapses to a line.
pub(crate) const XFORM_MIN_EXTENT: f32 = 2.0;

/// Corner order for every pin mode: [TL, TR, BR, BL], with each corner's
/// unit signs about the box center (mirrored pairs flip one sign).
pub(crate) const XFORM_CORNER_SIGNS: [(f32, f32); 4] =
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];

pub(crate) fn xform_anchors(w: f32, h: f32) -> [(f32, f32); 4] {
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
pub(crate) fn xform_extra_anchors(pins: &[(f32, f32)], w: f32, h: f32) -> Vec<(f32, f32)> {
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
pub(crate) fn xform_bounds(doc: &Document, w: f32, h: f32, margin: f32) -> (f32, f32, f32, f32) {
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

pub(crate) fn xform_bounds_corners(b: (f32, f32, f32, f32)) -> [(f32, f32); 4] {
    [(b.0, b.1), (b.2, b.1), (b.2, b.3), (b.0, b.3)]
}

pub(crate) fn xform_unit(v: (f32, f32)) -> (f32, f32) {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len < 1e-4 {
        (1.0, 0.0)
    } else {
        (v.0 / len, v.1 / len)
    }
}

pub(crate) fn xform_points_center(points: &[(f32, f32)]) -> (f32, f32) {
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let (sx, sy) = points
        .iter()
        .fold((0.0f32, 0.0f32), |(ax, ay), &(x, y)| (ax + x, ay + y));
    (sx / points.len() as f32, sy / points.len() as f32)
}

/// Lock a drag to its dominant axis (Shift while moving).
pub(crate) fn constrain_axis(dx: f32, dy: f32) -> (f32, f32) {
    if dx.abs() >= dy.abs() {
        (dx, 0.0)
    } else {
        (0.0, dy)
    }
}

/// Quantize an angle in degrees to 15° steps (Shift while rotating).
pub(crate) fn snap_angle(deg: f32) -> f32 {
    pub(crate) const STEP: f32 = 15.0;
    (deg / STEP).round() * STEP
}

/// Half extents of the four-corner frame: (half width, half height).
pub(crate) fn frame_extents(points: &[(f32, f32)]) -> (f32, f32) {
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
pub(crate) fn xform_axis_lines(a: f32, b: f32, cells: u32, max: f32) -> Vec<f32> {
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
pub(crate) fn xform_bounds_grid(
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
pub(crate) fn xform_delaunay(points: &[(f32, f32)]) -> Vec<[u32; 3]> {
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

pub(crate) fn xform_in_circumcircle(
    p: (f64, f64),
    a: (f64, f64),
    b: (f64, f64),
    c: (f64, f64),
) -> bool {
    let (ax, ay) = (a.0 - p.0, a.1 - p.1);
    let (bx, by) = (b.0 - p.0, b.1 - p.1);
    let (cx, cy) = (c.0 - p.0, c.1 - p.1);
    let det = (ax * ax + ay * ay) * (bx * cy - cx * by) - (bx * bx + by * by) * (ax * cy - cx * ay)
        + (cx * cx + cy * cy) * (ax * by - bx * ay);
    det > 0.0
}

pub(crate) fn xform_tri_area(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1)
}

/// A mesh is usable while every triangle keeps a real area (a collapsed
/// triangle has no sane inverse map) and all indices resolve.
pub(crate) fn xform_pins_ok(points: &[(f32, f32)], tris: &[[u32; 3]]) -> bool {
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
pub(crate) fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
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
pub(crate) fn pull_grid(
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
pub(crate) enum XformApply {
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
pub(crate) struct XformBase {
    pub(crate) pixels: Raster,
    pub(crate) mask: Option<Raster>,
    pub(crate) layer: usize,
    pub(crate) pins: Vec<(f32, f32)>,
    /// Canvas anchors the base topology was built with (pins already
    /// sitting on an anchor are not repeated).
    pub(crate) anchors: Vec<(f32, f32)>,
    /// Triangle topology the base pins belong to (pin modes); empty for
    /// the lattice path.
    pub(crate) tris: Vec<[u32; 3]>,
    pub(crate) rev: u64,
}

/// Recompute the active layer from the session base with the pins where
/// they are now: the one path that writes transform pixels. Captures the
/// base lazily on the first write (while the pixels still correspond to
/// the gesture-start pins), restores pristine pixels when the pins land
/// back on the base, and re-captures instead of warping a stale base
/// after external edits (painting, undo, filters) or a layer switch.
pub(crate) fn xform_preview(doc: &mut Document, x: &mut XformState) -> XformApply {
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
pub(crate) enum Grab {
    Pin(usize),
    /// Move every currently selected Puppet pin together.
    Selected,
    All,
    Rotate,
}

/// CSP-style "Click pin" tool property: clicking a Puppet pin either
/// selects it or deletes it outright.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PinTool {
    Select,
    Delete,
}

impl PinTool {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Select => "Select pin",
            Self::Delete => "Delete pin",
        }
    }
}

/// What a click on a pin does when it is already selected (or not).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PinSelect {
    Replace,
    Add,
    Subtract,
    Toggle,
}

impl PinSelect {
    pub(crate) fn label(self) -> &'static str {
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
pub(crate) struct XformState {
    pub(crate) mode: XformMode,
    /// Frame pins [TL, TR, BR, BL] plus any Puppet pins the user adds.
    pub(crate) pins: Vec<(f32, f32)>,
    /// Canvas anchors appended to `pins` when the mesh was built.
    pub(crate) mesh_anchors: Vec<(f32, f32)>,
    /// Triangle topology over `pins` followed by `mesh_anchors`. Rebuilt
    /// only when pins are added/removed (Delaunay on base positions),
    /// never per frame.
    pub(crate) tris: Vec<[u32; 3]>,
    /// Content bounds captured when the mode was armed: the frame pins
    /// start on these corners and the mesh lattice hugs them.
    pub(crate) bounds: Option<(f32, f32, f32, f32)>,
    /// Mesh lattice points (canvas edges + bounds-bounded interior).
    pub(crate) grid: Vec<(f32, f32)>,
    /// Extra margin (canvas px) around the content bounds for thin
    /// strokes that need more reference space, as in CSP's mesh expand.
    pub(crate) expand: f32,
    /// Interior cell counts picked from the panel (1..=8 per axis).
    pub(crate) mesh_cols: u32,
    pub(crate) mesh_rows: u32,
    /// Total lattice cell counts for the current grid (includes the
    /// canvas-edge cells outside the content bounds).
    pub(crate) grid_cols: u32,
    pub(crate) grid_rows: u32,
    pub(crate) grab: Option<Grab>,
    /// Pointer position (canvas coords) where the current grab began:
    /// drags move the pin by (pointer now - grab_at), so a long-press
    /// with no movement never writes a phantom position.
    pub(crate) grab_at: (f32, f32),
    /// Points as they were when the current grab started; a release
    /// applies only when they differ (drag-away-and-back included),
    /// cancel restores them, and the preview maps pins back onto them so
    /// content keeps following the pin across gestures.
    pub(crate) grab_from: Vec<(f32, f32)>,
    /// Skew drag axis for the current gesture: `Some(true)` slides a row
    /// horizontally, `Some(false)` slides a column vertically.
    pub(crate) skew_lock: Option<bool>,
    /// Most recently added/selected Puppet pin (the orange ring pin).
    pub(crate) active_pin: Option<usize>,
    /// Puppet pins selected for a group move. Indices match `pins`.
    pub(crate) selected_pins: Vec<usize>,
    /// CSP tool-property state for Puppet pin clicks.
    pub(crate) pin_tool: PinTool,
    pub(crate) pin_select: PinSelect,
    /// Live marquee in canvas coordinates (start, current) while the
    /// pointer drags from outside the mesh.
    pub(crate) marquee: Option<((f32, f32), (f32, f32))>,
    /// Live-preview session base (captured lazily; see [`xform_preview`]).
    pub(crate) base: Option<XformBase>,
    /// Document revision and active layer when the session was armed.
    /// Used before the first base capture, when there is no base revision
    /// to compare against, so undo/layer changes still discard stale pins.
    pub(crate) session_rev: u64,
    pub(crate) session_layer: usize,
    /// Whether the current gesture already pushed its undo checkpoint.
    pub(crate) checkpointed: bool,
}

impl XformState {
    pub(crate) fn off() -> Self {
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

    pub(crate) fn points(&self) -> &[(f32, f32)] {
        match self.mode {
            XformMode::Mesh => &self.grid,
            XformMode::Off => &[],
            _ => &self.pins,
        }
    }

    pub(crate) fn points_mut(&mut self) -> &mut [(f32, f32)] {
        match self.mode {
            XformMode::Mesh => &mut self.grid,
            XformMode::Off => &mut [],
            _ => &mut self.pins,
        }
    }

    pub(crate) fn grabbed_pin(&self) -> Option<usize> {
        match self.grab {
            Some(Grab::Pin(i)) => Some(i),
            _ => None,
        }
    }

    pub(crate) fn pin_selected(&self, i: usize) -> bool {
        self.selected_pins.contains(&i)
    }

    pub(crate) fn toggle_pin_selection(&mut self, i: usize) {
        if let Some(pos) = self.selected_pins.iter().position(|&s| s == i) {
            self.selected_pins.remove(pos);
        } else {
            self.selected_pins.push(i);
            self.selected_pins.sort_unstable();
        }
    }

    /// Apply the panel's click-select rule. Returns whether the pin is
    /// selected afterwards; an unselected pin must not begin a drag.
    pub(crate) fn click_pin(&mut self, i: usize, shift: bool) -> bool {
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
    pub(crate) fn prepare_pin_drag(&mut self, i: usize, shift: bool) {
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

    pub(crate) fn post_selection_active(&mut self, i: usize) {
        if self.pin_selected(i) {
            self.active_pin = Some(i);
        } else {
            self.active_pin = self.selected_pins.last().copied();
        }
    }

    /// Finish a marquee: Shift adds to the current selection; otherwise
    /// the panel's Select pins rule decides.
    pub(crate) fn marquee_select(&mut self, shift: bool) {
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

    pub(crate) fn begin_grab(&mut self, i: usize, at: (f32, f32)) {
        self.grab = Some(Grab::Pin(i));
        self.grab_at = at;
        self.grab_from = self.points().to_vec();
        self.checkpointed = false;
        self.skew_lock = None;
        if self.mode == XformMode::Puppet {
            self.active_pin = Some(i);
        }
    }

    pub(crate) fn begin_selected(&mut self, at: (f32, f32)) {
        if self.selected_pins.is_empty() {
            return;
        }
        self.grab = Some(Grab::Selected);
        self.grab_at = at;
        self.grab_from = self.points().to_vec();
        self.checkpointed = false;
        self.skew_lock = None;
    }

    pub(crate) fn begin_all(&mut self, at: (f32, f32)) {
        self.grab = Some(Grab::All);
        self.grab_at = at;
        self.grab_from = self.points().to_vec();
        self.checkpointed = false;
        self.skew_lock = None;
    }

    pub(crate) fn begin_rotate(&mut self, at: (f32, f32)) {
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
    pub(crate) fn drag_by_mod(&mut self, dx: f32, dy: f32, shift: bool, alt: bool) {
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
    pub(crate) fn drag_scale_mod(
        &mut self,
        i: usize,
        dx: f32,
        dy: f32,
        lock_aspect: bool,
        from_center: bool,
    ) {
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
    pub(crate) fn drag_rotate(&mut self, dx: f32, dy: f32, snap: bool) {
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

    pub(crate) fn drag_skew(&mut self, dx: f32, dy: f32) {
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

    pub(crate) fn drag_perspective(&mut self, dx: f32, dy: f32) {
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
    pub(crate) fn cancel_grab(&mut self) {
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
    pub(crate) fn reset_transform(&mut self, doc: &mut Document) {
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

    pub(crate) fn rebuild_tris(&mut self, doc: &Document) {
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
    pub(crate) fn arm(&mut self, mode: XformMode, doc: &Document) {
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

    pub(crate) fn set_mesh_cells(&mut self, cols: u32, rows: u32, doc: &Document) {
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

    pub(crate) fn mesh_point_active(&self, i: usize) -> bool {
        match (self.bounds, self.grid.get(i)) {
            (Some(b), Some(&(x, y))) => {
                x >= b.0 - 0.5 && x <= b.2 + 0.5 && y >= b.1 - 0.5 && y <= b.3 + 0.5
            }
            _ => false,
        }
    }

    pub(crate) fn add_pin(&mut self, at: (f32, f32), doc: &Document) -> bool {
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

    pub(crate) fn remove_pin(&mut self, i: usize, doc: &mut Document) -> bool {
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

    pub(crate) fn remove_pin_near(
        &mut self,
        at: (f32, f32),
        radius: f32,
        doc: &mut Document,
    ) -> bool {
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

    pub(crate) fn clear_pins(&mut self, doc: &mut Document) {
        if self.mode == XformMode::Puppet {
            // Clearing every pin removes every pin's effect: restore the
            // pristine session base, then reseed the frame pins.
            self.reset_transform(doc);
        }
    }

    pub(crate) fn point_in_frame(&self, at: (f32, f32)) -> bool {
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

    pub(crate) fn rotate_handle(&self, zoom: f32) -> Option<(f32, f32)> {
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
