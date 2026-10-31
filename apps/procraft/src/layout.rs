use crate::*;

/// Screen position of a vanishing-point handle: the true point when it is
/// on-canvas, otherwise clamped to the canvas edge (with a margin) so
/// off-canvas VPs — like the 2-point defaults — stay visible and grabbable.
/// Returns the position plus whether it was clamped.
pub(crate) fn vp_handle_pos(rect: egui::Rect, z: f32, vx: f32, vy: f32) -> (egui::Pos2, bool) {
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
pub(crate) const SLIDER_W: f32 = 100.0;

pub(crate) fn stepped_slider<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    step: f64,
    text: &str,
) {
    stepped_slider_inner(ui, value, range, step, text, false);
}

pub(crate) fn stepped_slider_label_end<Num: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut Num,
    range: std::ops::RangeInclusive<Num>,
    step: f64,
    text: &str,
) {
    stepped_slider_inner(ui, value, range, step, text, true);
}

pub(crate) fn stepped_slider_inner<Num: egui::emath::Numeric>(
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
pub(crate) fn quick_slider<Num: egui::emath::Numeric>(
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
pub(crate) const SWATCH: f32 = 24.0;

pub(crate) const SWATCH_GAP: f32 = 4.0;

/// Swatch grid at full height (never an inner scroller — the panel's own
/// scroll handles overflow, and separate scrollers with no explicit ids
/// share one scroll offset). Wrap width is pinned to 8 swatches per row
/// so Starter always lands 3x8 at any panel width.
pub(crate) fn swatch_grid(ui: &mut egui::Ui, mut add_contents: impl FnMut(&mut egui::Ui)) {
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
pub(crate) fn swatch_button(ui: &mut egui::Ui, rgb: [u8; 3], selected: bool) -> egui::Response {
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
pub(crate) fn downsample_half(full: &[u8], w: usize, h: usize) -> ColorImage {
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
pub(crate) fn rot_pt(p: egui::Pos2, c: egui::Pos2, ang: f32) -> egui::Pos2 {
    let (s, co) = ang.sin_cos();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    egui::Pos2::new(c.x + dx * co - dy * s, c.y + dx * s + dy * co)
}

/// Inverse of [`rot_pt`] (screen back to the unrotated view).
pub(crate) fn unrot_pt(p: egui::Pos2, c: egui::Pos2, ang: f32) -> egui::Pos2 {
    rot_pt(p, c, -ang)
}

/// Workspace dot grid spacing in canvas px. Steps snap to powers of two so
/// the dots land on the same points as the zoom crosses an octave.
pub(crate) const BOARD_DOT_STEP: f32 = 32.0;

pub(crate) const BOARD_DOT_SCREEN_MIN: f32 = 24.0;

pub(crate) const BOARD_DOT_SCREEN_MAX: f32 = 48.0;

/// Breathing room the fit zoom leaves around the page so the board stays
/// visible instead of the sheet pinning itself to the viewport edge.
pub(crate) const BOARD_FIT_MARGIN: f32 = 28.0;

pub(crate) fn board_fit_zoom(w: f32, h: f32, avail: egui::Vec2) -> f32 {
    let (w, h) = (w.max(1.0), h.max(1.0));
    ((avail.x - 2.0 * BOARD_FIT_MARGIN) / w)
        .min((avail.y - 2.0 * BOARD_FIT_MARGIN) / h)
        .clamp(0.05, 8.0)
}

/// Offset that centres content smaller than the viewport, leaving the board
/// visible on both sides (the page floats in the middle instead of pinning
/// to the top-left corner). Zero when the content overflows, so a zoomed-in
/// page keeps the viewport origin.
pub(crate) fn center_offset(content: egui::Vec2, avail: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        ((avail.x - content.x) * 0.5).max(0.0),
        ((avail.y - content.y) * 0.5).max(0.0),
    )
}

/// The page rect that is drawn and sensed. The layout allocates a
/// page-sized slot, then the board centring and the touch pan move it;
/// using the slot as the hit target instead leaves the page's right and
/// bottom strips (and anything panned) unable to start a stroke.
pub(crate) fn canvas_page_rect(slot: egui::Rect, avail: egui::Vec2, pan: egui::Vec2) -> egui::Rect {
    slot.translate(center_offset(slot.size(), avail))
        .translate(pan)
}

/// Land the canvas origin on a physical pixel when the view sits on a whole
/// device-pixel zoom (100 %, 200 % ...). A fractional origin makes the GPU
/// bilinearly resample every edge on the page — the raster is sharp, the
/// picture is not. Fractional zooms and rotation are left alone: there is no
/// grid to land on, and snapping would fight the pan.
pub(crate) fn snap_canvas_origin(min: egui::Pos2, zoom: f32, ppp: f32, angle: f32) -> egui::Pos2 {
    if angle != 0.0 || !ppp.is_finite() || ppp <= 0.0 {
        return min;
    }
    let device = zoom * ppp;
    if (device - device.round()).abs() > 1e-3 {
        return min;
    }
    egui::pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp)
}

pub(crate) fn board_dot_step(zoom: f32) -> f32 {
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
pub(crate) fn board_dot_span(step: f32, lo: f32, hi: f32) -> (i32, i32) {
    ((lo / step).ceil() as i32, (hi / step).floor() as i32)
}

/// Canvas-fixed dots on the workspace board. They pan and rotate with the
/// page and are skipped wherever the page covers them, so the artwork and
/// its texture stay untouched.
pub(crate) fn paint_board_dots(
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
pub(crate) fn brush_ring_radius(size: f32, zoom: f32) -> f32 {
    (size * zoom * 0.5).max(2.0)
}

pub(crate) const WIDE_BAR_W: f32 = 600.0;

pub(crate) fn bar_layout(width: f32, mobile: bool) -> BarLayout {
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
pub(crate) fn forced_layout() -> Option<BarLayout> {
    match std::env::var("PROCRAFT_FORCE_LAYOUT").ok()?.as_str() {
        "wide" => Some(BarLayout::Wide),
        "tablet" => Some(BarLayout::Tablet),
        "narrow" => Some(BarLayout::Narrow),
        _ => None,
    }
}

pub(crate) fn resolve_layout(forced: Option<BarLayout>, width: f32, mobile: bool) -> BarLayout {
    forced.unwrap_or_else(|| bar_layout(width, mobile))
}

pub(crate) fn effective_layout(width: f32, mobile: bool) -> BarLayout {
    resolve_layout(forced_layout(), width, mobile)
}

/// Presentation shell: desktop keeps the docked chrome, mobile floats it over
/// a full-screen canvas. Native mobile is the only automatic source; the env
/// override exists so host builds can be screenshotted in the touch shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Shell {
    Desktop,
    Mobile,
}

pub(crate) fn forced_shell() -> Option<bool> {
    match std::env::var("PROCRAFT_FORCE_SHELL").ok()?.as_str() {
        "mobile" => Some(true),
        "desktop" => Some(false),
        _ => None,
    }
}

pub(crate) fn resolve_shell(forced: Option<bool>, native_mobile: bool) -> Shell {
    if forced.unwrap_or(native_mobile) {
        Shell::Mobile
    } else {
        Shell::Desktop
    }
}

pub(crate) fn mobile_shell() -> bool {
    resolve_shell(
        forced_shell(),
        cfg!(any(target_os = "ios", target_os = "android")),
    ) == Shell::Mobile
}

/// Which slide-over is open in the mobile shell. Session-only: the desktop
/// dock keeps owning the persisted `show_tools`/`show_layers` flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MobileDrawer {
    None,
    Tools,
    Layers,
}

impl MobileDrawer {
    /// Tapping the open drawer's trigger closes it; any other trigger swaps.
    pub(crate) fn toggled(self, other: MobileDrawer) -> MobileDrawer {
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
pub(crate) enum StripDock {
    Left,
    Center,
    Right,
}

impl StripDock {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            StripDock::Left => "left",
            StripDock::Center => "center",
            StripDock::Right => "right",
        }
    }

    pub(crate) fn from_str(s: &str) -> StripDock {
        match s {
            "left" => StripDock::Left,
            "right" => StripDock::Right,
            _ => StripDock::Center,
        }
    }
}

/// Drawer width on touch screens: wide enough for the two-column cards,
/// narrow enough to leave a visible strip of canvas and the scrim.
pub(crate) fn mobile_drawer_width(viewport_w: f32) -> f32 {
    (viewport_w * 0.88).clamp(240.0, 360.0)
}

/// Phones give the drawer slightly more room: there is no top-left island
/// to peek around.
pub(crate) fn phone_drawer_width(viewport_w: f32) -> f32 {
    (viewport_w * 0.92).clamp(240.0, 360.0)
}

/// Paint tools that stay one tap away in the phone strip.
pub(crate) const PHONE_STRIP_TOOLS: [Tool; 3] = [Tool::Brush, Tool::Eraser, Tool::Smudge];

/// Bottom safe inset in points. iOS reports the home indicator; Android
/// hosts keep 0 until the platform bridge exposes one.
pub(crate) fn safe_bottom(mobile: bool) -> f32 {
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
pub(crate) fn forced_safe_top() -> Option<f32> {
    safe_inset_override(std::env::var("PROCRAFT_SAFE_TOP").ok().as_deref())
}

pub(crate) fn safe_inset_override(value: Option<&str>) -> Option<f32> {
    value?.trim().parse::<u32>().ok().map(clamp_status_inset)
}

/// Y offset (negative, for a bottom anchor) of the phone strip.
pub(crate) fn phone_strip_offset(safe_bottom: f32, pad: f32) -> f32 {
    -(safe_bottom + pad)
}

/// Y offset (negative, for a bottom anchor) of the options capsule: above
/// the strip and its gap on phones, directly above the inset elsewhere.
pub(crate) fn phone_capsule_offset(safe_bottom: f32, pad: f32, strip_h: f32, gap: f32) -> f32 {
    -(safe_bottom + pad + strip_h + gap)
}

/// Preferred width of a minimised bar (the phone's pocket pill and the
/// tablet's folded options capsule): grip, active tool, expand.
pub(crate) const PHONE_MINI_STRIP_W: f32 = 260.0;

/// Width of the drag grip: a dot column with a finger's slack on both sides.
pub(crate) const STRIP_GRIP_W: f32 = 24.0;

pub(crate) fn strip_mini_width(viewport_w: f32, pad: f32) -> f32 {
    (viewport_w - 2.0 * pad).clamp(180.0, PHONE_MINI_STRIP_W)
}

/// Widest left edge that still keeps a bar on screen.
pub(crate) fn strip_max_left(viewport_w: f32, pad: f32, width: f32) -> f32 {
    (viewport_w - pad - width).max(pad)
}

/// Left edge of a bar for a docked position.
pub(crate) fn strip_left(dock: StripDock, viewport_w: f32, pad: f32, width: f32) -> f32 {
    let max_left = strip_max_left(viewport_w, pad, width);
    match dock {
        StripDock::Left => pad,
        StripDock::Center => ((viewport_w - width) * 0.5).clamp(pad, max_left),
        StripDock::Right => max_left,
    }
}

/// Keep a dragged bar fully on screen.
pub(crate) fn strip_left_clamped(left: f32, viewport_w: f32, pad: f32, width: f32) -> f32 {
    left.clamp(pad, strip_max_left(viewport_w, pad, width))
}

/// Where a bar sits this frame: its dock plus any in-flight drag, clamped.
pub(crate) fn strip_left_now(
    dock: StripDock,
    offset: f32,
    viewport_w: f32,
    pad: f32,
    width: f32,
) -> f32 {
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
pub(crate) fn strip_dock_for(left: f32, viewport_w: f32, pad: f32, width: f32) -> StripDock {
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
pub(crate) fn strip_grip(ui: &mut egui::Ui, t: &design::Tokens, h: f32) -> egui::Response {
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
pub(crate) struct PhoneRoom {
    pub(crate) safe_top: f32,
    pub(crate) safe_bottom: f32,
    pub(crate) pad: f32,
    pub(crate) strip_h: f32,
    pub(crate) island_h: f32,
    pub(crate) gap: f32,
}

/// Below this span the phone drawer drops its strip gap and runs to the
/// bottom inset, so landscape keeps a usable panel height.
pub(crate) const DRAWER_TIGHT_SPAN: f32 = 320.0;

/// Drawer window `(top, height)` on phones: no top island, bottom clamped
/// above the strip unless the drawer is too short to fit there.
pub(crate) fn phone_drawer_span(viewport_bottom: f32, room: PhoneRoom) -> (f32, f32) {
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
pub(crate) fn mobile_capsule_width(viewport_w: f32) -> f32 {
    (viewport_w * 0.94).min(760.0)
}

/// Height of a floating cluster or capsule: one control row plus the
/// frame's 6 pt inner margin on both sides.
pub(crate) fn mobile_island_height(control_h: f32) -> f32 {
    control_h + 12.0
}

/// Drawer window `(top, height)`: starts below the floating clusters and
/// ends above the options capsule, so no drawer control hides under the
/// islands.
pub(crate) fn mobile_drawer_span(
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
pub(crate) fn clamp_status_inset(px: u32) -> f32 {
    (px as f32).clamp(0.0, 200.0)
}

/// Status toast timing: long enough to read one line, short enough that it
/// is gone before the next stroke.
pub(crate) const TOAST_HOLD: f32 = 2.5;

pub(crate) const TOAST_FADE: f32 = 0.4;

/// Toast opacity `age` seconds after the status changed.
pub(crate) fn toast_alpha(age: f32) -> f32 {
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
pub(crate) fn toast_offset(capsule_offset: f32, island_h: f32, gap: f32, has_capsule: bool) -> f32 {
    capsule_offset - if has_capsule { island_h + gap } else { 0.0 }
}

/// How far a drawer still has to travel: fully past the screen edge at 0,
/// flush with it at 1.
pub(crate) fn drawer_slide_distance(progress: f32, width: f32, pad: f32) -> f32 {
    (1.0 - progress.clamp(0.0, 1.0)) * (width + 2.0 * pad)
}

/// Mobile dialogs: at most 92% of the viewport in either axis, and short
/// enough that centring it still clears the status-bar strip. Desktop
/// windows keep their own sizing.
/// How a mobile sheet is presented: fitted size, the anchor that centres it
/// in the canvas area above the bottom chrome, and the drawer card frame.
pub(crate) struct MobileModal {
    pub(crate) fit: egui::Vec2,
    pub(crate) anchor: egui::Vec2,
    pub(crate) frame: egui::Frame,
}

pub(crate) fn modal_fit(
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
pub(crate) fn mobile_modal_anchor(
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
pub(crate) fn fitted_modal<'a>(
    win: egui::Window<'a>,
    sheet: Option<&MobileModal>,
) -> egui::Window<'a> {
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
pub(crate) fn modal_scroll(
    ui: &mut egui::Ui,
    sheet: Option<&MobileModal>,
    body: impl FnOnce(&mut egui::Ui),
) {
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
pub(crate) fn menu_popup_room(from: f32, to: f32) -> f32 {
    (to - from - 16.0).max(200.0)
}

/// Fade height for a scrollable popup body: nothing when the list already
/// ends in view, otherwise a short gradient so the last row reads as
/// continuing rather than clipped.
pub(crate) fn menu_fade_height(content_h: f32, viewport_h: f32, offset_y: f32) -> f32 {
    let below = content_h - viewport_h - offset_y;
    if below <= 1.0 {
        0.0
    } else {
        MENU_FADE_H.min(viewport_h * 0.25)
    }
}

pub(crate) const MENU_FADE_H: f32 = 24.0;

/// Paint `rect`'s bottom `height` points as a gradient into `fill`, over the
/// rows that are only half in view.
pub(crate) fn fade_bottom_edge(ui: &egui::Ui, rect: egui::Rect, fill: egui::Color32, height: f32) {
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
