//! Design tokens and the style pipeline: one place owns every colour,
//! radius, spacing, control height and type size, per theme and per device
//! class. `lib.rs` only wires it; the primitives land here as they are
//! built.

use crate::{BarLayout, Theme};
use egui::{Color32, CornerRadius, FontId, Stroke, Style, TextStyle};

/// One design decision set for a (theme, device class) pair.
pub(crate) struct Tokens {
    pub(crate) class: BarLayout,
    pub(crate) surface_0: Color32,
    pub(crate) surface_1: Color32,
    pub(crate) surface_2: Color32,
    pub(crate) surface_3: Color32,
    pub(crate) stroke: Color32,
    pub(crate) text_hi: Color32,
    pub(crate) text_lo: Color32,
    pub(crate) accent: Color32,
    pub(crate) accent_soft: Color32,
    pub(crate) danger: Color32,
    pub(crate) radius_control: u8,
    pub(crate) radius_card: u8,
    pub(crate) pad_x: f32,
    pub(crate) pad_y: f32,
    pub(crate) pad_x_card: f32,
    pub(crate) pad_y_card: f32,
    pub(crate) gap: f32,
    pub(crate) control_h: f32,
    pub(crate) slider_track: f32,
    pub(crate) slider_width_wide: f32,
    pub(crate) slider_width_touch: f32,
    pub(crate) type_body: f32,
    pub(crate) type_small: f32,
    pub(crate) type_heading: f32,
}

pub(crate) fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// Dot colour for the workspace board behind the page: a faint tint over
/// `panel_fill`, lightening dark themes and darkening light ones. Alpha
/// stays low so the board never competes with the artwork.
pub(crate) fn board_dot(theme: Theme) -> Color32 {
    match theme {
        Theme::Dark => rgba(0xFF, 0xFF, 0xFF, 0x28),
        Theme::Light | Theme::Paper => rgba(0x10, 0x12, 0x16, 0x24),
    }
}

pub(crate) fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub(crate) fn tokens(theme: Theme, class: BarLayout) -> Tokens {
    let (surface_0, surface_1, surface_2, surface_3, stroke, text_hi, text_lo, accent, danger) =
        match theme {
            Theme::Dark => (
                rgb(0x13, 0x13, 0x15),
                rgb(0x1B, 0x1B, 0x1E),
                rgb(0x26, 0x26, 0x2A),
                rgb(0x33, 0x33, 0x3A),
                rgb(0x3D, 0x3D, 0x45),
                rgb(0xF0, 0xF0, 0xF3),
                rgb(0x9C, 0x9C, 0xA6),
                rgb(0x4C, 0x8D, 0xFF),
                rgb(0xFF, 0x6B, 0x6B),
            ),
            Theme::Light => (
                rgb(0xED, 0xEF, 0xF2),
                rgb(0xF7, 0xF8, 0xFA),
                rgb(0xFF, 0xFF, 0xFF),
                rgb(0xDF, 0xE3, 0xE9),
                rgb(0xCD, 0xD3, 0xDB),
                rgb(0x16, 0x18, 0x1C),
                rgb(0x62, 0x6A, 0x76),
                rgb(0x2F, 0x6F, 0xEB),
                rgb(0xC0, 0x39, 0x2B),
            ),
            Theme::Paper => (
                rgb(0xF4, 0xEE, 0xE4),
                rgb(0xFA, 0xF6, 0xEE),
                rgb(0xFF, 0xFD, 0xF8),
                rgb(0xE7, 0xDC, 0xC9),
                rgb(0xD6, 0xC9, 0xB3),
                rgb(0x2A, 0x23, 0x1B),
                rgb(0x6E, 0x62, 0x52),
                rgb(0xB5, 0x65, 0x1D),
                rgb(0xA6, 0x3A, 0x21),
            ),
        };

    let (
        control_h,
        gap,
        pad_x,
        pad_y,
        pad_x_card,
        pad_y_card,
        radius_control,
        radius_card,
        slider_track,
        type_body,
        type_small,
        type_heading,
    ) = match class {
        BarLayout::Wide => (26.0, 6.0, 6.0, 6.0, 6.0, 8.0, 8, 10, 6.0, 14.0, 12.0, 15.0),
        BarLayout::Tablet => (
            44.0, 8.0, 10.0, 10.0, 8.0, 10.0, 12, 14, 10.0, 15.0, 13.0, 16.0,
        ),
        BarLayout::Narrow => (
            44.0, 6.0, 10.0, 8.0, 8.0, 10.0, 12, 14, 10.0, 15.0, 13.0, 16.0,
        ),
    };

    Tokens {
        class,
        surface_0,
        surface_1,
        surface_2,
        surface_3,
        stroke,
        text_hi,
        text_lo,
        accent,
        accent_soft: rgba(accent.r(), accent.g(), accent.b(), 61),
        danger,
        radius_control,
        radius_card,
        pad_x,
        pad_y,
        pad_x_card,
        pad_y_card,
        gap,
        control_h,
        slider_track,
        slider_width_wide: 110.0,
        slider_width_touch: 130.0,
        type_body,
        type_small,
        type_heading,
    }
}

fn paint_widget(
    w: &mut egui::style::WidgetVisuals,
    weak_bg_fill: Color32,
    bg_fill: Color32,
    bg_stroke: Stroke,
    fg: Color32,
    radius: u8,
) {
    w.weak_bg_fill = weak_bg_fill;
    w.bg_fill = bg_fill;
    w.bg_stroke = bg_stroke;
    w.fg_stroke = Stroke::new(1.0, fg);
    w.corner_radius = CornerRadius::same(radius);
    w.expansion = 0.0;
}

impl Tokens {
    /// Convert this token set into a complete egui style. The palette is
    /// written unconditionally, so the result never depends on what the
    /// base style's own dark/light mode was.
    pub(crate) fn apply(&self, base: &Style) -> Style {
        let mut s = base.clone();
        let r = self.radius_control;

        s.visuals.panel_fill = self.surface_1;
        s.visuals.window_fill = self.surface_2;
        s.visuals.extreme_bg_color = self.surface_0;
        s.visuals.faint_bg_color = self.surface_2;
        s.visuals.window_stroke = Stroke::new(1.0, self.stroke);

        paint_widget(
            &mut s.visuals.widgets.noninteractive,
            self.surface_1,
            self.surface_1,
            Stroke::NONE,
            self.text_hi,
            r,
        );
        paint_widget(
            &mut s.visuals.widgets.inactive,
            self.surface_2,
            self.surface_3,
            Stroke::new(1.0, self.stroke),
            self.text_hi,
            r,
        );
        paint_widget(
            &mut s.visuals.widgets.hovered,
            self.surface_3,
            self.surface_3,
            Stroke::new(1.0, self.accent),
            self.text_hi,
            r,
        );
        paint_widget(
            &mut s.visuals.widgets.active,
            self.surface_3,
            self.surface_3,
            Stroke::new(1.0, self.accent),
            self.text_hi,
            r,
        );
        paint_widget(
            &mut s.visuals.widgets.open,
            self.surface_3,
            self.surface_3,
            Stroke::new(1.0, self.accent),
            self.text_hi,
            r,
        );

        s.visuals.selection.bg_fill = self.accent_soft;
        s.visuals.selection.stroke = Stroke::new(1.0, self.accent);
        s.visuals.override_text_color = None;

        s.spacing.item_spacing = egui::vec2(self.gap, self.gap);
        s.spacing.button_padding = egui::vec2(self.pad_x, self.pad_y);
        s.spacing.interact_size = egui::vec2(self.control_h * 1.4, self.control_h);
        s.spacing.slider_rail_height = self.slider_track;
        s.spacing.slider_width = 140.0;

        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(self.type_body));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(self.type_body));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(self.type_small));
        s.text_styles
            .insert(TextStyle::Heading, FontId::proportional(self.type_heading));

        s
    }
}

/// Chrome icons are painted, never typeset. The glyphs they replace (↶ ↷ ☾
/// ☀) are absent from the fonts the iOS build ships, so text glyphs render
/// as tofu boxes there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Icon {
    Undo,
    Redo,
    Moon,
    Sun,
    Paper,
    ChevronUp,
    ChevronDown,
}

/// What a pill shows and how it behaves.
pub(crate) struct PillSpec<'a> {
    pub(crate) icon: Option<Icon>,
    pub(crate) glyph: Option<&'a str>,
    pub(crate) label: Option<&'a str>,
    pub(crate) selected: bool,
    pub(crate) enabled: bool,
}

/// Pill width: the text plus padding, but never shorter than one control,
/// so icon-only pills stay square rather than collapsing to a stub.
pub(crate) fn pill_width(t: &Tokens, text_width: f32) -> f32 {
    (text_width + 2.0 * t.pad_x).max(t.control_h)
}

/// True pill: fully rounded ends.
pub(crate) fn pill_radius(t: &Tokens) -> u8 {
    (t.control_h * 0.5).round() as u8
}

/// A pill button. The hit rect is the painted rect, so nothing is clickable
/// outside what is drawn.
pub(crate) fn pill_button(ui: &mut egui::Ui, t: &Tokens, spec: PillSpec<'_>) -> egui::Response {
    let shown = spec.glyph.or(spec.label).unwrap_or("");
    let font = FontId::proportional(t.type_body);
    let ink = if spec.enabled { t.text_hi } else { t.text_lo };
    let galley = ui.painter().layout_no_wrap(shown.to_owned(), font, ink);
    let width = pill_width(t, galley.size().x);

    let sense = if spec.enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, t.control_h), sense);
    let radius = CornerRadius::same(pill_radius(t));
    let hovered = spec.enabled && response.hovered();
    let fill = if spec.selected {
        t.accent
    } else if hovered {
        t.surface_3
    } else {
        t.surface_2
    };
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    if hovered || spec.selected {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, t.accent),
            egui::StrokeKind::Inside,
        );
    }
    let text_color = if spec.selected { Color32::WHITE } else { ink };
    if let Some(icon) = spec.icon {
        paint_icon(painter, rect, icon, text_color, fill);
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            shown,
            FontId::proportional(t.type_body),
            text_color,
        );
    }

    // Icon-only pills carry their name in a tooltip.
    match (spec.icon.is_some() || spec.glyph.is_some(), spec.label) {
        (true, Some(label)) => response.on_hover_text(label),
        _ => response,
    }
}

/// Square icon button for the floating clusters: the inactive/hover fills of
/// the widget style, with the icon painted inside so no font is involved.
pub(crate) fn icon_button(ui: &mut egui::Ui, t: &Tokens, icon: Icon, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let radius = CornerRadius::same(t.radius_control);
    let hovered = response.hovered();
    let fill = if hovered { t.surface_3 } else { t.surface_2 };
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, if hovered { t.accent } else { t.stroke }),
        egui::StrokeKind::Inside,
    );
    paint_icon(painter, rect, icon, t.text_hi, fill);
    let name = match icon {
        Icon::Undo => "Undo",
        Icon::Redo => "Redo",
        Icon::Moon | Icon::Sun | Icon::Paper => "Theme",
        Icon::ChevronUp => "Expand",
        Icon::ChevronDown => "Collapse",
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    response
}

/// Paint one icon centred in `rect`. `bg` is the fill behind it (the moon's
/// crescent is cut with it).
pub(crate) fn paint_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: Icon,
    ink: Color32,
    bg: Color32,
) {
    let s = rect.width().min(rect.height());
    let stroke = Stroke::new((s * 0.075).clamp(1.5, 3.0), ink);
    match icon {
        Icon::Undo | Icon::Redo => {
            let (arc, tangent) = arrow_geometry(rect, icon == Icon::Redo);
            painter.add(egui::Shape::line(arc.clone(), stroke));
            let tip = *arc.last().expect("arrow arc has points");
            let normal = egui::vec2(-tangent.y, tangent.x);
            let head = s * 0.17;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    tip + tangent * head,
                    tip + normal * (head * 0.62),
                    tip - normal * (head * 0.62),
                ],
                ink,
                Stroke::NONE,
            ));
        }
        Icon::Moon => {
            painter.circle_filled(rect.center(), s * 0.30, ink);
            painter.circle_filled(
                rect.center() + egui::vec2(s * 0.14, -s * 0.11),
                s * 0.27,
                bg,
            );
        }
        Icon::Sun => {
            painter.circle_filled(rect.center(), s * 0.11, ink);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = egui::vec2(a.cos(), a.sin());
                painter.line_segment(
                    [
                        rect.center() + d * (s * 0.20),
                        rect.center() + d * (s * 0.30),
                    ],
                    stroke,
                );
            }
        }
        Icon::Paper => {
            let page = egui::Rect::from_center_size(rect.center(), egui::vec2(s * 0.42, s * 0.52));
            painter.rect_stroke(
                page,
                CornerRadius::same((s * 0.06) as u8),
                stroke,
                egui::StrokeKind::Inside,
            );
            for i in 0..3 {
                let y = page.top() + page.height() * (0.26 + 0.22 * i as f32);
                painter.line_segment(
                    [
                        egui::pos2(page.left() + s * 0.07, y),
                        egui::pos2(page.right() - s * 0.07, y),
                    ],
                    Stroke::new(stroke.width * 0.8, ink),
                );
            }
        }
        Icon::ChevronDown | Icon::ChevronUp => {
            painter.add(egui::Shape::line(
                chevron_geometry(rect, icon == Icon::ChevronUp),
                stroke,
            ));
        }
    }
}

/// Three points of a chevron wide enough for a fingertip and small enough
/// to read as a disclosure control at 26 pt.
fn chevron_geometry(rect: egui::Rect, up: bool) -> Vec<egui::Pos2> {
    let s = rect.width().min(rect.height());
    let (w, h) = (s * 0.30, s * 0.15);
    let dir = if up { -1.0 } else { 1.0 };
    let c = rect.center();
    vec![
        egui::pos2(c.x - w * 0.5, c.y - dir * h * 0.5),
        egui::pos2(c.x, c.y + dir * h * 0.5),
        egui::pos2(c.x + w * 0.5, c.y - dir * h * 0.5),
    ]
}

/// Arc points (screen space, y down) plus the tangent the head points along:
/// starts on the right, sweeps over the top and ends upper-left, where an
/// undo arrow's head belongs. `mirror` flips it for redo.
fn arrow_geometry(rect: egui::Rect, mirror: bool) -> (Vec<egui::Pos2>, egui::Vec2) {
    let s = rect.width().min(rect.height());
    let r = s * 0.28;
    let center = rect.center() + egui::vec2(0.0, s * 0.03);
    // From just below the right shoulder, across the top, to the upper left.
    let (from, to) = (40.0f32, -160.0f32);
    let steps = 24;
    let flip = if mirror { -1.0 } else { 1.0 };
    let at = |deg: f32| {
        let a = deg.to_radians();
        egui::pos2(center.x + flip * r * a.cos(), center.y + r * a.sin())
    };
    let arc = (0..=steps)
        .map(|i| at(from + (to - from) * (i as f32 / steps as f32)))
        .collect();
    // Direction of travel at the end (decreasing angle), mirrored.
    let a = to.to_radians();
    let tangent = egui::vec2(flip * a.sin(), -a.cos());
    (arc, tangent)
}

/// Right-aligned numeric readout in a slot sized for `widest`, so a value
/// growing from two digits to three (99 → 100) never moves the controls
/// beside it.
pub(crate) fn readout_label(
    ui: &mut egui::Ui,
    t: &Tokens,
    text: &str,
    widest: &str,
) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let width = ui
        .painter()
        .layout_no_wrap(widest.to_owned(), font.clone(), Color32::PLACEHOLDER)
        .size()
        .x;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width.ceil() + 2.0, t.control_h),
        egui::Sense::hover(),
    );
    ui.painter().text(
        rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        text,
        font,
        ui.visuals().text_color(),
    );
    response
}

/// The longest value text a slider range can show, for
/// [`readout_label`]'s slot.
pub(crate) fn widest_value_text(lo: f32, hi: f32, step: f64) -> String {
    let text = |v: f32| {
        if step.fract() == 0.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.2}")
        }
    };
    let (a, b) = (text(lo), text(hi));
    if a.len() > b.len() {
        a
    } else {
        b
    }
}

/// Inner padding of a section card: vertical padding always, horizontal only
/// when the card is not `flush` (the swatch grid needs every pixel).
pub(crate) fn card_pad(t: &Tokens, flush: bool) -> egui::Vec2 {
    egui::vec2(if flush { 0.0 } else { t.pad_x_card }, t.pad_y_card)
}

/// Content width a card can offer inside a panel of `panel_width`
/// (`16.0` is the panel's own ~8 px margin on each side).
#[cfg(test)]
pub(crate) fn card_available_width(panel_width: f32, t: &Tokens, flush: bool) -> f32 {
    panel_width - 16.0 - 2.0 * card_pad(t, flush).x
}

/// A surface panel section: one rounded, stroked card per existing top-level
/// heading. Cards only wrap what was already there — they never re-cut it.
pub(crate) fn section_card(
    ui: &mut egui::Ui,
    t: &Tokens,
    title: Option<&str>,
    flush: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    section_card_actions(ui, t, title, flush, |_| {}, add);
}

/// A card whose title row also carries one or more right-aligned controls
/// (the panel collapse buttons), so a card does not spend a whole row on a
/// lone arrow.
pub(crate) fn section_card_actions(
    ui: &mut egui::Ui,
    t: &Tokens,
    title: Option<&str>,
    flush: bool,
    title_actions: impl FnOnce(&mut egui::Ui),
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::NONE
        .fill(t.surface_2)
        .stroke(Stroke::new(1.0, t.stroke))
        .corner_radius(CornerRadius::same(t.radius_card))
        .inner_margin(egui::Margin::from(card_pad(t, flush)))
        .show(ui, |ui| {
            // Cards are one column: keep every card as wide as the panel.
            ui.set_width(ui.available_width());
            if let Some(title) = title {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(title.to_uppercase())
                            .font(FontId::proportional(t.type_small))
                            .color(t.text_lo),
                    );
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        title_actions,
                    );
                });
                ui.add_space(6.0);
            }
            add(ui);
        });
}

/// Horizontal extent of a thick slider's rail: narrow enough that a bar of
/// controls still fits on desktop, wide enough for a finger to land on.
pub(crate) fn slider_track_width(t: &Tokens) -> f32 {
    if t.class == BarLayout::Wide {
        t.slider_width_wide
    } else {
        t.slider_width_touch
    }
}

/// Value at `x` px along a `width` px rail, clamped to the range. A
/// degenerate range (`lo == hi`) or a zero-width rail has one possible value.
pub(crate) fn value_from_track(x: f32, width: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    let (lo, hi) = (*range.start(), *range.end());
    if hi <= lo || width <= 0.0 {
        return lo;
    }
    let frac = (x / width).clamp(0.0, 1.0);
    lo + frac * (hi - lo)
}

/// How much of the rail the accent fill covers, 0..=1.
pub(crate) fn fill_fraction(v: f32, range: &std::ops::RangeInclusive<f32>) -> f32 {
    let (lo, hi) = (*range.start(), *range.end());
    if hi <= lo {
        return 0.0;
    }
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// A thick rail slider for the quick bar: label + value, `−`, the rail, `+`.
/// Only the rail is hit-tested, so the two steppers keep their own taps.
pub(crate) fn thick_slider(
    ui: &mut egui::Ui,
    t: &Tokens,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    label: &str,
    default: f32,
) -> egui::Response {
    let (lo, hi) = (*range.start(), *range.end());
    let value_text = if step.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    };
    let btn_w = if t.class == BarLayout::Wide {
        24.0
    } else {
        t.control_h
    };
    let widest = widest_value_text(lo, hi, step);
    let mut track: Option<egui::Response> = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        ui.label(label);
        readout_label(ui, t, &value_text, &widest);
        if ui
            .add_sized([btn_w, t.control_h], egui::Button::new("−"))
            .clicked()
        {
            *value = (*value - step as f32).clamp(lo, hi);
        }

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(slider_track_width(t), t.control_h),
            egui::Sense::click_and_drag(),
        );
        let radius = CornerRadius::same((t.slider_track / 2.0) as u8);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, t.surface_3);
        let filled = rect.width() * fill_fraction(*value, &range);
        if filled > 0.0 {
            let fill = egui::Rect::from_min_size(rect.min, egui::vec2(filled, rect.height()));
            painter.rect_filled(fill, radius, t.accent);
        }
        if resp.clicked() || resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                *value = value_from_track(pos.x - rect.left(), rect.width(), &range);
            }
        }
        if resp.double_clicked() {
            *value = default;
        }
        resp.widget_info(|| {
            egui::WidgetInfo::slider(true, *value as f64, format!("{label} {value_text}"))
        });
        track = Some(resp);

        if ui
            .add_sized([btn_w, t.control_h], egui::Button::new("+"))
            .clicked()
        {
            *value = (*value + step as f32).clamp(lo, hi);
        }
    });
    track.expect("thick_slider always allocates its rail")
}

/// Three-state cycle for the top-bar toggle: Dark → Light → Paper → Dark.
pub(crate) fn next_theme(theme: Theme) -> Theme {
    match theme {
        Theme::Dark => Theme::Light,
        Theme::Light => Theme::Paper,
        Theme::Paper => Theme::Dark,
    }
}

/// What the toggle shows: the *current* theme, so the icon reads as a state
/// (sun light, moon dark, page paper) rather than as the action.
pub(crate) fn theme_icon(theme: Theme) -> Icon {
    match theme {
        Theme::Dark => Icon::Moon,
        Theme::Light => Icon::Sun,
        Theme::Paper => Icon::Paper,
    }
}

/// The one pill that must never be folded into a menu: cycles Dark → Light →
/// Paper and returns true when it changed the theme.
pub(crate) fn theme_toggle(ui: &mut egui::Ui, t: &Tokens, theme: &mut Theme) -> bool {
    let next = next_theme(*theme);
    let response = pill_button(
        ui,
        t,
        PillSpec {
            icon: Some(theme_icon(*theme)),
            glyph: None,
            label: None,
            selected: false,
            enabled: true,
        },
    )
    .on_hover_text(format!(
        "{} theme · click for {}.",
        theme.name(),
        next.name()
    ));
    if response.clicked() {
        *theme = next;
        return true;
    }
    false
}

#[cfg(test)]
fn luminance_channel(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative-luminance contrast ratio, 1.0 (identical) .. 21.0.
#[cfg(test)]
pub(crate) fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let la = 0.2126 * luminance_channel(a.r())
        + 0.7152 * luminance_channel(a.g())
        + 0.0722 * luminance_channel(a.b());
    let lb = 0.2126 * luminance_channel(b.r())
        + 0.7152 * luminance_channel(b.g())
        + 0.0722 * luminance_channel(b.b());
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BarLayout, Theme};

    #[test]
    fn touch_classes_get_finger_sized_controls() {
        assert_eq!(tokens(Theme::Dark, BarLayout::Wide).control_h, 26.0);
        assert_eq!(tokens(Theme::Dark, BarLayout::Tablet).control_h, 44.0);
        assert_eq!(tokens(Theme::Dark, BarLayout::Narrow).control_h, 44.0);
        assert_eq!(tokens(Theme::Dark, BarLayout::Tablet).slider_track, 10.0);
        assert_eq!(tokens(Theme::Dark, BarLayout::Wide).slider_track, 6.0);
    }

    #[test]
    fn secondary_text_clears_aa_contrast_on_panels() {
        for theme in [Theme::Dark, Theme::Light, Theme::Paper] {
            let t = tokens(theme, BarLayout::Wide);
            let ratio = contrast_ratio(t.text_lo, t.surface_1);
            assert!(ratio >= 4.5, "{theme:?} text_lo on surface_1 = {ratio:.2}");
        }
    }

    #[test]
    fn primary_text_clears_aaa_contrast_on_panels() {
        for theme in [Theme::Dark, Theme::Light, Theme::Paper] {
            let t = tokens(theme, BarLayout::Wide);
            let ratio = contrast_ratio(t.text_hi, t.surface_1);
            assert!(ratio >= 7.0, "{theme:?} text_hi on surface_1 = {ratio:.2}");
        }
    }

    #[test]
    fn device_class_changes_metrics_so_the_cache_must_key_on_both() {
        let wide = tokens(Theme::Dark, BarLayout::Wide);
        let tablet = tokens(Theme::Dark, BarLayout::Tablet);
        assert_ne!(wide.control_h, tablet.control_h);
        assert_ne!(wide.pad_x, tablet.pad_x);
    }

    #[test]
    fn pill_is_at_least_one_control_tall_and_half_height_rounded() {
        let t = tokens(Theme::Dark, BarLayout::Tablet);
        assert_eq!(pill_width(&t, 0.0), 44.0);
        // 8 px of label plus 14 px padding a side still fits inside the
        // touch-height floor, so short labels keep a square pill.
        assert_eq!(pill_width(&t, 8.0), 44.0);
        assert!(pill_width(&t, 80.0) >= 80.0 + 2.0 * t.pad_x);
        assert_eq!(pill_radius(&t), 22);

        // Wide is 26 px tall: floor 26, radius 13.
        let w = tokens(Theme::Dark, BarLayout::Wide);
        assert_eq!(pill_width(&w, 0.0), 26.0);
        assert_eq!(pill_radius(&w), 13);
    }

    #[test]
    fn flush_cards_leave_the_swatch_grid_its_full_224px() {
        let t = tokens(Theme::Dark, BarLayout::Wide);
        assert_eq!(card_pad(&t, false).x, 6.0);
        assert_eq!(card_pad(&t, true).x, 0.0);
        assert_eq!(card_available_width(240.0, &t, true), 224.0);
        assert!(card_available_width(240.0, &t, false) < 224.0);
    }

    #[test]
    fn track_position_maps_to_the_range_and_clamps() {
        let r = 0.0..=100.0;
        assert_eq!(value_from_track(0.0, 200.0, &r), 0.0);
        assert_eq!(value_from_track(100.0, 200.0, &r), 50.0);
        assert_eq!(value_from_track(200.0, 200.0, &r), 100.0);
        assert_eq!(value_from_track(-40.0, 200.0, &r), 0.0);
        assert_eq!(value_from_track(999.0, 200.0, &r), 100.0);
        assert_eq!(fill_fraction(25.0, &r), 0.25);
        assert_eq!(fill_fraction(500.0, &r), 1.0);
    }

    #[test]
    fn slider_hit_area_is_only_the_track() {
        let wide = tokens(Theme::Dark, BarLayout::Wide);
        let tablet = tokens(Theme::Dark, BarLayout::Tablet);
        assert_eq!(slider_track_width(&wide), 110.0);
        assert_eq!(slider_track_width(&tablet), 130.0);
        assert!(slider_track_width(&tablet) < tablet.control_h * 7.0);
    }

    #[test]
    fn toggle_cycles_dark_light_paper_with_one_icon_each() {
        assert_eq!(next_theme(Theme::Dark), Theme::Light);
        assert_eq!(next_theme(Theme::Light), Theme::Paper);
        assert_eq!(next_theme(Theme::Paper), Theme::Dark);
        assert_eq!(theme_icon(Theme::Dark), Icon::Moon);
        assert_eq!(theme_icon(Theme::Light), Icon::Sun);
        assert_eq!(theme_icon(Theme::Paper), Icon::Paper);
    }

    /// iOS/Android ship only egui's bundled fonts (no Segoe UI Symbol), so a
    /// glyph outside those fonts is a tofu box on the device. Every string
    /// the `has_symbols == false` paths print must stay inside them.
    #[test]
    fn bundled_fonts_cover_every_glyph_the_fallback_paths_print() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();

        let mut strings: Vec<String> = [
            crate::Tool::Brush,
            crate::Tool::Eraser,
            crate::Tool::Smudge,
            crate::Tool::Select,
            crate::Tool::Lasso,
            crate::Tool::Fill,
            crate::Tool::Picker,
            crate::Tool::Move,
            crate::Tool::Hand,
            crate::Tool::Liquify,
            crate::Tool::Crop,
            crate::Tool::Shapes,
            crate::Tool::Text,
        ]
        .iter()
        .map(|t| crate::tool_label(*t, false).to_string())
        .collect();
        strings.extend(
            [
                "Brush", "Eraser", "Smudge", "<", ">", "Up", "Down", "·", "−", "+", "■", "○", "T",
            ]
            .map(str::to_string),
        );

        for s in strings {
            for c in s.chars().filter(|c| !c.is_ascii()) {
                let covered = ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(15.0), c));
                assert!(
                    covered,
                    "{s:?} prints {c:?}, missing from the bundled fonts"
                );
            }
        }
    }

    #[test]
    fn readout_slot_is_one_width_across_the_whole_range() {
        // The brush size range: 90 -> 100 -> 400 all fit the "400" slot.
        assert_eq!(widest_value_text(1.0, 400.0, 1.0), "400");
        assert_eq!(widest_value_text(-32.0, 32.0, 1.0), "-32");
        assert_eq!(widest_value_text(0.05, 1.0, 0.05), "1.00");
        assert_eq!(widest_value_text(0.0, 255.0, 1.0), "255");
    }

    #[test]
    fn arrow_icons_stay_inside_their_button_at_every_size() {
        for size in [26.0f32, 44.0, 48.0, 56.0] {
            let rect =
                egui::Rect::from_center_size(egui::pos2(100.0, 100.0), egui::vec2(size, size));
            for mirror in [false, true] {
                let (arc, tangent) = arrow_geometry(rect, mirror);
                let tip = *arc.last().expect("arc has points");
                let head = size * 0.17;
                let normal = egui::vec2(-tangent.y, tangent.x);
                let corners = [
                    tip + tangent * head,
                    tip + normal * (head * 0.62),
                    tip - normal * (head * 0.62),
                ];
                for p in arc.iter().copied().chain(corners) {
                    assert!(
                        rect.contains(p),
                        "size {size} mirror {mirror}: {p:?} escapes {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn chevrons_stay_inside_their_button_and_point_both_ways() {
        for size in [26.0f32, 44.0, 56.0] {
            let rect = egui::Rect::from_center_size(egui::pos2(50.0, 50.0), egui::vec2(size, size));
            for up in [false, true] {
                let pts = chevron_geometry(rect, up);
                for p in &pts {
                    assert!(rect.contains(*p), "size {size} up {up}: {p:?} escapes");
                }
                let tip = pts[1];
                let wings = (pts[0].y + pts[2].y) * 0.5;
                assert_eq!(pts[0].y, pts[2].y);
                assert!(
                    if up { tip.y < wings } else { tip.y > wings },
                    "size {size} up {up}: tip {tip:?} wings {wings}"
                );
            }
        }
    }

    #[test]
    fn board_dots_stay_faint_and_follow_the_theme() {
        // Premultiplied: white at 0x28 alpha reads back as r == g == b == a.
        let dark = board_dot(Theme::Dark);
        let light = board_dot(Theme::Light);
        let paper = board_dot(Theme::Paper);
        assert_eq!(dark.a(), 0x28);
        assert_eq!((dark.r(), dark.g(), dark.b()), (0x28, 0x28, 0x28));
        assert!(
            light.a() <= 0x30 && light.r() < 0x20,
            "light dots darken the board"
        );
        assert_eq!(paper, light);
    }

    #[test]
    fn apply_writes_the_palette_and_metrics_into_the_style() {
        let t = tokens(Theme::Dark, BarLayout::Wide);
        let s = t.apply(&egui::Style::default());
        assert_eq!(s.visuals.panel_fill, t.surface_1);
        assert_eq!(s.visuals.extreme_bg_color, t.surface_0);
        assert_eq!(s.visuals.widgets.inactive.bg_stroke.color, t.stroke);
        assert_eq!(s.spacing.interact_size.y, 26.0);
        assert_eq!(s.spacing.slider_rail_height, 6.0);
        // The palette wins over whatever the base style was, so writing the
        // Light slot with Paper never inherits a dark base.
        let paper = tokens(Theme::Paper, BarLayout::Wide);
        let dark_base = tokens(Theme::Dark, BarLayout::Wide).apply(&egui::Style::default());
        assert_eq!(paper.apply(&dark_base).visuals.panel_fill, paper.surface_1);
    }
}
