//! ProCraft by surfrpt1 (https://github.com/surfrpt1).
//! Original clean-room code, MIT licensed.
//!
//! ProCraft milestone 9: brush library (custom brushes + .pcbrush import/export),
//! plus all earlier features. Original code.

use eframe::egui::{self, Color32, ColorImage, Sense, TextureHandle, TextureOptions, Vec2};
// iOS has no rfd backend: same call shape, every dialog declined.
#[cfg(target_os = "android")]
use procraft_engine::ext_matches;
use procraft_engine::{
    analogous, check_font, clamp_frame, classify_touch_tap, complementary, curves_lut,
    export_gif_ext, export_timelapse, export_tip_png, file_size_label, gallery_thumb,
    import_brush_file_report, import_gif_document, import_gpl_file, levels_lut, next_frame,
    onion_ghost, open_brush_library, open_palette, open_psd, palette_from_rgba, pan_offset,
    prev_frame, rasterize_frame, rasterize_frame_region, save_brush_library, save_palette,
    save_psd, shift_wheel_remap, size_drag_factor, starter_palette, step_playback_visible, triadic,
    zoom_drag_factor, ActionId, AssistLine, BlendMode, Brush, BrushPreset, ChordKey, Color,
    DabDynamics, Document, DragActionId, GuideKind, Guides, KeyChord, LoopMode, ModSet,
    PaintTarget, Palette, PaletteSet, Raster, Settings, ShapePoly, Shortcuts, SizeDragMode,
    SymmetryMode, TapAction, TextBlock, TextStyle, ToolId, LAPSE_SIDE, MAX_ANIM_FPS, MAX_ANIM_HOLD,
    MAX_CANVAS_SIDE, MAX_FONT_BYTES, MAX_IMAGE_COLORS, MAX_LAPSE_FRAMES, MAX_LIBRARY_BRUSHES,
    MAX_OUTLINE_PX, MAX_PALETTES, MAX_PALETTE_COLORS, MAX_TEXT_SPACING, MIN_ANIM_FPS,
    MIN_ANIM_HOLD, MIN_TEXT_PX, SETTINGS_VERSION, TILE,
};
#[cfg(any(target_os = "ios", target_os = "android"))]
use rfd_shim as rfd;

mod app;
mod app_ui;
mod brushes;
mod canvas;
mod chrome;
mod crash;
mod design;
mod doc_actions;
mod entry;
mod file_menu;
mod files;
mod input;
mod layers;
mod layout;
mod mobile_ui;
mod paint;
mod panels;
mod pixels;
mod previews;
mod settings;
mod settings_ui;
mod shapes;
mod shortcuts;
mod shortcuts_ui;
mod stroke;
mod tform;
mod theme;
mod timelapse;
mod tools;
mod xform;

use app::*;
use brushes::*;
use files::*;
use input::*;
use layers::*;
use layout::*;
use paint::*;
use pixels::*;
use previews::*;
use shapes::*;
use shortcuts::*;
use stroke::*;
use tform::*;
use theme::*;
use tools::*;
use xform::*;

pub use entry::run;

#[cfg(test)]
mod tests;
