//! Bitmap -> vector tracing.
//!
//! 1. `trace_contours`: follow pixel-edge boundaries of the ink mask (ink is
//!    8-connected, holes 4-connected). Outer boundaries come out clockwise on
//!    screen (positive area with y down), holes counter-clockwise.
//! 2. `fit_contour`: detect corners, Gaussian-smooth the staircase between
//!    corners, then fit cubic Béziers (Schneider's algorithm) to a tolerance.

use super::geometry::Pt;

#[derive(Clone, Debug)]
pub struct Contour {
    /// Closed polygon on the pixel-corner grid.
    pub points: Vec<[i32; 2]>,
    pub area: f64,
    pub hole: bool,
    /// [min_x, min_y, max_x, max_y]
    pub bbox: [i32; 4],
    /// A pixel centre strictly inside this contour's region.
    pub inside: Pt,
    /// Index of the enclosing contour, if any.
    pub parent: Option<usize>,
    pub depth: usize,
}

pub fn trace_contours(mask: &[u8], w: usize, h: usize) -> Vec<Contour> {
    let (wi, hi) = (w as i32, h as i32);
    let get = |x: i32, y: i32| x >= 0 && y >= 0 && x < wi && y < hi && mask[(y * wi + x) as usize] == 1;
    let pix = |px: i32, py: i32, vx: i32, vy: i32| get(px + if vx > 0 { 0 } else { -1 }, py + if vy > 0 { 0 } else { -1 });
    let mut visited = vec![false; w * h];
    let mut contours = Vec::new();
    for y in 0..hi {
        for x in 0..wi {
            let i = (y * wi + x) as usize;
            if mask[i] != 1 || visited[i] || get(x, y - 1) {
                continue;
            }
            let mut pts = Vec::new();
            let (mut px, mut py, mut dx, mut dy) = (x, y, 1i32, 0i32);
            let mut guard = 4 * (w + 1) * (h + 1);
            loop {
                pts.push([px, py]);
                if dx == 1 {
                    visited[(py * wi + px) as usize] = true;
                }
                px += dx;
                py += dy;
                let r = pix(px, py, dx - dy, dy + dx); // right-ahead
                let l = pix(px, py, dx + dy, dy - dx); // left-ahead
                (dx, dy) = if r && !l {
                    (dx, dy) // straight
                } else if l {
                    (dy, -dx) // turn left (diagonal ink joins: 8-connected)
                } else {
                    (-dy, dx) // turn right
                };
                guard -= 1;
                if (px == x && py == y && dx == 1 && dy == 0) || guard == 0 {
                    break;
                }
            }
            let n = pts.len();
            let mut area2 = 0i64;
            for k in 0..n {
                let j = (k + n - 1) % n;
                area2 += pts[j][0] as i64 * pts[k][1] as i64 - pts[k][0] as i64 * pts[j][1] as i64;
            }
            let hole = area2 < 0;
            let mut bbox = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for p in &pts {
                bbox[0] = bbox[0].min(p[0]);
                bbox[1] = bbox[1].min(p[1]);
                bbox[2] = bbox[2].max(p[0]);
                bbox[3] = bbox[3].max(p[1]);
            }
            let inside = if hole { [x as f64 + 0.5, y as f64 - 0.5] } else { [x as f64 + 0.5, y as f64 + 0.5] };
            contours.push(Contour {
                points: pts,
                area: area2.abs() as f64 / 2.0,
                hole,
                bbox,
                inside,
                parent: None,
                depth: 0,
            });
        }
    }
    assign_parents(&mut contours);
    contours
}

pub fn point_in_polygon(p: Pt, poly: &[[i32; 2]]) -> bool {
    let (x, y) = (p[0], p[1]);
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0] as f64, poly[i][1] as f64);
        let (xj, yj) = (poly[j][0] as f64, poly[j][1] as f64);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn assign_parents(cs: &mut [Contour]) {
    let mut order: Vec<usize> = (0..cs.len()).collect();
    order.sort_by(|&a, &b| cs[a].area.partial_cmp(&cs[b].area).unwrap());
    let parents: Vec<Option<usize>> = (0..cs.len())
        .map(|ci| {
            let c = &cs[ci];
            order.iter().copied().find(|&pi| {
                let p = &cs[pi];
                pi != ci
                    && p.hole != c.hole
                    && p.area > c.area
                    && c.bbox[0] >= p.bbox[0]
                    && c.bbox[1] >= p.bbox[1]
                    && c.bbox[2] <= p.bbox[2]
                    && c.bbox[3] <= p.bbox[3]
                    && point_in_polygon(c.inside, &p.points)
            })
        })
        .collect();
    for (c, p) in cs.iter_mut().zip(&parents) {
        c.parent = *p;
    }
    for i in 0..cs.len() {
        let (mut d, mut p) = (0, cs[i].parent);
        while let Some(pi) = p {
            d += 1;
            p = cs[pi].parent;
            if d > 10_000 {
                break;
            }
        }
        cs[i].depth = d;
    }
}

// ------------------------------------------------------------- smoothing

fn gauss_kernel(sigma: f64) -> Vec<f64> {
    let r = (sigma * 3.0).ceil().max(1.0) as i64;
    (-r..=r).map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp()).collect()
}

fn smooth_closed(pts: &[Pt], sigma: f64) -> Vec<Pt> {
    let n = pts.len();
    if sigma <= 0.0 || n < 5 {
        return pts.to_vec();
    }
    let k = gauss_kernel(sigma);
    let r = (k.len() / 2) as i64;
    (0..n as i64)
        .map(|i| {
            let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
            for j in -r..=r {
                let p = pts[((i + j).rem_euclid(n as i64)) as usize];
                let wgt = k[(j + r) as usize];
                sx += p[0] * wgt;
                sy += p[1] * wgt;
                sw += wgt;
            }
            [sx / sw, sy / sw]
        })
        .collect()
}

/// Smooth an open polyline, keeping the endpoints fixed.
fn smooth_open(pts: &[Pt], sigma: f64) -> Vec<Pt> {
    let n = pts.len();
    if sigma <= 0.0 || n < 3 {
        return pts.to_vec();
    }
    let k = gauss_kernel(sigma);
    let big_r = k.len() / 2;
    (0..n)
        .map(|i| {
            let r = big_r.min(i).min(n - 1 - i) as i64;
            let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
            for j in -r..=r {
                let p = pts[(i as i64 + j) as usize];
                let wgt = k[(j + big_r as i64) as usize];
                sx += p[0] * wgt;
                sy += p[1] * wgt;
                sw += wgt;
            }
            [sx / sw, sy / sw]
        })
        .collect()
}

fn find_corners(pts: &[Pt], k: usize, threshold_deg: f64) -> Vec<usize> {
    let n = pts.len();
    if threshold_deg >= 180.0 || n < 4 * k {
        return Vec::new();
    }
    let ang: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b, c) = (pts[(i + n - k) % n], pts[i], pts[(i + k) % n]);
            let (ux, uy, vx, vy) = (b[0] - a[0], b[1] - a[1], c[0] - b[0], c[1] - b[1]);
            let d = ux.hypot(uy) * vx.hypot(vy);
            if d > 0.0 { ((ux * vx + uy * vy) / d).clamp(-1.0, 1.0).acos().to_degrees() } else { 0.0 }
        })
        .collect();
    (0..n)
        .filter(|&i| {
            ang[i] >= threshold_deg
                && (1..=k).all(|j| ang[(i + j) % n] <= ang[i] && ang[(i + n - j) % n] < ang[i])
        })
        .collect()
}

// ------------------------------------------------ Schneider Bézier fitting

#[inline] fn sub(a: Pt, b: Pt) -> Pt { [a[0] - b[0], a[1] - b[1]] }
#[inline] fn add(a: Pt, b: Pt) -> Pt { [a[0] + b[0], a[1] + b[1]] }
#[inline] fn mul(a: Pt, s: f64) -> Pt { [a[0] * s, a[1] * s] }
#[inline] fn dot(a: Pt, b: Pt) -> f64 { a[0] * b[0] + a[1] * b[1] }
#[inline] fn len(a: Pt) -> f64 { a[0].hypot(a[1]) }
#[inline] fn norm(a: Pt) -> Pt { let l = len(a); if l > 0.0 { [a[0] / l, a[1] / l] } else { [1.0, 0.0] } }

pub type Cubic = [Pt; 4];

fn bez(b: &Cubic, t: f64) -> Pt {
    let mt = 1.0 - t;
    let (a, c, d, e) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
    [
        a * b[0][0] + c * b[1][0] + d * b[2][0] + e * b[3][0],
        a * b[0][1] + c * b[1][1] + d * b[2][1] + e * b[3][1],
    ]
}

fn bez_d1(b: &Cubic, t: f64) -> Pt {
    let mt = 1.0 - t;
    add(add(mul(sub(b[1], b[0]), 3.0 * mt * mt), mul(sub(b[2], b[1]), 6.0 * mt * t)), mul(sub(b[3], b[2]), 3.0 * t * t))
}

fn bez_d2(b: &Cubic, t: f64) -> Pt {
    let a = add(sub(b[2], mul(b[1], 2.0)), b[0]);
    let c = add(sub(b[3], mul(b[2], 2.0)), b[1]);
    add(mul(a, 6.0 * (1.0 - t)), mul(c, 6.0 * t))
}

fn chord_params(d: &[Pt]) -> Vec<f64> {
    let mut u = vec![0.0];
    for i in 1..d.len() {
        u.push(u[i - 1] + len(sub(d[i], d[i - 1])));
    }
    let total = u.last().copied().unwrap_or(1.0).max(1e-12);
    u.iter().map(|v| v / total).collect()
}

fn generate_bezier(d: &[Pt], u: &[f64], t1: Pt, t2: Pt) -> Cubic {
    let (p0, p3) = (d[0], d[d.len() - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &t) in u.iter().enumerate() {
        let mt = 1.0 - t;
        let (b0, b1, b2, b3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
        let (a1, a2) = (mul(t1, b1), mul(t2, b2));
        c00 += dot(a1, a1);
        c01 += dot(a1, a2);
        c11 += dot(a2, a2);
        let tmp = sub(d[i], add(mul(p0, b0 + b1), mul(p3, b2 + b3)));
        x0 += dot(a1, tmp);
        x1 += dot(a2, tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let (mut al, mut ar) = (0.0, 0.0);
    if det.abs() > 1e-12 {
        al = (x0 * c11 - c01 * x1) / det;
        ar = (c00 * x1 - c01 * x0) / det;
    }
    let seg = len(sub(p3, p0));
    let eps = 1e-6 * seg;
    if al < eps || ar < eps || al > seg * 3.0 || ar > seg * 3.0 {
        al = seg / 3.0;
        ar = al;
    }
    [p0, add(p0, mul(t1, al)), add(p3, mul(t2, ar)), p3]
}

fn reparameterize(d: &[Pt], b: &Cubic, u: &[f64]) -> Vec<f64> {
    u.iter()
        .zip(d)
        .map(|(&t, &p)| {
            let diff = sub(bez(b, t), p);
            let (q1, q2) = (bez_d1(b, t), bez_d2(b, t));
            let den = dot(q1, q1) + dot(diff, q2);
            if den.abs() < 1e-12 { t } else { (t - dot(diff, q1) / den).clamp(0.0, 1.0) }
        })
        .collect()
}

fn max_error(d: &[Pt], b: &Cubic, u: &[f64]) -> (f64, usize) {
    let (mut max, mut split) = (0.0, d.len() / 2);
    for i in 1..d.len() - 1 {
        let v = sub(bez(b, u[i]), d[i]);
        let e = dot(v, v);
        if e >= max {
            max = e;
            split = i;
        }
    }
    (max, split)
}

fn fit_cubic(d: &[Pt], t1: Pt, t2: Pt, err: f64, out: &mut Vec<Cubic>, depth: u32) {
    let n = d.len();
    if n <= 2 || depth > 40 {
        let dist = len(sub(d[n - 1], d[0])) / 3.0;
        out.push([d[0], add(d[0], mul(t1, dist)), add(d[n - 1], mul(t2, dist)), d[n - 1]]);
        return;
    }
    let mut u = chord_params(d);
    let mut b = generate_bezier(d, &u, t1, t2);
    let (mut max, mut split) = max_error(d, &b, &u);
    let err2 = err * err;
    if max < err2 {
        out.push(b);
        return;
    }
    if max < err2 * 16.0 {
        for _ in 0..12 {
            u = reparameterize(d, &b, &u);
            b = generate_bezier(d, &u, t1, t2);
            (max, split) = max_error(d, &b, &u);
            if max < err2 {
                out.push(b);
                return;
            }
        }
    }
    let split = split.clamp(1, n - 2);
    let tc = norm(sub(d[split - 1], d[split + 1]));
    fit_cubic(&d[..=split], t1, tc, err, out, depth + 1);
    fit_cubic(&d[split..], mul(tc, -1.0), t2, err, out, depth + 1);
}

fn end_tangent(d: &[Pt], from_start: bool, count: usize) -> Pt {
    let n = d.len();
    let at = |i: usize| if from_start { d[i] } else { d[n - 1 - i] };
    let v = sub(at(count.min(n - 1)), at(0));
    if len(v) > 0.0 { norm(v) } else { norm(sub(at(n - 1), at(0))) }
}

/// Sharpen a detected corner: intersect lines fitted to the points just
/// before and just after it. Falls back to the original point.
fn snap_corner(pts: &[Pt], c: usize, before: usize, after: usize) -> Pt {
    let n = pts.len();
    let m1 = before.min(12);
    let m2 = after.min(12);
    if m1 < 3 || m2 < 3 {
        return pts[c];
    }
    let a: Vec<Pt> = (1..=m1).map(|i| pts[(c + n - i) % n]).collect();
    let b: Vec<Pt> = (1..=m2).map(|i| pts[(c + i) % n]).collect();
    let (la, lb) = (super::geometry::fit_line(&a), super::geometry::fit_line(&b));
    match super::geometry::intersect(&la, &lb) {
        Some(p) if super::geometry::dist(p, pts[c]) < 2.5 => p,
        _ => pts[c],
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SegKind {
    Line,
    Cubic,
}

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub kind: SegKind,
    pub pts: Cubic,
}

fn classify(b: Cubic, tol: f64) -> Segment {
    let chord = sub(b[3], b[0]);
    let l = len(chord);
    if l < 1e-9 {
        return Segment { kind: SegKind::Line, pts: b };
    }
    let dist = |p: Pt| ((p[0] - b[0][0]) * chord[1] - (p[1] - b[0][1]) * chord[0]).abs() / l;
    let proj = |p: Pt| dot(sub(p, b[0]), chord) / (l * l);
    let straight = dist(b[1]) < tol
        && dist(b[2]) < tol
        && (-0.01..1.01).contains(&proj(b[1]))
        && (-0.01..1.01).contains(&proj(b[2]));
    Segment { kind: if straight { SegKind::Line } else { SegKind::Cubic }, pts: b }
}

#[derive(Clone, Copy, Debug)]
pub struct FitOptions {
    /// Gaussian sigma in px.
    pub smoothing: f64,
    /// Max curve deviation in px.
    pub tolerance: f64,
    /// Turn angle (degrees) that counts as a sharp corner; >= 180 disables.
    pub corner_angle: f64,
}

/// Fit a closed contour with lines and cubic Béziers (pixel units).
pub fn fit_contour(points: &[[i32; 2]], o: &FitOptions) -> Vec<Segment> {
    let tol = o.tolerance.max(0.05);
    let n0 = points.len();
    // Pixel-edge midpoints: removes the worst of the staircase bias.
    let pts: Vec<Pt> = (0..n0)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n0]);
            [(a[0] + b[0]) as f64 / 2.0, (a[1] + b[1]) as f64 / 2.0]
        })
        .collect();
    let n = pts.len();
    if n < 8 {
        return (0..n)
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % n]);
                Segment { kind: SegKind::Line, pts: [p, p, q, q] }
            })
            .collect();
    }
    let k = ((2.0 * o.smoothing + 2.0).round() as usize).max(3);
    let corners = find_corners(&smooth_closed(&pts, 0.7), k, o.corner_angle);
    let mut segs: Vec<Cubic> = Vec::new();
    if corners.is_empty() {
        let mut sm = smooth_closed(&pts, o.smoothing);
        sm.push(sm[0]);
        let m = sm.len();
        let t = norm(sub(sm[1], sm[m - 2]));
        let mid = m / 2;
        let tm = norm(sub(sm[mid + 1], sm[mid - 1]));
        fit_cubic(&sm[..=mid], t, mul(tm, -1.0), tol, &mut segs, 0);
        fit_cubic(&sm[mid..], tm, mul(t, -1.0), tol, &mut segs, 0);
    } else {
        let nc = corners.len();
        let gap = |a: usize, b: usize| if b > a { b - a } else { b + n - a };
        let snapped: Vec<Pt> = (0..nc)
            .map(|c| {
                let before = gap(corners[(c + nc - 1) % nc], corners[c]) / 2;
                let after = gap(corners[c], corners[(c + 1) % nc]) / 2;
                snap_corner(&pts, corners[c], before.saturating_sub(1), after.saturating_sub(1))
            })
            .collect();
        for c in 0..nc {
            let (s, e) = (corners[c], corners[(c + 1) % nc]);
            let span = gap(s, e);
            let mut run: Vec<Pt> = (0..=span).map(|i| pts[(s + i) % n]).collect();
            run[0] = snapped[c];
            run[span] = snapped[(c + 1) % nc];
            let sm = smooth_open(&run, o.smoothing);
            let tk = (sm.len() / 3).clamp(2, (k * 2).max(4));
            let t1 = end_tangent(&sm, true, tk);
            let t2 = end_tangent(&sm, false, tk);
            fit_cubic(&sm, t1, t2, tol, &mut segs, 0);
        }
    }
    segs.into_iter().map(|b| classify(b, tol * 0.5)).collect()
}

/// Flatten segments into a polyline (for display / hit testing).
pub fn flatten(segs: &[Segment], steps: usize) -> Vec<Pt> {
    let mut out = Vec::new();
    for s in segs {
        match s.kind {
            SegKind::Line => out.push(s.pts[0]),
            SegKind::Cubic => {
                for i in 0..steps {
                    out.push(bez(&s.pts, i as f64 / steps as f64));
                }
            }
        }
    }
    out
}
