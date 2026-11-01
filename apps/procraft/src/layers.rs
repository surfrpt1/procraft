use crate::*;

/// One row of the Layers panel, stack order: a group header (with its
/// nesting depth for indentation and how many layers it holds) or a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayerPanelRow {
    Group {
        id: u32,
        depth: usize,
        members: usize,
    },
    Layer {
        index: usize,
        depth: usize,
    },
}

/// Flatten the layer stack into panel rows. Headers sit above their topmost
/// member; groups with no layers anywhere (fresh, or emptied by moving or
/// deleting members) still get a row so they never vanish, root-level ones
/// at the top. Collapsed groups hide their subtree but keep their header.
pub(crate) fn layer_panel_rows(doc: &Document) -> Vec<LayerPanelRow> {
    let chains: Vec<Vec<u32>> = (0..doc.layers.len())
        .map(|i| {
            let mut up = Vec::new();
            let mut seen = Vec::new();
            let mut g = doc.layers[i].group;
            while let Some(id) = g {
                if seen.contains(&id) {
                    break;
                }
                seen.push(id);
                let Some(gr) = doc.group(id) else {
                    break;
                };
                up.push(id);
                g = gr.parent;
            }
            up.reverse();
            up
        })
        .collect();
    let mut live: Vec<u32> = Vec::new();
    for chain in &chains {
        for id in chain {
            if !live.contains(id) {
                live.push(*id);
            }
        }
    }
    let members = |id: u32| chains.iter().filter(|c| c.contains(&id)).count();
    let collapsed = |id: u32| doc.group(id).is_some_and(|g| g.collapsed);
    let mut rows = Vec::new();
    let mut printed: Vec<u32> = Vec::new();
    push_layerless_groups(doc, None, 0, &live, &mut printed, &mut rows);
    for i in (0..doc.layers.len()).rev() {
        let chain = &chains[i];
        for (depth, id) in chain.iter().enumerate() {
            if printed.contains(id) || chain[..depth].iter().any(|a| collapsed(*a)) {
                continue;
            }
            printed.push(*id);
            rows.push(LayerPanelRow::Group {
                id: *id,
                depth,
                members: members(*id),
            });
            if !collapsed(*id) {
                push_layerless_groups(doc, Some(*id), depth + 1, &live, &mut printed, &mut rows);
            }
        }
        if chain.iter().any(|id| collapsed(*id)) {
            continue;
        }
        rows.push(LayerPanelRow::Layer {
            index: i,
            depth: chain.len(),
        });
    }
    rows
}

/// Append the layerless groups under `parent` (newest first), recursing into
/// their own layerless children and honoring collapse.
pub(crate) fn push_layerless_groups(
    doc: &Document,
    parent: Option<u32>,
    depth: usize,
    live: &[u32],
    printed: &mut Vec<u32>,
    rows: &mut Vec<LayerPanelRow>,
) {
    let mut children: Vec<u32> = doc
        .groups
        .iter()
        .filter(|g| g.parent == parent && !live.contains(&g.id) && !printed.contains(&g.id))
        .map(|g| g.id)
        .collect();
    children.sort_unstable_by(|a, b| b.cmp(a));
    for id in children {
        printed.push(id);
        let collapsed = doc.group(id).is_some_and(|g| g.collapsed);
        rows.push(LayerPanelRow::Group {
            id,
            depth,
            members: 0,
        });
        if !collapsed {
            push_layerless_groups(doc, Some(id), depth + 1, live, printed, rows);
        }
    }
}

/// Group-header button presses, applied after the row loop so the panel can
/// draw from `&self.doc` without holding a borrow while it mutates it.
#[derive(Default)]
pub(crate) struct GroupRowActions {
    pub(crate) collapse: Option<u32>,
    pub(crate) eye: Option<u32>,
    pub(crate) add: Option<u32>,
    pub(crate) ungroup: Option<u32>,
    pub(crate) opacity: Option<(u32, f32)>,
    pub(crate) rename: Option<(u32, String)>,
    /// `(group, up)` from the header menu.
    pub(crate) reorder: Option<(u32, bool)>,
}

/// What a batch of header presses changed: `dirty` means the canvas ghost
/// needs a rebuild, `status` is the line to show.
#[derive(Default)]
pub(crate) struct GroupActionReport {
    pub(crate) status: Option<String>,
    pub(crate) dirty: bool,
}

/// Apply the header presses collected during the row loop. Split out of the
/// draw so the wiring is testable without an egui context.
pub(crate) fn apply_group_actions(doc: &mut Document, acts: &GroupRowActions) -> GroupActionReport {
    let mut report = GroupActionReport::default();
    if let Some(gid) = acts.collapse {
        let cur = doc.group(gid).is_some_and(|g| g.collapsed);
        doc.set_group_collapsed(gid, !cur);
    }
    if let Some(gid) = acts.eye {
        let cur = doc.group(gid).is_none_or(|g| g.visible);
        doc.set_group_visible(gid, !cur);
        report.dirty = true;
    }
    if let Some((gid, up)) = acts.reorder {
        if doc.move_group(gid, up) {
            let name = doc.group(gid).map_or(String::new(), |g| g.name.clone());
            report.status = Some(format!("{name} moved {}.", if up { "up" } else { "down" }));
            report.dirty = true;
        }
    }
    if let Some(gid) = acts.add {
        let active = doc.active;
        doc.move_layer_into_group(active, Some(gid));
        report.dirty = true;
        report.status = Some(format!(
            "{} moved into the group.",
            doc.layers[doc.active].name
        ));
    }
    if let Some(gid) = acts.ungroup {
        doc.ungroup(gid);
        report.dirty = true;
        report.status = Some("Group disbanded (layers kept).".to_string());
    }
    if let Some((gid, op)) = acts.opacity {
        doc.set_group_opacity(gid, op);
        report.dirty = true;
    }
    if let Some((gid, name)) = acts.rename.as_ref() {
        doc.rename_group(*gid, name.clone());
    }
    report
}

/// Inline member count for a group header (`(3)`). Empty groups show an
/// "empty" marker instead, so they get no badge.
pub(crate) fn group_member_badge(members: usize) -> Option<String> {
    (members > 0).then(|| format!("({members})"))
}

/// Layers-panel selection after a row click. A plain click selects one
/// row; `toggle` (Ctrl/Cmd-click or long-press) adds or removes the row;
/// `extend` (Shift-click) takes the range from the active layer. The
/// selection never goes empty, so there is always an active row to act on.
pub(crate) fn next_layer_selection(
    selection: &[usize],
    active: usize,
    target: usize,
    toggle: bool,
    extend: bool,
) -> Vec<usize> {
    if extend {
        let (lo, hi) = (active.min(target), active.max(target));
        return (lo..=hi).collect();
    }
    if toggle {
        let mut next: Vec<usize> = selection.iter().copied().filter(|&i| i != target).collect();
        if next.len() == selection.len() {
            next.push(target);
        }
        next.sort_unstable();
        next.dedup();
        if next.is_empty() {
            next.push(target);
        }
        return next;
    }
    vec![target]
}

/// Consume a pending "bring this row into view" request when the row it
/// names is drawn. The request must fire once: the header redraws every
/// frame, and repeating the scroll would fight the user's own scrolling.
pub(crate) fn take_scroll_request(request: &mut Option<u32>, id: u32) -> bool {
    if *request == Some(id) {
        *request = None;
        true
    } else {
        false
    }
}

/// Status line after "New group": naming the parent explains why the new
/// group landed indented inside an existing one.
pub(crate) fn group_created_status(name: &str, parent: Option<&str>, targets: &str) -> String {
    match parent {
        Some(parent) => format!("{name} created inside {parent} with {targets}."),
        None => format!("{name} created with {targets}."),
    }
}

/// Layers-panel footer text: layer count, the active layer's name and the
/// document's allocated tile memory (painted tiles plus masks). Fresh
/// layers cost nothing until a tile is touched, so this starts at `0 B`
/// and grows by 256 KB per painted tile.
pub(crate) fn layer_readout(doc: &Document) -> String {
    format!(
        "{} layer(s), active: {} · {}",
        doc.layers.len(),
        doc.layers
            .get(doc.active)
            .map(|l| l.name.as_str())
            .unwrap_or("-"),
        file_size_label(doc.layer_alloc_bytes() as u64)
    )
}

/// Downscale the composite to a `side`-bounding box by sampling
/// `Document::sample` on a block grid (max 2x2 samples per output pixel).
/// Unlike `flatten` + `gallery_thumb`, this never materialises the full
/// canvas (268 MB at 8192²), so it stays cheap on the UI thread while
/// recording a time-lapse or rebuilding the Gallery. Aspect is preserved;
/// documents already within `side` keep their dimensions (no upscale).
/// Returns `(rgba, width, height)`; empty input yields an empty thumbnail.
pub(crate) fn sampled_thumb(doc: &Document, side: u32) -> (Vec<u8>, usize, usize) {
    let (w, h) = (doc.w, doc.h);
    if w == 0 || h == 0 || side == 0 {
        return (Vec::new(), 0, 0);
    }
    let longest = w.max(h);
    let (tw, th) = if longest <= side {
        (w, h)
    } else {
        let scale = side as f64 / longest as f64;
        (
            ((w as f64 * scale).round() as u32).max(1),
            ((h as f64 * scale).round() as u32).max(1),
        )
    };
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    for ty in 0..th {
        // Source rows under this output row: probe the first and last one,
        // so a block averages across its extent instead of one corner.
        let sy0 = ty * h / th;
        let sy1 = ((ty + 1) * h / th).saturating_sub(1).max(sy0);
        for tx in 0..tw {
            let sx0 = tx * w / tw;
            let sx1 = ((tx + 1) * w / tw).saturating_sub(1).max(sx0);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for &sy in &[sy0, sy1] {
                for &sx in &[sx0, sx1] {
                    if let Some(px) = doc.sample(sx, sy) {
                        r += px[0] as u32;
                        g += px[1] as u32;
                        b += px[2] as u32;
                        n += 1;
                    }
                }
            }
            if n == 0 {
                continue;
            }
            let d = ((ty * tw + tx) * 4) as usize;
            out[d] = ((r + n / 2) / n) as u8;
            out[d + 1] = ((g + n / 2) / n) as u8;
            out[d + 2] = ((b + n / 2) / n) as u8;
            out[d + 3] = 255;
        }
    }
    (out, tw as usize, th as usize)
}
