//! ProCraft engine by surfrpt1 (https://github.com/surfrpt1).
//! Layered raster documents, a soft-round brush, undo and PNG import/export.
//! Original clean-room code, MIT licensed; no GUI types in this crate.

mod animation;
mod brush;
mod document;
mod guides;
mod import_filter;
mod palette;
mod pointer;
mod probrush;
mod psd;
mod raster;
mod settings;
mod shortcuts;
mod tap;
mod text;

pub use animation::{
    clamp_frame, export_gif, export_gif_ext, export_timelapse, import_gif_document, next_frame,
    onion_ghost, prev_frame, rasterize_frame, rasterize_frame_region, step_playback,
    step_playback_visible, visible_frames, LoopMode, LAPSE_SIDE, MAX_ANIM_FPS, MAX_ANIM_HOLD,
    MAX_GIF_FRAMES, MAX_LAPSE_FRAMES, MIN_ANIM_FPS, MIN_ANIM_HOLD,
};
pub use brush::{
    open_brush_library, presets, save_brush_library, Brush, BrushPreset, BrushTip, Color,
    DabDynamics, ShapePoly, Stroke, Taper, MAX_LIBRARY_BRUSHES, MAX_TIP_SIDE,
};
pub use document::{
    curves_lut, gallery_thumb, levels_lut, BlendMode, Document, Layer, LayerGroup, PaintTarget,
    SymmetryMode, MAX_CANVAS_SIDE,
};
pub use guides::{AssistLine, GuideKind, Guides, ASSIST_DEADZONE, VP_BOUND};
pub use import_filter::{ext_matches, file_size_label};
pub use palette::{
    analogous, complementary, hsl_to_rgb, import_gpl_file, import_gpl_text, open_palette,
    palette_from_rgba, rgb_to_hsl, save_palette, starter_palette, triadic, Palette, PaletteSet,
    MAX_IMAGE_COLORS, MAX_PALETTES, MAX_PALETTE_COLORS,
};
pub use pointer::{pan_offset, shift_wheel_remap, size_drag_factor, zoom_drag_factor};
pub use probrush::{
    export_tip_png, import_brush_file, import_brush_file_report, ImportOutcome, ImportedBrush,
};
pub use psd::{open_psd, save_psd};
pub use raster::{Raster, TILE};
pub use settings::{Settings, SETTINGS_VERSION};
pub use shortcuts::{
    ActionId, ChordKey, DragActionId, KeyChord, ModSet, Shortcuts, SizeDragMode, StoredShortcuts,
    ToolId,
};
pub use tap::{classify_touch_tap, TapAction};
pub use text::{
    check_font, layout_text, layout_text_styled, TextBlock, TextLayout, TextStyle, FONT_BYTES,
    MAX_FONT_BYTES, MAX_LAYOUT_SIDE, MAX_OUTLINE_PX, MAX_TEXT_BYTES, MAX_TEXT_CHARS,
    MAX_TEXT_LINES, MAX_TEXT_PX, MAX_TEXT_SPACING, MIN_TEXT_PX,
};
