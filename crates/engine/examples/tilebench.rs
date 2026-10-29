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
        // One 256² patch of content per layer, all in the centre tile, so
        // 100 layers cost 25 MB instead of 25 GB. Step 4 in x is enough to
        // allocate the tile: blend cost is per output pixel regardless.
        for y in py0..(py0 + TILE).min(side) {
            for x in (px0..(px0 + TILE).min(side)).step_by(4) {
                layer
                    .pixels
                    .set4(((y * side + x) * 4) as usize, [200, 40, 40, 200]);
            }
        }
    }
    let plan = doc.stack_plan();
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
