use crate::*;

/// Host captures and remote device testing force the viewport-resolution
/// canvas (spec §4) regardless of document size or GPU limits.
pub(crate) fn viewport_forced() -> bool {
    std::env::var("PROCRAFT_FORCE_VIEWPORT_CANVAS").is_ok_and(|v| v == "1")
}

/// Textured quad for a rotated canvas view: `corners` are TL, TR, BR, BL
/// with matching unit UVs, tinted (egui has no canvas transform, so the
/// texture stays axis-aligned and the quad does the rotating).
pub(crate) fn paint_textured_quad(
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
pub(crate) fn shape_preview_points(
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
pub(crate) fn paint_ants_path(painter: &egui::Painter, pts: &[egui::Pos2], time: f64) {
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
pub(crate) fn paint_selection(painter: &egui::Painter, corners: [egui::Pos2; 4], time: f64) {
    let loop_pts = [corners[0], corners[1], corners[2], corners[3], corners[0]];
    paint_ants_path(painter, &loop_pts, time);
}

/// Eyedropper loupe cells: an odd-sided square of composited colors
/// around `(cx, cy)`, missing edge cells filled with the page background.
/// Returns the cells plus the cursor's cell position inside the grid.
pub(crate) fn loupe_cells(doc: &Document, cx: u32, cy: u32, side: u32) -> (Vec<[u8; 4]>, u32, u32) {
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
pub(crate) fn paint_loupe(
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
pub(crate) fn paint_path(painter: &egui::Painter, pts: &[egui::Pos2], close: bool) {
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
pub(crate) fn paint_mask_edges(
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

/// What the mask-view branch must do this frame.
#[derive(Debug, PartialEq)]
pub(crate) enum MaskAction {
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
pub(crate) fn mask_action(view_mask: bool, has_mask: bool, mask_shown: bool) -> MaskAction {
    match (view_mask, has_mask) {
        (true, true) => MaskAction::Show,
        (false, true) => MaskAction::Idle,
        _ if mask_shown => MaskAction::Drop,
        _ => MaskAction::Idle,
    }
}
