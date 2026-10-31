use crate::*;

/// One Transform-layer panel press.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum TformOp {
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
pub(crate) const TFORM_SCALE_MIN: f32 = 0.25;

pub(crate) const TFORM_SCALE_MAX: f32 = 4.0;

/// Fold degrees into (-180, 180] so the angle slider never winds up.
pub(crate) fn wrap_degrees(deg: f32) -> f32 {
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
pub(crate) struct TformBase {
    pub(crate) pixels: Raster,
    pub(crate) mask: Option<Raster>,
    pub(crate) layer: usize,
    pub(crate) rev: u64,
    pub(crate) ox: i32,
    pub(crate) oy: i32,
}

/// Transform-layer button session state (see [`TformBase`]).
pub(crate) struct TformPress {
    pub(crate) base: Option<TformBase>,
    /// Cumulative uniform scale about the content center.
    pub(crate) scale: f32,
    /// Cumulative rotation in degrees.
    pub(crate) degrees: f32,
    pub(crate) flip_h: bool,
    pub(crate) flip_v: bool,
    /// The current gesture already opened its undo step; live slider
    /// frames share it instead of piling one snapshot per frame.
    pub(crate) checkpointed: bool,
}

impl TformPress {
    pub(crate) fn new() -> Self {
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
    pub(crate) fn end_gesture(&mut self) {
        self.checkpointed = false;
    }
}

/// True when the session base no longer matches the layer in front of it.
pub(crate) fn tform_base_stale(doc: &Document, t: &TformPress) -> bool {
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
pub(crate) fn tform_refresh(doc: &Document, t: &mut TformPress) {
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
pub(crate) fn flip_buf_h(buf: &mut [u8], w: usize, h: usize, stride: usize) {
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
pub(crate) fn flip_buf_v(buf: &mut [u8], w: usize, h: usize, stride: usize) {
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
pub(crate) fn flip_raster(r: &mut Raster, horizontal: bool) {
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
pub(crate) fn tform_apply(
    doc: &mut Document,
    t: &mut TformPress,
    op: TformOp,
    live: bool,
) -> XformApply {
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
pub(crate) fn tform_button(doc: &mut Document, t: &mut TformPress, op: TformOp) -> XformApply {
    let result = tform_apply(doc, t, op, false);
    t.end_gesture();
    result
}

/// Status line describing the session transform, e.g. `125%, 15 deg`.
pub(crate) fn tform_status(t: &TformPress) -> String {
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
