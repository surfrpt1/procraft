use super::*;

/// Test-only: an RGBA raster sized to the document from dense bytes.
fn px_rgba(doc: &Document, bytes: &[u8]) -> Raster {
    Raster::from_dense(doc.w, doc.h, 4, bytes)
}

/// Test-only: a 1-bpp mask sized to the document from dense bytes.
fn px_mask(doc: &Document, bytes: &[u8]) -> Raster {
    Raster::from_dense(doc.w, doc.h, 1, bytes)
}

#[test]
fn revision_advances_on_dirty_marks() {
    let mut doc = Document::new(8, 8);
    let r0 = doc.revision();
    doc.mark_full_dirty();
    assert_eq!(doc.revision(), r0 + 1);
    doc.mark_dirty(1, 1, 2, 2);
    assert_eq!(doc.revision(), r0 + 2);
    // A dab fully outside the canvas paints nothing: no bump.
    doc.mark_dirty(5, 5, 1, 1);
    assert_eq!(doc.revision(), r0 + 2);
}

#[test]
fn region_restore_rewinds_only_the_rect() {
    let mut doc = Document::new(8, 8);
    let idx = |x: u32, y: u32| ((y * 8 + x) * 4) as usize;
    doc.layers[0].pixels.set4(idx(1, 1), [10, 0, 0, 255]);
    doc.layers[0].pixels.set4(idx(6, 6), [20, 0, 0, 255]);
    doc.checkpoint();
    doc.layers[0].pixels.set4(idx(1, 1), [99, 0, 0, 255]);
    doc.layers[0].pixels.set4(idx(6, 6), [99, 0, 0, 255]);
    assert!(doc.restore_checkpoint_region((0, 0, 3, 3)));
    assert_eq!(doc.layers[0].pixels.get4(idx(1, 1)), [10, 0, 0, 255]);
    // Outside the rect the live edit stands: the replay redraws it.
    assert_eq!(doc.layers[0].pixels.get4(idx(6, 6)), [99, 0, 0, 255]);
}

#[test]
fn region_restore_copies_wide_bands_across_tiles() {
    // 600 px wide crosses two 256-px tile borders.
    let mut doc = Document::new(600, 4);
    let idx = |x: u32, y: u32| ((y * 600 + x) * 4) as usize;
    for x in 0..600 {
        doc.layers[0].pixels.set4(idx(x, 1), [7, 0, 0, 255]);
    }
    doc.checkpoint();
    for x in 0..600 {
        doc.layers[0].pixels.set4(idx(x, 1), [99, 0, 0, 255]);
    }
    doc.layers[0].pixels.set4(idx(0, 0), [5, 0, 0, 255]);
    assert!(doc.restore_checkpoint_region((0, 1, 599, 1)));
    for x in 0..600 {
        assert_eq!(
            doc.layers[0].pixels.get4(idx(x, 1)),
            [7, 0, 0, 255],
            "x={x}"
        );
    }
    assert_eq!(doc.layers[0].pixels.get4(idx(0, 0)), [5, 0, 0, 255]);
}

#[test]
fn region_restore_refills_a_mask_the_stroke_created() {
    let mut doc = Document::new(4, 2);
    doc.checkpoint();
    // Mask created after the checkpoint: it has no snapshot to copy back.
    doc.layers[0].mask = Some(Raster::from_dense(
        4,
        2,
        1,
        &[10, 20, 30, 40, 50, 60, 70, 80],
    ));
    assert!(doc.restore_checkpoint_region((0, 0, 3, 1)));
    assert_eq!(
        doc.layers[0].mask.as_ref().unwrap().to_dense(),
        vec![255; 8]
    );
}

#[test]
fn region_restore_follows_the_checkpoint_offset() {
    let mut doc = Document::new(8, 2);
    // Layer shifted left by one: canvas x maps to local x + 1.
    doc.layers[0].ox = -1;
    let at = ((8 + 2) * 4) as usize;
    doc.layers[0].pixels.set4(at, [3, 0, 0, 255]);
    doc.checkpoint();
    doc.layers[0].pixels.set4(at, [99, 0, 0, 255]);
    assert!(doc.restore_checkpoint_region((0, 1, 7, 1)));
    assert_eq!(doc.layers[0].pixels.get4(at), [3, 0, 0, 255]);
}

#[test]
fn opaque_top_layer_wins() {
    let mut doc = Document::new(2, 1);
    // Bottom layer: solid red.
    {
        let l = &mut doc.layers[0];
        l.pixels = Raster::from_dense(2, 1, 4, &[255, 0, 0, 255, 255, 0, 0, 255]);
    }
    doc.add_layer("top".to_string());
    // Top layer, first pixel green, second transparent.
    {
        let l = &mut doc.layers[1];
        l.pixels = Raster::from_dense(2, 1, 4, &[0, 255, 0, 255, 0, 0, 0, 0]);
    }
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[0, 255, 0, 255]);
    assert_eq!(&flat[4..8], &[255, 0, 0, 255]);
}

#[test]
fn undo_restores_pixels() {
    let mut doc = Document::new(1, 1);
    doc.checkpoint();
    doc.layers[0].pixels = px_rgba(&doc, &[9, 9, 9, 9]);
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
    assert!(!doc.undo());
}

#[test]
fn restore_last_checkpoint_rewinds_without_dropping_undo() {
    let mut doc = Document::new(1, 1);
    assert!(!doc.restore_last_checkpoint());
    doc.checkpoint();
    doc.layers[0].pixels = px_rgba(&doc, &[9, 9, 9, 9]);
    assert!(doc.restore_last_checkpoint());
    assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
    // The checkpoint survives the rewind and still undoes normally.
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
}

#[test]
fn redo_restores_undone_work() {
    let mut doc = Document::new(1, 1);
    assert!(!doc.redo());
    doc.checkpoint();
    doc.layers[0].pixels = px_rgba(&doc, &[9, 9, 9, 9]);
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
    assert!(doc.redo());
    assert_eq!(doc.layers[0].pixels, vec![9, 9, 9, 9]);
    assert!(!doc.redo());
    // Undo still works after a redo.
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
}

#[test]
fn layer_rename_is_undoable_and_redoable() {
    let mut doc = Document::new(1, 1);
    doc.layers[0].name = "Base".to_string();
    assert!(doc.rename_layer(0, "Sky".to_string()));
    assert_eq!(doc.layers[0].name, "Sky");
    assert!(doc.undo());
    assert_eq!(doc.layers[0].name, "Base");
    assert!(doc.redo());
    assert_eq!(doc.layers[0].name, "Sky");
}

#[test]
fn rename_undo_follows_stroke_order() {
    let mut doc = Document::new(1, 1);
    doc.layers[0].name = "Base".to_string();
    doc.checkpoint();
    doc.layers[0].pixels = px_rgba(&doc, &[9, 9, 9, 255]);
    assert!(doc.rename_layer(0, "Sky".to_string()));
    assert!(doc.undo());
    assert_eq!(doc.layers[0].name, "Base");
    assert_eq!(doc.layers[0].pixels, vec![9, 9, 9, 255]);
    assert!(doc.undo());
    assert_eq!(doc.layers[0].pixels, vec![0, 0, 0, 0]);
    assert!(doc.redo());
    assert_eq!(doc.layers[0].pixels, vec![9, 9, 9, 255]);
    assert!(doc.redo());
    assert_eq!(doc.layers[0].name, "Sky");
}

#[test]
fn renaming_to_the_same_name_adds_no_undo_step() {
    let mut doc = Document::new(1, 1);
    doc.layers[0].name = "Base".to_string();
    assert!(!doc.rename_layer(0, "Base".to_string()));
    assert!(!doc.undo());
}

#[test]
fn group_rename_is_undoable_and_redoable() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("top".to_string());
    let id = doc.group_layers(&[0, 1], "Group".to_string()).unwrap();
    assert!(doc.rename_group(id, "Sky".to_string()));
    assert_eq!(doc.group(id).unwrap().name, "Sky");
    assert!(doc.undo());
    assert_eq!(doc.group(id).unwrap().name, "Group");
    assert!(doc.redo());
    assert_eq!(doc.group(id).unwrap().name, "Sky");
}

#[test]
fn group_hide_conceals_members_in_flatten() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels = px_rgba(&doc, &[255, 0, 0, 255, 255, 0, 0, 255]);
    doc.add_layer("top".to_string());
    doc.layers[1].pixels = px_rgba(&doc, &[0, 0, 255, 255, 0, 0, 255, 255]);
    let g = doc.new_group("G".to_string());
    doc.move_layer_into_group(1, Some(g));
    assert!(doc.is_layer_shown(1));
    doc.set_group_visible(g, false);
    assert!(!doc.is_layer_shown(1));
    assert!(doc.is_layer_shown(0));
    // Top pixel falls through to the red layer below.
    assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
    doc.set_group_visible(g, true);
    assert_eq!(&doc.flatten()[0..4], &[0, 0, 255, 255]);
}

#[test]
fn nested_groups_need_every_level_visible() {
    let mut doc = Document::new(1, 1);
    let outer = doc.new_group("outer".to_string());
    let inner = doc.new_group("inner".to_string());
    doc.groups
        .iter_mut()
        .find(|g| g.id == inner)
        .unwrap()
        .parent = Some(outer);
    doc.move_layer_into_group(0, Some(inner));
    assert!(doc.is_layer_shown(0));
    doc.set_group_visible(inner, false);
    assert!(!doc.is_layer_shown(0));
    doc.set_group_visible(inner, true);
    doc.set_group_visible(outer, false);
    assert!(!doc.is_layer_shown(0));
}

#[test]
fn ungroup_spills_members_up_one_level() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    let outer = doc.new_group("outer".to_string());
    let inner = doc.new_group("inner".to_string());
    doc.groups
        .iter_mut()
        .find(|g| g.id == inner)
        .unwrap()
        .parent = Some(outer);
    doc.move_layer_into_group(0, Some(inner));
    doc.move_layer_into_group(0, Some(outer));
    doc.ungroup(inner);
    assert!(doc.group(inner).is_none());
    assert_eq!(doc.layers[0].group, Some(outer));
    assert_eq!(doc.layers[1].group, Some(outer));
    doc.ungroup(outer);
    assert!(doc.groups.is_empty());
    assert!(doc.layers.iter().all(|l| l.group.is_none()));
}

#[test]
fn move_out_of_group_keeps_the_stack_position() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    doc.add_layer("c".to_string());
    let g = doc.group_layer(0, "g".to_string()).expect("layer 0 groups");
    assert_eq!(doc.layers[0].group, Some(g));
    assert!(doc.move_layer_out_of_group(0));
    assert_eq!(doc.layers[0].group, None);
    // Leaving the group must not restack the canvas.
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Layer 1", "b", "c"]);
    // A top-level layer has nowhere to go.
    assert!(!doc.move_layer_out_of_group(0));
    // Out-of-range indexes are a no-op, not a panic.
    assert!(!doc.move_layer_out_of_group(9));
}

#[test]
fn move_out_of_a_nested_group_lands_in_the_parent() {
    let mut doc = Document::new(1, 1);
    let outer = doc.new_group("outer".to_string());
    doc.move_layer_into_group(0, Some(outer));
    let inner = doc
        .group_layer(0, "inner".to_string())
        .expect("layer 0 groups");
    assert!(doc.move_layer_out_of_group(0));
    assert_eq!(doc.layers[0].group, Some(outer));
    assert!(doc.group(inner).is_some());
}

#[test]
fn move_group_shifts_its_whole_block() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    doc.add_layer("c".to_string());
    let g = doc.group_layer(0, "g".to_string()).expect("layer 0 groups");
    doc.active = 0;
    assert!(doc.move_group(g, true));
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["b", "Layer 1", "c"]);
    assert_eq!(doc.layers[0].group, None);
    assert_eq!(doc.layers[1].group, Some(g));
    assert_eq!(doc.layers[doc.active].name, "Layer 1");
    assert!(doc.move_group(g, true));
    assert_eq!(doc.layers[2].group, Some(g));
    assert_eq!(doc.layers[doc.active].name, "Layer 1");
    // Already at the top of the stack.
    assert!(!doc.can_move_group(g, true));
    assert!(!doc.move_group(g, true));
    assert!(doc.move_group(g, false));
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["b", "Layer 1", "c"]);
    assert_eq!(doc.layers[doc.active].name, "Layer 1");
}

#[test]
fn move_group_refuses_interleaved_members() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    doc.add_layer("c".to_string());
    doc.add_layer("d".to_string());
    let g = doc
        .group_layers(&[0, 2], "g".to_string())
        .expect("two layers group");
    assert!(!doc.can_move_group(g, true));
    assert!(!doc.move_group(g, true));
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Layer 1", "b", "c", "d"]);
}

#[test]
fn move_group_carries_nested_layers() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    let outer = doc.new_group("outer".to_string());
    let inner = doc.new_group("inner".to_string());
    doc.groups
        .iter_mut()
        .find(|g| g.id == inner)
        .unwrap()
        .parent = Some(outer);
    doc.layers[0].group = Some(outer);
    doc.layers[1].group = Some(inner);
    doc.add_layer("above".to_string());
    assert!(doc.can_move_group(outer, true));
    assert!(doc.move_group(outer, true));
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["above", "Layer 1", "b"]);
    assert_eq!(doc.layers[1].group, Some(outer));
    assert_eq!(doc.layers[2].group, Some(inner));
    assert_eq!(doc.group(inner).unwrap().parent, Some(outer));
    // A group with no layers has no stacking position to move.
    let empty = doc.new_group("empty".to_string());
    assert!(!doc.can_move_group(empty, true));
    assert!(!doc.can_move_group(empty, false));
}

#[test]
fn group_layer_wraps_a_layer_in_place() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("mid".to_string());
    doc.add_layer("top".to_string());
    doc.active = 1;
    let g = doc
        .group_layer(1, "Group 1".to_string())
        .expect("layer 1 groups");
    assert_eq!(doc.layers[1].group, Some(g));
    assert!(doc
        .group(g)
        .is_some_and(|gr| gr.parent.is_none() && !gr.collapsed && gr.visible));
    // Grouping must not restack the canvas.
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Layer 1", "mid", "top"]);
    assert_eq!(doc.active, 1);
    // Out-of-range indexes are a no-op, not a panic.
    let before = doc.groups.len();
    assert!(doc.group_layer(99, "nope".to_string()).is_none());
    assert_eq!(doc.groups.len(), before);
}

#[test]
fn group_layer_nests_inside_the_parent_group() {
    let mut doc = Document::new(1, 1);
    let outer = doc.new_group("outer".to_string());
    doc.move_layer_into_group(0, Some(outer));
    let inner = doc
        .group_layer(0, "inner".to_string())
        .expect("layer 0 groups");
    assert_eq!(doc.group(inner).unwrap().parent, Some(outer));
    assert_eq!(doc.layers[0].group, Some(inner));
    // The outer group now only holds the nested group, not the layer.
    assert_eq!(
        doc.layers.iter().filter(|l| l.group == Some(outer)).count(),
        0
    );
}

#[test]
fn shared_group_requires_one_common_parent() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    doc.add_layer("c".to_string());
    let g = doc.new_group("g".to_string());
    doc.layers[0].group = Some(g);
    doc.layers[1].group = Some(g);
    assert_eq!(doc.shared_group(&[0, 1]), Some(g));
    // Mixed parentage and empty/unknown indexes have no common group.
    assert_eq!(doc.shared_group(&[0, 2]), None);
    assert_eq!(doc.shared_group(&[]), None);
    assert_eq!(doc.shared_group(&[9]), None);
    // A dangling group id is not a home.
    doc.layers[0].group = Some(999);
    assert_eq!(doc.shared_group(&[0]), None);
}

#[test]
fn group_layers_wraps_every_selected_layer() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("mid".to_string());
    doc.add_layer("top".to_string());
    let g = doc
        .group_layers(&[0, 2], "Both".to_string())
        .expect("two layers group");
    assert_eq!(doc.groups.len(), 1);
    assert!(doc.group(g).is_some_and(|gr| gr.parent.is_none()));
    assert_eq!(doc.layers[0].group, Some(g));
    assert_eq!(doc.layers[1].group, None);
    assert_eq!(doc.layers[2].group, Some(g));
    // Non-contiguous members keep their stacking positions.
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Layer 1", "mid", "top"]);
}

#[test]
fn group_layers_ignores_bad_and_duplicate_indexes() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    assert!(doc.group_layers(&[], "none".to_string()).is_none());
    assert!(doc.group_layers(&[5, 9], "none".to_string()).is_none());
    assert!(doc.groups.is_empty());
    let g = doc
        .group_layers(&[1, 1, 0], "both".to_string())
        .expect("valid indexes group");
    assert_eq!(doc.layers[0].group, Some(g));
    assert_eq!(doc.layers[1].group, Some(g));
    assert_eq!(doc.groups.len(), 1);
}

#[test]
fn group_layers_nests_only_when_the_selection_shares_a_group() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    doc.add_layer("c".to_string());
    let outer = doc.new_group("outer".to_string());
    doc.layers[0].group = Some(outer);
    doc.layers[1].group = Some(outer);
    let inner = doc
        .group_layers(&[0, 1], "inner".to_string())
        .expect("shared-parent selection groups");
    assert_eq!(doc.group(inner).unwrap().parent, Some(outer));
    assert_eq!(doc.layers[0].group, Some(inner));
    assert_eq!(doc.layers[1].group, Some(inner));

    // A selection spanning parents lands at the top level.
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    doc.add_layer("c".to_string());
    let g = doc.new_group("g".to_string());
    doc.layers[0].group = Some(g);
    let mixed = doc
        .group_layers(&[0, 2], "mixed".to_string())
        .expect("mixed selection groups");
    assert_eq!(doc.group(mixed).unwrap().parent, None);
    assert_eq!(doc.layers[0].group, Some(mixed));
    assert_eq!(doc.layers[1].group, None);
    assert_eq!(doc.layers[2].group, Some(mixed));
}

#[test]
fn move_into_group_lands_adjacent() {
    let mut doc = Document::new(1, 1);
    doc.add_layer("b".to_string());
    doc.add_layer("c".to_string());
    let g = doc.new_group("G".to_string());
    // Bottom layer joins the group: inserted at the top end.
    doc.move_layer_into_group(0, Some(g));
    assert_eq!(doc.layers[2].name, "Layer 1");
    assert_eq!(doc.layers[2].group, Some(g));
    assert_eq!(doc.active, 2);
    // Unknown groups and indices are ignored.
    doc.move_layer_into_group(0, Some(999));
    assert!(doc.layers[0].group.is_none());
    doc.move_layer_into_group(99, Some(g));
    assert_eq!(doc.layers.len(), 3);
}

#[test]
fn v8_roundtrip_keeps_groups() {
    let mut doc = Document::new(2, 1);
    let g = doc.new_group("Sketch".to_string());
    doc.set_group_visible(g, false);
    doc.move_layer_into_group(0, Some(g));
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v8.pcraft");
    doc.save_project(&path).unwrap();
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[..8], b"PCRAFT12");
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.groups.len(), 1);
    assert_eq!(back.groups[0].name, "Sketch");
    assert!(!back.groups[0].visible);
    assert_eq!(back.layers[0].group, Some(g));
    assert!(!back.is_layer_shown(0));
    // Ids keep increasing after a reload (no reuse).
    let mut back = back;
    let g2 = back.new_group("More".to_string());
    assert!(g2 > g);
}

#[test]
fn group_opacity_fades_members() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels = px_rgba(&doc, &([255, 0, 0, 255].repeat(2)));
    let g = doc.new_group("Fade".to_string());
    doc.move_layer_into_group(0, Some(g));
    assert_eq!(doc.group_opacity(0), 1.0);
    doc.set_group_opacity(g, 0.5);
    assert_eq!(doc.group_opacity(0), 0.5);
    // Red at half alpha over the white page.
    assert_eq!(&doc.flatten()[0..4], &[255, 128, 128, 255]);
    // Nested products multiply: 0.5 x 0.5.
    let outer = doc.new_group("Outer".to_string());
    doc.groups.iter_mut().find(|x| x.id == g).unwrap().parent = Some(outer);
    doc.set_group_opacity(outer, 0.5);
    assert_eq!(doc.group_opacity(0), 0.25);
    assert_eq!(&doc.flatten()[0..4], &[255, 191, 191, 255]);
    // Zero-opacity groups conceal like hidden ones.
    doc.set_group_opacity(g, 0.0);
    assert_eq!(&doc.flatten()[0..4], &[255, 255, 255, 255]);
    // Frames fade with their group too (playback, onion, GIF):
    // fully faded means no ghost pixels at all.
    let ghost = crate::onion_ghost(&doc, 0, 1.0);
    assert_eq!(&ghost[0..4], &[0, 0, 0, 0]);
}

#[test]
fn v9_roundtrip_keeps_group_opacity() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels = px_rgba(&doc, &([255, 0, 0, 255].repeat(2)));
    let g = doc.new_group("Fade".to_string());
    doc.set_group_opacity(g, 0.5);
    doc.move_layer_into_group(0, Some(g));
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v9.pcraft");
    doc.save_project(&path).unwrap();
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[..8], b"PCRAFT12");
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.groups.len(), 1);
    assert_eq!(back.groups[0].opacity, 0.5);
    assert_eq!(back.layers[0].group, Some(g));
    // Opacity composes after reload too.
    assert_eq!(&back.flatten()[0..4], &[255, 128, 128, 255]);
}

#[test]
fn v8_files_open_with_opaque_groups() {
    // Hand-build a minimal version-8 file (group record without the
    // v9 opacity tail).
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v8-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT08");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Symmetry off.
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&[0]); // No mirror center.
    buf.extend_from_slice(&1u32.to_le_bytes()); // One group.
    buf.extend_from_slice(&7u32.to_le_bytes()); // id
    buf.extend_from_slice(&[0]); // No parent.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"G");
    buf.extend_from_slice(&[1]); // Visible.
    buf.extend_from_slice(&[0]); // Expanded.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Blend: Normal.
    buf.extend_from_slice(&[0]); // Clip: off.
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&[1]); // In group 7.
    buf.extend_from_slice(&7u32.to_le_bytes());
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[9u8; 2 * 4]).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&[0]); // No mask.
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.groups.len(), 1);
    assert_eq!(back.groups[0].opacity, 1.0);
    assert_eq!(back.layers[0].group, Some(7));
}

#[test]
fn v10_roundtrip_keeps_frame_holds() {
    let mut doc = Document::new(2, 1);
    doc.add_layer("F2".to_string());
    doc.layers[0].hold = 3;
    doc.layers[1].hold = 8;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v10.pcraft");
    doc.save_project(&path).unwrap();
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[..8], b"PCRAFT12");
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].hold, 3);
    assert_eq!(back.layers[1].hold, 8);
}

#[test]
fn v9_files_open_with_unit_holds() {
    // Hand-build a minimal version-9 file (group opacity present, no
    // layer hold tails).
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v9-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT09");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Symmetry off.
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&[0]); // No mirror center.
    buf.extend_from_slice(&1u32.to_le_bytes()); // One group.
    buf.extend_from_slice(&7u32.to_le_bytes()); // id
    buf.extend_from_slice(&[0]); // No parent.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"G");
    buf.extend_from_slice(&[1]); // Visible.
    buf.extend_from_slice(&[0]); // Expanded.
    buf.extend_from_slice(&0.5f32.to_le_bytes()); // Group opacity.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Blend: Normal.
    buf.extend_from_slice(&[0]); // Clip: off.
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&[1]); // In group 7.
    buf.extend_from_slice(&7u32.to_le_bytes());
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[9u8; 2 * 4]).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&[0]); // No mask.
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].hold, 1);
    assert_eq!(back.groups[0].opacity, 0.5);
    assert_eq!(back.layers[0].group, Some(7));
}

#[test]
fn v10_project_opens_text_less() {
    // Hand-build a minimal version-10 file (frame hold tail, no v11
    // text record).
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v10-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT10");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Symmetry off.
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&[0]); // No mirror center.
    buf.extend_from_slice(&0u32.to_le_bytes()); // No groups.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Blend: Normal.
    buf.extend_from_slice(&[0]); // Clip: off.
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&[0]); // No group.
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[9u8; 2 * 4]).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&[0]); // No mask.
    buf.extend_from_slice(&1u32.to_le_bytes()); // Frame hold.
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(!back.layers[0].is_text());
}

#[test]
fn save_emits_v12_magic() {
    let doc = Document::new(2, 1);
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-magic.pcraft");
    doc.save_project(&path).unwrap();
    let head = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(&head[..8], b"PCRAFT12");
}

/// Hand-build a v12 record with one layer holding one tile. Returns the
/// buffer plus the offsets the validation tests mutate.
struct V12Fixture {
    buf: Vec<u8>,
    count_at: usize,
    tx_at: usize,
    len_at: usize,
    payload_at: usize,
    payload_end: usize,
}

fn v12_fixture(tx: u16, ty: u16, raw: &[u8]) -> V12Fixture {
    use std::io::Write;
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT12");
    buf.extend_from_slice(&256u32.to_le_bytes());
    buf.extend_from_slice(&256u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes()); // One layer.
    buf.extend_from_slice(&0u32.to_le_bytes()); // Active layer.
    buf.extend_from_slice(&[0]); // Symmetry off.
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&[0]); // No mirror center.
    buf.extend_from_slice(&0u32.to_le_bytes()); // No groups.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Blend: Normal.
    buf.extend_from_slice(&[0]); // Clip: off.
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&[0]); // No group.
    let count_at = buf.len();
    buf.extend_from_slice(&1u32.to_le_bytes()); // One tile.
    let tx_at = buf.len();
    buf.extend_from_slice(&tx.to_le_bytes());
    buf.extend_from_slice(&ty.to_le_bytes());
    let len_at = buf.len();
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(raw).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    let payload_at = buf.len();
    buf.extend_from_slice(&packed);
    let payload_end = buf.len();
    buf.extend_from_slice(&[0]); // No mask.
    buf.extend_from_slice(&1u32.to_le_bytes()); // Frame hold.
    buf.extend_from_slice(&[0]); // No text.
    V12Fixture {
        buf,
        count_at,
        tx_at,
        len_at,
        payload_at,
        payload_end,
    }
}

#[test]
fn v12_roundtrip_keeps_sparse_layers_small() {
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-sparse.procraft");
    let mut doc = Document::new(8192, 8192);
    doc.add_layer("second".into());
    doc.layers[0].pixels.set4(0, [1, 2, 3, 255]);
    doc.save_project(&path).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size < 64 * 1024, "sparse 8K project is {size} bytes");
    let back = Document::open_project(&path).unwrap();
    assert_eq!(back.layers[0].pixels.get4(0), [1, 2, 3, 255]);
    assert_eq!(back.layers[0].pixels.alloc_bytes(), 256 * 1024);
    assert_eq!(back.layers[1].pixels.alloc_bytes(), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn v12_roundtrip_omits_fully_erased_tiles() {
    use crate::brush::{Brush, Color};
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-erased.procraft");
    let mut doc = Document::new(8192, 8192);
    // Paint black, then erase it: the eraser only zeroes alpha, so the
    // tile stays allocated (allocated-but-empty), which is the case the
    // writer must drop.
    let paint = Brush {
        size: 16.0,
        softness: 0.0,
        ..Default::default()
    };
    doc.dab(&paint, Color::rgb(0, 0, 0), 16.0, 16.0);
    assert_eq!(doc.layers[0].pixels.allocated_tiles().count(), 1);
    let eraser = Brush {
        eraser: true,
        size: 32.0,
        ..paint
    };
    doc.dab(&eraser, Color::rgb(0, 0, 0), 16.0, 16.0);
    assert_eq!(
        doc.layers[0].pixels.allocated_tiles().count(),
        1,
        "the erased tile stays allocated"
    );
    assert!(
        doc.layers[0]
            .pixels
            .allocated_tiles()
            .all(|(_, _, t)| t.iter().all(|&b| b == 0)),
        "the erased tile is all zero"
    );
    doc.save_project(&path).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size < 64 * 1024, "fully erased 8K project is {size} bytes");
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].pixels.alloc_bytes(), 0);
}

#[test]
fn v12_roundtrip_omits_fully_erased_colored_tiles() {
    use crate::brush::{Brush, Color};
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-erased-colored.procraft");
    let mut doc = Document::new(8192, 8192);
    // Coloured counterpart of the black case: the eraser canonicalises
    // alpha-0 pixels to [0, 0, 0, 0], so a fully erased coloured tile is
    // all-zero and the writer omits it. Without that, reload would
    // re-allocate 256 KiB for a tile nobody can see.
    let paint = Brush {
        size: 16.0,
        softness: 0.0,
        ..Default::default()
    };
    doc.dab(&paint, Color::rgb(200, 40, 40), 16.0, 16.0);
    assert_eq!(doc.layers[0].pixels.allocated_tiles().count(), 1);
    let eraser = Brush {
        eraser: true,
        size: 32.0,
        ..paint
    };
    doc.dab(&eraser, Color::rgb(0, 0, 0), 16.0, 16.0);
    assert!(
        doc.layers[0]
            .pixels
            .allocated_tiles()
            .all(|(_, _, t)| t.iter().all(|&b| b == 0)),
        "erased coloured pixels are canonical zero, not [200, 40, 40, 0]"
    );
    doc.save_project(&path).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size < 64 * 1024, "fully erased 8K project is {size} bytes");
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        back.layers[0].pixels.alloc_bytes(),
        0,
        "the erased coloured tile allocates nothing on reload"
    );
}

#[test]
fn v12_roundtrip_keeps_masks_and_groups() {
    let mut doc = Document::new(300, 300);
    doc.add_mask(0);
    let last = 299 * 300 + 299;
    doc.layers[0].mask.as_mut().unwrap().set(last, 0);
    doc.layers[0].pixels.set4(0, [10, 20, 30, 255]);
    doc.layers[0].pixels.set4(last * 4, [40, 50, 60, 255]);
    let g = doc.new_group("G".to_string());
    doc.set_group_opacity(g, 0.5);
    doc.layers[0].group = Some(g);
    doc.add_layer("empty".to_string());
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-tiles.pcraft");
    doc.save_project(&path).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers.len(), 2);
    assert_eq!(back.layers[0].pixels.get4(0), [10, 20, 30, 255]);
    assert_eq!(back.layers[0].pixels.get4(last * 4), [40, 50, 60, 255]);
    // Two painted corner tiles, not the whole 2x2 grid.
    assert_eq!(back.layers[0].pixels.alloc_bytes(), 2 * 256 * 1024);
    assert_eq!(back.layers[1].pixels.alloc_bytes(), 0);
    assert_eq!(back.layers[0].mask.as_ref().unwrap().get(last), 0);
    assert_eq!(back.layers[0].group, Some(g));
    assert_eq!(back.group_opacity(0), 0.5);
    assert_eq!(back.flatten(), doc.flatten());
}

#[test]
fn v12_roundtrip_keeps_empty_and_nested_groups() {
    let mut doc = Document::new(16, 16);
    let outer = doc.new_group("Outer".to_string());
    doc.move_layer_into_group(0, Some(outer));
    let inner = doc.group_layer(0, "Inner".to_string()).unwrap();
    let empty = doc.new_group("Empty".to_string());
    doc.set_group_collapsed(empty, true);
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-nested-groups.pcraft");
    doc.save_project(&path).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.groups.len(), 3);
    let outer_back = back.group(outer).expect("outer group survives");
    assert_eq!(outer_back.name, "Outer");
    assert_eq!(outer_back.parent, None);
    let inner_back = back.group(inner).expect("nested group survives");
    assert_eq!(inner_back.name, "Inner");
    assert_eq!(inner_back.parent, Some(outer));
    assert_eq!(back.layers[0].group, Some(inner));
    let empty_back = back.group(empty).expect("empty group survives");
    assert_eq!(empty_back.name, "Empty");
    assert_eq!(empty_back.parent, None);
    assert!(empty_back.collapsed);
    assert_eq!(back.flatten(), doc.flatten());
}

#[test]
fn v11_file_resaves_as_v12_with_identical_flatten() {
    // Hand-build a valid version-11 record (dense pixels, mask, text
    // tail) so the resave exercises the dense-to-tile split.
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v11-to-v12.pcraft");
    let resave = dir.join("procraft-v11-to-v12-resaved.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT11");
    buf.extend_from_slice(&4u32.to_le_bytes());
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Symmetry off.
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&[0]); // No mirror center.
    buf.extend_from_slice(&0u32.to_le_bytes()); // No groups.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Blend: Normal.
    buf.extend_from_slice(&[0]); // Clip: off.
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&[0]); // No group.
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[7u8; 4 * 2 * 4]).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&[1]); // Has a mask.
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[200u8; 4 * 2]).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&1u32.to_le_bytes()); // Frame hold.
    buf.extend_from_slice(&[1]); // Has text.
    buf.extend_from_slice(&4.0f32.to_le_bytes());
    buf.extend_from_slice(&3.0f32.to_le_bytes());
    buf.extend_from_slice(&12.0f32.to_le_bytes());
    buf.extend_from_slice(&[0, 0, 0, 255]);
    buf.extend_from_slice(&10u32.to_le_bytes());
    buf.extend_from_slice(&10u32.to_le_bytes());
    buf.extend_from_slice(&0.0f32.to_le_bytes());
    buf.extend_from_slice(&0.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]); // No outline.
    buf.extend_from_slice(&[0]); // No font.
    let t = b"hi";
    buf.extend_from_slice(&(t.len() as u32).to_le_bytes());
    buf.extend_from_slice(t);
    let head = buf.clone();
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let flat = back.flatten();
    back.save_project(&resave).unwrap();
    let raw = std::fs::read(&resave).unwrap();
    assert_eq!(&raw[..8], b"PCRAFT12");
    assert_eq!(&head[..8], b"PCRAFT11", "the crafted input stays v11");
    let again = Document::open_project(&resave).unwrap();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&resave);
    assert_eq!(again.flatten(), flat);
    assert_eq!(again.layers[0].pixels, vec![7u8; 4 * 2 * 4]);
    assert_eq!(again.layers[0].mask.as_ref().unwrap().get(3), 200);
    assert!(again.layers[0].is_text());
    assert_eq!(again.layers[0].text.as_ref().unwrap().text, "hi");
}

#[test]
fn v12_tile_table_rejects_corrupt_records() {
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v12-corrupt.pcraft");
    let mut raw = vec![0u8; 256 * 256 * 4];
    raw[..4].copy_from_slice(&[1, 2, 3, 255]);
    let base = v12_fixture(0, 0, &raw);
    let check = |bad: &[u8]| -> String {
        std::fs::write(&path, bad).unwrap();
        let err = Document::open_project(&path).unwrap_err();
        assert!(!err.is_empty(), "empty error message");
        err
    };
    // Absurd tile count.
    let mut bad = base.buf.clone();
    bad[base.count_at..base.count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(check(&bad).contains("tile count"));
    // Tile outside the 1x1 grid.
    let mut bad = base.buf.clone();
    bad[base.tx_at..base.tx_at + 2].copy_from_slice(&1u16.to_le_bytes());
    assert!(check(&bad).contains("tile"));
    // Absurd payload length.
    let mut bad = base.buf.clone();
    bad[base.len_at..base.len_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    check(&bad);
    // Corrupt payload bytes (zlib checksum catches them).
    let mut bad = base.buf.clone();
    for b in bad[base.payload_at..base.payload_end].iter_mut() {
        *b ^= 0xFF;
    }
    check(&bad);
    // A payload shorter than the full tile must not be padded silently.
    let short = v12_fixture(0, 0, &[1, 2, 3, 255]);
    assert!(check(&short.buf).contains("corrupt tile"));
    // Truncations at every field boundary fail closed.
    for n in [
        base.count_at,
        base.count_at + 2,
        base.tx_at,
        base.tx_at + 2,
        base.len_at,
        base.len_at + 2,
        base.payload_at,
        base.payload_at + 1,
        base.payload_end,
    ] {
        check(&base.buf[..n]);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn v11_load_rejects_hostile_text_records() {
    use crate::text::{MAX_FONT_BYTES, MAX_TEXT_BYTES, MAX_TEXT_CHARS, MAX_TEXT_LINES};
    use std::io::Write;
    let build = |font: Option<(u32, Vec<u8>)>, x: f32, text: &str| -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PCRAFT11");
        buf.extend_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&[255, 255, 255, 255]);
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Symmetry off.
        buf.extend_from_slice(&[6]);
        buf.extend_from_slice(&[0]); // No mirror center.
        buf.extend_from_slice(&0u32.to_le_bytes()); // No groups.
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(b"L");
        buf.extend_from_slice(&[1]);
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]); // Blend: Normal.
        buf.extend_from_slice(&[0]); // Clip: off.
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&[0]); // No group.
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&[9u8; 2 * 4]).unwrap();
        let packed = enc.finish().unwrap();
        buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        buf.extend_from_slice(&packed);
        buf.extend_from_slice(&[0]); // No mask.
        buf.extend_from_slice(&1u32.to_le_bytes()); // Frame hold.
                                                    // v11 text record.
        buf.extend_from_slice(&[1]); // Has text.
        buf.extend_from_slice(&x.to_le_bytes());
        buf.extend_from_slice(&8.0f32.to_le_bytes());
        buf.extend_from_slice(&24.0f32.to_le_bytes());
        buf.extend_from_slice(&[0, 0, 0, 255]);
        buf.extend_from_slice(&5u32.to_le_bytes());
        buf.extend_from_slice(&7u32.to_le_bytes());
        buf.extend_from_slice(&0.0f32.to_le_bytes());
        buf.extend_from_slice(&0.0f32.to_le_bytes());
        buf.extend_from_slice(&[0]); // No outline.
        match &font {
            Some((len, bytes)) => {
                buf.extend_from_slice(&[1]);
                buf.extend_from_slice(&len.to_le_bytes());
                buf.extend_from_slice(bytes);
            }
            None => buf.extend_from_slice(&[0]),
        }
        let t = text.as_bytes();
        buf.extend_from_slice(&(t.len() as u32).to_le_bytes());
        buf.extend_from_slice(t);
        buf
    };
    let dir = std::env::temp_dir();
    // Font declared over the 2 MiB cap.
    let path = dir.join("procraft-v11-hostile-font.pcraft");
    std::fs::write(
        &path,
        build(Some((MAX_FONT_BYTES as u32 + 1, Vec::new())), 4.0, "hi"),
    )
    .unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
    // Non-finite anchor.
    let path = dir.join("procraft-v11-hostile-nan.pcraft");
    std::fs::write(&path, build(None, f32::NAN, "hi")).unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
    // Text over the 16 KiB cap.
    let path = dir.join("procraft-v11-hostile-text.pcraft");
    std::fs::write(&path, build(None, 4.0, &"x".repeat(MAX_TEXT_BYTES + 1))).unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
    // Within the byte cap but past the editor's char cap.
    let path = dir.join("procraft-v11-hostile-chars.pcraft");
    std::fs::write(&path, build(None, 4.0, &"x".repeat(MAX_TEXT_CHARS + 1))).unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
    // Within both byte and char caps but past the line cap.
    let path = dir.join("procraft-v11-hostile-lines.pcraft");
    std::fs::write(&path, build(None, 4.0, &"x\n".repeat(MAX_TEXT_LINES + 1))).unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
    // At the line cap with a trailing newline: split('\n') — the
    // layout's own count — sees one segment more than lines() does,
    // so the editor would refuse this file without the load check.
    let path = dir.join("procraft-v11-hostile-trailnl.pcraft");
    std::fs::write(&path, build(None, 4.0, &"x\n".repeat(MAX_TEXT_LINES))).unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
    // Structurally valid record but unparseable font bytes.
    let path = dir.join("procraft-v11-hostile-badfont.pcraft");
    std::fs::write(&path, build(Some((100, vec![0xAA; 100])), 4.0, "hi")).unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn jpeg_export_decodes_at_canvas_size() {
    let mut doc = Document::new(16, 12);
    // Solid red: lossy error stays one-sided and easy to bound.
    let px = &mut doc.layers[0].pixels;
    for i in 0..px.len() / 4 {
        px.set4(i * 4, [255, 0, 0, 255]);
    }
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-m28.jpg");
    doc.save_jpeg(&path, 90).unwrap();
    let raw = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(&raw[..2], &[0xFF, 0xD8]); // JPEG SOI marker
    let img = image::load_from_memory(&raw).unwrap().to_rgb8();
    assert_eq!((img.width(), img.height()), (16, 12));
    let p = img.get_pixel(8, 6);
    assert!(p[0] > 200 && p[1] < 60 && p[2] < 60);
}

#[test]
fn jpeg_rejects_bad_quality() {
    let doc = Document::new(4, 4);
    let dir = std::env::temp_dir();
    assert!(doc.save_jpeg(&dir.join("procraft-m28-q0.jpg"), 0).is_err());
    assert!(doc
        .save_jpeg(&dir.join("procraft-m28-q101.jpg"), 101)
        .is_err());
}

#[test]
fn tiff_export_roundtrips_lossless() {
    let mut doc = Document::new(8, 6);
    let px = &mut doc.layers[0].pixels;
    for i in 0..px.len() / 4 {
        px.set4(i * 4, [(i % 251) as u8, 17, 200, 255]);
    }
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-m28.tiff");
    doc.save_tiff(&path).unwrap();
    let raw = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(raw.starts_with(b"II*\0") || raw.starts_with(b"MM\0*"));
    let img = image::load_from_memory(&raw).unwrap().to_rgba8();
    assert_eq!((img.width(), img.height()), (8, 6));
    assert_eq!(img.as_raw(), &doc.flatten());
}

#[test]
fn crop_keeps_content_and_rebases_offsets() {
    // 6x4 with a red block in buffer x=0..=1, layer shifted ox=2 so it
    // shows at canvas x=2..=3. Crop x=1..=4: the block lands at x=1..=2
    // and the layer re-bases to ox=0 (buffer == canvas after a crop).
    let mut doc = Document::new(6, 4);
    let px = &mut doc.layers[0].pixels;
    for y in 0..4u32 {
        for x in 0..2u32 {
            let o = ((y * 6 + x) * 4) as usize;
            px.set(o, 255);
            px.set(o + 3, 255);
        }
    }
    doc.layers[0].ox = 2;
    assert_eq!(&doc.flatten()[8..12], &[255, 0, 0, 255]);
    doc.selection = Some((0, 0, 0, 0));
    doc.crop(1, 0, 4, 3).unwrap();
    assert_eq!((doc.w, doc.h), (4, 4));
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
    assert_eq!(&flat[4..8], &[255, 0, 0, 255]);
    assert_eq!(&flat[8..12], &[255, 0, 0, 255]);
    assert_eq!(doc.layers[0].ox, 0);
    // The corner selection shifted fully outside, so it drops.
    assert!(doc.selection.is_none());
}

#[test]
fn rehome_pad_and_trim_carry_content() {
    let mut doc = Document::new(4, 4);
    let o = (5 * 4) as usize;
    let px = &mut doc.layers[0].pixels;
    px.set(o + 2, 255);
    px.set(o + 3, 255);
    doc.selection = Some((1, 1, 2, 2));
    doc.set_symmetry_center(1.0, 1.0);
    // Pad to 8x8: old canvas sits at (2, 2).
    doc.rehome_canvas(8, 8, 2, 2).unwrap();
    assert_eq!((doc.w, doc.h), (8, 8));
    let flat = doc.flatten();
    assert_eq!(&flat[((3 * 8 + 3) * 4) as usize..][..4], &[0, 0, 255, 255]);
    assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
    // A re-home re-bases every layer: buffer == canvas, ox/oy back to 0.
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    assert_eq!(doc.selection, Some((3, 3, 4, 4)));
    assert_eq!(doc.symmetry_center_px(), (3.0, 3.0));
    // Trim back: content returns home.
    doc.rehome_canvas(4, 4, -2, -2).unwrap();
    let flat = doc.flatten();
    assert_eq!(&flat[(5 * 4)..][..4], &[0, 0, 255, 255]);
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    assert_eq!(doc.selection, Some((1, 1, 2, 2)));
}

#[test]
fn crop_leading_trim_keeps_trailing_corner_content() {
    // A leading-edge trim used to blank the rightmost x0 cols / bottom
    // y0 rows: the carry kept buffer cells [0,new) but the new canvas
    // reads cells shifted by x0. The bottom-right corner must ride along.
    let mut doc = Document::new(6, 4);
    let at = |x: u32, y: u32| ((y * 6 + x) * 4) as usize;
    doc.layers[0].pixels.set4(at(5, 3), [255, 0, 0, 255]);
    doc.layers[0].pixels.set4(at(4, 2), [0, 255, 0, 255]);
    doc.crop(1, 1, 5, 3).unwrap();
    assert_eq!((doc.w, doc.h), (5, 3));
    let flat = doc.flatten();
    let n_at = |x: u32, y: u32| ((y * 5 + x) * 4) as usize;
    // Old (5,3) lands at (4,2); old (4,2) at (3,1); (0,0) is page.
    assert_eq!(&flat[n_at(4, 2)..n_at(4, 2) + 4], &[255, 0, 0, 255]);
    assert_eq!(&flat[n_at(3, 1)..n_at(3, 1) + 4], &[0, 255, 0, 255]);
    assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
}

#[test]
fn resize_center_shrink_keeps_middle_content() {
    // Center-anchor shrink 8->6 trims one column per side (dx=-1): old
    // x=6 must land at new x=5, and old x=7 (trimmed) must be gone.
    let mut doc = Document::new(8, 4);
    let at = |x: u32, y: u32| ((y * 8 + x) * 4) as usize;
    doc.layers[0].pixels.set4(at(6, 1), [0, 255, 0, 255]);
    doc.layers[0].pixels.set4(at(7, 1), [0, 0, 255, 255]);
    doc.rehome_canvas(6, 4, -1, 0).unwrap();
    let flat = doc.flatten();
    let n_at = |x: u32, y: u32| ((y * 6 + x) * 4) as usize;
    assert_eq!(&flat[n_at(5, 1)..n_at(5, 1) + 4], &[0, 255, 0, 255]);
    assert!(flat
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| p != &[0, 0, 255, 255]));
}

#[test]
fn crop_with_offset_layer_rebases_to_zero() {
    // Layer shifted ox=1: buffer cell 1 shows at canvas 2, cell 4 at
    // canvas 5, cell 0 at canvas 1 (trimmed away). Crop x=2..=5 -> 4x4:
    // old canvas shifts by dx=-2 and the layer re-bases to ox=0.
    let mut doc = Document::new(6, 4);
    let at = |x: u32, y: u32| ((y * 6 + x) * 4) as usize;
    doc.layers[0].pixels.set4(at(0, 0), [255, 0, 0, 255]);
    doc.layers[0].pixels.set4(at(1, 0), [0, 255, 0, 255]);
    doc.layers[0].pixels.set4(at(4, 0), [0, 0, 255, 255]);
    doc.layers[0].ox = 1;
    doc.crop(2, 0, 5, 3).unwrap();
    assert_eq!((doc.w, doc.h), (4, 4));
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[0, 255, 0, 255]);
    assert_eq!(&flat[12..16], &[0, 0, 255, 255]);
    assert!(flat
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| p != &[255, 0, 0, 255]));
}

#[test]
fn canvas_ops_reject_bad_sizes_and_clear_history() {
    let mut doc = Document::new(6, 4);
    doc.checkpoint();
    assert!(doc.undo());
    doc.checkpoint();
    doc.rehome_canvas(8, 8, 0, 0).unwrap();
    // Old snapshots mismatch the new buffers, so history is gone.
    assert!(!doc.undo());
    assert!(!doc.redo());
    assert!(doc.crop(5, 0, 9, 3).is_err());
    assert!(doc.crop(3, 0, 1, 3).is_err());
    assert!(doc.rehome_canvas(0, 8, 0, 0).is_err());
    assert!(doc.rehome_canvas(8193, 8, 0, 0).is_err());
    // Failed ops touch nothing.
    assert_eq!((doc.w, doc.h), (8, 8));
}

#[test]
fn canvas_cap_allows_8k_but_not_more() {
    assert_eq!(MAX_CANVAS_SIDE, 8192);
    // Grow a small document to the cap: a same-size rehome
    // short-circuits, so growth is the real boundary.
    let mut doc = Document::new(64, 64);
    doc.rehome_canvas(8192, 8192, 0, 0).unwrap();
    assert_eq!((doc.w, doc.h), (8192, 8192));
    // Area outside the old 64x64 content reads the page background.
    assert_eq!(doc.sample(8000, 8000), Some([255, 255, 255]));
    let err = Document::new(64, 64)
        .rehome_canvas(8193, 64, 0, 0)
        .unwrap_err();
    assert!(err.contains("want 1-8192 each"), "{err}");
}

#[test]
fn v7_project_opens_groupless() {
    // Hand-build a minimal version-7 file (center flag, no groups).
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v7-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT07");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0]); // Symmetry off.
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&[0]); // No mirror center.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[9u8; 2 * 4]).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&[0]); // No mask.
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(back.groups.is_empty());
    assert!(back.layers[0].group.is_none());
    assert!(back.is_layer_shown(0));
    assert_eq!(back.layers[0].pixels, vec![9u8; 8]);
}

#[test]
fn undo_keeps_count_cap_on_small_canvas() {
    let mut doc = Document::new(16, 16);
    for _ in 0..40 {
        doc.checkpoint();
    }
    assert_eq!(doc.undo.len(), 32);
    assert!(doc.undo_bytes <= MAX_UNDO_BYTES);
}

#[test]
fn undo_byte_budget_binds_on_large_canvas() {
    // 2048x2048 RGBA of allocated tiles = 16 MiB per step: the 256 MiB
    // budget caps at 16, well under the 32-count backstop.
    let mut doc = Document::new(2048, 2048);
    // Touch every tile first: a blank layer holds no tiles now, so only
    // real storage can make the byte budget bind.
    doc.layers[0].pixels.fill(255);
    for _ in 0..40 {
        doc.checkpoint();
    }
    assert_eq!(doc.undo.len(), 16);
    assert!(doc.undo_bytes <= MAX_UNDO_BYTES);
    // The newest step is still undoable after budget eviction.
    assert!(doc.undo());
}

#[test]
fn new_blend_modes_do_the_math() {
    use BlendMode::*;
    let b = BlendMode::Normal;
    assert_eq!(b.apply(10.0, 20.0), 10.0);
    assert_eq!(Darken.apply(10.0, 20.0), 10.0);
    assert_eq!(Lighten.apply(10.0, 20.0), 20.0);
    assert_eq!(Difference.apply(10.0, 20.0), 10.0);
    assert_eq!(Difference.apply(200.0, 50.0), 150.0);
    // Exclusion of identical mid-grays dips below either input.
    let e = Exclusion.apply(128.0, 128.0);
    assert!(e < 128.0 && e > 100.0, "{e}");
    // Soft light keeps mid-gray near itself, hard light pushes contrast.
    let s = SoftLight.apply(128.0, 100.0);
    assert!((s - 100.0).abs() < 12.0, "{s}");
    assert_eq!(HardLight.apply(0.0, 200.0), 0.0);
    assert_eq!(HardLight.apply(255.0, 200.0), 255.0);
    assert_eq!(BlendMode::ALL.len(), 10);
}

#[test]
fn mask_multiplies_layer_alpha() {
    let mut doc = Document::new(4, 1);
    doc.background = [255, 255, 255, 255];
    doc.layers[0].pixels = px_rgba(
        &doc,
        &(vec![
            255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
        ]),
    );
    doc.add_mask(0);
    // Conceal the left half.
    if let Some(m) = doc.layers[0].mask.as_mut() {
        m.set(0, 0);
        m.set(1, 0);
    }
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
    assert_eq!(&flat[8..12], &[255, 0, 0, 255]);
    // Clearing the mask reveals everything again.
    doc.clear_mask(0);
    assert!(doc.layers[0].mask.is_none());
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[255, 0, 0, 255]);
}

#[test]
fn clip_confines_layer_to_content_below() {
    let mut doc = Document::new(4, 1);
    doc.background = [255, 255, 255, 255];
    // Base: single red dot at x=1.
    doc.layers[0].pixels.set(7, 255);
    doc.layers[0].pixels.set(4, 255);
    doc.add_layer("top".to_string());
    doc.layers[1].pixels = px_rgba(&doc, &([0, 255, 0, 255].repeat(4)));
    doc.layers[1].clip = true;
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[255, 255, 255, 255]);
    assert_eq!(&flat[4..8], &[0, 255, 0, 255]);
    assert_eq!(&flat[8..12], &[255, 255, 255, 255]);
    // Unclipped, the green covers everything.
    doc.layers[1].clip = false;
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[0, 255, 0, 255]);
}

#[test]
fn mask_painting_uses_luminance_and_undoes() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(16, 16);
    doc.paint_target = PaintTarget::Mask;
    let brush = Brush {
        size: 8.0,
        softness: 0.0,
        ..Default::default()
    };
    assert!(doc.layers[0].mask.is_none());
    doc.checkpoint();
    doc.dab(&brush, Color::rgb(0, 0, 0), 8.0, 8.0);
    // Black paint concealed the middle; mask auto-materialized.
    let m = doc.layers[0].mask.as_ref().expect("mask created");
    assert!(m.to_dense().iter().any(|&b| b < 255));
    // Eraser reveals back toward white.
    let before: Vec<u8> = m.to_dense();
    let eraser = Brush {
        eraser: true,
        ..brush
    };
    doc.dab(&eraser, Color::rgb(0, 0, 0), 8.0, 8.0);
    let after = doc.layers[0].mask.as_ref().unwrap().to_dense();
    assert!(after.iter().zip(before.iter()).all(|(&a, &b)| a >= b));
    assert!(after.iter().any(|&b| b > 0));
    // Undo restores the pre-stroke state (no mask at all).
    assert!(doc.undo());
    assert!(doc.layers[0].mask.is_none());
    assert!(doc.redo());
    assert!(doc.layers[0].mask.is_some());
}

#[test]
fn v6_roundtrip_keeps_masks_clips_and_blends() {
    let mut doc = Document::new(4, 2);
    doc.layers[0].blend = BlendMode::Difference;
    doc.layers[0].pixels = px_rgba(&doc, &[200; 4 * 2 * 4]);
    doc.add_mask(0);
    doc.layers[0].mask.as_mut().unwrap().set(3, 0);
    doc.add_layer("clipped".to_string());
    doc.layers[1].blend = BlendMode::SoftLight;
    doc.layers[1].clip = true;
    doc.layers[1].pixels = px_rgba(&doc, &[7; 4 * 2 * 4]);
    doc.layers[1].ox = -3;
    doc.layers[1].oy = 2;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v6.pcraft");
    doc.save_project(&path).unwrap();
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[..8], b"PCRAFT12");
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].blend, BlendMode::Difference);
    assert_eq!(back.layers[0].mask.as_ref().unwrap().get(3), 0);
    assert_eq!(back.layers[1].blend, BlendMode::SoftLight);
    assert!(back.layers[1].clip);
    assert_eq!(back.layers[1].pixels, vec![7; 4 * 2 * 4]);
    assert_eq!((back.layers[1].ox, back.layers[1].oy), (-3, 2));
}

#[test]
fn v5_files_open_with_zero_offsets() {
    // Minimal v5 file: like v4 plus clip byte, no ox/oy fields.
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v5-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT05");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]); // blend: Normal
    buf.extend_from_slice(&[0]); // clip: off
    let raw = vec![9u8; 2 * 4];
    let packed = {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap()
    };
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&[0]); // mask flag: none
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].pixels, raw);
    assert_eq!((back.layers[0].ox, back.layers[0].oy), (0, 0));
}

#[test]
fn v4_files_open_without_masks_or_clips() {
    // Minimal v4 file: zlib pixels, blend byte, no clip/mask fields.
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v4-nomask.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT04");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    let raw = vec![9u8; 2 * 4];
    let packed = {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap()
    };
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].pixels, raw);
    assert!(back.layers[0].mask.is_none());
    assert!(!back.layers[0].clip);
}

#[test]
fn merge_bakes_mask_and_clip() {
    let mut doc = Document::new(2, 1);
    // Below: blue, left pixel only.
    doc.layers[0].pixels = px_rgba(&doc, &[0, 0, 255, 255, 0, 0, 0, 0]);
    doc.add_layer("top".to_string());
    doc.layers[1].pixels = px_rgba(&doc, &[255, 0, 0, 255, 255, 0, 0, 255]);
    doc.add_mask(1);
    // Mask hides the right pixel of the top layer.
    doc.layers[1].mask.as_mut().unwrap().set(1, 0);
    doc.layers[1].clip = true;
    assert!(doc.merge_down(1).unwrap());
    // Left: red over blue = red. Right: top hidden by mask AND outside
    // the blue content, so it stays transparent.
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    assert_eq!(doc.layers[0].pixels.get4(4), [0, 0, 0, 0]);
}

#[test]
fn transforms_carry_the_mask() {
    let mut doc = Document::new(4, 1);
    doc.add_mask(0);
    doc.layers[0].mask.as_mut().unwrap().set(0, 0);
    doc.translate_layer(0, 1, 0);
    // With offsets, mask data doesn't move; offset changes instead.
    assert_eq!(doc.layers[0].ox, 1);
    assert_eq!(doc.layers[0].mask.as_ref().unwrap().get(0), 0);
    doc.flip_layer_horizontal(0).unwrap();
    // 4-pixel wide: horizontal flip mirrors mask indices 0<->3, 1<->2.
    assert_eq!(doc.layers[0].mask.as_ref().unwrap().get(3), 0);
    assert_eq!(doc.layers[0].ox, -1);
    // Affine keeps dimensions and mask presence.
    doc.affine_layer(0, 1.0, 90.0).unwrap();
    assert!(doc.layers[0].mask.is_some());
    assert_eq!(doc.layers[0].mask.as_ref().unwrap().len(), 4);
}

#[test]
fn new_stroke_clears_redo() {
    let mut doc = Document::new(1, 1);
    doc.checkpoint();
    doc.layers[0].pixels = px_rgba(&doc, &[1, 1, 1, 1]);
    assert!(doc.undo());
    doc.checkpoint();
    doc.layers[0].pixels = px_rgba(&doc, &[2, 2, 2, 2]);
    assert!(!doc.redo());
}

#[test]
fn multiply_blends_channels() {
    let mut doc = Document::new(1, 1);
    doc.background = [255, 255, 255, 255];
    doc.layers[0].pixels = px_rgba(&doc, &[128, 200, 64, 255]);
    doc.layers[0].blend = BlendMode::Multiply;
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[128, 200, 64, 255]);
    // Screen over black returns the source unchanged.
    doc.background = [0, 0, 0, 255];
    doc.layers[0].blend = BlendMode::Screen;
    let flat = doc.flatten();
    assert_eq!(&flat[0..4], &[128, 200, 64, 255]);
    // Screen over mid-gray lightens past the source.
    doc.background = [128, 128, 128, 255];
    let flat = doc.flatten();
    assert!(flat[0] > 128 && flat[1] > 200 && flat[2] > 64, "{flat:?}");
}

#[test]
fn project_roundtrip_keeps_everything() {
    let mut doc = Document::new(4, 3);
    doc.background = [10, 20, 30, 255];
    doc.layers[0].pixels = px_rgba(&doc, &[7; 4 * 3 * 4]);
    doc.layers[0].opacity = 0.5;
    doc.layers[0].blend = BlendMode::Overlay;
    doc.symmetry = SymmetryMode::Both;
    doc.set_symmetry_center(1.5, 1.0);
    doc.add_layer("second".to_string());
    doc.layers[1].visible = false;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-roundtrip.pcraft");
    doc.save_project(&path).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!((back.w, back.h), (4, 3));
    assert_eq!(back.background, [10, 20, 30, 255]);
    assert_eq!(back.layers.len(), 2);
    assert_eq!(back.layers[0].opacity, 0.5);
    assert_eq!(back.layers[0].blend, BlendMode::Overlay);
    assert_eq!(back.layers[0].pixels, vec![7; 4 * 3 * 4]);
    assert_eq!(back.layers[1].name, "second");
    assert!(!back.layers[1].visible);
    assert_eq!(back.active, 1);
    assert_eq!(back.symmetry, SymmetryMode::Both);
    assert_eq!(back.symmetry_center, Some((1.5, 1.0)));
    assert!(Document::open_project(&dir.join("missing.pcraft")).is_err());
}

#[test]
fn project_bytes_open_like_the_saved_file_and_a_clone_is_a_snapshot() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(300, 260);
    doc.layers[0].pixels = px_rgba(&doc, &[9; 4 * 300 * 260]);
    let bytes = doc.save_project_bytes().unwrap();
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-bytes.pcraft");
    std::fs::write(&path, &bytes).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.w, 300);
    assert_eq!(back.layers[0].pixels, vec![9; 4 * 300 * 260]);
    // Autosave encodes a clone on a worker thread: painting on the live
    // document must leave that snapshot's bytes untouched.
    let snapshot = doc.clone();
    doc.dab(&Brush::default(), Color::rgb(1, 2, 3), 10.0, 10.0);
    assert_eq!(snapshot.save_project_bytes().unwrap(), bytes);
    assert_ne!(doc.save_project_bytes().unwrap(), bytes);
}

#[test]
fn v1_project_still_opens_with_symmetry_off() {
    // Hand-build a minimal version-1 file (no symmetry byte).
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v1-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT01");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&[9u8; 2 * 4]);
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!((back.w, back.h), (2, 1));
    assert_eq!(back.symmetry, SymmetryMode::Off);
    assert_eq!(back.layers[0].pixels, vec![9u8; 8]);
}

#[test]
fn moved_mirror_center_shifts_paint() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(21, 21);
    doc.symmetry = SymmetryMode::Horizontal;
    // Default middle: x=2 mirrors to x=18.
    assert!(SymmetryMode::Horizontal
        .mirrored_pixels(21, 21, 6, 10.0, 10.0, 2, 5)
        .contains(&(18, 5)));
    // Moved center x=5: x=2 mirrors to x=8 instead.
    doc.set_symmetry_center(5.0, 10.0);
    assert_eq!(doc.symmetry_center_px(), (5.0, 10.0));
    let brush = Brush {
        size: 3.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color::rgb(255, 0, 0), 2.5, 5.5);
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 21 + x) * 4 + 3) as usize);
    assert!(at(&doc, 2, 5) > 0);
    assert!(at(&doc, 8, 5) > 0);
    assert_eq!(at(&doc, 18, 5), 0);
    // Reset restores the middle; out-of-range values clamp.
    doc.reset_symmetry_center();
    assert_eq!(doc.symmetry_center, None);
    doc.set_symmetry_center(1e9, -1e9);
    assert_eq!(doc.symmetry_center, Some((16384.0, -16384.0)));
}

#[test]
fn v6_project_opens_with_centered_symmetry() {
    // Hand-build a minimal version-6 file (no center bytes).
    use std::io::Write;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v6-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT06");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[1]); // Horizontal.
    buf.extend_from_slice(&[6]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&0i32.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes());
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[9u8; 2 * 4]).unwrap();
    let packed = enc.finish().unwrap();
    buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    buf.extend_from_slice(&packed);
    buf.extend_from_slice(&[0]); // No mask.
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.symmetry, SymmetryMode::Horizontal);
    assert_eq!(back.symmetry_center, None);
    assert_eq!(back.symmetry_center_px(), (0.5, 0.0));
    assert_eq!(back.layers[0].pixels, vec![9u8; 8]);
}

#[test]
fn flip_horizontal_mirrors_pixels() {
    let mut doc = Document::new(4, 1);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    doc.flip_layer_horizontal(0).unwrap();
    assert_eq!(doc.layers[0].pixels.get4(12), [255, 0, 0, 255]);
    assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 0, 0]);
    // Double flip restores.
    doc.flip_layer_horizontal(0).unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
}

#[test]
fn flip_vertical_mirrors_pixels() {
    let mut doc = Document::new(1, 4);
    doc.layers[0].pixels.set4(0, [0, 255, 0, 255]);
    doc.flip_layer_vertical(0).unwrap();
    assert_eq!(doc.layers[0].pixels.get4(12), [0, 255, 0, 255]);
    doc.flip_layer_vertical(0).unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [0, 255, 0, 255]);
}

#[test]
fn translate_shifts_pixels() {
    let mut doc = Document::new(4, 1);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    doc.translate_layer(0, 2, 0);
    // With offsets, pixels don't move; offset changes instead.
    assert_eq!(doc.layers[0].ox, 2);
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
}

#[test]
fn move_off_canvas_and_back_restores() {
    // The reported Move bug: content pushed fully off-canvas vanished
    // on return because translate cropped destructively. Offsets keep
    // every pixel, so the round trip is exact.
    let mut doc = Document::new(8, 4);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    doc.layers[0].pixels.set4(4, [0, 255, 0, 255]);
    let want = doc.layers[0].pixels.clone();
    // Paint shows at the origin.
    assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
    // Fully off-canvas: composite shows only the page, buffer intact.
    doc.translate_layer(0, 100, 0);
    assert!(doc
        .flatten()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| p == &[255, 255, 255, 255]));
    assert_eq!(doc.layers[0].pixels, want);
    // Back home: pixel-perfect, buffer untouched throughout.
    doc.translate_layer(0, -100, 0);
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    assert_eq!(doc.layers[0].pixels, want);
    assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
    assert_eq!(&doc.flatten()[4..8], &[0, 255, 0, 255]);
    // Partially off-canvas: visible part composites, rest survives.
    doc.translate_layer(0, -1, 0);
    assert_eq!(&doc.flatten()[0..4], &[0, 255, 0, 255]);
    doc.translate_layer(0, 1, 0);
    assert_eq!(&doc.flatten()[0..4], &[255, 0, 0, 255]);
}

#[test]
fn move_undo_redo_restores_offset() {
    let mut doc = Document::new(8, 8);
    doc.checkpoint();
    doc.translate_layer(0, 30, -12);
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (30, -12));
    assert!(doc.undo());
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    assert!(doc.redo());
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (30, -12));
}

#[test]
fn paint_after_move_lands_shifted() {
    // Brush input is canvas-space; storage follows the layer offset,
    // so paint appears under the cursor on moved layers.
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(32, 32);
    doc.translate_layer(0, 5, 0);
    let brush = Brush {
        size: 3.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color::rgb(255, 0, 0), 10.5, 10.5);
    // Composite shows paint at the canvas dab point...
    let at = |d: &Document, x: u32, y: u32| {
        let o = ((y * 32 + x) * 4) as usize;
        [
            d.flatten()[o],
            d.flatten()[o + 1],
            d.flatten()[o + 2],
            d.flatten()[o + 3],
        ]
    };
    assert_eq!(at(&doc, 10, 10), [255, 0, 0, 255]);
    // ...stored shifted in the buffer (local 5, 10).
    let o = ((10 * 32 + 5) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get4(o), [255, 0, 0, 255]);
    // A far-moved layer still paints the page: the buffer slides
    // home (only invisible tails trim) instead of refusing the dab.
    doc.translate_layer(0, 1000, 0);
    doc.dab(&brush, Color::rgb(0, 255, 0), 10.5, 10.5);
    assert_eq!(at(&doc, 10, 10), [0, 255, 0, 255]);
}

#[test]
fn merge_with_offsets_bakes_positions() {
    // dst blue at canvas 0; src red lives at buffer 0 but ox=1.
    let mut doc = Document::new(3, 1);
    doc.layers[0].pixels.set4(0, [0, 0, 255, 255]);
    doc.add_layer("top".to_string());
    doc.layers[1].pixels.set4(0, [255, 0, 0, 255]);
    doc.layers[1].ox = 1;
    assert!(doc.merge_down(1).unwrap());
    // Red baked at canvas 1 (buffer 1), blue kept at canvas 0.
    assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 255, 255]);
    assert_eq!(doc.layers[0].pixels.get4(4), [255, 0, 0, 255]);
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
}

#[test]
fn flip_mirrors_offset_about_center() {
    let mut doc = Document::new(8, 8);
    doc.layers[0].ox = 3;
    doc.layers[0].oy = -2;
    doc.flip_layer_horizontal(0).unwrap();
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (-3, -2));
    doc.flip_layer_vertical(0).unwrap();
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (-3, 2));
}

#[test]
fn mask_view_follows_offset() {
    let mut doc = Document::new(4, 1);
    doc.add_mask(0);
    doc.layers[0].mask.as_mut().unwrap().set(0, 200);
    // Unshifted: viewer shows the value at canvas 0.
    assert_eq!(&doc.mask_view().unwrap()[0..4], &[200, 200, 200, 255]);
    // Shifted right: canvas 0 reads concealed, canvas 1 shows it.
    doc.translate_layer(0, 1, 0);
    let view = doc.mask_view().unwrap();
    assert_eq!(&view[0..4], &[0, 0, 0, 255]);
    assert_eq!(&view[4..8], &[200, 200, 200, 255]);
}

#[test]
fn mask_view_region_matches_the_sub_rect_of_mask_view() {
    let mut doc = Document::new(300, 300);
    doc.add_mask(0);
    // Partially painted mask: every byte differs, some are zero.
    let pattern: Vec<u8> = (0..300u32 * 300)
        .map(|i| ((i * 7 + i / 300) % 256) as u8)
        .collect();
    doc.layers[0].mask.as_mut().unwrap().replace_dense(&pattern);
    // Non-zero offset: the left 251 columns and the bottom 37 rows hold
    // no layer content, so those region pixels must read concealed.
    doc.layers[0].ox = 251;
    doc.layers[0].oy = -37;
    let full = doc.mask_view().unwrap();
    for &(x0, y0, x1, y1) in &[
        (0u32, 0u32, 63u32, 47u32), // fully outside the layer
        (200, 0, 299, 60),          // straddles the left edge
        (251, 250, 299, 299),       // straddles the bottom edge
        (0, 0, 299, 299),           // the whole canvas
    ] {
        let region = doc.mask_view_region(x0, y0, x1, y1).unwrap();
        let rw = (x1 - x0 + 1) as usize;
        assert_eq!(region.len(), rw * (y1 - y0 + 1) as usize * 4);
        for y in y0..=y1 {
            let got = (y - y0) as usize * rw * 4;
            let want = (y as usize * 300 + x0 as usize) * 4;
            assert_eq!(
                &region[got..got + rw * 4],
                &full[want..want + rw * 4],
                "row {y} of ({x0}, {y0})-({x1}, {y1})"
            );
        }
    }
    assert!(
        doc.mask_view_region(0, 0, 63, 47)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 0, 0, 255]),
        "out-of-layer pixels read concealed"
    );
}

#[test]
fn bilinear_edges_keep_their_colour_over_transparency() {
    // One opaque red pixel beside transparent black. A straight-RGBA
    // blend would give dark red (128, 0, 0, 128) at the midpoint; the
    // premultiplied blend keeps the hue and only fades coverage.
    let mut px = vec![0u8; 2 * 2 * 4];
    px[0..4].copy_from_slice(&[255, 0, 0, 255]);
    let s = Document::sample_bilinear(&px, 2, 2, 0.5, 0.0);
    assert_eq!(&s[..3], &[255, 0, 0], "sample was {s:?}");
    assert!((s[3] as i32 - 128).abs() <= 1, "alpha was {}", s[3]);
    // An exact hit still returns the pixel bit-for-bit.
    assert_eq!(
        Document::sample_bilinear(&px, 2, 2, 0.0, 0.0),
        [255, 0, 0, 255]
    );
    // Transparent neighbourhoods stay fully transparent, never grey.
    assert_eq!(Document::sample_bilinear(&px, 2, 2, 1.0, 1.0), [0, 0, 0, 0]);
}

#[test]
fn affine_identity_keeps_pixels() {
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.fill(100);
    let before = doc.layers[0].pixels.clone();
    doc.affine_layer(0, 1.0, 0.0).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
    // 2x zoom of solid color stays solid.
    doc.affine_layer(0, 2.0, 0.0).unwrap();
    assert!(doc.layers[0]
        .pixels
        .to_dense()
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| p[0] == 100 && p[3] == 100));
}

fn opaque_bbox_center(doc: &Document) -> (f32, f32) {
    let (w, h) = (doc.w, doc.h);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if doc.layers[0].pixels.get(((y * w + x) * 4 + 3) as usize) != 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    ((x0 + x1 + 1) as f32 * 0.5, (y0 + y1 + 1) as f32 * 0.5)
}

#[test]
fn affine_spins_and_grows_about_paint_center() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(64, 64);
    let brush = Brush {
        size: 30.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    // Off-center hard rect: the old canvas-center pivot would orbit
    // it ~6px per 15° click; rotating in place only turns corners.
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 12, 12, 19, 19);
    let before = opaque_bbox_center(&doc);
    assert!((before.0 - 16.0).abs() < 1.0 && (before.1 - 16.0).abs() < 1.0);
    let painted = doc.layers[0].pixels.clone();
    doc.affine_layer(0, 1.0, 15.0).unwrap();
    // Rotation applied (corners moved) but the paint stayed put.
    assert_ne!(doc.layers[0].pixels, painted);
    let after = opaque_bbox_center(&doc);
    assert!(
        (after.0 - before.0).abs() <= 2.0 && (after.1 - before.1).abs() <= 2.0,
        "before={before:?} after={after:?}"
    );
    // Scale grows about the same center.
    doc.affine_layer(0, 1.25, 0.0).unwrap();
    let grown = opaque_bbox_center(&doc);
    assert!(
        (grown.0 - before.0).abs() <= 3.0 && (grown.1 - before.1).abs() <= 3.0,
        "before={before:?} grown={grown:?}"
    );
    // Offsets ride along untouched (no rounding damage).
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
}

#[test]
fn affine_quarter_turns_are_lossless() {
    // Pivot-stable block: distinct values per pixel catch any smear,
    // and the centered bbox keeps the pivot fixed across turns.
    let mut doc = Document::new(16, 16);
    for y in 4..12u32 {
        for x in 4..12u32 {
            let o = ((y * 16 + x) * 4) as usize;
            doc.layers[0].pixels.set(o, (x * 16 + y) as u8);
            doc.layers[0].pixels.set(o + 1, 200);
            doc.layers[0].pixels.set(o + 2, 100);
            doc.layers[0].pixels.set(o + 3, 255);
        }
    }
    let before = doc.layers[0].pixels.clone();
    let count: usize = before.to_dense().chunks(4).filter(|p| p[3] != 0).count();
    doc.affine_layer(0, 1.0, 90.0).unwrap();
    assert_eq!(
        doc.layers[0]
            .pixels
            .to_dense()
            .chunks(4)
            .filter(|p| p[3] != 0)
            .count(),
        count
    );
    // Four quarter-turns return every byte exactly.
    for _ in 0..3 {
        doc.affine_layer(0, 1.0, 90.0).unwrap();
    }
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn distort_identity_quad_is_noop() {
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.fill(100);
    let before = doc.layers[0].pixels.clone();
    doc.distort_layer(0, [(0.0, 0.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)])
        .unwrap();
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn distort_pin_moves_paint_and_mask() {
    // One opaque pixel at the top-left; pin that corner to the
    // canvas center. Dest (0,0) now samples the empty center, and
    // the mask travels with the paint.
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    let mut m = vec![0u8; 64];
    m[0] = 255;
    doc.layers[0].mask = Some(px_mask(&doc, &(m)));
    doc.distort_layer(0, [(4.0, 4.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)])
        .unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 0, 0]);
    assert_eq!(doc.layers[0].mask.as_ref().unwrap().get(0), 0);
    // Bad index is a silent no-op, never a panic.
    let before = doc.layers[0].pixels.clone();
    doc.distort_layer(9, [(0.0, 0.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)])
        .unwrap();
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn distort_degenerate_canvas_is_noop() {
    let mut doc = Document::new(1, 1);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    doc.distort_layer(0, [(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)])
        .unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
}

fn identity_lattice(w: f32, h: f32, cols: u32, rows: u32) -> Vec<(f32, f32)> {
    let mut grid = Vec::new();
    for j in 0..=rows {
        for i in 0..=cols {
            grid.push((i as f32 * w / cols as f32, j as f32 * h / rows as f32));
        }
    }
    grid
}

#[test]
fn warp_identity_lattice_is_noop() {
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.fill(100);
    let before = doc.layers[0].pixels.clone();
    let grid = identity_lattice(8.0, 8.0, 2, 2);
    doc.warp_layer(0, 2, 2, &grid).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn warp_pull_moves_paint_and_mask() {
    // One opaque pixel at the top-left; drag the lattice center to
    // the origin. Dest (4,4) sits exactly on lattice point (1,1),
    // so it samples the painted texel; the mask travels with it.
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    let mut m = vec![0u8; 64];
    m[0] = 255;
    doc.layers[0].mask = Some(px_mask(&doc, &(m)));
    let mut grid = identity_lattice(8.0, 8.0, 2, 2);
    // Row-major (row 1, col 1) of the 3-wide lattice.
    grid[4] = (0.0, 0.0);
    doc.warp_layer(0, 2, 2, &grid).unwrap();
    let o = ((4 * 8 + 4) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get4(o), [255, 0, 0, 255]);
    assert_eq!(
        doc.layers[0]
            .mask
            .as_ref()
            .unwrap()
            .get((4 * 8 + 4) as usize),
        255
    );
    // Bad grid (wrong point count) is a silent no-op.
    let before = doc.layers[0].pixels.clone();
    doc.warp_layer(0, 2, 2, &[(0.0, 0.0)]).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn warp_degenerate_canvas_is_noop() {
    let mut doc = Document::new(1, 1);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    doc.warp_layer(0, 1, 1, &[(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)])
        .unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
}

fn tri_pins(w: f32, h: f32, center: (f32, f32)) -> Vec<(f32, f32)> {
    vec![
        (0.0, 0.0),
        (w - 1.0, 0.0),
        (w - 1.0, h - 1.0),
        (0.0, h - 1.0),
        center,
    ]
}

const TRI_FAN: [[u32; 3]; 4] = [[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]];

#[test]
fn tri_identity_pins_are_noop() {
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.fill(100);
    let before = doc.layers[0].pixels.clone();
    let src = tri_pins(8.0, 8.0, (4.0, 4.0));
    doc.warp_layer_tris(0, &src, &src, &TRI_FAN).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn tri_pin_pull_moves_paint_and_mask() {
    // Paint sits exactly under the center pin; drag the pin to (2,2).
    // The destination point (2,2) is the moved pin itself, so it
    // samples the paint's original spot and the mask travels with it.
    let mut doc = Document::new(8, 8);
    let o = ((4 * 8 + 4) * 4) as usize;
    doc.layers[0].pixels.set4(o, [255, 0, 0, 255]);
    let mut m = vec![0u8; 64];
    m[(4 * 8 + 4) as usize] = 255;
    doc.layers[0].mask = Some(px_mask(&doc, &(m)));
    let src = tri_pins(8.0, 8.0, (4.0, 4.0));
    let dst = tri_pins(8.0, 8.0, (2.0, 2.0));
    doc.warp_layer_tris(0, &src, &dst, &TRI_FAN).unwrap();
    let d = ((2 * 8 + 2) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get4(d), [255, 0, 0, 255]);
    assert_eq!(
        doc.layers[0]
            .mask
            .as_ref()
            .unwrap()
            .get((2 * 8 + 2) as usize),
        255
    );
    assert_eq!(doc.layers[0].pixels.get(o + 3), 0);
}

#[test]
fn tri_corner_pins_stay_fixed_while_center_moves() {
    // The canvas corners are pins too: content near an unmoved
    // corner keeps sampling itself, so a center pull does not drag
    // the whole layer.
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    let src = tri_pins(8.0, 8.0, (4.0, 4.0));
    let dst = tri_pins(8.0, 8.0, (2.0, 2.0));
    doc.warp_layer_tris(0, &src, &dst, &TRI_FAN).unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
}

#[test]
fn tri_bad_input_is_noop() {
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.fill(100);
    let before = doc.layers[0].pixels.clone();
    let src = tri_pins(8.0, 8.0, (4.0, 4.0));
    doc.warp_layer_tris(9, &src, &src, &TRI_FAN).unwrap();
    doc.warp_layer_tris(0, &src, &src[..3], &TRI_FAN).unwrap();
    doc.warp_layer_tris(0, &src, &src, &[[0, 1, 99]]).unwrap();
    doc.warp_layer_tris(0, &src, &src, &[]).unwrap();
    let mut flat = Document::new(1, 1);
    flat.layers[0].pixels.set4(0, [255, 0, 0, 255]);
    flat.warp_layer_tris(0, &src, &src, &TRI_FAN).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
    assert_eq!(flat.layers[0].pixels.get4(0), [255, 0, 0, 255]);
}

#[test]
fn content_bounds_covers_the_painted_pixels() {
    let mut doc = Document::new(8, 8);
    assert_eq!(doc.content_bounds(0), None);
    doc.layers[0]
        .pixels
        .set(((3 * 8 + 2) * 4 + 3) as usize, 255);
    doc.layers[0]
        .pixels
        .set(((5 * 8 + 6) * 4 + 3) as usize, 255);
    assert_eq!(doc.content_bounds(0), Some((2.0, 3.0, 6.0, 5.0)));
    assert_eq!(doc.content_bounds(9), None);
}

/// A document with paint, a mask, an offset and a layer above it, so
/// every transform has real work and real buffers to allocate.
fn oom_doc() -> Document {
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.set4(0, [10, 20, 30, 255]);
    doc.layers[0]
        .pixels
        .set4(((7 * 8 + 7) * 4) as usize, [40, 50, 60, 255]);
    doc.add_mask(0);
    doc.layers[0].mask.as_mut().unwrap().set(0, 0);
    doc.layers[0].ox = 2;
    doc.layers[0].oy = 1;
    doc.add_layer("top".to_string());
    doc.layers[1].pixels.set4(0, [1, 2, 3, 255]);
    doc.active = 0;
    doc
}

type Snapshot = (
    Vec<Vec<u8>>,
    Vec<Option<Vec<u8>>>,
    Vec<(i32, i32)>,
    u32,
    u32,
    Option<(u32, u32, u32, u32)>,
    Option<Vec<u8>>,
);

fn snapshot(doc: &Document) -> Snapshot {
    (
        doc.layers.iter().map(|l| l.pixels.to_dense()).collect(),
        doc.layers
            .iter()
            .map(|l| l.mask.as_ref().map(|m| m.to_dense()))
            .collect(),
        doc.layers.iter().map(|l| (l.ox, l.oy)).collect(),
        doc.w,
        doc.h,
        doc.selection,
        doc.selection_mask.clone(),
    )
}

/// Make every fallible buffer allocation of `op` fail in turn: each
/// failure must come back as an error with the document untouched, so
/// a tablet that runs out of memory mid-gesture keeps its pixels.
fn assert_oom_is_atomic(op: impl FnMut(&mut Document) -> Result<(), String>) {
    assert_oom_is_atomic_with(oom_doc, op);
}

fn assert_oom_is_atomic_with(
    make: impl Fn() -> Document,
    mut op: impl FnMut(&mut Document) -> Result<(), String>,
) {
    use crate::raster::with_failed_alloc;
    let mut failures = 0;
    for nth in 0..12 {
        let mut doc = make();
        let before = snapshot(&doc);
        match with_failed_alloc(nth, || op(&mut doc)) {
            Err(e) => {
                assert!(e.contains("out of memory"), "nth {nth}: {e}");
                assert_eq!(snapshot(&doc), before, "nth {nth} left the doc changed");
                failures += 1;
            }
            Ok(()) => break,
        }
    }
    assert!(failures > 0, "the op took no fallible allocation");
}

#[test]
fn transform_allocation_failures_leave_the_document_untouched() {
    assert_oom_is_atomic(|d| d.affine_layer(0, 1.5, 30.0));
    assert_oom_is_atomic(|d| d.distort_layer(0, [(4.0, 4.0), (7.0, 0.0), (7.0, 7.0), (0.0, 7.0)]));
    let grid = identity_lattice(8.0, 8.0, 2, 2);
    assert_oom_is_atomic(|d| d.warp_layer(0, 2, 2, &grid));
    let src = tri_pins(8.0, 8.0, (4.0, 4.0));
    let dst = tri_pins(8.0, 8.0, (2.0, 2.0));
    assert_oom_is_atomic(|d| d.warp_layer_tris(0, &src, &dst, &TRI_FAN));
    assert_oom_is_atomic(|d| d.flip_layer_horizontal(0));
    assert_oom_is_atomic(|d| d.flip_layer_vertical(0));
    assert_oom_is_atomic(|d| d.merge_down(1).map(|_| ()));
    assert_oom_is_atomic(|d| d.rehome_canvas(6, 6, 1, 1));
}

#[test]
fn filter_allocation_failures_leave_the_document_untouched() {
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = 255 - i as u8;
    }
    assert_oom_is_atomic(|d| d.adjust_tone(0, 10, 1.5));
    assert_oom_is_atomic(|d| d.blur_layer(0, 2));
    assert_oom_is_atomic(|d| d.sharpen_layer(0, 0.5));
    assert_oom_is_atomic(|d| d.apply_lut(0, &lut));
    assert_oom_is_atomic(|d| d.adjust_hsl(0, 30.0, 1.5, 0.1));
    assert_oom_is_atomic(|d| d.color_balance(0, 10, -10, 5));
    assert_oom_is_atomic(|d| d.colorize(0, [200, 40, 30], 0.5));
}

#[test]
fn selection_allocation_failures_leave_the_selection_untouched() {
    use crate::brush::{Brush, Color};
    let selected = || {
        let mut doc = oom_doc();
        doc.selection_mask = Some(vec![255; (doc.w * doc.h) as usize]);
        doc
    };
    assert_oom_is_atomic_with(selected, |d| d.feather_selection(2).map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| d.grow_selection(1).map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| d.smooth_selection().map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| d.invert_selection().map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| d.translate_selection(1, 1).map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| d.flip_selection_h().map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| d.flip_selection_v().map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| d.select_opaque(0).map(|_| ()));
    assert_oom_is_atomic_with(selected, |d| {
        let pts = [(1.0, 1.0), (6.0, 1.0), (1.0, 6.0)];
        d.set_lasso(&pts).map(|_| ())
    });
    let brush = Brush {
        size: 1.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    assert_oom_is_atomic_with(selected, |d| {
        d.stroke_selection(&brush, Color::rgb(0, 255, 0), 1)
            .map(|_| ())
    });
}

#[test]
fn tone_brightness_contrast_and_alpha() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels = px_rgba(&doc, &[100, 100, 100, 200, 100, 100, 100, 200]);
    doc.adjust_tone(0, 10, 1.0).unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [110, 110, 110, 200]);
    // Zero contrast flattens to mid-gray; alpha still untouched.
    doc.adjust_tone(0, 0, 0.0).unwrap();
    assert_eq!(doc.layers[0].pixels.get4(0), [128, 128, 128, 200]);
    // Identity is a no-op.
    let before = doc.layers[0].pixels.clone();
    doc.adjust_tone(0, 0, 1.0).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn blur_spreads_dot_keeps_flat() {
    let mut doc = Document::new(8, 8);
    doc.layers[0].pixels.fill(77);
    let before = doc.layers[0].pixels.clone();
    doc.blur_layer(0, 3).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
    // Single opaque dot on transparency spreads out.
    let mut doc = Document::new(9, 9);
    doc.layers[0]
        .pixels
        .set(((4 * 9 + 4) * 4 + 3) as usize, 255);
    doc.layers[0].pixels.set(((4 * 9 + 4) * 4) as usize, 255);
    doc.blur_layer(0, 2).unwrap();
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 9 + x) * 4 + 3) as usize);
    assert!(at(&doc, 4, 4) < 255);
    assert!(at(&doc, 5, 4) > 0);
    assert_eq!(at(&doc, 0, 0), 0);
}

#[test]
fn sharpen_boosts_edge_contrast() {
    // Left half 100, right half 200.
    let mut doc = Document::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            let v = if x < 4 { 100u8 } else { 200u8 };
            let o = ((y * 8 + x) * 4) as usize;
            doc.layers[0].pixels.set(o, v);
            doc.layers[0].pixels.set(o + 1, v);
            doc.layers[0].pixels.set(o + 2, v);
            doc.layers[0].pixels.set(o + 3, 255);
        }
    }
    doc.sharpen_layer(0, 1.0).unwrap();
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 8 + x) * 4) as usize);
    assert!(at(&doc, 3, 4) < 100, "dark edge pixel dips");
    assert!(
        at(&doc, 4, 4) > 200 || at(&doc, 4, 4) == 255,
        "bright edge pixel rises"
    );
    // Flat area is untouched by sharpening.
    assert_eq!(at(&doc, 0, 0), 100);
    assert_eq!(at(&doc, 7, 7), 200);
}

#[test]
fn filters_respect_selection() {
    let mut doc = Document::new(6, 6);
    doc.layers[0].pixels.fill(255);
    doc.selection = Some((0, 0, 2, 2));
    doc.adjust_tone(0, -50, 1.0).unwrap();
    assert_eq!(doc.layers[0].pixels.get(0), 205);
    let o = ((5 * 6 + 5) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get(o), 255);
    doc.blur_layer(0, 2).unwrap();
    assert_eq!(doc.layers[0].pixels.get(o), 255);
    doc.clear_selection();
    assert!(doc.selection.is_none() && doc.selection_mask.is_none());
}

/// The clamped separable box blur straight from the definition: the
/// row-major `box_blur` must reproduce it byte for byte.
fn naive_box_blur(src: &[u8], w: u32, h: u32, r: i32) -> Vec<u8> {
    let (w, h) = (w as i32, h as i32);
    let wu = w as u32;
    let window = (2 * r + 1) as u32;
    let px = |buf: &[u8], x: i32, y: i32, ch: usize| -> u32 {
        let x = x.clamp(0, w - 1) as u32;
        let y = y.clamp(0, h - 1) as u32;
        buf[((y * wu + x) * 4) as usize + ch] as u32
    };
    let mut tmp = vec![0u8; src.len()];
    for y in 0..h {
        let mut sum = [0u32; 4];
        for dx in -r..=r {
            for (ch, s) in sum.iter_mut().enumerate() {
                *s += px(src, dx, y, ch);
            }
        }
        for x in 0..w {
            for ch in 0..4 {
                tmp[((y as u32 * wu + x as u32) * 4) as usize + ch] = (sum[ch] / window) as u8;
            }
            for (ch, s) in sum.iter_mut().enumerate() {
                *s += px(src, x + r + 1, y, ch);
                *s -= px(src, x - r, y, ch);
            }
        }
    }
    let mut out = vec![0u8; src.len()];
    for x in 0..w {
        let mut sum = [0u32; 4];
        for dy in -r..=r {
            for (ch, s) in sum.iter_mut().enumerate() {
                *s += px(&tmp, x, dy, ch);
            }
        }
        for y in 0..h {
            for ch in 0..4 {
                out[((y as u32 * wu + x as u32) * 4) as usize + ch] = (sum[ch] / window) as u8;
            }
            for (ch, s) in sum.iter_mut().enumerate() {
                *s += px(&tmp, x, y + r + 1, ch);
                *s -= px(&tmp, x, y - r, ch);
            }
        }
    }
    out
}

#[test]
fn box_blur_matches_the_naive_reference() {
    // Single-row/column and sub-window canvases exercise the clamped
    // edge extension; the wider sizes cover interior and tile seams.
    for (w, h) in [(1u32, 1u32), (3, 7), (9, 5), (33, 64), (40, 37)] {
        let n = (w * h) as usize * 4;
        let src: Vec<u8> = (0..n)
            .map(|i| ((i * 37 + i / 7 * 11) % 251) as u8)
            .collect();
        for r in [1, 2, 3, 5, 25] {
            assert_eq!(
                Document::box_blur(&src, w, h, r).unwrap(),
                naive_box_blur(&src, w, h, r),
                "{w}x{h} r{r}"
            );
        }
        assert_eq!(
            Document::box_blur(&src, w, h, 0).unwrap(),
            src,
            "{w}x{h} r0"
        );
    }
}

#[test]
fn blur_layer_matches_the_dense_reference() {
    let (w, h) = (40u32, 30u32);
    let src: Vec<u8> = (0..(w * h * 4) as usize)
        .map(|i| ((i * 53 + 7) % 256) as u8)
        .collect();
    let want = naive_box_blur(&src, w, h, 3);
    // No selection: every pixel takes the blurred value.
    let mut doc = Document::new(w, h);
    doc.layers[0].pixels = px_rgba(&doc, &src);
    doc.blur_layer(0, 3).unwrap();
    assert_eq!(doc.layers[0].pixels.to_dense(), want);
    // Rect selection: only the inside changes.
    let mut doc = Document::new(w, h);
    doc.layers[0].pixels = px_rgba(&doc, &src);
    doc.selection = Some((10, 8, 25, 20));
    doc.blur_layer(0, 3).unwrap();
    let got = doc.layers[0].pixels.to_dense();
    for y in 0..h {
        for x in 0..w {
            let inside = (10..=25).contains(&x) && (8..=20).contains(&y);
            let o = ((y * w + x) * 4) as usize;
            let expect = if inside { &want } else { &src };
            assert_eq!(&got[o..o + 4], &expect[o..o + 4], "({x},{y})");
        }
    }
    // Feathered edge: the mask weight mixes the original and the blur.
    let mut doc = Document::new(w, h);
    doc.layers[0].pixels = px_rgba(&doc, &src);
    doc.selection = Some((10, 8, 25, 20));
    assert!(doc.feather_selection(2).unwrap());
    let sel = doc.selection.unwrap();
    let mask = doc.selection_mask.clone().unwrap();
    doc.blur_layer(0, 3).unwrap();
    let got = doc.layers[0].pixels.to_dense();
    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            let inside = x >= sel.0 && x <= sel.2 && y >= sel.1 && y <= sel.3;
            let m = mask[(y * w + x) as usize];
            let expect = if inside && m != 0 {
                let k = m as f32 / 255.0;
                [
                    (src[o] as f32 * (1.0 - k) + want[o] as f32 * k).round() as u8,
                    (src[o + 1] as f32 * (1.0 - k) + want[o + 1] as f32 * k).round() as u8,
                    (src[o + 2] as f32 * (1.0 - k) + want[o + 2] as f32 * k).round() as u8,
                    (src[o + 3] as f32 * (1.0 - k) + want[o + 3] as f32 * k).round() as u8,
                ]
            } else {
                [src[o], src[o + 1], src[o + 2], src[o + 3]]
            };
            assert_eq!(&got[o..o + 4], &expect, "({x},{y}) mask {m}");
        }
    }
}

#[test]
fn kaleidoscope_segments_clamp() {
    let at = |m: SymmetryMode, seg: u8| m.mirrored_pixels(21, 21, seg, 10.0, 10.0, 5, 5).len();
    assert_eq!(at(SymmetryMode::Kaleidoscope, 6), 6);
    assert_eq!(at(SymmetryMode::Kaleidoscope, 1), 2);
    assert_eq!(at(SymmetryMode::Kaleidoscope, 99), 16);
    assert_eq!(at(SymmetryMode::Both, 9), 4);
}

#[test]
fn kaleidoscope_paints_n_copies() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(21, 21);
    doc.symmetry = SymmetryMode::Kaleidoscope;
    doc.symmetry_segments = 4;
    let brush = Brush {
        size: 3.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color::rgb(255, 0, 0), 14.5, 10.5);
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 21 + x) * 4 + 3) as usize);
    for (x, y) in [(14, 10), (10, 14), (6, 10), (10, 6)] {
        assert!(at(&doc, x, y) > 0, "missing dab at {x},{y}");
    }
    assert_eq!(at(&doc, 0, 0), 0);
}

#[test]
fn roundtrip_keeps_kaleidoscope() {
    let mut doc = Document::new(4, 3);
    doc.symmetry = SymmetryMode::Kaleidoscope;
    doc.symmetry_segments = 9;
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-kaleido.pcraft");
    doc.save_project(&path).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.symmetry, SymmetryMode::Kaleidoscope);
    assert_eq!(back.symmetry_segments, 9);
}

#[test]
fn v2_project_opens_with_default_segments() {
    // Minimal version-2 file: one symmetry byte, no segments byte.
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v2-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT02");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[255, 255, 255, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[1]); // Horizontal.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&[5u8; 2 * 4]);
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.symmetry, SymmetryMode::Horizontal);
    assert_eq!(back.symmetry_segments, 6);
}

#[test]
fn merge_down_composites_and_removes() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels = px_rgba(&doc, &[0, 0, 255, 255, 0, 0, 255, 255]);
    doc.add_layer("top".to_string());
    doc.layers[1].pixels = px_rgba(&doc, &[255, 0, 0, 255, 0, 0, 0, 0]);
    assert!(doc.merge_down(1).unwrap());
    assert_eq!(doc.layers.len(), 1);
    assert_eq!(doc.active, 0);
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
    assert_eq!(doc.layers[0].pixels.get4(4), [0, 0, 255, 255]);
    // Bottom layer refuses.
    assert!(!doc.merge_down(0).unwrap());
    // Invisible top contributes nothing but still merges away.
    doc.add_layer("ghost".to_string());
    doc.layers[1].visible = false;
    doc.layers[1].pixels = px_rgba(&doc, &[0, 255, 0, 255, 0, 255, 0, 255]);
    assert!(doc.merge_down(1).unwrap());
    assert_eq!(doc.layers[0].pixels.get4(0), [255, 0, 0, 255]);
}

#[test]
fn duplicate_copies_layer() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels = px_rgba(&doc, &[7; 8]);
    doc.duplicate_layer(0);
    assert_eq!(doc.layers.len(), 2);
    assert_eq!(doc.active, 1);
    assert_eq!(doc.layers[1].name, "Layer 1 copy");
    assert_eq!(doc.layers[1].pixels, vec![7; 8]);
    doc.duplicate_layer(9);
    assert_eq!(doc.layers.len(), 2);
}

#[test]
fn sample_reads_composite() {
    let mut doc = Document::new(2, 1);
    doc.layers[0].pixels = px_rgba(&doc, &[10, 20, 30, 255, 0, 0, 0, 0]);
    assert_eq!(doc.sample(0, 0), Some([10, 20, 30]));
    assert_eq!(doc.sample(9, 9), None);
}

#[test]
fn lasso_selects_interior_only() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(20, 20);
    let pts = vec![(0.0, 0.0), (12.0, 0.0), (0.0, 12.0), (0.0, 0.0)];
    let bbox = doc
        .set_lasso(&pts)
        .expect("lasso builds")
        .expect("lasso selects");
    assert!(doc.pixel_selected(2, 2));
    assert!(!doc.pixel_selected(15, 15));
    assert_eq!(doc.selection, Some(bbox));
    assert!(doc.selection_pixel_count() > 0);
    // Painting is constrained to the lasso.
    let brush = Brush {
        opacity: 1.0,
        ..Default::default()
    };
    doc.fill_rect(&brush, Color::rgb(9, 9, 9), 0, 0, 19, 19);
    let inside = ((2 * 20 + 2) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get(inside + 3), 255);
    let outside = ((15 * 20 + 15) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get(outside + 3), 0);
    // Degenerate loops clear.
    assert!(doc.set_lasso(&[]).unwrap().is_none());
    assert!(doc.selection_mask.is_none());
    doc.clear_selection();
    assert!(doc.selection.is_none() && doc.selection_mask.is_none());
    assert_eq!(doc.selection_pixel_count(), 0);
}

#[test]
fn shape_fill_respects_selection() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(10, 10);
    let brush = Brush {
        opacity: 1.0,
        ..Default::default()
    };
    doc.selection = Some((2, 2, 5, 5));
    doc.fill_rect(&brush, Color::rgb(1, 2, 3), 0, 0, 9, 9);
    assert_eq!(doc.layers[0].pixels.get4(0), [0, 0, 0, 0]);
    let o = ((3 * 10 + 3) * 4) as usize;
    assert_eq!(doc.layers[0].pixels.get(o + 3), 255);
}

#[test]
fn v3_project_still_opens_raw() {
    // Hand-build a minimal version-3 file (raw pixels, symmetry + segments).
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v3-compat.pcraft");
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PCRAFT03");
    buf.extend_from_slice(&2u32.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&[1, 2, 3, 255]);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&[3]); // Both.
    buf.extend_from_slice(&[9]); // Segments.
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(b"L");
    buf.extend_from_slice(&[1]);
    buf.extend_from_slice(&1.0f32.to_le_bytes());
    buf.extend_from_slice(&[0]);
    buf.extend_from_slice(&[6u8; 2 * 4]);
    std::fs::write(&path, &buf).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.symmetry, SymmetryMode::Both);
    assert_eq!(back.symmetry_segments, 9);
    assert_eq!(back.layers[0].pixels, vec![6u8; 8]);
}

#[test]
fn v4_roundtrip_compresses_sparse_canvas() {
    // Mostly-transparent canvas must shrink far below raw size.
    let doc = Document::new(128, 128);
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v4-size.pcraft");
    doc.save_project(&path).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    let raw = 128usize * 128 * 4;
    assert!(
        meta.len() as usize * 4 < raw,
        "compressed {} vs raw {raw}",
        meta.len()
    );
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[..8], b"PCRAFT12");
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].pixels, doc.layers[0].pixels);
}

#[test]
fn v4_noisy_pixels_still_roundtrip() {
    // Incompressible noise must survive the zlib round trip exactly.
    let mut doc = Document::new(32, 32);
    let mut x: u32 = 0x12345678;
    let bytes: Vec<u8> = (0..32 * 32 * 4)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect();
    doc.layers[0].pixels = Raster::from_dense(32, 32, 4, &bytes);
    let want = doc.layers[0].pixels.clone();
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v4-noise.pcraft");
    doc.save_project(&path).unwrap();
    let back = Document::open_project(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.layers[0].pixels, want);
}

#[test]
fn v4_corrupt_pixels_rejected() {
    let doc = Document::new(8, 8);
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-v4-corrupt.pcraft");
    doc.save_project(&path).unwrap();
    let mut buf = std::fs::read(&path).unwrap();
    // Flip bytes inside the compressed payload (last 8 bytes).
    let n = buf.len();
    for b in buf[n - 8..].iter_mut() {
        *b ^= 0xFF;
    }
    std::fs::write(&path, &buf).unwrap();
    assert!(Document::open_project(&path).is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn m45_import_rejects_oversize_dimensions() {
    // M45: a phone panorama wider than the canvas cap must fail
    // closed instead of adopting an oversize layer (OOM guard).
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-m45-oversize.png");
    let wide = MAX_CANVAS_SIDE + 8;
    let tall = 100u32;
    let raw = vec![200u8; (wide * tall * 4) as usize];
    let mut out = Vec::new();
    {
        use image::ImageEncoder;
        let enc = image::codecs::png::PngEncoder::new(&mut out);
        enc.write_image(&raw, wide, tall, image::ExtendedColorType::Rgba8)
            .unwrap();
    }
    std::fs::write(&path, &out).unwrap();
    let mut doc = Document::new(8, 8);
    let err = doc.load_background(&path).unwrap_err();
    assert!(err.contains("too large"), "unexpected: {err}");
    // Failed import touches nothing.
    assert_eq!((doc.w, doc.h), (8, 8));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn m45_open_project_rejects_huge_file() {
    // M45: a 200MB+ file must fail closed on metadata alone,
    // never read_to_end into memory.
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-m45-huge.pcraft");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(200 * 1024 * 1024 + 1).unwrap();
    drop(f);
    let err = Document::open_project(&path).unwrap_err();
    assert!(err.contains("too large"), "unexpected: {err}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn m45_project_fuzz_truncations_and_flips_never_panic() {
    // M45: truncations fail closed (Err); sampled bit-flips never
    // panic (Ok allowed — flipping a background byte is still a
    // valid file, just a different color).
    let doc = Document::new(8, 8);
    let dir = std::env::temp_dir();
    let path = dir.join("procraft-m45-fuzz.pcraft");
    doc.save_project(&path).unwrap();
    let full = std::fs::read(&path).unwrap();
    // Truncations: empty, 1 byte, mid, and len-1 all fail closed.
    for n in [0usize, 1, 8, full.len() / 2, full.len() - 1] {
        std::fs::write(&path, &full[..n]).unwrap();
        assert!(Document::open_project(&path).is_err(), "trunc {n} opened");
    }
    // Bit-flips: magic flips must fail closed; every flip must not
    // panic (the return value is dropped — a panic fails the test).
    for i in [0usize, 4, 8, 12, 32, 64] {
        if i >= full.len() {
            continue;
        }
        let mut bad = full.clone();
        bad[i] ^= 0xFF;
        std::fs::write(&path, &bad).unwrap();
        let _ = Document::open_project(&path);
    }
    // Magic-byte flips specifically are never valid files.
    for i in [0usize, 4] {
        let mut bad = full.clone();
        bad[i] ^= 0xFF;
        std::fs::write(&path, &bad).unwrap();
        assert!(Document::open_project(&path).is_err(), "flip {i} opened");
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn strokes_fully_outside_canvas_are_safe() {
    use crate::brush::{Brush, BrushTip, Color};
    // Regression: a dab fully right/below the canvas used to record an
    // inverted dirty rect, and the texture upload's `x1 - x0 + 1` then
    // underflowed and quit the app.
    let mut doc = Document::new(32, 32);
    let brush = Brush {
        size: 8.0,
        ..Default::default()
    };
    let clean = doc.layers[0].pixels.clone();
    for (from, to) in [
        (Some((100.0, 100.0)), (400.0, 400.0)),   // right-bottom
        (Some((-400.0, 100.0)), (-100.0, 100.0)), // left
        (Some((100.0, -400.0)), (100.0, -100.0)), // above
        (None, (1000.0, 1000.0)),                 // single dab far outside
    ] {
        doc.stroke_to(&brush, Color::rgb(255, 0, 0), from, to, 0.0);
    }
    assert_eq!(doc.layers[0].pixels, clean);
    // No poisoned region may survive: consumers subtract safely.
    if let Some((x0, y0, x1, y1)) = doc.take_dirty() {
        assert!(x0 <= x1 && y0 <= y1);
        let bytes = doc.flatten_region(x0, y0, x1, y1);
        let w = (x1 - x0 + 1) as usize;
        let h = (y1 - y0 + 1) as usize;
        assert_eq!(bytes.len(), w * h * 4);
    }
    // Same for stamp tips and mask painting.
    let tip = BrushTip::from_mask(2, 2, vec![255; 4]).unwrap();
    let stamp = Brush {
        size: 16.0,
        tip: Some(tip),
        ..Brush::default()
    };
    doc.stroke_to(
        &stamp,
        Color::rgb(1, 2, 3),
        Some((200.0, 200.0)),
        (500.0, 500.0),
        0.0,
    );
    assert_eq!(doc.layers[0].pixels, clean);
    doc.paint_target = PaintTarget::Mask;
    doc.stroke_to(
        &brush,
        Color::rgb(0, 0, 0),
        Some((200.0, 200.0)),
        (500.0, 500.0),
        0.0,
    );
    assert!(doc.layers[0].mask.is_none());
    doc.paint_target = PaintTarget::Color;
}

fn selected_rect(w: u32, h: u32, r: (u32, u32, u32, u32)) -> Document {
    let mut doc = Document::new(w, h);
    doc.selection = Some(r);
    doc
}

#[test]
fn lasso_writes_full_strength_masks() {
    let mut doc = Document::new(10, 10);
    doc.set_lasso(&[(2.0, 2.0), (2.0, 5.0), (5.0, 5.0), (5.0, 2.0)])
        .unwrap();
    let mask = doc.selection_mask.clone().unwrap();
    assert!(mask.iter().all(|&b| b == 0 || b == 255));
    assert_eq!(
        doc.selection_pixel_count(),
        mask.iter().filter(|&&b| b != 0).count()
    );
}

#[test]
fn feather_ramps_and_expands_bounds() {
    let mut doc = selected_rect(20, 20, (5, 5, 14, 14));
    assert!(doc.feather_selection(3).unwrap());
    let m = doc.selection_mask.clone().unwrap();
    let at = |x: u32, y: u32| m[(y * 20 + x) as usize];
    // Deep interior stays full, far exterior stays empty.
    assert_eq!(at(10, 10), 255);
    assert_eq!(at(0, 0), 0);
    // The old hard edge is now a ramp (partial coverage just outside).
    let ramp = at(3, 10);
    assert!(ramp > 0 && ramp < 255, "{ramp}");
    // Bounds grew to contain the falloff; coverage is conserved by blur.
    assert_eq!(doc.selection, Some((2, 2, 17, 17)));
    let count = doc.selection_pixel_count() as i32;
    assert!((count - 100).abs() <= 2, "{count}");
    // Radius 0 only normalizes, and empty selections report false.
    let mut doc2 = selected_rect(20, 20, (5, 5, 14, 14));
    assert!(doc2.feather_selection(0).unwrap());
    assert_eq!(doc2.selection_pixel_count(), 100);
    let mut doc3 = Document::new(8, 8);
    assert!(!doc3.feather_selection(4).unwrap());
}

#[test]
fn grow_and_shrink_track_bounds() {
    let mut doc = selected_rect(20, 20, (5, 5, 14, 14));
    assert!(doc.grow_selection(2).unwrap());
    assert_eq!(doc.selection, Some((3, 3, 16, 16)));
    assert!(doc.selection_pixel_count() > 100);
    assert!(doc.grow_selection(-2).unwrap());
    assert_eq!(doc.selection, Some((5, 5, 14, 14)));
    assert_eq!(doc.selection_pixel_count(), 100);
    // Shrinking to nothing clears everything.
    assert!(!doc.grow_selection(-50).unwrap());
    assert!(doc.selection.is_none() && doc.selection_mask.is_none());
    // Zero on nothing stays nothing.
    assert!(!doc.grow_selection(0).unwrap());
    let mut doc2 = Document::new(8, 8);
    assert!(!doc2.grow_selection(3).unwrap());
}

#[test]
fn smooth_fills_pinholes_keeps_solids() {
    let mut doc = Document::new(12, 12);
    doc.selection_mask = Some(vec![255u8; 144]);
    doc.selection_mask.as_mut().unwrap()[5 * 12 + 5] = 0;
    assert!(doc.smooth_selection().unwrap());
    // Single-pixel hole heals (blur r2 keeps it over threshold).
    assert_eq!(doc.selection_mask.clone().unwrap()[5 * 12 + 5], 255);
    // Solid rects survive smoothing byte-identical in the interior.
    let mut doc2 = selected_rect(20, 20, (5, 5, 14, 14));
    assert!(doc2.smooth_selection().unwrap());
    assert_eq!(
        doc2.selection_mask.clone().unwrap()[(10 * 20 + 10) as usize],
        255
    );
    let mut doc3 = Document::new(8, 8);
    assert!(!doc3.smooth_selection().unwrap());
}

#[test]
fn invert_flips_to_full_canvas_complement() {
    let mut doc = selected_rect(10, 10, (2, 2, 5, 5));
    assert!(doc.invert_selection().unwrap());
    let m = doc.selection_mask.clone().unwrap();
    assert_eq!(m[(3 * 10 + 3) as usize], 0);
    assert_eq!(m[0], 255);
    assert_eq!(doc.selection, Some((0, 0, 9, 9)));
    assert_eq!(doc.selection_pixel_count(), 100 - 16);
    let mut doc2 = Document::new(8, 8);
    assert!(!doc2.invert_selection().unwrap());
}

#[test]
fn opaque_selects_painted_pixels() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(10, 10);
    let brush = Brush {
        size: 3.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color::rgb(255, 0, 0), 2.5, 2.5);
    assert!(doc.select_opaque(8).unwrap());
    let n = doc.selection_pixel_count();
    assert!(n > 0 && n < 100, "{n}");
    // Bounds hug the dab.
    let (x0, y0, x1, y1) = doc.selection.unwrap();
    assert!(x1 - x0 <= 4 && y1 - y0 <= 4);
    // Untouched layers clear the selection.
    let mut doc2 = Document::new(8, 8);
    assert!(!doc2.select_opaque(8).unwrap());
    assert!(doc2.selection.is_none());
}

#[test]
fn stroke_paints_only_the_edge() {
    use crate::brush::{Brush, Color};
    let mut doc = selected_rect(12, 12, (2, 2, 7, 7));
    let brush = Brush {
        size: 1.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.checkpoint();
    assert!(doc
        .stroke_selection(&brush, Color::rgb(0, 255, 0), 1)
        .unwrap());
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 12 + x) * 4) as usize + 3);
    // Edge pixels painted, deep interior untouched.
    assert!(at(&doc, 2, 4) > 0);
    assert!(at(&doc, 7, 7) > 0);
    assert_eq!(at(&doc, 4, 4), 0);
    assert_eq!(at(&doc, 0, 0), 0);
    // One undo step removes the whole outline.
    assert!(doc.undo());
    assert_eq!(at(&doc, 2, 4), 0);
    // No selection, no paint.
    let mut doc2 = Document::new(8, 8);
    assert!(!doc2
        .stroke_selection(&brush, Color::rgb(0, 255, 0), 1)
        .unwrap());
}

#[test]
fn feathered_masks_partially_apply_paint_and_tone() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(20, 20);
    doc.selection = Some((5, 5, 14, 14));
    assert!(doc.feather_selection(3).unwrap());
    // A ramp pixel just outside the old edge carries partial coverage.
    let m = doc.selection_mask.clone().unwrap();
    let ramp = m[(4 * 20 + 10) as usize];
    // Tone applies proportionally through the ramp.
    doc.checkpoint();
    doc.adjust_tone(0, 100, 1.0).unwrap();
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 20 + x) * 4) as usize);
    assert_eq!(at(&doc, 10, 10), 100);
    let edge = at(&doc, 10, 4);
    if ramp > 0 {
        assert!(edge > 0 && edge < 100, "{edge} ramp {ramp}");
    } else {
        assert_eq!(edge, 0);
    }
    assert_eq!(at(&doc, 0, 0), 0);
    // Dabs fade through the same ramp.
    let mut doc2 = Document::new(20, 20);
    doc2.selection = Some((5, 5, 14, 14));
    assert!(doc2.feather_selection(3).unwrap());
    let brush = Brush {
        size: 1.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc2.dab(&brush, Color::rgb(255, 255, 255), 10.5, 4.5);
    let a = doc2.layers[0].pixels.get(((4 * 20 + 10) * 4) as usize + 3);
    let ramp2 = doc2.selection_mask.clone().unwrap()[(4 * 20 + 10) as usize];
    if ramp2 > 0 {
        assert!(a > 0 && a < 255, "{a} ramp {ramp2}");
    } else {
        assert_eq!(a, 0);
    }
}

#[test]
fn gallery_thumb_downscales_square() {
    // 4x4 where pixel (x,y) holds red=x*64+y*16: each 2x2 block's
    // top-left must win the single output pixel (nearest-neighbor).
    let mut src = vec![0u8; 4 * 4 * 4];
    for y in 0..4 {
        for x in 0..4 {
            let i = ((y * 4 + x) * 4) as usize;
            src[i] = (x * 64 + y * 16) as u8;
            src[i + 1] = 1;
            src[i + 2] = 2;
            src[i + 3] = 255;
        }
    }
    let (out, tw, th) = gallery_thumb(&src, 4, 4, 2);
    assert_eq!((tw, th), (2, 2));
    let at = |x: usize, y: usize| out[(y * 2 + x) * 4];
    assert_eq!(at(0, 0), 0);
    assert_eq!(at(1, 0), 128);
    assert_eq!(at(0, 1), 32);
    assert_eq!(at(1, 1), 160);
}

#[test]
fn gallery_thumb_preserves_aspect_and_skips_upscale() {
    let src = vec![7u8; 8 * 4 * 4];
    let (out, tw, th) = gallery_thumb(&src, 8, 4, 4);
    assert_eq!((tw, th), (4, 2));
    assert_eq!(out.len(), 4 * 2 * 4);
    let small = vec![9u8; 2 * 2 * 4];
    let (same, w, h) = gallery_thumb(&small, 2, 2, 96);
    assert_eq!((w, h), (2, 2));
    assert_eq!(same, small);
}

#[test]
fn gallery_thumb_rejects_bad_input() {
    let (out, w, h) = gallery_thumb(&[1u8; 10], 4, 4, 96);
    assert!(out.is_empty() && w == 0 && h == 0);
    let (out, w, h) = gallery_thumb(&[], 0, 0, 96);
    assert!(out.is_empty() && w == 0 && h == 0);
}

#[test]
fn translate_selection_moves_rect() {
    let mut doc = Document::new(10, 10);
    doc.selection = Some((2, 2, 5, 5));
    assert!(doc.translate_selection(3, 1).unwrap());
    assert_eq!(doc.selection, Some((5, 3, 8, 6)));
    // Weights survive a mask nudge (bake materializes the rect first).
    let m = doc.selection_mask.clone().unwrap();
    assert!(m.iter().all(|&b| b == 0 || b == 255));
}

#[test]
fn translate_selection_off_canvas_clears() {
    let mut doc = Document::new(4, 4);
    doc.selection = Some((0, 0, 1, 1));
    assert!(!doc.translate_selection(10, 0).unwrap());
    assert!(doc.selection.is_none() && doc.selection_mask.is_none());
    // Partial push keeps the on-canvas tail.
    let mut doc = Document::new(4, 4);
    doc.selection = Some((0, 0, 1, 1));
    assert!(doc.translate_selection(3, 0).unwrap());
    assert_eq!(doc.selection, Some((3, 0, 3, 1)));
}

#[test]
fn flip_selection_mirrors_rect_and_mask() {
    let mut doc = Document::new(10, 10);
    doc.selection = Some((1, 2, 4, 6));
    assert!(doc.flip_selection_h().unwrap());
    assert_eq!(doc.selection, Some((5, 2, 8, 6)));
    let mut doc = Document::new(10, 10);
    doc.selection = Some((1, 2, 4, 6));
    assert!(doc.flip_selection_v().unwrap());
    assert_eq!(doc.selection, Some((1, 3, 4, 7)));
    // Weighted mask pixels land mirrored (feather fringe included).
    let mut doc = Document::new(4, 4);
    let mut mask = vec![0u8; 16];
    mask[0] = 128;
    doc.selection_mask = Some(mask);
    assert!(doc.flip_selection_h().unwrap());
    let m = doc.selection_mask.clone().unwrap();
    assert_eq!(m[3], 128);
    assert!(doc.flip_selection_v().unwrap());
    let m = doc.selection_mask.clone().unwrap();
    assert_eq!(m[15], 128);
}

#[test]
fn levels_lut_identity_and_crush() {
    let id = levels_lut(0, 1.0, 255);
    for (i, v) in id.iter().enumerate() {
        assert_eq!(*v, i as u8);
    }
    // Degenerate range refuses to clip: identity, not black.
    assert_eq!(levels_lut(200, 1.0, 100)[64], 64);
    let crush = levels_lut(128, 1.0, 255);
    assert_eq!(crush[64], 0);
    assert_eq!(crush[255], 255);
    // Gamma > 1 darkens mids; < 1 lifts them.
    assert!(levels_lut(0, 2.0, 255)[128] < 128);
    assert!(levels_lut(0, 0.5, 255)[128] > 128);
}

#[test]
fn curves_lut_identity_pins_ends() {
    let id = curves_lut(0, 0, 0);
    for (i, v) in id.iter().enumerate() {
        assert_eq!(*v, i as u8);
    }
    let lift = curves_lut(100, 0, 0);
    assert_eq!(lift[0], 0);
    assert_eq!(lift[255], 255);
    assert_eq!(lift[64], 164);
    // Between-point interpolation is linear.
    assert_eq!(lift[32], 82);
}

#[test]
fn apply_lut_honors_selection() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(4, 4);
    let brush = Brush {
        size: 20.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color::rgb(100, 100, 100), 1.5, 1.5);
    let lut = levels_lut(128, 1.0, 255);
    // Only pixel (0,0) selected: it crushes, the rest is untouched.
    doc.selection = Some((0, 0, 0, 0));
    doc.apply_lut(0, &lut).unwrap();
    assert_eq!(doc.layers[0].pixels.get(0), 0);
    // Pixel (1,1) offset 20 in the 4x4 RGBA buffer.
    let o = 20usize;
    assert_eq!(doc.layers[0].pixels.get(o), 100);
    // Whole canvas: mid gray crushes to 0, alpha stays 255.
    doc.clear_selection();
    doc.apply_lut(0, &lut).unwrap();
    assert_eq!(doc.layers[0].pixels.get(o), 0);
    assert_eq!(doc.layers[0].pixels.get(o + 3), 255);
}

#[test]
fn hsl_adjust_invariants() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(4, 4);
    let brush = Brush {
        size: 20.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    // Gray has no hue: hue spins leave it gray, lightness lifts it.
    doc.dab(&brush, Color::rgb(128, 128, 128), 1.5, 1.5);
    doc.adjust_hsl(0, 180.0, 2.0, 0.0).unwrap();
    let o = 20usize;
    assert_eq!(doc.layers[0].pixels.get(o), doc.layers[0].pixels.get(o + 1));
    assert_eq!(
        doc.layers[0].pixels.get(o + 1),
        doc.layers[0].pixels.get(o + 2)
    );
    doc.adjust_hsl(0, 0.0, 1.0, 1.0).unwrap();
    assert_eq!(doc.layers[0].pixels.get(o), 255);
    // Pure red rotated +120 lands on green.
    let mut doc = Document::new(2, 2);
    doc.dab(&brush, Color::rgb(255, 0, 0), 0.5, 0.5);
    doc.adjust_hsl(0, 120.0, 1.0, 0.0).unwrap();
    let dense = doc.layers[0].pixels.to_dense();
    let px = &dense[0..3];
    assert!(px[1] > 200 && px[0] < 55 && px[2] < 55, "{px:?}");
}

#[test]
fn balance_and_colorize_behave() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(2, 2);
    let brush = Brush {
        size: 20.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color::rgb(100, 100, 100), 0.5, 0.5);
    doc.color_balance(0, 51, 0, 0).unwrap();
    assert_eq!(doc.layers[0].pixels.get(0), 151);
    assert_eq!(doc.layers[0].pixels.get(1), 100);
    // No-op guards leave pixels alone.
    let before = doc.layers[0].pixels.clone();
    doc.color_balance(0, 0, 0, 0).unwrap();
    doc.colorize(0, [255, 0, 0], 0.0).unwrap();
    assert_eq!(doc.layers[0].pixels, before);
    // Colorize mid-gray to red: keeps luminance, takes the hue.
    let mut doc = Document::new(2, 2);
    doc.dab(&brush, Color::rgb(128, 128, 128), 0.5, 0.5);
    doc.colorize(0, [255, 0, 0], 1.0).unwrap();
    let dense = doc.layers[0].pixels.to_dense();
    let px = &dense[0..3];
    assert!(px[0] > 200 && px[1] < 80 && px[2] < 80, "{px:?}");
    // Selection gate: excluded pixels keep their color.
    let mut doc = Document::new(4, 4);
    doc.dab(&brush, Color::rgb(100, 100, 100), 1.5, 1.5);
    doc.selection = Some((0, 0, 0, 0));
    doc.color_balance(0, 51, 51, 51).unwrap();
    assert_eq!(doc.layers[0].pixels.get(0), 151);
    assert_eq!(doc.layers[0].pixels.get(20), 100);
}

#[test]
fn liquify_pushes_pixels_along_the_drag() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(32, 32);
    let brush = Brush {
        size: 30.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    // Solid white bar on the left half.
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 4, 4, 11, 27);
    // Push right at the bar's edge: the bar's rim must advance.
    doc.liquify(0, 11.5, 15.5, 6.0, 0.0, 16.0, 1.0);
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
    assert!(at(&doc, 14, 15) > 200, "rim advanced");
    assert_eq!(at(&doc, 24, 15), 0, "far field untouched");
    // Zero push and zero strength are no-ops.
    let before = doc.layers[0].pixels.clone();
    doc.liquify(0, 11.5, 15.5, 0.0, 0.0, 16.0, 1.0);
    doc.liquify(0, 11.5, 15.5, 6.0, 0.0, 16.0, 0.0);
    assert_eq!(doc.layers[0].pixels, before);
}

#[test]
fn liquify_pushes_across_a_tile_seam() {
    use crate::brush::{Brush, Color};
    // x = 256 is a tile seam; the push pulls red from the left tile
    // into the right one, so the region read must cross the seam.
    let mut doc = Document::new(520, 40);
    let brush = Brush {
        opacity: 1.0,
        ..Default::default()
    };
    doc.fill_rect(&brush, Color::rgb(255, 0, 0), 0, 0, 255, 39);
    doc.fill_rect(&brush, Color::rgb(0, 0, 255), 256, 0, 519, 39);
    doc.liquify(0, 256.0, 20.0, 10.0, 0.0, 24.0, 1.0);
    let at = |x: u32| doc.layers[0].pixels.get4(((20 * 520 + x) * 4) as usize);
    assert_eq!(
        at(257),
        [255, 0, 0, 255],
        "red is pulled across the seam from the left tile"
    );
    assert_eq!(at(400), [0, 0, 255, 255], "far field untouched");
}

#[test]
fn liquify_moves_only_pixels_inside_the_selection() {
    let (w, h) = (64u32, 32u32);
    let mut doc = Document::new(w, h);
    // Ramp images so a moved pixel reads back as a different value.
    let mut bytes = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            bytes[o] = (x * 4) as u8;
            bytes[o + 1] = (y * 8) as u8;
            bytes[o + 3] = 255;
        }
    }
    doc.layers[0].pixels = Raster::from_dense(w, h, 4, &bytes);
    let before = doc.layers[0].pixels.clone();
    let at = |r: &Raster, x: u32, y: u32| r.get4(((y * w + x) * 4) as usize);
    // The disc straddles the selection edge at x = 31: inside pixels
    // warp, outside pixels must stay byte-identical.
    doc.selection = Some((0, 0, 31, 31));
    doc.liquify(0, 28.0, 16.0, 0.0, 6.0, 40.0, 1.0);
    let moved = at(&doc.layers[0].pixels, 20, 16);
    assert!(
        (94..=96).contains(&(moved[1] as i32)),
        "selected pixel sampled the row above: {moved:?}"
    );
    for (x, y) in [(36, 16), (40, 20), (63, 16)] {
        assert_eq!(
            at(&doc.layers[0].pixels, x, y),
            at(&before, x, y),
            "unselected ({x},{y})"
        );
    }
}

#[test]
fn liquify_scales_the_push_by_mask_coverage() {
    let (w, h) = (64u32, 32u32);
    let mut doc = Document::new(w, h);
    // A horizontal ramp in red and a vertical one in green, so a sampled
    // position reads back as the position it came from.
    let mut bytes = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            bytes[o] = (x * 4) as u8;
            bytes[o + 1] = (y * 8) as u8;
            bytes[o + 3] = 255;
        }
    }
    doc.layers[0].pixels = Raster::from_dense(w, h, 4, &bytes);
    let mut mask = vec![255u8; (w * h) as usize];
    mask[(16 * w + 28) as usize] = 64; // k = 64/255
    mask[(16 * w + 30) as usize] = 0;
    doc.selection_mask = Some(mask);
    doc.liquify(0, 28.0, 16.0, 8.0, 0.0, 40.0, 1.0);
    let at = |x: u32, y: u32| doc.layers[0].pixels.get4(((y * w + x) * 4) as usize);
    // A quarter-covered pixel moves a quarter of the way: it samples
    // ~2 px back, where the ramp reads ~104.
    let quarter = at(28, 16);
    assert!(
        (103..=105).contains(&(quarter[0] as i32)),
        "quarter push sampled x~26: {quarter:?}"
    );
    assert_eq!(quarter[1], 128, "no vertical displacement");
    assert_eq!(at(30, 16), [120, 128, 0, 255], "k = 0 keeps the pixel");
}

#[test]
fn liquify_matches_a_dense_field_reference() {
    // Characterization: the tiled field and the region-bounded snapshot
    // reads must reproduce a whole-canvas field warp bit-for-bit,
    // including pushes whose reverse samples read far outside the
    // rewritten disc (and clamp at the canvas edge).
    fn field_warp(
        src: &[u8],
        w: u32,
        h: u32,
        pushes: &[(f32, f32, f32, f32, f32, f32)],
    ) -> Vec<u8> {
        let mut off = vec![[0.0f32; 2]; (w * h) as usize];
        let mut touched = vec![false; (w * h) as usize];
        let field_at = |off: &[[f32; 2]], x: f32, y: f32| -> [f32; 2] {
            let x = x.clamp(0.0, w as f32 - 1.0);
            let y = y.clamp(0.0, h as f32 - 1.0);
            let (x0, y0) = (x.floor() as u32, y.floor() as u32);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let at = |x: u32, y: u32| off[(y * w + x) as usize];
            let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            [
                a[0] * (1.0 - fx) * (1.0 - fy)
                    + b[0] * fx * (1.0 - fy)
                    + c[0] * (1.0 - fx) * fy
                    + d[0] * fx * fy,
                a[1] * (1.0 - fx) * (1.0 - fy)
                    + b[1] * fx * (1.0 - fy)
                    + c[1] * (1.0 - fx) * fy
                    + d[1] * fx * fy,
            ]
        };
        for &(x, y, dx, dy, size, strength) in pushes {
            let before = off.clone();
            let radius = size * 0.5;
            let bx0 = ((x - radius).floor().max(0.0)) as u32;
            let by0 = ((y - radius).floor().max(0.0)) as u32;
            let bx1 = ((x + radius).ceil().min(w as f32 - 1.0).max(0.0)) as u32;
            let by1 = ((y + radius).ceil().min(h as f32 - 1.0).max(0.0)) as u32;
            for py in by0..=by1 {
                for px in bx0..=bx1 {
                    let dist = (((px as f32 + 0.5 - x).powi(2) + (py as f32 + 0.5 - y).powi(2))
                        .sqrt()
                        / radius)
                        .min(1.0);
                    let f = 0.5 + 0.5 * (std::f32::consts::PI * dist).cos();
                    let d = [-dx * strength * f, -dy * strength * f];
                    let prev = field_at(&before, px as f32 + d[0], py as f32 + d[1]);
                    let o = (py * w + px) as usize;
                    off[o][0] = (prev[0] + d[0]).clamp(-(px as f32), (w - 1 - px) as f32);
                    off[o][1] = (prev[1] + d[1]).clamp(-(py as f32), (h - 1 - py) as f32);
                    touched[o] = true;
                }
            }
        }
        // Premultiplied bilinear, unpremultiplied on write (the
        // documented sampling rule; no selection, so k = 1).
        let sample = |sx: f32, sy: f32| -> [u8; 4] {
            let sx = sx.clamp(0.0, w as f32 - 1.0);
            let sy = sy.clamp(0.0, h as f32 - 1.0);
            let (cx0, cy0) = (sx.floor() as u32, sy.floor() as u32);
            let (cx1, cy1) = ((cx0 + 1).min(w - 1), (cy0 + 1).min(h - 1));
            let (fx, fy) = ((sx - cx0 as f32) as f64, (sy - cy0 as f32) as f64);
            let at = |x: u32, y: u32| -> [u8; 4] {
                let o = ((y * w + x) * 4) as usize;
                [src[o], src[o + 1], src[o + 2], src[o + 3]]
            };
            let (mut pr, mut pg, mut pb, mut pa) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for (x, y, weight) in [
                (cx0, cy0, (1.0 - fx) * (1.0 - fy)),
                (cx1, cy0, fx * (1.0 - fy)),
                (cx0, cy1, (1.0 - fx) * fy),
                (cx1, cy1, fx * fy),
            ] {
                let [r, g, b, a] = at(x, y);
                let a = a as f64 * weight;
                pa += a;
                pr += r as f64 * a;
                pg += g as f64 * a;
                pb += b as f64 * a;
            }
            if pa <= 0.0 {
                return [0, 0, 0, 0];
            }
            [
                (pr / pa).round().clamp(0.0, 255.0) as u8,
                (pg / pa).round().clamp(0.0, 255.0) as u8,
                (pb / pa).round().clamp(0.0, 255.0) as u8,
                pa.round().clamp(0.0, 255.0) as u8,
            ]
        };
        let mut out = src.to_vec();
        for py in 0..h {
            for px in 0..w {
                if !touched[(py * w + px) as usize] {
                    continue;
                }
                let [ox, oy] = off[(py * w + px) as usize];
                let o = ((py * w + px) * 4) as usize;
                out[o..o + 4].copy_from_slice(&sample(px as f32 + ox, py as f32 + oy));
            }
        }
        out
    }
    // Deterministic pseudo-random paint so any region-copy slip shows.
    let bytes: Vec<u8> = (0..32 * 32 * 4)
        .map(|i| ((i * 37 + 11) % 256) as u8)
        .collect();
    // Reach (|d| = 11.4) far beyond the disc: samples come from the
    // ring outside the rewritten pixels, then a second push folds in.
    let mut doc = Document::new(32, 32);
    doc.layers[0].pixels = Raster::from_dense(32, 32, 4, &bytes);
    doc.liquify(0, 20.0, 12.0, 9.0, -7.0, 14.0, 1.0);
    doc.liquify(0, 10.0, 20.0, -8.0, 5.0, 18.0, 0.6);
    assert_eq!(
        doc.layers[0].pixels,
        field_warp(
            &bytes,
            32,
            32,
            &[
                (20.0, 12.0, 9.0, -7.0, 14.0, 1.0),
                (10.0, 20.0, -8.0, 5.0, 18.0, 0.6)
            ]
        )
    );
    // Corner warp: reads clamp against the canvas edge.
    let mut doc = Document::new(32, 32);
    doc.layers[0].pixels = Raster::from_dense(32, 32, 4, &bytes);
    doc.liquify(0, 2.0, 2.0, 5.0, 4.0, 14.0, 0.7);
    assert_eq!(
        doc.layers[0].pixels,
        field_warp(&bytes, 32, 32, &[(2.0, 2.0, 5.0, 4.0, 14.0, 0.7)])
    );
}

#[test]
fn repeated_pushes_carry_the_rim_without_smearing_it() {
    use crate::brush::{Brush, Color};
    let brush = Brush {
        size: 30.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    let mut doc = Document::new(64, 32);
    doc.fill_rect(&brush, Color::rgb(255, 0, 0), 0, 0, 31, 31);
    // Content is carried with the pointer: ten pushes from one spot keep
    // pulling the rim outward instead of summing a force at fixed pixels.
    for _ in 0..10 {
        doc.liquify(0, 32.0, 16.0, 8.0, 0.0, 40.0, 1.0);
    }
    let px = |x: u32| doc.layers[0].pixels.get4(((16 * 64 + x) * 4) as usize);
    let rim = (0..64u32).rev().find(|&x| px(x)[3] > 0).unwrap();
    assert!(rim > 31, "the rim did not advance: {rim}");
    // The moved rim stays one filter wide: it steps straight from opaque
    // to clear between neighboring pixels instead of spreading over a
    // ramp, and every inked pixel is still pure red.
    let mut biggest = 0u8;
    for x in 0..63u32 {
        let [r, g, bl, a] = px(x);
        biggest = biggest.max(a.abs_diff(px(x + 1)[3]));
        if a > 0 {
            assert_eq!([r, g, bl], [255, 0, 0], "x={x} alpha={a}");
        }
    }
    assert_eq!(biggest, 255, "the rim smeared instead of stepping");
}

#[test]
fn liquify_drag_carries_content_with_the_pointer() {
    let (w, h) = (640u32, 360u32);
    let mut doc = Document::new(w, h);
    // A 6 px bar across the drag line at y = 180.
    for y in 60..=300u32 {
        for x in 312..=317u32 {
            doc.layers[0]
                .pixels
                .set4(((y * w + x) * 4) as usize, [30, 144, 255, 255]);
        }
    }
    // A 160 px drag at strength 0.5, fed as ~4 px pointer segments and
    // starting 70 px short of the bar (it enters the disc's rim).
    doc.liquify_begin(0);
    let (x0, x1, y) = (242.0f32, 402.0f32, 180.0f32);
    let n = 40u32;
    let mut at = x0;
    for i in 1..=n {
        let to = x0 + (x1 - x0) * i as f32 / n as f32;
        doc.liquify_segment(0, (at, y), (to, y), 128.0, 0.5);
        at = to;
    }
    doc.liquify_end();
    // The bar's centre (315.5) travels about half the drag: the offsets
    // are carried with the paint instead of being summed at fixed pixels,
    // which would stop pulling as soon as the disc's rim passed over.
    let (mut mass, mut sum, mut max_a, mut inked) = (0.0f32, 0.0f32, 0u8, 0u32);
    for x in 200..520u32 {
        let p = doc.layers[0].pixels.get4(((180 * w + x) * 4) as usize);
        if p[3] > 0 {
            inked += 1;
            mass += p[3] as f32;
            sum += x as f32 * p[3] as f32;
            max_a = max_a.max(p[3]);
        }
    }
    let advance = sum / mass.max(1.0) - 315.5;
    assert!(
        (40.0..=60.0).contains(&advance),
        "bar advanced {advance:.1} px (want ~50)"
    );
    // One resample of the snapshot: the bar keeps its width, its solid
    // interior, and a rim no wider than the filter.
    assert_eq!(max_a, 255, "the bar was softened");
    assert!(inked <= 8, "the bar smeared over {inked} px");
}

#[test]
fn liquify_keeps_edge_color_at_partial_alpha() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(64, 32);
    let brush = Brush {
        size: 30.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.fill_rect(&brush, Color::rgb(255, 0, 0), 10, 4, 30, 27);
    // A half-strength push stretches the bar's rim into fractional
    // alpha. Straight-alpha filtering would drag transparent black into
    // it (and the rim turns [227, 0, 0]); premultiplied keeps [255, 0, 0].
    doc.liquify(0, 30.5, 16.0, 6.0, 0.0, 40.0, 0.5);
    let mut partial = 0;
    for x in 0..64u32 {
        let [r, g, b, a] = doc.layers[0].pixels.get4(((16 * 64 + x) * 4) as usize);
        if a > 0 && a < 255 {
            partial += 1;
            assert_eq!([r, g, b], [255, 0, 0], "x={x} alpha={a}");
        }
    }
    assert!(partial > 0, "the push must leave a soft rim");
}

#[test]
fn liquify_starts_a_new_snapshot_after_a_paint() {
    use crate::brush::{Brush, Color};
    let (w, h) = (32u32, 32u32);
    let mut doc = Document::new(w, h);
    let bytes: Vec<u8> = (0..w * h * 4)
        .map(|i| ((i * 37 + 11) % 256) as u8)
        .collect();
    doc.layers[0].pixels = Raster::from_dense(w, h, 4, &bytes);
    doc.liquify(0, 16.0, 16.0, 6.0, 0.0, 24.0, 1.0);
    // Paint over everything: the open stroke's snapshot is now stale,
    // and the next push must warp the new pixels, not resurrect the old.
    let brush = Brush {
        size: 40.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 0, 0, w - 1, h - 1);
    for _ in 0..4 {
        doc.liquify(0, 16.0, 16.0, 6.0, 0.0, 24.0, 1.0);
    }
    for y in 0..h {
        for x in 0..w {
            assert_eq!(
                doc.layers[0].pixels.get4(((y * w + x) * 4) as usize),
                [255, 255, 255, 255],
                "({x},{y})"
            );
        }
    }
}

#[test]
fn liquify_steps_cover_the_whole_pointer_segment() {
    // Within the cap: one push, ending exactly at the pointer.
    let steps = liquify_steps((0.0, 0.0), (3.0, 4.0), 10.0);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0], ((3.0, 4.0), (3.0, 4.0)));
    // A fast move is split so every push stays under the cap, the
    // pushes chain end-to-end, and the last anchor is the pointer —
    // nothing is left over for the release to drop.
    let steps = liquify_steps((0.0, 0.0), (120.0, 160.0), 10.0);
    assert_eq!(steps.len(), 20);
    let (mut at, mut sum) = ((0.0, 0.0), (0.0, 0.0));
    for ((x, y), (dx, dy)) in &steps {
        assert!(dx.hypot(*dy) <= 10.0 + 1e-4, "step {dx},{dy}");
        assert!(
            (at.0 + dx - x).abs() < 1e-4 && (at.1 + dy - y).abs() < 1e-4,
            "gap before {x},{y}"
        );
        at = (*x, *y);
        sum = (sum.0 + dx, sum.1 + dy);
    }
    assert_eq!(at, (120.0, 160.0));
    assert!((sum.0 - 120.0).abs() < 1e-3 && (sum.1 - 160.0).abs() < 1e-3);
    // A pathological one-frame jump stays bounded, and an idle hand has
    // nothing to push.
    assert_eq!(liquify_steps((0.0, 0.0), (10_000.0, 0.0), 10.0).len(), 64);
    assert!(liquify_steps((7.0, 8.0), (7.0, 8.0), 5.0).is_empty());
}

#[test]
fn liquify_segment_matches_manual_stepping() {
    let (w, h) = (96u32, 32u32);
    let mut doc = Document::new(w, h);
    let bytes: Vec<u8> = (0..w * h * 4)
        .map(|i| ((i * 37 + 11) % 256) as u8)
        .collect();
    doc.layers[0].pixels = Raster::from_dense(w, h, 4, &bytes);
    let mut manual = doc.clone();
    let (from, to, size, strength) = ((6.0, 10.0), (84.0, 24.0), 24.0, 0.8);
    doc.liquify_segment(0, from, to, size, strength);
    for ((x, y), (dx, dy)) in liquify_steps(from, to, (size * 0.125).max(2.0)) {
        manual.liquify(0, x, y, dx, dy, size, strength);
    }
    assert_eq!(doc.layers[0].pixels, manual.layers[0].pixels);
}

#[test]
fn liquify_segment_matches_manual_stepping_with_symmetry() {
    // A segment renders its swept path once, per mirror; stepping push
    // by push renders every disc. The pixels must come out identical.
    use crate::brush::{Brush, Color};
    let (w, h) = (96u32, 64u32);
    let mut doc = Document::new(w, h);
    let brush = Brush {
        size: 30.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 10, 10, 40, 50);
    doc.symmetry = SymmetryMode::Horizontal;
    let mut manual = doc.clone();
    let (from, to, size, strength) = ((20.0, 18.0), (44.0, 40.0), 20.0, 0.8);
    doc.liquify_segment(0, from, to, size, strength);
    for ((x, y), (dx, dy)) in liquify_steps(from, to, (size * 0.125).max(2.0)) {
        manual.liquify(0, x, y, dx, dy, size, strength);
    }
    assert_eq!(doc.layers[0].pixels, manual.layers[0].pixels);
}

#[test]
fn liquify_mirrors_with_symmetry() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(32, 32);
    let brush = Brush {
        size: 30.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    // Two bars mirrored about the canvas middle: [3,7] and [24,28].
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 3, 8, 7, 24);
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 24, 8, 28, 24);
    doc.symmetry = SymmetryMode::Horizontal;
    // Push right at the left bar's rim: the mirrored push runs left
    // at the mirrored point with a mirrored direction.
    doc.liquify(0, 7.5, 15.5, 6.0, 0.0, 16.0, 1.0);
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
    assert!(at(&doc, 10, 15) > 200, "primary rim advanced");
    assert!(at(&doc, 21, 15) > 200, "mirrored rim advanced");
    assert_eq!(at(&doc, 15, 15), 0, "between the discs untouched");
}

#[test]
fn liquify_mirrors_kaleidoscope_push_vectors() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(32, 32);
    let brush = Brush {
        size: 30.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    // East bar and its k=1 (90 deg) image, origin at the middle.
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 21, 13, 23, 19);
    doc.fill_rect(&brush, Color::rgb(255, 255, 255), 13, 21, 19, 23);
    doc.set_symmetry_center(16.0, 16.0);
    doc.symmetry = SymmetryMode::Kaleidoscope;
    doc.symmetry_segments = 4;
    // Push up at the east bar's middle: k=1 mirrors the point 90
    // degrees CCW (south) and rotates the push vector to point west.
    doc.liquify(0, 23.0, 16.0, 0.0, -6.0, 16.0, 1.0);
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
    assert!(at(&doc, 23, 13) > 200, "primary push advanced the east rim");
    assert!(at(&doc, 12, 23) > 200, "k=1 push pulled the south bar west");
}

#[test]
fn fit_shift_math() {
    // Inside: no slide.
    assert_eq!(Document::fit_shift(100, 100, 30, 0, 40, 10, 50, 20), (0, 0));
    // Left overhang slides right; right overhang slides left.
    assert_eq!(
        Document::fit_shift(100, 100, 60, 0, 10, 10, 20, 20),
        (50, 0)
    );
    assert_eq!(
        Document::fit_shift(100, 100, -60, 0, 80, 10, 90, 20),
        (-51, 0)
    );
    // Fully displaced bboxes still absorb (only invisible tails trim).
    assert_eq!(
        Document::fit_shift(100, 100, 5000, 0, 10, 10, 20, 20),
        (4990, 0)
    );
    assert_eq!(Document::fit_shift(0, 0, 0, 0, 0, 0, 0, 0), (0, 0));
}

#[test]
fn absorb_keeps_content_on_screen() {
    let mut doc = Document::new(32, 32);
    // Paint a marker, move the layer, absorb where the marker shows.
    doc.layers[0].pixels.set(((4 * 32 + 4) * 4) as usize, 255);
    doc.layers[0]
        .pixels
        .set(((4 * 32 + 4) * 4 + 3) as usize, 255);
    doc.translate_layer(0, 10, 0);
    // Canvas (0,4) maps outside the buffer: slide home, marker rides
    // along and still shows at canvas (14,4).
    assert!(doc.absorb_offset(0, 0, 4, 0, 4));
    let flat = doc.flatten();
    assert_eq!(flat[((4 * 32 + 14) * 4) as usize], 255);
    // Offset shrank by the slide; second absorb is a no-op.
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    assert!(!doc.absorb_offset(0, 14, 4, 14, 4));
}

#[test]
fn paint_works_full_page_after_move() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(32, 32);
    doc.translate_layer(0, 20, 0);
    // The strip the offset pushed out of the buffer paints again.
    let brush = Brush {
        size: 4.0,
        softness: 0.0,
        opacity: 1.0,
        ..Default::default()
    };
    doc.dab(&brush, Color::rgb(255, 255, 255), 2.0, 16.0);
    let at = |d: &Document, x: u32, y: u32| d.layers[0].pixels.get(((y * 32 + x) * 4) as usize);
    // Composite shows the dab where the user put it.
    let flat = doc.flatten();
    assert!(flat[((16 * 32 + 2) * 4) as usize] > 200);
    // And pre-move content survived the slide (moved, not wiped).
    assert_eq!((doc.layers[0].ox, doc.layers[0].oy), (0, 0));
    let _ = at;
}

#[test]
fn rasterize_keeps_pixels_drops_block_and_undoes() {
    use crate::text::{TextBlock, TextStyle};
    let mut doc = Document::new(200, 120);
    let block = TextBlock {
        text: "Hi".into(),
        size_px: 32.0,
        color: [0, 0, 0, 255],
        style: TextStyle::default(),
        x: 10.0,
        y: 10.0,
        w: 0,
        h: 0,
    };
    let (idx, ..) = doc.add_text_layer("T", block).unwrap();
    let pixels = doc.layers[idx].pixels.clone();
    doc.checkpoint();
    assert!(doc.rasterize_text_layer(idx));
    assert!(!doc.layers[idx].is_text());
    assert_eq!(
        doc.layers[idx].pixels, pixels,
        "rasterize must not touch pixels"
    );
    assert!(!doc.rasterize_text_layer(idx)); // idempotent
    assert!(doc.undo()); // rasterize is undoable
    assert!(doc.layers[idx].is_text());
    assert_eq!(doc.layers[idx].pixels, pixels);
}

#[test]
fn merge_down_onto_text_layer_rasterizes_target() {
    use crate::text::{TextBlock, TextStyle};
    let mut doc = Document::new(64, 64);
    let block = TextBlock {
        text: "Hi".into(),
        size_px: 24.0,
        color: [0, 0, 0, 255],
        style: TextStyle::default(),
        x: 2.0,
        y: 2.0,
        w: 0,
        h: 0,
    };
    let (idx, ..) = doc.add_text_layer("T", block).unwrap();
    doc.add_layer("Layer over".into()); // becomes index idx+1
    assert!(doc.merge_down(idx + 1).unwrap());
    // The baked result must not pretend to still be editable text.
    assert!(!doc.layers[idx].is_text());
}

#[test]
fn text_layer_at_hits_bounds_topmost_visible() {
    use crate::text::{TextBlock, TextStyle};
    let mut doc = Document::new(200, 120);
    let a = TextBlock {
        text: "A".into(),
        size_px: 32.0,
        color: [0, 0, 0, 255],
        style: TextStyle::default(),
        x: 10.0,
        y: 10.0,
        w: 0,
        h: 0,
    };
    let (i, w, h) = doc.add_text_layer("A", a).unwrap();
    assert_eq!(doc.text_layer_at(10.0, 10.0), Some(i));
    // Inside near-edge (block bounds are [x, x+w) x [y, y+h)).
    assert_eq!(doc.text_layer_at(w as f32 + 9.9, h as f32 + 9.9), Some(i));
    assert_eq!(doc.text_layer_at(w as f32 + 10.1, 10.0), None);
    assert_eq!(doc.text_layer_at(-1.0, -1.0), None);
    doc.layers[i].visible = false;
    assert_eq!(doc.text_layer_at(10.0, 10.0), None);
    doc.layers[i].visible = true;
    // Topmost visible text layer wins.
    let b = TextBlock {
        text: "B".into(),
        size_px: 32.0,
        color: [0, 0, 0, 255],
        style: TextStyle::default(),
        x: 10.0,
        y: 10.0,
        w: 0,
        h: 0,
    };
    let (j, ..) = doc.add_text_layer("B", b).unwrap();
    assert_eq!(doc.text_layer_at(12.0, 12.0), Some(j));
}

#[test]
fn crop_shifts_text_block_anchor_with_content() {
    use crate::text::{TextBlock, TextStyle};
    let mut doc = Document::new(240, 160);
    let block = TextBlock {
        text: "Hi".into(),
        size_px: 32.0,
        color: [255, 0, 0, 255],
        style: TextStyle::default(),
        x: 60.0,
        y: 40.0,
        w: 0,
        h: 0,
    };
    let (idx, w, h) = doc.add_text_layer("T", block).unwrap();
    let sample = |d: &Document, x0: u32, y0: u32| {
        let l = &d.layers[idx];
        let mut out = Vec::new();
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let bx = x as i32 - l.ox;
                let by = y as i32 - l.oy;
                if bx < 0 || by < 0 || bx as u32 >= d.w || by as u32 >= d.h {
                    out.extend_from_slice(&[0, 0, 0, 0]);
                    continue;
                }
                let o = ((by as u32 * d.w + bx as u32) * 4) as usize;
                out.extend_from_slice(&l.pixels.get4(o));
            }
        }
        out
    };
    let before = sample(&doc, 60, 40);
    assert!(before.iter().any(|&b| b != 0), "text must render pixels");
    doc.crop(10, 10, 149, 149).unwrap(); // dx=-10, dy=-10
                                         // The anchor moved with its content.
    let t = doc.layers[idx].text.as_ref().unwrap();
    assert_eq!((t.x, t.y), (50.0, 30.0));
    // Pixels read back identically at the shifted canvas coords, and a
    // rerender at the new anchor reproduces them exactly.
    assert_eq!(sample(&doc, 50, 30), before);
    doc.rerender_text_layer(idx).unwrap();
    assert_eq!(sample(&doc, 50, 30), before);
    // One hit, at the new location; the old one is gone.
    assert_eq!(doc.text_layer_at(51.0, 31.0), Some(idx));
    assert_eq!(doc.text_layer_at(40.0, 20.0), None);
}

#[test]
fn blank_layers_allocate_no_tiles() {
    let mut doc = Document::new(2048, 2048);
    for i in 0..100 {
        doc.add_layer(format!("L{i}"));
    }
    assert_eq!(doc.layers.len(), 101);
    assert_eq!(doc.layer_alloc_bytes(), 0);
}

#[test]
fn undo_checkpoint_costs_touched_tiles_only() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(600, 600); // 3x3 tiles
    let brush = Brush::default();
    doc.dab(&brush, Color([255, 0, 0, 255]), 10.0, 10.0);
    doc.checkpoint();
    doc.dab(&brush, Color([0, 0, 255, 255]), 550.0, 550.0);
    assert!(
        doc.undo_bytes <= 2 * 256 * 1024,
        "one checkpoint of a two-tile layer: {} bytes",
        doc.undo_bytes
    );
    assert!(doc.undo());
    assert_eq!(
        doc.layers[0].pixels.get4(((550 * 600 + 550) * 4) as usize),
        [0; 4]
    );
    assert_eq!(
        doc.layers[0].pixels.get(((10 * 600 + 10) * 4 + 3) as usize),
        255
    );
}

#[test]
fn duplicate_layer_shares_tiles_until_painted() {
    use crate::brush::{Brush, Color};
    let mut doc = Document::new(600, 600);
    doc.dab(&Brush::default(), Color([0, 255, 0, 255]), 12.0, 12.0);
    doc.duplicate_layer(0);
    // Per-layer accounting counts the shared slot twice; the process holds
    // one tile because the Arc is shared.
    assert_eq!(doc.layer_alloc_bytes(), 2 * 256 * 1024);
    doc.active = 1;
    doc.dab(&Brush::default(), Color([1, 2, 3, 255]), 300.0, 300.0);
    assert_eq!(doc.layer_alloc_bytes(), 3 * 256 * 1024);
}

#[test]
fn fallible_export_buffers_match_the_infallible_paths() {
    let mut doc = Document::new(5, 4);
    doc.layers[0].pixels.set(3, 200);
    assert_eq!(doc.try_flatten().unwrap(), doc.flatten());
    let (baked, mask) = doc.baked_layer(0).unwrap().unwrap();
    assert_eq!(baked.len(), 5 * 4 * 4);
    assert_eq!(mask, None);
    // A missing layer is Ok(None) (skip it), not an error (OOM).
    assert!(doc.baked_layer(99).unwrap().is_none());
}

/// The outline stroke paints dense row spans now; every mask-boundary
/// pixel (and nothing else) must carry ink, through the layer offset.
#[test]
fn selection_outline_paints_the_mask_boundary() {
    use crate::brush::{Brush, Color};
    let (w, h) = (300u32, 200u32);
    let brush = Brush {
        opacity: 1.0,
        ..Default::default()
    };
    let mut doc = Document::new(w, h);
    let mut mask = vec![0u8; (w * h) as usize];
    for y in 40..160u32 {
        for x in 60..240u32 {
            if (x as i32 - 150).abs() + (y as i32 - 100).abs() <= 60 {
                mask[(y * w + x) as usize] = 255;
            }
        }
    }
    doc.selection_mask = Some(mask.clone());
    doc.translate_layer(0, 5, -3);
    assert!(doc
        .stroke_selection(&brush, Color::rgb(10, 200, 20), 1)
        .unwrap());

    let inside = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && x < w as i64
            && y < h as i64
            && mask[(y as usize) * w as usize + x as usize] > 127
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let edge = inside(x, y)
                && !(inside(x - 1, y) && inside(x + 1, y) && inside(x, y - 1) && inside(x, y + 1));
            // Canvas (x, y) stores at buffer (x - 5, y + 3).
            let (lx, ly) = (x - 5, y + 3);
            let a = if lx < 0 || ly < 0 || lx >= w as i64 || ly >= h as i64 {
                0
            } else {
                let o = ((ly as usize * w as usize) + lx as usize) * 4;
                doc.layers[0].pixels.get(o + 3)
            };
            assert_eq!(a == 255, edge, "({x},{y})");
        }
    }
}
