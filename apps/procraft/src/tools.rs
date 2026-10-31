use crate::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    Brush,
    Eraser,
    Smudge,
    Select,
    Lasso,
    Fill,
    Picker,
    Move,
    Hand,
    Liquify,
    Crop,
    Shapes,
    Text,
}

/// Tool radio label: icon glyphs on symbol-font platforms, plain words
/// where the glyphs would be tofu (iOS). Only glyphs the bundled fonts
/// cover (`○`) are unconditional; everything else has a word fallback.
pub(crate) fn tool_label(tool: Tool, symbols: bool) -> &'static str {
    match tool {
        Tool::Brush => {
            if symbols {
                "✏ Brush"
            } else {
                "Brush"
            }
        }
        Tool::Eraser => {
            if symbols {
                "⌫ Eraser"
            } else {
                "Eraser"
            }
        }
        Tool::Smudge => {
            if symbols {
                "💧 Smudge"
            } else {
                "Smudge"
            }
        }
        Tool::Select => {
            if symbols {
                "⬚ Select"
            } else {
                "Select"
            }
        }
        Tool::Lasso => {
            if symbols {
                "◌ Lasso"
            } else {
                "Lasso"
            }
        }
        Tool::Fill => {
            if symbols {
                "◧ Fill"
            } else {
                "Fill"
            }
        }
        Tool::Picker => {
            if symbols {
                "◉ Pick"
            } else {
                "Pick"
            }
        }
        Tool::Move => {
            if symbols {
                "↔ Move"
            } else {
                "Move"
            }
        }
        Tool::Hand => {
            if symbols {
                "✋ Hand"
            } else {
                "Hand"
            }
        }
        Tool::Liquify => {
            if symbols {
                "➰ Liquify"
            } else {
                "Liquify"
            }
        }
        Tool::Crop => {
            if symbols {
                "◫ Crop"
            } else {
                "Crop"
            }
        }
        Tool::Shapes => "○ Shapes",
        Tool::Text => "T Text",
    }
}

/// Compact glyph for the phone strip's paint-tool segment. The strip only
/// has room for icons if the drawers are to stay visible.
pub(crate) fn tool_glyph(tool: Tool) -> &'static str {
    match tool {
        Tool::Brush => "✏",
        Tool::Eraser => "⌫",
        Tool::Smudge => "💧",
        _ => "•",
    }
}

/// Phone-strip face for a paint tool: the glyph where the bundled fonts
/// cover it, the plain name where they do not.
pub(crate) fn strip_tool_face(tool: Tool, has_symbols: bool) -> &'static str {
    if has_symbols {
        return tool_glyph(tool);
    }
    match tool {
        Tool::Brush => "Brush",
        Tool::Eraser => "Eraser",
        _ => "Smudge",
    }
}

/// One control in the context-sensitive bar under the top bar. The bar is
/// driven by this list so every tool's quick options live in one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuickControl {
    Size,
    Opacity,
    Softness,
    Spacing,
    Stabilizer,
    Strength,
    Tolerance,
    Grow,
    PickerPx,
    Feather,
    SelGrow,
    OpaqueThreshold,
    OutlinePx,
    Invert,
    ClearSelection,
    SmoothSelection,
    ShapeKind,
    ShapeFill,
    ShapeSides,
    CropApply,
    CropCancel,
    TextSize,
    TextLeading,
    TextTracking,
    TextOutline,
    EditText,
}

pub(crate) fn quick_controls(tool: Tool) -> &'static [QuickControl] {
    use QuickControl::*;
    match tool {
        Tool::Brush | Tool::Eraser => &[Size, Opacity, Softness, Spacing, Stabilizer],
        Tool::Smudge => &[Size, Strength],
        Tool::Select | Tool::Lasso => &[
            Feather,
            SelGrow,
            OpaqueThreshold,
            OutlinePx,
            Invert,
            ClearSelection,
            SmoothSelection,
        ],
        Tool::Fill => &[Tolerance, Grow],
        Tool::Picker => &[PickerPx],
        Tool::Liquify => &[Size, Strength],
        Tool::Shapes => &[ShapeKind, ShapeFill, ShapeSides],
        Tool::Crop => &[CropApply, CropCancel],
        Tool::Text => &[TextSize, TextLeading, TextTracking, TextOutline, EditText],
        Tool::Move | Tool::Hand => &[],
    }
}

/// Finger-sized hit height for the quick bar, from the same token table the
/// rest of the chrome uses.
pub(crate) fn quick_hit_height(layout: BarLayout) -> f32 {
    design::tokens(Theme::Dark, layout).control_h
}

/// Phones and tablets get one horizontally scrollable row (vertical space
/// is canvas); desktop wraps inline.
pub(crate) fn quick_scrolls(layout: BarLayout) -> bool {
    layout != BarLayout::Wide
}

/// Toolbar order, shared by the tools panel list and its mobile grid.
pub(crate) const TOOL_ORDER: [Tool; 13] = [
    Tool::Brush,
    Tool::Eraser,
    Tool::Smudge,
    Tool::Select,
    Tool::Lasso,
    Tool::Fill,
    Tool::Picker,
    Tool::Move,
    Tool::Hand,
    Tool::Liquify,
    Tool::Crop,
    Tool::Shapes,
    Tool::Text,
];

/// M49: shortcut id for an app tool (mirror of `tool_from_id`).
pub(crate) fn tool_id_for(tool: Tool) -> ToolId {
    match tool {
        Tool::Brush => ToolId::Brush,
        Tool::Eraser => ToolId::Eraser,
        Tool::Smudge => ToolId::Smudge,
        Tool::Select => ToolId::Select,
        Tool::Lasso => ToolId::Lasso,
        Tool::Fill => ToolId::Fill,
        Tool::Picker => ToolId::Picker,
        Tool::Move => ToolId::Move,
        Tool::Hand => ToolId::Hand,
        Tool::Liquify => ToolId::Liquify,
        Tool::Crop => ToolId::Crop,
        Tool::Shapes => ToolId::Shapes,
        Tool::Text => ToolId::Text,
    }
}

/// M49: the app's Tool enum for a shortcut ToolId (toolbar order).
pub(crate) fn tool_from_id(id: ToolId) -> Tool {
    match id {
        ToolId::Brush => Tool::Brush,
        ToolId::Eraser => Tool::Eraser,
        ToolId::Smudge => Tool::Smudge,
        ToolId::Select => Tool::Select,
        ToolId::Lasso => Tool::Lasso,
        ToolId::Fill => Tool::Fill,
        ToolId::Picker => Tool::Picker,
        ToolId::Move => Tool::Move,
        ToolId::Hand => Tool::Hand,
        ToolId::Liquify => Tool::Liquify,
        ToolId::Crop => Tool::Crop,
        ToolId::Shapes => Tool::Shapes,
        ToolId::Text => Tool::Text,
    }
}

/// Size for the cursor ring, from the size the tool actually uses.
/// `None` for tools without a size ring.
pub(crate) fn tool_ring_size(tool: Tool, brush_size: f32, liquify_size: f32) -> Option<f32> {
    match tool {
        Tool::Brush | Tool::Eraser | Tool::Smudge => Some(brush_size),
        Tool::Liquify => Some(liquify_size),
        _ => None,
    }
}

/// Columns for the tools panel: the mobile drawer packs two, and three when
/// the drawer is shot (landscape) so every tool fits without scrolling; the
/// desktop dock is always a list.
pub(crate) fn tool_grid_columns(width: f32, compact: bool, mobile: bool) -> usize {
    if !mobile || width < 300.0 {
        1
    } else if compact {
        3
    } else {
        2
    }
}
