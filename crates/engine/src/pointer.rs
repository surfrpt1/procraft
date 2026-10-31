//! Pointer-input math shared by mouse, touch and pen: wheel-axis remap
//! and view-pan clamping. Pure functions (no GUI types) so the app canvas
//! shares exactly the logic the host tests pin down.

/// Shift+wheel means horizontal pan (the backend passes the wheel
/// through untouched, so the app remaps it). A genuine horizontal tick
/// (trackpad) is left alone; without shift everything passes through.
pub fn shift_wheel_remap(dx: f32, dy: f32, shift: bool) -> (f32, f32) {
    if shift && dy != 0.0 && dx == 0.0 {
        (dy, 0.0)
    } else {
        (dx, dy)
    }
}

/// Ctrl+middle-drag zoom (pen-button workflow): vertical drag distance
/// maps onto the same x1.25 wheel step the bar buttons use, so every zoom
/// path feels identical. `dy` is screen px dragged (negative = up = in).
/// Pure so the app canvas shares exactly what host tests pin down.
pub fn zoom_drag_factor(dy: f32) -> f32 {
    1.25f32.powf(-dy / 120.0)
}

/// Brush-size drag (Shift by default, remappable): horizontal travel
/// scales the paint size on the same x1.25/120px step the wheel zoom
/// and Ctrl+MMB use, so all three gestures feel identical. `dx` is
/// screen px dragged (positive = right = grow).
pub fn size_drag_factor(dx: f32) -> f32 {
    1.25f32.powf(dx / 120.0)
}

/// Pan the view by `delta`, clamped so a 60px margin of canvas always
/// stays on screen (the canvas can never get lost off-view). `cw`/`ch`
/// are canvas size in screen px, `aw`/`ah` the viewport size.
pub fn pan_offset(
    offset: (f32, f32),
    delta: (f32, f32),
    cw: f32,
    ch: f32,
    aw: f32,
    ah: f32,
) -> (f32, f32) {
    (
        safe_clamp(offset.0 + delta.0, -cw + 60.0, aw - 60.0),
        safe_clamp(offset.1 + delta.1, -ch + 60.0, ah - 60.0),
    )
}

/// `f32::clamp` panics when `lo > hi`, which happens whenever the
/// viewport is narrower than the 120px canvas margin (first layout frame,
/// split-screen, tiny windows). Ordering the bounds keeps the canvas
/// between them instead of crashing; non-finite offsets recenter.
#[inline]
fn safe_clamp(val: f32, bound_a: f32, bound_b: f32) -> f32 {
    if !val.is_finite() {
        return 0.0;
    }
    val.clamp(bound_a.min(bound_b), bound_a.max(bound_b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_wheel_moves_horizontal() {
        // Plain wheel passes through untouched.
        assert_eq!(shift_wheel_remap(0.0, 40.0, false), (0.0, 40.0));
        // Shift turns a vertical tick into a horizontal one.
        assert_eq!(shift_wheel_remap(0.0, 40.0, true), (40.0, 0.0));
        // A real horizontal tick (trackpad) is left alone, shift or not.
        assert_eq!(shift_wheel_remap(12.0, 0.0, true), (12.0, 0.0));
        assert_eq!(shift_wheel_remap(0.0, 0.0, true), (0.0, 0.0));
    }

    #[test]
    fn zoom_drag_factor_matches_wheel_steps() {
        // No movement: identity.
        assert_eq!(zoom_drag_factor(0.0), 1.0);
        // 120px up: one wheel-step in (x1.25, same step as the bar buttons).
        assert_eq!(zoom_drag_factor(-120.0), 1.25);
        // 120px down: one wheel-step out.
        assert!((zoom_drag_factor(120.0) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn size_drag_factor_mirrors_the_zoom_step() {
        assert_eq!(size_drag_factor(0.0), 1.0);
        assert_eq!(size_drag_factor(120.0), 1.25); // drag right = grow
        assert!((size_drag_factor(-120.0) - 0.8).abs() < 1e-6);
        assert!(size_drag_factor(60.0) > 1.0 && size_drag_factor(60.0) < 1.25);
    }

    #[test]
    fn pan_offset_clamps_to_canvas_with_margin() {
        // Canvas 800x600 at zoom 1 in a 1000x800 view: x in [-740, 940].
        assert_eq!(
            pan_offset((0.0, 0.0), (100.0, 50.0), 800.0, 600.0, 1000.0, 800.0),
            (100.0, 50.0)
        );
        // Far drags pin at the 60px margin, never lose the canvas.
        assert_eq!(
            pan_offset((0.0, 0.0), (5000.0, -5000.0), 800.0, 600.0, 1000.0, 800.0),
            (940.0, -540.0)
        );
        // Small canvas in a big view still clamps symmetric margins.
        assert_eq!(
            pan_offset((0.0, 0.0), (-5.0, -5.0), 100.0, 100.0, 1000.0, 800.0),
            (-5.0, -5.0)
        );
    }

    #[test]
    fn pan_offset_survives_viewports_narrower_than_the_margin() {
        // Inverted bounds (-cw + 60 > aw - 60) used to panic inside
        // f32::clamp: a 50px canvas in a 40px view, and a zero-size view
        // on the first layout frame.
        assert_eq!(
            pan_offset((0.0, 0.0), (10.0, -10.0), 50.0, 50.0, 40.0, 40.0),
            (10.0, -10.0)
        );
        assert_eq!(
            pan_offset((0.0, 0.0), (10.0, -10.0), 50.0, 50.0, 0.0, 0.0),
            (10.0, -10.0)
        );
        assert_eq!(
            pan_offset((0.0, 0.0), (0.0, 0.0), 0.0, 0.0, 0.0, 0.0),
            (0.0, 0.0)
        );
    }

    #[test]
    fn pan_offset_recenters_on_non_finite_input() {
        assert_eq!(
            pan_offset((f32::NAN, 0.0), (10.0, 10.0), 800.0, 600.0, 1000.0, 800.0),
            (0.0, 10.0)
        );
        assert_eq!(
            pan_offset(
                (0.0, 0.0),
                (f32::INFINITY, f32::NEG_INFINITY),
                800.0,
                600.0,
                1000.0,
                800.0
            ),
            (0.0, 0.0)
        );
    }
}
