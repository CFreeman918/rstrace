#![allow(clippy::manual_range_contains)]
//! End-to-end tests on a synthetic, perspective-distorted photo of a frame
//! with known shapes, plus unit checks of the tracer.

use rstrace::core::geometry::{self as g, Pt};
use rstrace::core::trace::{self, FitOptions};
use rstrace::{FrameGeom, Options, Pipeline, SvgStyle};

const FW: f64 = 263.0;
const FH: f64 = 173.0;
const BORDER: f64 = 37.0;

fn in_tri(p: Pt, t: [Pt; 3]) -> bool {
    let s = |a: Pt, b: Pt, c: Pt| (a[0] - c[0]) * (b[1] - c[1]) - (b[0] - c[0]) * (a[1] - c[1]);
    let (d1, d2, d3) = (s(p, t[0], t[1]), s(p, t[1], t[2]), s(p, t[2], t[0]));
    !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
}

/// Brightness of the world at (x, y) mm (frame-opening coordinates).
fn world(x: f64, y: f64) -> f64 {
    let in_opening = (0.0..=FW).contains(&x) && (0.0..=FH).contains(&y);
    if !in_opening {
        let in_frame = (-BORDER..=FW + BORDER).contains(&x) && (-BORDER..=FH + BORDER).contains(&y);
        if in_frame {
            // Dark frame with light reference dots, like the real thing.
            let (u, v) = ((x + BORDER).rem_euclid(20.0), (y + BORDER).rem_euclid(20.0));
            if (y < -BORDER / 2.0 || y > FH + BORDER / 2.0) && (u - 10.0).hypot(v - 10.0) < 3.0 {
                return 220.0;
            }
            return 35.0;
        }
        return 200.0; // table / paper outside
    }
    // Circle r=30 at (60,60)
    if (x - 60.0).hypot(y - 60.0) <= 30.0 {
        return 25.0;
    }
    // Rectangle 70x50 at (130,40) with a 30x20 hole at (150,55)
    if (130.0..=200.0).contains(&x) && (40.0..=90.0).contains(&y)
        && !((150.0..=180.0).contains(&x) && (55.0..=75.0).contains(&y))
    {
        return 25.0;
    }
    if in_tri([x, y], [[40.0, 160.0], [110.0, 160.0], [75.0, 110.0]]) {
        return 25.0;
    }
    // A thin pencil-ish line, which must survive as a shape too.
    if (215.0..=250.0).contains(&x) && (y - 140.0).abs() < 0.6 {
        return 60.0;
    }
    235.0
}

fn render(w: usize, h: usize, corners: [Pt; 4]) -> Vec<u8> {
    let hm = g::homography(&corners, &[[0.0, 0.0], [FW, 0.0], [FW, FH], [0.0, FH]]).unwrap();
    let mut rgba = vec![0u8; w * h * 4];
    let mut s: u64 = 12345;
    for y in 0..h {
        for x in 0..w {
            let mut v = 0.0;
            for oy in [0.25, 0.75] {
                for ox in [0.25, 0.75] {
                    let p = g::apply(&hm, x as f64 + ox, y as f64 + oy);
                    v += world(p[0], p[1]);
                }
            }
            v /= 4.0;
            let light = 0.7 + 0.3 * (x as f64 / w as f64) * (1.0 - 0.4 * y as f64 / h as f64);
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let noise = ((s >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 12.0;
            let v = (v * light + noise).clamp(0.0, 255.0);
            let i = (y * w + x) * 4;
            rgba[i] = v as u8;
            rgba[i + 1] = (v * 0.98) as u8;
            rgba[i + 2] = (v * 0.95) as u8;
            rgba[i + 3] = 255;
        }
    }
    rgba
}

const PW: usize = 1400;
const PH: usize = 1050;
const TRUE: [Pt; 4] = [[260.0, 230.0], [1180.0, 280.0], [1130.0, 860.0], [220.0, 800.0]];

fn bbox_mm(segs: &[trace::Segment], ppm: f64) -> [f64; 4] {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in trace::flatten(segs, 32) {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b.map(|v| v / ppm)
}

#[test]
fn detects_frame_and_traces_at_real_scale() {
    let photo = render(PW, PH, TRUE);
    let mut pipe = Pipeline::new(photo, PW, PH);
    let det = pipe.detect().expect("frame found");
    for (i, c) in det.corners.iter().enumerate() {
        let d = g::dist(*c, TRUE[i]);
        assert!(d < 1.5, "corner {i} off by {d:.2}px: {c:?} vs {:?}", TRUE[i]);
    }

    let frame = FrameGeom::oriented(det.corners, FW, FH);
    assert_eq!((frame.width_mm, frame.height_mm), (FW, FH));
    let o = Options::default();
    let r = pipe.run(&frame, &o).unwrap();
    let ppm = r.px_per_mm;
    let outers: Vec<_> = r.shapes.iter().filter(|s| !s.contour.hole).collect();
    let holes: Vec<_> = r.shapes.iter().filter(|s| s.contour.hole).collect();
    assert_eq!(outers.len(), 4, "outer shapes");
    assert_eq!(holes.len(), 1, "holes");

    // Photo resolution is ~3.5 px/mm, so 0.6 mm is well under 2 photo pixels.
    let tol = 0.6;
    let near = |a: f64, b: f64| (a - b).abs() < tol;
    let circle = outers.iter().map(|s| bbox_mm(&s.segs, ppm)).find(|b| b[0] < 40.0 && b[1] < 40.0).unwrap();
    assert!(near(circle[0], 30.0) && near(circle[2], 90.0) && near(circle[1], 30.0) && near(circle[3], 90.0), "circle {circle:?}");
    let rect = outers.iter().map(|s| bbox_mm(&s.segs, ppm)).find(|b| b[0] > 120.0 && b[1] < 50.0).unwrap();
    assert!(near(rect[0], 130.0) && near(rect[2], 200.0) && near(rect[1], 40.0) && near(rect[3], 90.0), "rect {rect:?}");
    let hole = bbox_mm(&holes[0].segs, ppm);
    assert!(near(hole[0], 150.0) && near(hole[2], 180.0) && near(hole[1], 55.0) && near(hole[3], 75.0), "hole {hole:?}");
    // Hole belongs to the rectangle.
    let parent = holes[0].contour.parent.expect("hole has parent");
    assert!(bbox_mm(&r.shapes[parent].segs, ppm)[0] > 120.0);

    // SVG: real size and Shaper encoding.
    let svg = r.to_svg(&frame, &o, &[]);
    assert!(svg.contains(r#"width="263mm" height="173mm""#));
    assert_eq!(svg.matches("<path").count(), 5);
    assert_eq!(svg.matches(r##"fill="#FFFFFF""##).count(), 1);

    // Excluding the rectangle (click inside it) drops it and its hole.
    let click = [135.0 * ppm, 45.0 * ppm];
    let ex = r.excluded(&[click]);
    assert_eq!(ex.iter().filter(|&&e| e).count(), 2);
    let svg = r.to_svg(&frame, &o, &ex);
    assert_eq!(svg.matches("<path").count(), 3);

    // Rotated output swaps the dimensions.
    let rot = frame.rotated();
    assert_eq!((rot.width_mm, rot.height_mm), (FH, FW));
    let o2 = Options { style: SvgStyle::Filled, crop_to_content: true, ..Options::default() };
    let r2 = pipe.run(&rot, &o2).unwrap();
    let svg2 = r2.to_svg(&rot, &o2, &[]);
    assert_eq!(svg2.matches("<path").count(), 1);
}

#[test]
fn square_keeps_sharp_corners() {
    let (w, h) = (120, 120);
    let mut m = vec![0u8; w * h];
    for y in 20..100 {
        for x in 20..100 {
            m[y * w + x] = 1;
        }
    }
    let cs = trace::trace_contours(&m, w, h);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].area, 6400.0);
    assert!(!cs[0].hole);
    let segs = trace::fit_contour(&cs[0].points, &FitOptions { smoothing: 2.0, tolerance: 0.8, corner_angle: 60.0 });
    assert!((4..=8).contains(&segs.len()), "segments: {}", segs.len());
    assert!(segs.iter().all(|s| s.kind == trace::SegKind::Line));
}

#[test]
fn hole_and_diagonal_connectivity() {
    let (w, h) = (10, 10);
    let mut m = vec![0u8; w * h];
    for y in 2..8 {
        for x in 2..8 {
            m[y * w + x] = 1;
        }
    }
    for y in 4..6 {
        for x in 4..6 {
            m[y * w + x] = 0;
        }
    }
    let cs = trace::trace_contours(&m, w, h);
    assert_eq!(cs.len(), 2);
    let hole = cs.iter().find(|c| c.hole).unwrap();
    assert_eq!(hole.area, 4.0);
    assert_eq!(hole.depth, 1);

    let mut d = vec![0u8; 16];
    d[5] = 1;
    d[10] = 1;
    let cs = trace::trace_contours(&d, 4, 4);
    assert_eq!(cs.len(), 1, "diagonal pixels are one 8-connected shape");
}

/// `cargo test -- --ignored write_sample_photo` writes target/sample_photo.png
/// for trying the app without a real frame.
#[test]
#[ignore]
fn write_sample_photo() {
    let photo = render(PW, PH, TRUE);
    image::save_buffer("target/sample_photo.png", &photo, PW as u32, PH as u32, image::ExtendedColorType::Rgba8).unwrap();
}

fn ring_mask(w: usize, h: usize, cx: f64, cy: f64, r: f64, width: f64, m: &mut [u8]) {
    for y in 0..h {
        for x in 0..w {
            let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy);
            if (d - r).abs() <= width / 2.0 {
                m[y * w + x] = 1;
            }
        }
    }
}

#[test]
fn line_modes_convert_only_thin_closed_loops() {
    use rstrace::core::pipeline::apply_line_mode;
    use rstrace::LineMode;
    let (w, h) = (200, 200);
    // Washer drawn as two pen circles, plus a filled disk and an open stroke.
    let mut m = vec![0u8; w * h];
    ring_mask(w, h, 70.0, 70.0, 50.0, 4.0, &mut m);
    ring_mask(w, h, 70.0, 70.0, 20.0, 4.0, &mut m);
    for y in 0..h {
        for x in 0..w {
            if (x as f64 - 160.0).hypot(y as f64 - 160.0) < 25.0 {
                m[y * w + x] = 1; // filled disk
            }
        }
    }
    for x in 20..120 {
        for y in 170..173 {
            m[y * w + x] = 1; // open line
        }
    }
    let ink = trace::trace_contours(&m, w, h);
    assert_eq!(ink.len(), 2 * 2 + 1 + 1, "two rings (4 edges), disk, line");

    let area = |cs: &[trace::Contour], hole: bool| cs.iter().filter(|c| c.hole == hole).map(|c| c.area).sum::<f64>();
    for mode in [LineMode::Centre, LineMode::Inside, LineMode::Outside] {
        let out = apply_line_mode(&m, w, h, mode, 8.0);
        let cs = trace::trace_contours(&out, w, h);
        let outers = cs.iter().filter(|c| !c.hole).count();
        let holes = cs.iter().filter(|c| c.hole).count();
        assert_eq!((outers, holes), (3, 1), "{mode:?}: washer (outer+hole), disk, open line");
        let washer = std::f64::consts::PI * (50f64.powi(2) - 20f64.powi(2));
        let a = area(&cs, false) - area(&cs, true);
        assert!(a > washer, "{mode:?}: filled washer present");
    }
    // Centre path of the outer circle sits on the drawn radius.
    let out = apply_line_mode(&m, w, h, LineMode::Centre, 8.0);
    let cs = trace::trace_contours(&out, w, h);
    let big = cs.iter().filter(|c| !c.hole).max_by(|a, b| a.area.partial_cmp(&b.area).unwrap()).unwrap();
    let r = (big.area / std::f64::consts::PI).sqrt();
    assert!((r - 50.0).abs() < 1.0, "centre radius {r}");
}
