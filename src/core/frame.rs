//! Locate the Trace frame's inner opening in a photo.
//!
//! The paper inside the frame is a bright region completely enclosed by the
//! darker frame. After closing thin dark features (the drawing's lines), the
//! large bright connected region that does not touch the photo border is the
//! opening. Its convex hull is fitted with a quadrilateral, then each side is
//! refined to sub-pixel accuracy on the full-resolution image by finding the
//! dark-frame -> bright-paper edge along the side's normal.

use super::geometry::{self as g, Line, Pt};
use super::imgproc::{self as ip, Gray};

pub struct Detection {
    /// [TL, TR, BR, BL] in source pixel coordinates.
    pub corners: [Pt; 4],
    pub confidence: f64,
}

fn coarse_detect(small: &Gray) -> Option<([Pt; 4], f64)> {
    let (w, h) = (small.width, small.height);
    let blurred = ip::box_blur(small, 1);
    let t = ip::otsu(blurred.data.iter().copied());
    let bright: Vec<u8> = blurred.data.iter().map(|&v| (v >= t) as u8).collect();
    let r = ((w.min(h) as f64 * 0.008).round() as usize).max(2);
    let bright = ip::morph(&ip::morph(&bright, w, h, r, true), w, h, r, false);

    let lab = ip::label(&bright, w, h, 1, false);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    // Per-component row extents.
    let n = lab.areas.len();
    let mut rows: Vec<Vec<(usize, usize, usize)>> = vec![Vec::new(); n];
    for y in 0..h {
        let mut x = 0;
        while x < w {
            let l = lab.labels[y * w + x];
            if l < 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && lab.labels[y * w + x] == l {
                x += 1;
            }
            let rr = &mut rows[l as usize];
            match rr.last_mut() {
                Some(last) if last.0 == y => {
                    last.1 = last.1.min(start);
                    last.2 = last.2.max(x);
                }
                _ => rr.push((y, start, x)),
            }
        }
    }
    let mut best: Option<([Pt; 4], f64)> = None;
    for id in 0..n {
        let area = lab.areas[id];
        if (area as f64) < (w * h) as f64 * 0.04 || lab.touches_border[id] {
            continue;
        }
        let mut pts = Vec::with_capacity(rows[id].len() * 4);
        for &(y, x0, x1) in &rows[id] {
            let (y, x0, x1) = (y as f64, x0 as f64, x1 as f64);
            pts.extend_from_slice(&[[x0, y], [x1, y], [x0, y + 1.0], [x1, y + 1.0]]);
        }
        let hull = g::convex_hull(&pts);
        let Some(quad) = g::max_area_quad(&hull) else { continue };
        let qa = g::polygon_area(&quad).abs();
        let ha = g::polygon_area(&hull).abs();
        let fill = area as f64 / ha;
        let quadness = qa / ha;
        if quadness < 0.85 {
            continue;
        }
        let qx = quad.iter().map(|p| p[0]).sum::<f64>() / 4.0;
        let qy = quad.iter().map(|p| p[1]).sum::<f64>() / 4.0;
        let dc = (qx - cx).hypot(qy - cy) / (w as f64).hypot(h as f64);
        let score = (ha / (w * h) as f64) * quadness * (fill + 0.3).min(1.0) * (1.0 - dc);
        if best.as_ref().map_or(true, |b| score > b.1) {
            best = Some((quad, score));
        }
    }
    best
}

fn refine_side(img: &Gray, a: Pt, b: Pt, inward: Pt, search: f64) -> Option<Line> {
    let len = g::dist(a, b);
    let (ux, uy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
    let (mut nx, mut ny) = (-uy, ux);
    if nx * inward[0] + ny * inward[1] < 0.0 {
        nx = -nx;
        ny = -ny;
    }
    const N: usize = 80;
    let step = 0.5;
    let steps = (search / step).ceil() as i64;
    let mut pts = Vec::new();
    let mut prof = Vec::with_capacity((2 * steps + 1) as usize);
    for k in 0..N {
        let t = 0.1 + 0.8 * (k as f64 / (N - 1) as f64);
        let (px, py) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
        prof.clear();
        for s in -steps..=steps {
            let d = s as f64 * step;
            let mut v = 0.0;
            for o in -1..=1 {
                let o = o as f64;
                v += img.sample(px + nx * d + ux * o, py + ny * d + uy * o);
            }
            prof.push(v / 3.0);
        }
        let grad = |i: usize| if i >= 1 && i + 1 < prof.len() { prof[i + 1] - prof[i - 1] } else { 0.0 };
        let (mut bi, mut bg) = (0usize, 0.0);
        for i in 1..prof.len() - 1 {
            let gv = grad(i);
            if gv > bg {
                bg = gv;
                bi = i;
            }
        }
        if bi == 0 || bg < 6.0 {
            continue;
        }
        let (gm, gp) = (grad(bi - 1), grad(bi + 1));
        let denom = gm - 2.0 * bg + gp;
        let mut off = if denom < 0.0 { 0.5 * (gm - gp) / denom } else { 0.0 };
        if !off.is_finite() || off.abs() > 1.0 {
            off = 0.0;
        }
        let d = (bi as f64 - steps as f64 + off) * step;
        pts.push([px + nx * d, py + ny * d]);
    }
    if pts.len() < 10 {
        return None;
    }
    let mut line = g::fit_line(&pts);
    for _ in 0..3 {
        let ds: Vec<f64> = pts.iter().map(|&p| g::line_dist(&line, p)).collect();
        let mut sorted = ds.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let tol = (sorted[sorted.len() * 6 / 10] * 2.5).max(1.0);
        let keep: Vec<Pt> = pts.iter().zip(&ds).filter(|(_, &d)| d <= tol).map(|(p, _)| *p).collect();
        if keep.len() < 8 {
            break;
        }
        line = g::fit_line(&keep);
    }
    Some(line)
}

/// Detect the frame opening in a full-resolution gray image.
pub fn detect_frame(gray: &Gray) -> Option<Detection> {
    let (small, scale) = ip::downscale(gray, 700);
    let (quad, score) = coarse_detect(&small)?;
    let quad = quad.map(|p| [p[0] * scale, p[1] * scale]);
    let mut quad = g::order_corners(&quad);
    let cx = quad.iter().map(|p| p[0]).sum::<f64>() / 4.0;
    let cy = quad.iter().map(|p| p[1]).sum::<f64>() / 4.0;
    let search = (scale * 3.0).max(6.0);
    let lines: Vec<Option<Line>> = (0..4)
        .map(|i| {
            let (a, b) = (quad[i], quad[(i + 1) % 4]);
            let (mx, my) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0);
            refine_side(gray, a, b, [cx - mx, cy - my], search)
        })
        .collect();
    if lines.iter().all(Option::is_some) {
        let l: Vec<Line> = lines.into_iter().flatten().collect();
        let refined: Vec<Option<Pt>> = (0..4).map(|i| g::intersect(&l[(i + 3) % 4], &l[i])).collect();
        if refined.iter().enumerate().all(|(i, p)| p.is_some_and(|p| g::dist(p, quad[i]) < search * 2.0)) {
            for i in 0..4 {
                quad[i] = refined[i].unwrap();
            }
        }
    }
    Some(Detection { corners: quad, confidence: score })
}
