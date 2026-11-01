use crate::*;

/// In-progress multi-finger tap (undo/redo): tracks the widest finger
/// count, start time/center, and whether it moved (a pan/pinch is not
/// a tap). Fires on clean release; see the top of `App::ui`.
pub(crate) struct TapTrack {
    pub(crate) n: usize,
    pub(crate) t0: f64,
    pub(crate) c0: egui::Pos2,
    pub(crate) moved: bool,
}

/// M49: how one key event resolves while capture mode is armed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CaptureOutcome {
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
pub(crate) fn resolve_capture_event(ev: &egui::Event) -> CaptureOutcome {
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
pub(crate) fn drag_size(base: f32, travel: f32) -> f32 {
    (base * size_drag_factor(travel)).clamp(BRUSH_SIZE_MIN, BRUSH_SIZE_MAX)
}

/// M49: the brush-size drag's travel in px for `mode`, measured from the
/// gesture's start point `anchor` to the current pointer `pos`.
/// Horizontal and vertical are signed (right/up grows); distance is the
/// straight-line length, so any move grows the brush.
pub(crate) fn size_drag_travel(mode: SizeDragMode, anchor: egui::Pos2, pos: egui::Pos2) -> f32 {
    match mode {
        SizeDragMode::Horizontal => pos.x - anchor.x,
        SizeDragMode::Vertical => anchor.y - pos.y,
        SizeDragMode::Distance => (pos - anchor).length(),
    }
}

/// M49: human label for the size-drag direction picker.
pub(crate) fn size_drag_mode_label(mode: SizeDragMode) -> &'static str {
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
pub(crate) fn space_hand_step(
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

/// M49: brush-size drag gesture state — the size at press plus the press
/// point; travel from `anchor` along the configured direction (see
/// [`SizeDragMode`]) drives the size from `base`, and the cursor ring
/// stays pinned at the anchor while the gesture runs.
pub(crate) struct SizeDrag {
    pub(crate) base: f32,
    pub(crate) anchor: egui::Pos2,
}
