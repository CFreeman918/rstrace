//! Regression test on a real phone photo of a Shaper Trace frame.
//!
//! `fixtures/trace_ruler_150mm.webp`: a 150 mm ruler outlined in pen inside
//! the frame, shot at a fairly oblique angle in portrait orientation. The far
//! (top) side of the outline is much fainter than the rest, and the frame's
//! opening has rounded corners.

use rstrace::core::geometry::Pt;
use rstrace::core::trace;
use rstrace::{FrameGeom, LineMode, Options, Pipeline, TRACE_CAPTURE_MM};
use std::path::Path;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/trace_ruler_150mm.webp");

/// Corners of the opening as found when this test was written (photo pixels).
const EXPECTED_CORNERS: [Pt; 4] = [[641.2, 786.9], [2092.7, 763.6], [2477.4, 2862.1], [466.3, 2984.6]];

/// (length, width) of a shape along its principal axes, in mm.
fn dims_mm(segs: &[trace::Segment], ppm: f64) -> (f64, f64) {
    let pts: Vec<Pt> = trace::flatten(segs, 16);
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p[0]).sum::<f64>() / n;
    let my = pts.iter().map(|p| p[1]).sum::<f64>() / n;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let (dx, dy) = (p[0] - mx, p[1] - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (ux, uy) = (th.cos(), th.sin());
    let along: Vec<f64> = pts.iter().map(|p| (p[0] - mx) * ux + (p[1] - my) * uy).collect();
    let across: Vec<f64> = pts.iter().map(|p| -(p[0] - mx) * uy + (p[1] - my) * ux).collect();
    let span = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
    (span(&along) / ppm, span(&across) / ppm)
}

#[test]
fn real_trace_photo_ruler_150mm() {
    let photo = rstrace::load::load_photo(Path::new(FIXTURE)).expect("fixture loads");
    assert_eq!((photo.width, photo.height), (3072, 4080));
    let mut pipe = Pipeline::new(photo.rgba, photo.width, photo.height);

    // Frame: found, and in the same place (within a few pixels on a 4080px photo).
    let det = pipe.detect().expect("frame detected");
    for (i, (c, e)) in det.corners.iter().zip(EXPECTED_CORNERS).enumerate() {
        let d = (c[0] - e[0]).hypot(c[1] - e[1]);
        assert!(d < 4.0, "corner {i} moved {d:.1}px: {c:?} vs {e:?}");
    }
    let frame = FrameGeom::oriented(det.corners, TRACE_CAPTURE_MM.0, TRACE_CAPTURE_MM.1);
    assert_eq!((frame.width_mm, frame.height_mm), (173.0, 263.0), "portrait photo -> portrait output");

    // Each mode must give exactly one closed path around the ruler.
    // Lengths include the small pen hooks at the corners; the pen line is
    // ~0.6 mm wide, so inside < 150 < centre < outside.
    let cases = [
        (LineMode::Inside, 149.0..150.3, 10.5..12.5),
        (LineMode::Centre, 150.0..151.3, 11.5..15.0),
        (LineMode::Outside, 150.5..153.5, 12.0..16.5),
    ];
    for (mode, len_range, width_range) in cases {
        let o = Options { line_mode: mode, ..Options::default() };
        let r = pipe.run(&frame, &o).expect("trace runs");
        assert_eq!(r.shapes.len(), 1, "{mode:?}: expected one closed outline, got {}", r.shapes.len());
        assert!(!r.shapes[0].contour.hole);
        let (len, width) = dims_mm(&r.shapes[0].segs, r.px_per_mm);
        assert!(len_range.contains(&len), "{mode:?}: length {len:.2} mm not in {len_range:?}");
        assert!(width_range.contains(&width), "{mode:?}: width {width:.2} mm not in {width_range:?}");
    }

    // Ink mode traces both edges of the pen line: outer path + hole.
    let o = Options { line_mode: LineMode::Ink, ..Options::default() };
    let r = pipe.run(&frame, &o).unwrap();
    let outers = r.shapes.iter().filter(|s| !s.contour.hole).count();
    let holes = r.shapes.iter().filter(|s| s.contour.hole).count();
    assert_eq!((outers, holes), (1, 1), "ink mode: both edges of one closed line");

    // Without faint-line recovery the far end breaks up (documents why it exists).
    let o = Options { catch_faint: false, close_gaps_mm: 0.0, line_mode: LineMode::Ink, ..Options::default() };
    let r = pipe.run(&frame, &o).unwrap();
    assert!(r.shapes.len() != 2 || r.shapes.iter().all(|s| !s.contour.hole), "plain threshold unexpectedly closed the outline");
}
