//! Real-scale SVG output (millimetres).

use super::trace::{SegKind, Segment};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum SvgStyle {
    /// Shaper Origin colour encoding: outer boundaries = exterior cut
    /// (black fill), holes = interior cut (white fill, black stroke).
    Shaper,
    /// One black even-odd filled path.
    Filled,
    /// Hairline outlines only.
    Outline,
}

pub struct SvgShape<'a> {
    pub segs: &'a [Segment],
    pub hole: bool,
    pub depth: usize,
}

fn fmt(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    let s = format!("{r:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Path data with pixel coords scaled by `s` (mm per px) and offset by `o` mm.
pub fn path_data(segs: &[Segment], s: f64, o: [f64; 2]) -> String {
    let mut d = String::new();
    let p = |pt: [f64; 2]| format!("{} {}", fmt(pt[0] * s + o[0]), fmt(pt[1] * s + o[1]));
    if let Some(first) = segs.first() {
        let _ = write!(d, "M{}", p(first.pts[0]));
    }
    for g in segs {
        match g.kind {
            SegKind::Line => {
                let _ = write!(d, "L{}", p(g.pts[3]));
            }
            SegKind::Cubic => {
                let _ = write!(d, "C{} {} {}", p(g.pts[1]), p(g.pts[2]), p(g.pts[3]));
            }
        }
    }
    if !d.is_empty() {
        d.push('Z');
    }
    d
}

/// `view` = [x, y, width, height] in mm of the region to export.
pub fn build_svg(shapes: &[SvgShape], px_per_mm: f64, view: [f64; 4], style: SvgStyle) -> String {
    let s = 1.0 / px_per_mm;
    let o = [-view[0], -view[1]];
    let (w, h) = (fmt(view[2]), fmt(view[3]));
    let mut sorted: Vec<&SvgShape> = shapes.iter().collect();
    sorted.sort_by_key(|c| c.depth);
    let mut body = String::new();
    match style {
        SvgStyle::Filled => {
            let d: String = sorted.iter().map(|c| path_data(c.segs, s, o)).collect();
            if !d.is_empty() {
                let _ = writeln!(body, r##"  <path fill="#000000" fill-rule="evenodd" stroke="none" d="{d}"/>"##);
            }
        }
        SvgStyle::Outline => {
            for c in &sorted {
                let _ = writeln!(
                    body,
                    r##"  <path fill="none" stroke="#000000" stroke-width="0.25" d="{}"/>"##,
                    path_data(c.segs, s, o)
                );
            }
        }
        SvgStyle::Shaper => {
            for c in &sorted {
                let d = path_data(c.segs, s, o);
                if c.hole {
                    let _ = writeln!(body, r##"  <path fill="#FFFFFF" stroke="#000000" stroke-width="0.25" d="{d}"/>"##);
                } else {
                    let _ = writeln!(body, r##"  <path fill="#000000" stroke="none" d="{d}"/>"##);
                }
            }
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n{body}</svg>\n"
    )
}
