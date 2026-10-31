#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeKind {
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
    pub(crate) const ALL: [ShapeKind; 8] = [
        ShapeKind::Line,
        ShapeKind::Rect,
        ShapeKind::Ellipse,
        ShapeKind::Triangle,
        ShapeKind::Diamond,
        ShapeKind::Star,
        ShapeKind::Arrow,
        ShapeKind::Polygon,
    ];

    pub(crate) fn name(self) -> &'static str {
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
    pub(crate) fn fillable(self) -> bool {
        !matches!(self, ShapeKind::Line | ShapeKind::Arrow)
    }
}
