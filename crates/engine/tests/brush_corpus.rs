use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use procraft_engine::import_brush_file_report;
use procraft_engine::{Brush, Color, DabDynamics, Document};

const SUPPORTED: &[&str] = &[
    "abr", "bmp", "brush", "brushset", "bundle", "gbr", "gif", "jpeg", "jpg", "json", "kpp",
    "pcbrush", "png", "sut", "tif", "tiff", "webp",
];

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SUPPORTED.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn collect(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if is_supported(&path) {
            files.push(path);
        }
    }
}

fn render_smoke(brush: &Brush) -> usize {
    let mut doc = Document::new(512, 512);
    let dynamics = DabDynamics {
        pressure: 1.0,
        speed_norm: 0.5,
        angle: 0.3,
        taper: 1.0,
    };
    doc.stroke_to_dyn(
        brush,
        Color::rgb(0, 0, 0),
        Some((32.0, 256.0)),
        dynamics,
        (480.0, 256.0),
        dynamics,
        0.0,
    );
    doc.layers[0]
        .pixels
        .to_dense()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] != 0)
        .count()
}

#[test]
#[ignore = "set PROCRAFT_BRUSH_CORPUS to a directory of real brush files"]
fn real_brush_corpus_imports() {
    let root = env::var("PROCRAFT_BRUSH_CORPUS")
        .expect("set PROCRAFT_BRUSH_CORPUS to a directory of real brush files");
    let root = PathBuf::from(root);
    assert!(
        root.is_dir(),
        "corpus is not a directory: {}",
        root.display()
    );

    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no supported brush files under {}",
        root.display()
    );

    let mut failures = Vec::new();
    for path in &files {
        let display = path.strip_prefix(&root).unwrap_or(path).display();
        match import_brush_file_report(path) {
            Ok(outcome) if outcome.brushes.is_empty() => {
                failures.push(format!("{display}: imported no brushes"));
            }
            Ok(outcome) => {
                println!(
                    "OK {display}: {} brushes, {} skipped, {} warnings",
                    outcome.brushes.len(),
                    outcome.skipped,
                    outcome.warnings.len()
                );
                for warning in &outcome.warnings {
                    println!("  warning: {warning}");
                }
                let extension = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let authored = extension == "pcbrush" || extension == "json";
                for brush in &outcome.brushes {
                    if !authored {
                        assert!(
                            brush.brush.size <= 64.0,
                            "{} imported at {} px, above the 64 px default ceiling ({display})",
                            brush.name,
                            brush.brush.size
                        );
                    }
                    let tip = brush
                        .brush
                        .tip
                        .as_ref()
                        .map(|tip| format!("{}x{}", tip.w, tip.h))
                        .unwrap_or_else(|| "none".to_string());
                    let grain = brush
                        .brush
                        .grain_tip
                        .as_ref()
                        .map(|tip| format!("{}x{}", tip.w, tip.h))
                        .unwrap_or_else(|| "none".to_string());
                    let dual = brush
                        .brush
                        .dual_tip
                        .as_ref()
                        .map(|tip| format!("{}x{}@{:.2}", tip.w, tip.h, brush.brush.dual_scale))
                        .unwrap_or_else(|| "none".to_string());
                    let pressure_curve = brush
                        .brush
                        .pressure_curve
                        .as_ref()
                        .map(|curve| curve.len())
                        .unwrap_or(0);
                    let opacity_curve = brush
                        .brush
                        .pressure_opacity_curve
                        .as_ref()
                        .map(|curve| curve.len())
                        .unwrap_or(0);
                    let velocity_size_curve = brush
                        .brush
                        .velocity_size_curve
                        .as_ref()
                        .map(|curve| curve.len())
                        .unwrap_or(0);
                    let velocity_opacity_curve = brush
                        .brush
                        .velocity_opacity_curve
                        .as_ref()
                        .map(|curve| curve.len())
                        .unwrap_or(0);
                    println!(
                        "  brush: {} size={:.1} opacity={:.2} softness={:.2} spacing={:.2} tip={tip} grain={grain} grain_strength={:.2} grain_scale={:.2} grain_angle={:.0} angle={:.0} angle_scatter={:.2} wet_edge={:.2} dual={dual} pressure_curve={pressure_curve} opacity_curve={opacity_curve} velocity_size_curve={velocity_size_curve} velocity_opacity_curve={velocity_opacity_curve}",
                        brush.name,
                        brush.brush.size,
                        brush.brush.opacity,
                        brush.brush.softness,
                        brush.brush.spacing,
                        brush.brush.grain,
                        brush.brush.grain_scale,
                        brush.brush.grain_angle.to_degrees(),
                        brush.brush.angle.to_degrees(),
                        brush.brush.angle_scatter,
                        brush.brush.wet_edge
                    );
                    let painted = render_smoke(&brush.brush);
                    println!("    render_smoke painted={painted}");
                    assert!(painted > 0, "{} rendered no pixels ({display})", brush.name);
                }
            }
            Err(error) => {
                println!("FAIL {display}: {error}");
                failures.push(format!("{display}: {error}"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} corpus files failed:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
