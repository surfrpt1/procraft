//! Undo checkpoints and history.

use super::*;

impl Document {
    /// Snapshot the active layer for one undo step (call when a stroke starts).
    /// A new checkpoint discards the redo history.
    pub fn checkpoint(&mut self) {
        // A new undo step is a new stroke: drop any liquify field so its
        // displacements cannot leak into the fresh step.
        self.liquify = None;
        if let Some(entry) = self.snapshot() {
            self.push_undo(entry);
            self.redo.clear();
            self.redo_bytes = 0;
        }
    }

    /// Drop the newest checkpoint without restoring it: a failed operation
    /// that allocated before writing (filters, selection buffers) must not
    /// leave an empty undo step behind.
    pub fn discard_last_checkpoint(&mut self) {
        if let Some(entry) = self.undo.pop() {
            self.undo_bytes -= entry.byte_size();
        }
    }

    /// Push onto the undo stack, evicting oldest-first while over either cap.
    /// The newest step is always kept, even if it alone exceeds the budget.
    fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_bytes += entry.byte_size();
        self.undo.push(entry);
        while self.undo.len() > 1
            && (self.undo.len() > MAX_UNDO_ENTRIES || self.undo_bytes > MAX_UNDO_BYTES)
        {
            self.undo_bytes -= self.undo[0].byte_size();
            self.undo.remove(0);
        }
    }

    /// Push onto the redo stack, same dual-cap eviction as [`Self::push_undo`].
    fn push_redo(&mut self, entry: UndoEntry) {
        self.redo_bytes += entry.byte_size();
        self.redo.push(entry);
        while self.redo.len() > 1
            && (self.redo.len() > MAX_UNDO_ENTRIES || self.redo_bytes > MAX_UNDO_BYTES)
        {
            self.redo_bytes -= self.redo[0].byte_size();
            self.redo.remove(0);
        }
    }

    /// Copy of the active layer for the history stacks, if there is one.
    fn snapshot(&self) -> Option<UndoEntry> {
        self.layers.get(self.active).map(|layer| UndoEntry {
            layer: self.active,
            pixels: layer.pixels.clone(),
            mask: layer.mask.clone(),
            ox: layer.ox,
            oy: layer.oy,
            text: layer.text.clone(),
        })
    }

    /// Rewind the active layer to the newest checkpoint without consuming it.
    /// The stroke replay path uses this to redraw the stroke with taper.
    pub fn restore_last_checkpoint(&mut self) -> bool {
        let Some(entry) = self.undo.last() else {
            return false;
        };
        let index = entry.layer;
        {
            let Some(layer) = self.layers.get_mut(index) else {
                return false;
            };
            layer.pixels.clone_from(&entry.pixels);
            layer.mask.clone_from(&entry.mask);
            layer.ox = entry.ox;
            layer.oy = entry.oy;
            layer.text.clone_from(&entry.text);
        }
        self.active = index.min(self.layers.len().saturating_sub(1));
        self.mark_full_dirty();
        true
    }

    /// Rewind only `rect` (inclusive canvas pixels) of the active layer to
    /// the newest checkpoint, leaving the rest of the layer alone: the
    /// tapered stroke replay pays for the stroke bbox instead of a full
    /// layer copy (256 MiB at 8K). The checkpoint stays on the undo stack.
    pub fn restore_checkpoint_region(&mut self, rect: (u32, u32, u32, u32)) -> bool {
        let Some(entry) = self.undo.last() else {
            return false;
        };
        let index = entry.layer;
        let (x0, y0) = (
            rect.0.min(self.w.saturating_sub(1)),
            rect.1.min(self.h.saturating_sub(1)),
        );
        let (x1, y1) = (
            rect.2.min(self.w.saturating_sub(1)),
            rect.3.min(self.h.saturating_sub(1)),
        );
        if x0 > x1 || y0 > y1 {
            return false;
        }
        let (w, h) = (self.w as i32, self.h as i32);
        {
            let Some(layer) = self.layers.get_mut(index) else {
                return false;
            };
            let (lox, loy) = (entry.ox, entry.oy);
            layer.ox = entry.ox;
            layer.oy = entry.oy;
            // One run per row: the rewind copies whole spans into the tiles
            // instead of a get/set per byte (a full-8K stroke bbox would
            // otherwise be hundreds of millions of calls).
            for py in y0..=y1 {
                let ly = py as i32 - loy;
                if !(0..h).contains(&ly) {
                    continue;
                }
                let start = (x0 as i32 - lox).max(0);
                let end = (x1 as i32 - lox).min(w - 1);
                if start > end {
                    continue;
                }
                let (start, ly, run) = (start as u32, ly as u32, (end - start + 1) as u32);
                layer
                    .pixels
                    .copy_run(&entry.pixels, start, ly, start, ly, run);
            }
            // A mask the stroke created has no checkpoint copy: it started
            // fully revealed (255).
            if let Some(layer_mask) = layer.mask.as_mut() {
                for py in y0..=y1 {
                    let ly = py as i32 - loy;
                    if !(0..h).contains(&ly) {
                        continue;
                    }
                    let start = (x0 as i32 - lox).max(0);
                    let end = (x1 as i32 - lox).min(w - 1);
                    if start > end {
                        continue;
                    }
                    let (start, ly, run) = (start as u32, ly as u32, (end - start + 1) as u32);
                    match entry.mask.as_ref() {
                        Some(m) => layer_mask.copy_run(m, start, ly, start, ly, run),
                        None => layer_mask.fill_run(ly, start, run, 255),
                    }
                }
            }
        }
        self.active = index.min(self.layers.len().saturating_sub(1));
        self.mark_dirty(x0, y0, x1, y1);
        true
    }

    /// Restore the most recent snapshot. Returns false when history is empty.
    pub fn undo(&mut self) -> bool {
        if let Some(entry) = self.undo.pop() {
            self.undo_bytes -= entry.byte_size();
            if let Some(cur) = self.snapshot() {
                self.push_redo(cur);
                if let Some(layer) = self.layers.get_mut(entry.layer) {
                    layer.pixels = entry.pixels;
                    layer.mask = entry.mask;
                    layer.ox = entry.ox;
                    layer.oy = entry.oy;
                    layer.text = entry.text;
                    self.active = entry.layer.min(self.layers.len().saturating_sub(1));
                    self.mark_full_dirty();
                    return true;
                }
            }
        }
        false
    }

    /// Re-apply the most recently undone snapshot. Returns false when empty.
    pub fn redo(&mut self) -> bool {
        if let Some(entry) = self.redo.pop() {
            self.redo_bytes -= entry.byte_size();
            if let Some(cur) = self.snapshot() {
                self.push_undo(cur);
                if let Some(layer) = self.layers.get_mut(entry.layer) {
                    layer.pixels = entry.pixels;
                    layer.mask = entry.mask;
                    layer.ox = entry.ox;
                    layer.oy = entry.oy;
                    layer.text = entry.text;
                    self.active = entry.layer.min(self.layers.len().saturating_sub(1));
                    self.mark_full_dirty();
                    return true;
                }
            }
        }
        false
    }
}
