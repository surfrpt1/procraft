use crate::*;

/// Per-point stabilizer coefficient for an `n`-point input trail: applying
/// it `n` times composes to the same per-frame convergence as one step of
/// `a` (the single-step coefficient), so trail stamping never changes the
/// stabilizer's feel. `n <= 1` returns `a` bit-for-bit.
pub(crate) fn stabilizer_substep(a: f32, n: usize) -> f32 {
    if n <= 1 || a <= 0.0 || a >= 1.0 {
        return a;
    }
    1.0 - (1.0 - a).powf(1.0 / n as f32)
}

pub(crate) const SPEED_NORM_PX_PER_SEC: f32 = 2000.0;

pub(crate) fn sample_pointer(
    smooth_pt: Option<(f32, f32)>,
    last_target: Option<(f32, f32)>,
    target: (f32, f32),
    a: f32,
    dt: f32,
    zoom: f32,
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
            // Target is canvas space; velocity dynamics are a hand motion,
            // so convert to screen pixels before normalizing or speed
            // tapering would change with every zoom level.
            (dist * zoom / dt.max(f32::EPSILON) / SPEED_NORM_PX_PER_SEC).clamp(0.0, 1.0)
        }
        None => 0.0,
    };
    (smooth, speed)
}

/// Quadratic piece to stamp: (start, control, end).
pub(crate) type StrokePiece = ((f32, f32), (f32, f32), (f32, f32));

/// Midpoint-smoothed stroke path (classic quadratic smoothing): stamps a
/// curve through the midpoints of consecutive input samples, each sample
/// as the control point, so sparse samples (60 Hz vsync cap, OS mouse
/// coalescing) still ink as curves instead of chords. The pen trails the
/// raw pointer by half a sample and never overshoots it.
pub(crate) struct StrokeCurve {
    /// Last emitted curve position; None until the stroke's first point.
    pub(crate) e: Option<(f32, f32)>,
    /// Last raw input point.
    pub(crate) b: (f32, f32),
}

impl StrokeCurve {
    pub(crate) fn new() -> Self {
        Self {
            e: None,
            b: (0.0, 0.0),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.e = None;
    }

    /// Feed the next raw point; returns the quadratic piece (start,
    /// control, end) to stamp. None for the first point of a stroke
    /// (that one stamps a plain dab).
    pub(crate) fn push(&mut self, c: (f32, f32)) -> Option<StrokePiece> {
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
    pub(crate) fn finish(&mut self) -> Option<((f32, f32), (f32, f32))> {
        let e = self.e.take()?;
        Some((e, self.b))
    }
}

pub(crate) fn stroke_mid(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

/// Point on the quadratic Bezier `p0 -> c -> p1` at `t` in `0..=1`.
pub(crate) fn stroke_quad_pt(p0: (f32, f32), c: (f32, f32), p1: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    (
        u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
        u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
    )
}

/// Linear sub-steps to stamp a curve piece of `len` px: one per ~4 px so
/// each stamped sub-step is visually straight; capped for giant jumps.
pub(crate) fn stroke_steps(len: f32) -> usize {
    (len / 4.0).ceil().clamp(1.0, 64.0) as usize
}

/// How long one frame may spend stamping a live stroke. A dab is ~35 us on
/// an 8K canvas, so a full-canvas flick (thousands of dabs) would block for
/// most of a second if stamped in one frame; past this the batch tail waits
/// for the next frame (see `pending_pts`).
pub(crate) const STAMP_BUDGET: std::time::Duration = std::time::Duration::from_millis(5);

/// Seconds between autosave recovery snapshots.
pub(crate) const AUTOSAVE_SECS: f64 = 120.0;

#[derive(Clone, Copy)]
pub(crate) struct RecordedSegment {
    pub(crate) from: Option<(f32, f32)>,
    pub(crate) from_dyn: DabDynamics,
    pub(crate) to: (f32, f32),
    pub(crate) to_dyn: DabDynamics,
}

#[derive(Default)]
pub(crate) struct RecordedStroke {
    pub(crate) segments: Vec<RecordedSegment>,
    pub(crate) total_len: f32,
}

impl RecordedStroke {
    pub(crate) fn reset(&mut self) {
        self.segments.clear();
        self.total_len = 0.0;
    }

    pub(crate) fn push(
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

/// Taper factor for a live dab ending `leg` px after `travelled` px of
/// stroke. The end ramp is not knowable while the pen is down; it is applied
/// once by [`taper_replay`] on release. Asking for it live would paint the
/// whole stroke thin and repaint it full-width at lift.
pub(crate) fn live_taper_at(brush: &Brush, travelled: f32, leg: f32) -> f32 {
    brush.taper.live_factor(travelled + leg, brush.size)
}

/// A taper replay in progress. A long 8K stroke is thousands of dabs, so
/// re-stamping it with the end ramp in one go froze the frame the pen was
/// lifted on; [`taper_step`] spends one frame's stamping budget at a time
/// instead, and the canvas shows the partial redraw.
pub(crate) struct TaperReplay {
    pub(crate) brush: Brush,
    pub(crate) color: Color,
    /// Next recorded segment to stamp and the path length before it.
    pub(crate) cursor: usize,
    pub(crate) travelled: f32,
    pub(crate) leftover: f32,
    /// Content revision after the last step. Anything else changing pixels
    /// (undo, a new stroke) cancels the replay rather than stamping over it.
    pub(crate) rev: u64,
}

/// What one [`taper_step`] did.
pub(crate) enum TaperStep {
    More,
    Done,
    /// The document moved on (undo, a new stroke): the replay is void and
    /// its stroke state must be left alone.
    Cancelled,
}

/// Start a taper replay: rewind the live stroke to the checkpoint inside
/// `rect` (the bbox it painted) and prepare to redraw it with the end ramp.
/// The start ramp already matches because live dabs use the same distance
/// scale. The checkpoint stays on the undo stack, so one undo still
/// removes the whole stroke.
pub(crate) fn taper_begin(
    doc: &mut Document,
    brush: &Brush,
    color: Color,
    stroke: &RecordedStroke,
    rect: Option<(u32, u32, u32, u32)>,
) -> Option<TaperReplay> {
    if stroke.segments.is_empty() || stroke.total_len <= 0.0 {
        return None;
    }
    if brush.taper.start <= 0.0 && brush.taper.end <= 0.0 {
        return None;
    }
    let restored = match rect {
        Some(rect) => doc.restore_checkpoint_region(rect),
        None => doc.restore_last_checkpoint(),
    };
    if !restored {
        return None;
    }
    Some(TaperReplay {
        brush: brush.clone(),
        color,
        cursor: 0,
        travelled: 0.0,
        leftover: 0.0,
        rev: doc.revision(),
    })
}

/// Stamp the replay for at most `STAMP_BUDGET`, resuming exactly where the
/// last call stopped. Segments are independent in order, so slicing the
/// work across frames paints the same bytes a single pass would.
pub(crate) fn taper_step(
    doc: &mut Document,
    job: &mut TaperReplay,
    stroke: &RecordedStroke,
) -> TaperStep {
    taper_step_with(STAMP_BUDGET, doc, job, stroke)
}

pub(crate) fn taper_step_with(
    budget: std::time::Duration,
    doc: &mut Document,
    job: &mut TaperReplay,
    stroke: &RecordedStroke,
) -> TaperStep {
    if doc.revision() != job.rev {
        return TaperStep::Cancelled;
    }
    let started = std::time::Instant::now();
    while job.cursor < stroke.segments.len() {
        let segment = &stroke.segments[job.cursor];
        let leg = match segment.from {
            Some((fx, fy)) => ((segment.to.0 - fx).powi(2) + (segment.to.1 - fy).powi(2)).sqrt(),
            None => 0.0,
        };
        let from_dist = job.travelled;
        job.travelled += leg;
        let to_dist = job.travelled;
        let mut from_dyn = segment.from_dyn;
        from_dyn.taper = job.brush.taper.distance_factor(
            from_dist,
            stroke.total_len - from_dist,
            job.brush.size,
        );
        let mut to_dyn = segment.to_dyn;
        to_dyn.taper =
            job.brush
                .taper
                .distance_factor(to_dist, stroke.total_len - to_dist, job.brush.size);
        let (_, lo) = doc.stroke_to_dyn(
            &job.brush,
            job.color,
            segment.from,
            from_dyn,
            segment.to,
            to_dyn,
            job.leftover,
        );
        job.leftover = lo;
        job.cursor += 1;
        if started.elapsed() >= budget && job.cursor < stroke.segments.len() {
            job.rev = doc.revision();
            return TaperStep::More;
        }
    }
    TaperStep::Done
}

/// Flush the smoothed stroke's final tail (curve cursor -> last raw
/// sample) at pointer release. Only the painting branch owns this: the
/// smudge tool advances its own last-point chain and must never run a
/// brush flush from the shared curve state (that drew a stale brush
/// segment from the smudge position to an earlier brush stroke).
#[allow(clippy::too_many_arguments)]
pub(crate) fn flush_stroke_tail(
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
