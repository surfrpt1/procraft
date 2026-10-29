//! Composite cost per tile at 4K and 8K for 1/10/100 layers.
//!
//! Every layer holds one loaded 256² patch in the canvas centre tile, and
//! the per-tile benchmark composites exactly that tile: 100 layers means
//! 100 layers of real blending, not 99 fast paths over empty tiles.
//!
//! Run: cargo run --release -p procraft-engine --example tilebench

use procraft_engine::{Document, TILE};
use std::time::Instant;

/// Repetitions of the centre-tile composite, averaged into one figure.
const REPS: u32 = 20;

fn bench(side: u32, layers: usize) -> (f64, f64, usize) {
    let mut doc = Document::new(side, side);
    let (cx, cy) = (doc.tiles_x() / 2, doc.tiles_y() / 2);
    let (px0, py0) = (cx * TILE, cy * TILE);
    for i in 0..layers {
        if i > 0 {
            doc.add_layer(format!("L{i}"));
        }
        let layer = doc.layers.last_mut().expect("layer");
        layer.opacity = 0.6;
        // One full 256² patch of content per layer, all in the centre tile,
        // so 100 layers cost 25 MB instead of 25 GB. The fill has to be
        // dense: `composite_tile` early-outs per pixel once the accumulated
        // alpha is 0, so a sparse patch would time mostly fast paths and
        // the tile column would be an optimistic bound, not a solid tile.
        for y in py0..(py0 + TILE).min(side) {
            for x in px0..(px0 + TILE).min(side) {
                layer
                    .pixels
                    .set4(((y * side + x) * 4) as usize, [200, 40, 40, 200]);
            }
        }
    }
    let plan = doc.stack_plan();
    // R1 in code: the timed tile must hold blended content, not the bare
    // page. Composite it once and require a pixel that differs from the
    // backdrop. A plain `assert!` because the example runs in release.
    let probe = doc.composite_tile(&plan, cx, cy);
    let page = doc.background;
    assert!(
        probe
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[0] != page[0] || p[1] != page[1] || p[2] != page[2]),
        "tilebench: centre tile ({cx}, {cy}) holds no blended content"
    );
    let start = Instant::now();
    for _ in 0..REPS {
        std::hint::black_box(doc.composite_tile(&plan, cx, cy));
    }
    let per_tile = start.elapsed().as_secs_f64() * 1000.0 / f64::from(REPS);
    let start = Instant::now();
    std::hint::black_box(doc.flatten());
    let full = start.elapsed().as_secs_f64() * 1000.0;
    (per_tile, full, doc.layer_alloc_bytes())
}

fn main() {
    println!(
        "{:>6} {:>7} {:>12} {:>12}",
        "side", "layers", "tile ms", "flatten ms"
    );
    for side in [4096u32, 8192] {
        for layers in [1usize, 10, 100] {
            let (tile, full, alloc) = bench(side, layers);
            println!("{side:>6} {layers:>7} {tile:>12.2} {full:>12.1}");
            if side == 8192 && layers == 100 {
                println!();
                println!(
                    "layer_alloc_bytes (8K, 100 layers): {alloc} bytes ({:.1} MiB)",
                    alloc as f64 / (1024.0 * 1024.0)
                );
            }
        }
    }
}
