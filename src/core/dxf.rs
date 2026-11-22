//! DXF (R12 / AC1009, ASCII) output in millimetres.
//!
//! R12 is the most widely readable DXF flavour (CAD, CAM, laser and CNC
//! software alike). Curves are flattened into closed POLYLINEs to a chord
//! tolerance, which every importer handles consistently. Outer boundaries go
//! on layer OUTSIDE and holes on layer INSIDE.

use super::svg::SvgShape;
use super::trace::{flatten_tol, Segment};
use std::fmt::Write;

pub const LAYER_OUTSIDE: &str = "OUTSIDE";
pub const LAYER_INSIDE: &str = "INSIDE";

fn num(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

fn pair(out: &mut String, code: u16, value: impl std::fmt::Display) {
    let _ = writeln!(out, "{code:>3}\n{value}");
}

/// `view` = [x, y, width, height] in mm (same as the SVG export). DXF's Y
/// axis points up, so drawings are flipped to keep them the right way round,
/// with the view's bottom-left corner at the origin.
pub fn build_dxf(shapes: &[SvgShape], px_per_mm: f64, view: [f64; 4], tolerance_mm: f64) -> String {
    let k = 1.0 / px_per_mm;
    let tol_px = (tolerance_mm * px_per_mm).max(1e-3);
    let to_mm = |p: [f64; 2]| [p[0] * k - view[0], view[3] - (p[1] * k - view[1])];

    let mut ents = String::new();
    let mut sorted: Vec<&SvgShape> = shapes.iter().collect();
    sorted.sort_by_key(|s| s.depth);
    for s in sorted {
        let pts = polyline(s.segs, tol_px);
        if pts.len() < 3 {
            continue;
        }
        let layer = if s.hole { LAYER_INSIDE } else { LAYER_OUTSIDE };
        pair(&mut ents, 0, "POLYLINE");
        pair(&mut ents, 8, layer);
        pair(&mut ents, 66, 1);
        pair(&mut ents, 10, 0);
        pair(&mut ents, 20, 0);
        pair(&mut ents, 30, 0);
        pair(&mut ents, 70, 1); // closed
        for p in pts {
            let q = to_mm(p);
            pair(&mut ents, 0, "VERTEX");
            pair(&mut ents, 8, layer);
            pair(&mut ents, 10, num(q[0]));
            pair(&mut ents, 20, num(q[1]));
            pair(&mut ents, 30, 0);
        }
        pair(&mut ents, 0, "SEQEND");
        pair(&mut ents, 8, layer);
    }

    let mut out = String::new();
    // HEADER
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "HEADER");
    pair(&mut out, 9, "$ACADVER");
    pair(&mut out, 1, "AC1009");
    pair(&mut out, 9, "$INSUNITS");
    pair(&mut out, 70, 4); // millimetres
    pair(&mut out, 9, "$MEASUREMENT");
    pair(&mut out, 70, 1); // metric
    pair(&mut out, 9, "$EXTMIN");
    pair(&mut out, 10, 0);
    pair(&mut out, 20, 0);
    pair(&mut out, 30, 0);
    pair(&mut out, 9, "$EXTMAX");
    pair(&mut out, 10, num(view[2]));
    pair(&mut out, 20, num(view[3]));
    pair(&mut out, 30, 0);
    pair(&mut out, 0, "ENDSEC");
    // TABLES: line type + layers
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "TABLES");
    pair(&mut out, 0, "TABLE");
    pair(&mut out, 2, "LTYPE");
    pair(&mut out, 70, 1);
    pair(&mut out, 0, "LTYPE");
    pair(&mut out, 2, "CONTINUOUS");
    pair(&mut out, 70, 0);
    pair(&mut out, 3, "Solid line");
    pair(&mut out, 72, 65);
    pair(&mut out, 73, 0);
    pair(&mut out, 40, "0.0");
    pair(&mut out, 0, "ENDTAB");
    pair(&mut out, 0, "TABLE");
    pair(&mut out, 2, "LAYER");
    pair(&mut out, 70, 3);
    for (name, color) in [("0", 7), (LAYER_OUTSIDE, 7), (LAYER_INSIDE, 1)] {
        pair(&mut out, 0, "LAYER");
        pair(&mut out, 2, name);
        pair(&mut out, 70, 0);
        pair(&mut out, 62, color);
        pair(&mut out, 6, "CONTINUOUS");
    }
    pair(&mut out, 0, "ENDTAB");
    pair(&mut out, 0, "ENDSEC");
    // ENTITIES
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "ENTITIES");
    out.push_str(&ents);
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "EOF");
    out
}

/// Closed polyline (last point != first) through the segments.
fn polyline(segs: &[Segment], tol_px: f64) -> Vec<[f64; 2]> {
    let mut pts = flatten_tol(segs, tol_px);
    // Drop consecutive duplicates (and the closing point).
    pts.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-9);
    if pts.len() > 1 {
        let (f, l) = (pts[0], pts[pts.len() - 1]);
        if (f[0] - l[0]).hypot(f[1] - l[1]) < 1e-9 {
            pts.pop();
        }
    }
    pts
}
