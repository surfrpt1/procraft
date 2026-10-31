use crate::*;

impl ProCraftApp {
    pub(crate) fn choose_brush(&mut self, slot: BrushSlot, brush: &Brush) {
        // A preset carries its own settings: adopt them as the Brush
        // tool's slot, leaving the eraser and smudge slots alone.
        self.choose_tool(Tool::Brush);
        self.brush = brush.clone();
        self.tool_brushes.save(Tool::Brush, &self.brush);
        self.brush_slot = Some(slot);
    }

    pub(crate) fn brush_slot_active(&self, slot: BrushSlot) -> bool {
        slot_is_active(self.brush_slot, slot)
    }

    pub(crate) fn remove_custom_brush(&mut self, index: usize) -> Option<String> {
        if index >= self.custom_brushes.len() {
            return None;
        }
        let name = self.custom_brushes.remove(index).name;
        self.brush_slot = slot_after_remove(self.brush_slot, index);
        Some(name)
    }

    /// Unique display name for a new custom brush: "Foo", "Foo (2)", ...
    pub(crate) fn unique_brush_name(&self, want: &str) -> String {
        let want = want.trim();
        if self.custom_brushes.iter().all(|p| p.name != want) {
            return want.to_string();
        }
        for n in 2..=MAX_LIBRARY_BRUSHES + 2 {
            let cand = format!("{want} ({n})");
            if self.custom_brushes.iter().all(|p| p.name != cand) {
                return cand;
            }
        }
        want.to_string()
    }

    /// Append imported presets with dedup names (shared by desktop Import
    /// and the iOS picker resolve path). Returns the status message.
    pub(crate) fn absorb_brushes(
        &mut self,
        loaded: LoadedBrushes,
        path: &std::path::Path,
    ) -> String {
        let LoadedBrushes {
            brushes,
            mut warnings,
            mut skipped,
        } = loaded;
        let mut added = 0;
        let mut capped = 0;
        for p in brushes {
            if self.custom_brushes.len() >= MAX_LIBRARY_BRUSHES {
                capped += 1;
                continue;
            }
            let name = self.unique_brush_name(&p.name);
            self.custom_brushes.push(BrushPreset {
                name,
                brush: p.brush,
            });
            added += 1;
        }
        if capped > 0 {
            skipped += capped;
            warnings.push(format!("Library is full; {capped} brushes were not added."));
        }
        brush_import_status(added, skipped, &warnings, path)
    }

    /// Append an imported palette's swatches to the active palette,
    /// skipping exact duplicates (shared by desktop Import, image/canvas
    /// extraction and the iOS picker resolve path).
    pub(crate) fn absorb_colors(&mut self, pal: Palette, from: &str) -> String {
        let mut added = 0;
        if let Some(active) = self.palettes.active_mut() {
            for c in pal.colors {
                if active.colors.len() >= MAX_PALETTE_COLORS {
                    break;
                }
                if !active.colors.contains(&c) {
                    active.colors.push(c);
                    added += 1;
                }
            }
        }
        format!("Imported {added} swatches from {from}.")
    }

    /// Extract a palette from RGBA pixels and absorb it (shared by the
    /// image-file and canvas arms on both platforms).
    pub(crate) fn image_palette_status(
        &mut self,
        name: &str,
        pixels: &[u8],
        w: u32,
        h: u32,
        from: &str,
    ) -> String {
        match palette_from_rgba(name, pixels, w, h, MAX_IMAGE_COLORS) {
            Ok(pal) => self.absorb_colors(pal, from),
            Err(e) => format!("Palette from image failed: {e}"),
        }
    }

    /// Scrub a filter slider with a live preview: checkpoint + snapshot on
    /// first touch, then restore-and-reapply on change, so the whole
    /// gesture is one undo step and values never compound. `kind` selects
    /// the baseline slot (see [`FilterPreview`]); a changed layer (or
    /// mismatched pixels after undo/layer edits) restarts it. Re-applies
    /// closer than [`PREVIEW_MIN_DT`] are skipped and flag the preview
    /// `stale` — Apply restamps the latest values (see `restamp_preview`).
    /// A preview whose layer changed underneath (paint stroke, undo, layer
    /// op, new/opened doc…) never restores its stale snapshot — that wipe
    /// is exactly "my paint disappeared" — it restarts on current pixels.
    pub(crate) fn filter_scrub(&mut self, kind: u8, apply: impl Fn(&mut Document, usize)) {
        if self.text_paint_blocked() {
            return;
        }
        let layer = self.doc.active;
        // Stale-snapshot guard: the live layer must still match what our
        // last apply left behind (or the snapshot on a fresh preview).
        let expired = match &self.filter_preview {
            Some(fp) => match self.doc.layers.get(fp.layer) {
                Some(l) => l.pixels.fingerprint() != fp.live_hash,
                None => true,
            },
            None => false,
        };
        if expired {
            self.filter_preview = None;
        }
        let fresh = match &self.filter_preview {
            Some(fp) => {
                fp.kind != kind
                    || fp.layer != layer
                    || self
                        .doc
                        .layers
                        .get(layer)
                        .is_none_or(|l| l.pixels.len() != fp.pixels.len())
            }
            None => true,
        };
        if fresh {
            self.doc.checkpoint();
            match self.doc.layers.get(layer) {
                Some(l) => {
                    let live_hash = l.pixels.fingerprint();
                    self.filter_preview = Some(FilterPreview {
                        kind,
                        layer,
                        pixels: l.pixels.clone(),
                        last: std::time::Instant::now() - PREVIEW_MIN_DT,
                        stale: false,
                        live_hash,
                    });
                }
                None => {
                    self.filter_preview = None;
                    return;
                }
            }
        }
        if let Some(mut fp) = self.filter_preview.take() {
            if self.doc.layers.get(fp.layer).is_none() {
                return;
            }
            // Throttle: the layer keeps the last preview; Apply catches up.
            if !fresh && fp.last.elapsed() < PREVIEW_MIN_DT {
                fp.stale = true;
                self.filter_preview = Some(fp);
                return;
            }
            if let Some(l) = self.doc.layers.get_mut(fp.layer) {
                l.pixels.clone_from(&fp.pixels);
            }
            apply(&mut self.doc, fp.layer);
            if let Some(l) = self.doc.layers.get(fp.layer) {
                fp.live_hash = l.pixels.fingerprint();
            }
            fp.last = std::time::Instant::now();
            fp.stale = false;
            self.filter_preview = Some(fp);
        }
    }

    /// Re-apply current slider values over the scrub snapshot (for a
    /// throttled-stale preview at Apply time, or to refresh a current
    /// one). The scrub's checkpoint still covers undo — no new step.
    /// If the layer moved on underneath (see `filter_scrub`), the stale
    /// preview is dropped and the values apply fresh on the active layer
    /// (new undo step) — the intervening work is always kept.
    pub(crate) fn restamp_preview(&mut self, apply: impl Fn(&mut Document, usize)) {
        if self.text_paint_blocked() {
            return;
        }
        let expired = match &self.filter_preview {
            Some(fp) => match self.doc.layers.get(fp.layer) {
                Some(l) => l.pixels.fingerprint() != fp.live_hash,
                None => true,
            },
            None => return,
        };
        if expired {
            self.filter_preview = None;
            self.doc.checkpoint();
            let layer = self.doc.active;
            if self.doc.layers.get(layer).is_some() {
                apply(&mut self.doc, layer);
            }
            return;
        }
        if let Some(mut fp) = self.filter_preview.take() {
            if self.doc.layers.get(fp.layer).is_some() {
                if let Some(l) = self.doc.layers.get_mut(fp.layer) {
                    l.pixels.clone_from(&fp.pixels);
                }
                apply(&mut self.doc, fp.layer);
                if let Some(l) = self.doc.layers.get(fp.layer) {
                    fp.live_hash = l.pixels.fingerprint();
                }
            }
            self.filter_preview = Some(fp);
        }
    }

    /// M38 Typography: validate imported TTF/OTF bytes and keep them as
    /// the session face (shared by desktop Import and the iOS resolve).
    pub(crate) fn absorb_font(&mut self, bytes: &[u8], name: &str) -> String {
        if bytes.len() > MAX_FONT_BYTES {
            return "Font too large (max 2 MB).".to_string();
        }
        match check_font(bytes) {
            Ok(()) => {
                self.text_font_name = Some(name.to_string());
                self.text_font_bytes = Some(bytes.to_vec());
                format!("Font {name} ready.")
            }
            Err(e) => format!("Font {name} failed: {e}"),
        }
    }

    /// M39 Brush depth: take the first stamp tip in a brush file as the
    /// current brush's dual tip (shared by desktop Import and iOS resolve).
    pub(crate) fn absorb_dual_tip(&mut self, path: &std::path::Path) -> String {
        match load_brush_file(path) {
            Ok(libs) => match libs.into_iter().find_map(|p| p.brush.tip) {
                Some(tip) => {
                    let (w, h) = (tip.w, tip.h);
                    self.brush.dual_tip = Some(tip);
                    format!("Dual tip set ({w}x{h}).")
                }
                None => "No stamp tip in that file.".to_string(),
            },
            Err(e) => format!("Dual import failed: {e}"),
        }
    }

    /// M38 Typography: the current dialog settings as an engine style.
    pub(crate) fn text_style(&self) -> TextStyle {
        TextStyle {
            font_bytes: self.text_font_bytes.clone(),
            leading: self.text_leading,
            tracking: self.text_tracking,
            outline: self.text_outline.then(|| {
                (
                    Color::rgb(
                        self.text_outline_rgb[0],
                        self.text_outline_rgb[1],
                        self.text_outline_rgb[2],
                    ),
                    self.text_outline_w,
                )
            }),
        }
    }
}

/// Every extension `import_brush_file` can dispatch.
pub(crate) const BRUSH_IMPORT_EXTS: &[&str] = &[
    "pcbrush", "json", "abr", "brush", "brushset", "kpp", "bundle", "sut", "gbr", "png", "jpg",
    "jpeg", "webp", "bmp", "gif", "tif", "tiff",
];

/// Any brush file that can supply a dual tip.
pub(crate) const TIP_IMPORT_EXTS: &[&str] = BRUSH_IMPORT_EXTS;

/// Brush size range shared by the quick slider, the panel slider and drag
/// gestures. The upper bound matches the engine's validation limit.
pub(crate) const BRUSH_SIZE_MIN: f32 = 1.0;

pub(crate) const BRUSH_SIZE_MAX: f32 = 400.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrushSlot {
    Builtin(usize),
    Custom(usize),
}

pub(crate) fn slot_is_active(selected: Option<BrushSlot>, candidate: BrushSlot) -> bool {
    selected == Some(candidate)
}

pub(crate) fn slot_after_remove(selected: Option<BrushSlot>, removed: usize) -> Option<BrushSlot> {
    match selected {
        Some(BrushSlot::Custom(index)) if index == removed => None,
        Some(BrushSlot::Custom(index)) if index > removed => Some(BrushSlot::Custom(index - 1)),
        other => other,
    }
}

/// Brush preview strip in points, as laid out in the panel.
pub const PREVIEW_W: f32 = 96.0;

pub const PREVIEW_H: f32 = 24.0;

/// Preview stroke diameter as a fraction of the strip height. Presets carry
/// their own size (Airbrush is 64 px) and a full-size stroke on a 24 pt strip
/// is a cropped band, not a preview of the edge.
pub(crate) const PREVIEW_SIZE_FRAC: f32 = 0.45;
