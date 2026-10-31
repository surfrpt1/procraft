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

/// Brush settings a paint tool remembers across tool switches. Spacing and
/// stabilizer stay shared; Liquify carries its own size and strength.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToolBrush {
    pub(crate) size: f32,
    pub(crate) opacity: f32,
    pub(crate) softness: f32,
}

/// Per-tool slots behind the live brush. `save` remembers the active tool's
/// settings, `load` restores a returning tool's own — tools without a slot
/// (Liquify, Fill, selection, ...) leave the live brush untouched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToolBrushes {
    brush: ToolBrush,
    eraser: ToolBrush,
    smudge: ToolBrush,
}

impl ToolBrushes {
    /// Seed every slot from the starting brush so the first switch to a
    /// paint tool lands on the values the user began with.
    pub(crate) fn new(default: &Brush) -> Self {
        let slot = ToolBrush {
            size: default.size,
            opacity: default.opacity,
            softness: default.softness,
        };
        Self {
            brush: slot,
            eraser: slot,
            smudge: slot,
        }
    }

    fn slot_mut(&mut self, tool: Tool) -> Option<&mut ToolBrush> {
        match tool {
            Tool::Brush => Some(&mut self.brush),
            Tool::Eraser => Some(&mut self.eraser),
            Tool::Smudge => Some(&mut self.smudge),
            _ => None,
        }
    }

    fn slot(&self, tool: Tool) -> Option<&ToolBrush> {
        match tool {
            Tool::Brush => Some(&self.brush),
            Tool::Eraser => Some(&self.eraser),
            Tool::Smudge => Some(&self.smudge),
            _ => None,
        }
    }

    /// Remember the live brush under `tool`.
    pub(crate) fn save(&mut self, tool: Tool, live: &Brush) {
        if let Some(slot) = self.slot_mut(tool) {
            *slot = ToolBrush {
                size: live.size,
                opacity: live.opacity,
                softness: live.softness,
            };
        }
    }

    /// Adopt `tool`'s stored settings into the live brush.
    pub(crate) fn load(&self, tool: Tool, live: &mut Brush) {
        if let Some(slot) = self.slot(tool) {
            live.size = slot.size;
            live.opacity = slot.opacity;
            live.softness = slot.softness;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn live(size: f32, opacity: f32, softness: f32) -> Brush {
        Brush {
            size,
            opacity,
            softness,
            ..Brush::default()
        }
    }

    #[test]
    fn every_paint_tool_starts_from_the_brush_defaults() {
        let defaults = Brush::default();
        let slots = ToolBrushes::new(&defaults);
        for tool in [Tool::Brush, Tool::Eraser, Tool::Smudge] {
            let mut b = live(7.0, 0.3, 0.9);
            slots.load(tool, &mut b);
            assert_eq!(
                (b.size, b.opacity, b.softness),
                (defaults.size, defaults.opacity, defaults.softness),
                "{tool:?} does not start from the brush defaults"
            );
        }
    }

    #[test]
    fn switching_tools_restores_each_paint_tools_own_parameters() {
        let mut slots = ToolBrushes::new(&Brush::default());
        let mut b = Brush::default();

        slots.save(Tool::Brush, &live(10.0, 0.9, 0.1));

        slots.load(Tool::Eraser, &mut b);
        assert_eq!((b.size, b.opacity, b.softness), (24.0, 1.0, 0.2));
        slots.save(Tool::Eraser, &live(40.0, 0.5, 0.7));

        slots.load(Tool::Smudge, &mut b);
        assert_eq!((b.size, b.opacity, b.softness), (24.0, 1.0, 0.2));
        slots.save(Tool::Smudge, &live(70.0, 0.8, 0.3));

        slots.load(Tool::Brush, &mut b);
        assert_eq!((b.size, b.opacity, b.softness), (10.0, 0.9, 0.1));
        slots.load(Tool::Eraser, &mut b);
        assert_eq!((b.size, b.opacity, b.softness), (40.0, 0.5, 0.7));
        slots.load(Tool::Smudge, &mut b);
        assert_eq!((b.size, b.opacity, b.softness), (70.0, 0.8, 0.3));
    }

    #[test]
    fn tools_without_a_slot_leave_the_live_brush_alone() {
        let mut slots = ToolBrushes::new(&Brush::default());
        let mut b = live(33.0, 0.4, 0.6);
        for tool in [
            Tool::Liquify,
            Tool::Fill,
            Tool::Select,
            Tool::Lasso,
            Tool::Picker,
            Tool::Move,
            Tool::Hand,
            Tool::Crop,
            Tool::Shapes,
            Tool::Text,
        ] {
            slots.save(tool, &b);
            slots.load(tool, &mut b);
            assert_eq!(
                (b.size, b.opacity, b.softness),
                (33.0, 0.4, 0.6),
                "{tool:?} changed the live brush"
            );
        }
    }

    #[test]
    fn adopting_a_preset_updates_only_the_brush_slot() {
        let mut slots = ToolBrushes::new(&Brush::default());
        let eraser = live(40.0, 0.5, 0.7);
        slots.save(Tool::Eraser, &eraser);
        slots.save(Tool::Brush, &live(6.0, 0.7, 0.05));

        let mut b = Brush::default();
        slots.load(Tool::Brush, &mut b);
        assert_eq!((b.size, b.opacity, b.softness), (6.0, 0.7, 0.05));
        slots.load(Tool::Eraser, &mut b);
        assert_eq!((b.size, b.opacity, b.softness), (40.0, 0.5, 0.7));
    }
}
