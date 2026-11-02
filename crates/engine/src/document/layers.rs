//! Layer and group management.

use super::*;

impl Document {
    pub fn add_layer(&mut self, name: String) {
        self.layers.push(Layer::blank(name, self.w, self.h));
        self.active = self.layers.len() - 1;
        self.mark_full_dirty();
    }

    pub fn remove_layer(&mut self, index: usize) {
        if self.layers.len() <= 1 || index >= self.layers.len() {
            return;
        }
        self.layers.remove(index);
        self.active = self.active.min(self.layers.len() - 1);
        self.mark_full_dirty();
    }

    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return;
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        self.active = to;
        self.mark_full_dirty();
    }

    /// Look up a group by id.
    pub fn group(&self, id: u32) -> Option<&LayerGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub(crate) fn group_mut(&mut self, id: u32) -> Option<&mut LayerGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Product of a layer's ancestor group opacities (1.0 when ungrouped):
    /// what compositing, frames and export multiply into the layer alpha.
    /// Corrupt parent cycles break the same bounded walk as visibility, and
    /// non-finite stored values read as opaque (writers sanitize).
    pub fn group_opacity(&self, index: usize) -> f32 {
        let Some(layer) = self.layers.get(index) else {
            return 1.0;
        };
        let mut op = 1.0f32;
        let mut parent = layer.group;
        let mut guard = self.groups.len().saturating_add(1);
        while let Some(id) = parent {
            if guard == 0 {
                break;
            }
            guard -= 1;
            let Some(g) = self.group(id) else {
                break;
            };
            let o = g.opacity;
            op *= if o.is_finite() {
                o.clamp(0.0, 1.0)
            } else {
                1.0
            };
            if op <= 0.0 {
                return 0.0;
            }
            parent = g.parent;
        }
        op
    }

    /// True when the layer and every ancestor group are visible (what
    /// compositing, playback, export and onion paths must honor instead of
    /// the bare layer flag).
    pub fn is_layer_shown(&self, index: usize) -> bool {
        let Some(layer) = self.layers.get(index) else {
            return false;
        };
        if !layer.visible {
            return false;
        }
        let mut parent = layer.group;
        // Bounded walk: a corrupt parent cycle can never hang compositing.
        let mut guard = self.groups.len().saturating_add(1);
        while let Some(id) = parent {
            if guard == 0 {
                break;
            }
            guard -= 1;
            let Some(g) = self.group(id) else {
                break;
            };
            if !g.visible {
                return false;
            }
            parent = g.parent;
        }
        true
    }

    /// Create an empty top-level group and return its id. Structural like
    /// add/remove layer: not undoable, marks the view dirty.
    pub fn new_group(&mut self, name: String) -> u32 {
        self.push_group(name, None)
    }

    /// Create a group around one layer in place. Out-of-range indexes
    /// change nothing and return `None`.
    pub fn group_layer(&mut self, index: usize, name: String) -> Option<u32> {
        self.group_layers(&[index], name)
    }

    /// Create a group around every listed layer, preserving the stacking
    /// order (Photoshop's Group Layers). Out-of-range and duplicate
    /// indexes are ignored; an empty or entirely invalid list returns
    /// `None`. The group nests inside the selection's common group when
    /// all of it already shares one, and lands at the top level when the
    /// selection spans groups.
    pub fn group_layers(&mut self, indexes: &[usize], name: String) -> Option<u32> {
        let mut picked: Vec<usize> = indexes
            .iter()
            .copied()
            .filter(|&i| i < self.layers.len())
            .collect();
        picked.sort_unstable();
        picked.dedup();
        if picked.is_empty() {
            return None;
        }
        let parent = self.shared_group(&picked);
        let id = self.push_group(name, parent);
        for &i in &picked {
            self.layers[i].group = Some(id);
        }
        self.mark_full_dirty();
        Some(id)
    }

    /// The group every listed layer already belongs to, if any. Invalid
    /// indexes are ignored. `None` when the list is empty, spans groups,
    /// sits at the top level, or names a group that no longer exists.
    pub fn shared_group(&self, indexes: &[usize]) -> Option<u32> {
        let mut valid = indexes.iter().copied().filter(|&i| i < self.layers.len());
        let first = valid.next()?;
        let parent = self.layers[first].group?;
        if valid.all(|i| self.layers[i].group == Some(parent)) && self.group(parent).is_some() {
            Some(parent)
        } else {
            None
        }
    }

    fn push_group(&mut self, name: String, parent: Option<u32>) -> u32 {
        // Ids start at 1; 0 always reads as "no group".
        let id = self.next_group_id.max(1);
        self.next_group_id = id.saturating_add(1).max(1);
        self.groups.push(LayerGroup {
            id,
            name,
            visible: true,
            collapsed: false,
            opacity: 1.0,
            parent,
        });
        self.mark_full_dirty();
        id
    }

    pub fn rename_group(&mut self, id: u32, name: String) {
        if let Some(g) = self.group_mut(id) {
            g.name = name;
        }
    }

    pub fn set_group_visible(&mut self, id: u32, visible: bool) {
        if let Some(g) = self.group_mut(id) {
            g.visible = visible;
        }
        self.mark_full_dirty();
    }

    /// Set a group's opacity (non-finite reads as opaque). Structural like
    /// visibility: not undoable, marks the view dirty.
    pub fn set_group_opacity(&mut self, id: u32, opacity: f32) {
        if let Some(g) = self.group_mut(id) {
            g.opacity = if opacity.is_finite() {
                opacity.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        self.mark_full_dirty();
    }

    pub fn set_group_collapsed(&mut self, id: u32, collapsed: bool) {
        if let Some(g) = self.group_mut(id) {
            g.collapsed = collapsed;
        }
    }

    /// Move a layer into a group (or to the top level with `None`),
    /// keeping group members contiguous: the layer lands next to the
    /// group's current members (topmost side).
    pub fn move_layer_into_group(&mut self, index: usize, group: Option<u32>) {
        if index >= self.layers.len() {
            return;
        }
        if let Some(id) = group {
            if self.group(id).is_none() {
                return;
            }
        }
        let mut layer = self.layers.remove(index);
        layer.group = group;
        let at = match group {
            None => self.layers.len(),
            Some(id) => self
                .layers
                .iter()
                .rposition(|l| l.group == Some(id))
                .map_or(self.layers.len(), |p| p + 1),
        };
        // `active` tracked the removed layer: follow it to its new home.
        self.layers.insert(at, layer);
        self.active = at;
        self.mark_full_dirty();
    }

    /// Move a layer up one level: out of its group into the group's parent
    /// (the top level for a root group). The stack position is kept, so the
    /// canvas does not restack. Returns false for a top-level or invalid
    /// layer.
    pub fn move_layer_out_of_group(&mut self, index: usize) -> bool {
        let Some(group) = self.layers.get(index).and_then(|l| l.group) else {
            return false;
        };
        let parent = self.group(group).and_then(|g| g.parent);
        self.layers[index].group = parent;
        self.mark_full_dirty();
        true
    }

    /// Layer indexes inside `id`, nested groups included.
    pub fn group_layer_indexes(&self, id: u32) -> Vec<usize> {
        let mut ids = vec![id];
        let mut k = 0;
        while k < ids.len() {
            let cur = ids[k];
            for g in &self.groups {
                if g.parent == Some(cur) && !ids.contains(&g.id) {
                    ids.push(g.id);
                }
            }
            k += 1;
        }
        (0..self.layers.len())
            .filter(|&i| self.layers[i].group.is_some_and(|g| ids.contains(&g)))
            .collect()
    }

    /// Whether `move_group` would change the stack: the group's layers must
    /// form one contiguous block with a layer to swap past.
    pub fn can_move_group(&self, id: u32, up: bool) -> bool {
        let list = self.group_layer_indexes(id);
        let Some((&lo, &hi)) = list.first().zip(list.last()) else {
            return false;
        };
        if hi - lo + 1 != list.len() {
            return false;
        }
        if up {
            hi + 1 < self.layers.len()
        } else {
            lo > 0
        }
    }

    /// Move a group's whole layer block one step up (`up`) or down, carrying
    /// nested groups with it. Returns false and leaves the stack alone for
    /// empty groups and for groups interleaved with other layers.
    pub fn move_group(&mut self, id: u32, up: bool) -> bool {
        if !self.can_move_group(id, up) {
            return false;
        }
        let list = self.group_layer_indexes(id);
        let (lo, hi) = (list[0], list[list.len() - 1]);
        if up {
            let other = self.layers.remove(hi + 1);
            self.layers.insert(lo, other);
            if self.active == hi + 1 {
                self.active = lo;
            } else if self.active >= lo && self.active <= hi {
                self.active += 1;
            }
        } else {
            let other = self.layers.remove(lo - 1);
            self.layers.insert(hi, other);
            if self.active == lo - 1 {
                self.active = hi;
            } else if self.active >= lo && self.active <= hi {
                self.active -= 1;
            }
        }
        self.mark_full_dirty();
        true
    }

    /// Disband a group: member layers (and subgroups) move up one level,
    /// the group row disappears. Contents always survive (no destructive
    /// group delete in v1).
    pub fn ungroup(&mut self, id: u32) {
        let Some(pos) = self.groups.iter().position(|g| g.id == id) else {
            return;
        };
        let parent = self.groups[pos].parent;
        self.groups.remove(pos);
        for layer in self.layers.iter_mut() {
            if layer.group == Some(id) {
                layer.group = parent;
            }
        }
        for g in self.groups.iter_mut() {
            if g.parent == Some(id) {
                g.parent = parent;
            }
        }
        self.active = self.active.min(self.layers.len().saturating_sub(1));
        self.mark_full_dirty();
    }
}
