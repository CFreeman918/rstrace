//! Perspective geometry: homographies, warping, line fitting, hulls.

use super::imgproc::Gray;

pub type Pt = [f64; 2];

/// Row-major 3x3 homography.
pub type Homography = [f64; 9];

/// Solve an n x n linear system with partial pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap())?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in 0..n {
            if r == c {
                continue;
            }
            let f = a[r][c] / a[c][c];
            if f == 0.0 {
                continue;
            }
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    Some((0..n).map(|i| b[i] / a[i][i]).collect())
}

/// Homography mapping src[i] -> dst[i].
pub fn homography(src: &[Pt; 4], dst: &[Pt; 4]) -> Option<Homography> {
    let mut a = Vec::with_capacity(8);
    let mut b = Vec::with_capacity(8);
    for i in 0..4 {
        let ([x, y], [u, v]) = (src[i], dst[i]);
        a.push(vec![x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y]);
        b.push(u);
        a.push(vec![0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y]);
        b.push(v);
    }
    let h = solve(a, b)?;
    Some([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0])
}

#[inline]
pub fn apply(h: &Homography, x: f64, y: f64) -> Pt {
    let d = h[6] * x + h[7] * y + h[8];
    [
        (h[0] * x + h[1] * y + h[2]) / d,
        (h[3] * x + h[4] * y + h[5]) / d,
    ]
}

/// Generic warp: `hinv` maps destination (pixel-corner coords) to source.
/// `channels` interleaved f32-convertible samples. 2x2 supersampling when
/// shrinking strongly.
fn warp_generic<F: Fn(usize) -> f32>(
    src: F,
    sw: usize,
    sh: usize,
    channels: usize,
    hinv: &Homography,
    dw: usize,
    dh: usize,
    mut put: impl FnMut(usize, &[f32]),
) {
    let a = apply(hinv, dw as f64 / 2.0, dh as f64 / 2.0);
    let b = apply(hinv, dw as f64 / 2.0 + 1.0, dh as f64 / 2.0);
    let ratio = (b[0] - a[0]).hypot(b[1] - a[1]);
    let offs: &[f64] = if ratio > 1.6 { &[-0.25, 0.25] } else { &[0.0] };
    let inv = 1.0 / (offs.len() * offs.len()) as f32;
    let mut acc = vec![0f32; channels];
    let (swf, shf) = ((sw - 1) as f64, (sh - 1) as f64);
    for y in 0..dh {
        for x in 0..dw {
            acc.iter_mut().for_each(|v| *v = 0.0);
            for &oy in offs {
                for &ox in offs {
                    let [sx, sy] = apply(hinv, x as f64 + 0.5 + ox, y as f64 + 0.5 + oy);
                    let sx = (sx - 0.5).clamp(0.0, swf);
                    let sy = (sy - 0.5).clamp(0.0, shf);
                    let (x0, y0) = (sx as usize, sy as usize);
                    let x1 = (x0 + 1).min(sw - 1);
                    let y1 = (y0 + 1).min(sh - 1);
                    let (tx, ty) = ((sx - x0 as f64) as f32, (sy - y0 as f64) as f32);
                    let (i00, i10) = ((y0 * sw + x0) * channels, (y0 * sw + x1) * channels);
                    let (i01, i11) = ((y1 * sw + x0) * channels, (y1 * sw + x1) * channels);
                    for c in 0..channels {
                        acc[c] += (src(i00 + c) * (1.0 - tx) + src(i10 + c) * tx) * (1.0 - ty)
                            + (src(i01 + c) * (1.0 - tx) + src(i11 + c) * tx) * ty;
                    }
                }
            }
            acc.iter_mut().for_each(|v| *v *= inv);
            put(y * dw + x, &acc);
        }
    }
}

pub fn warp_gray(src: &Gray, hinv: &Homography, dw: usize, dh: usize) -> Gray {
    let mut out = Gray::new(dw, dh);
    warp_generic(
        |i| src.data[i],
        src.width,
        src.height,
        1,
        hinv,
        dw,
        dh,
        |i, v| out.data[i] = v[0],
    );
    out
}

pub fn warp_rgba(
    src: &[u8],
    sw: usize,
    sh: usize,
    hinv: &Homography,
    dw: usize,
    dh: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    warp_generic(
        |i| src[i] as f32,
        sw,
        sh,
        4,
        hinv,
        dw,
        dh,
        |i, v| {
            for c in 0..4 {
                out[i * 4 + c] = v[c].round().clamp(0.0, 255.0) as u8;
            }
        },
    );
    out
}

/// Line as point + unit direction.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub p: Pt,
    pub d: Pt,
}

/// Total-least-squares fit.
pub fn fit_line(points: &[Pt]) -> Line {
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p[0]).sum::<f64>() / n;
    let my = points.iter().map(|p| p[1]).sum::<f64>() / n;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in points {
        let (dx, dy) = (p[0] - mx, p[1] - my);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    Line {
        p: [mx, my],
        d: [theta.cos(), theta.sin()],
    }
}

pub fn line_dist(l: &Line, p: Pt) -> f64 {
    ((p[0] - l.p[0]) * l.d[1] - (p[1] - l.p[1]) * l.d[0]).abs()
}

pub fn intersect(a: &Line, b: &Line) -> Option<Pt> {
    let det = a.d[0] * b.d[1] - a.d[1] * b.d[0];
    if det.abs() < 1e-9 {
        return None;
    }
    let t = ((b.p[0] - a.p[0]) * b.d[1] - (b.p[1] - a.p[1]) * b.d[0]) / det;
    Some([a.p[0] + t * a.d[0], a.p[1] + t * a.d[1]])
}

fn cross(o: Pt, a: Pt, b: Pt) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Andrew's monotone chain convex hull.
pub fn convex_hull(points: &[Pt]) -> Vec<Pt> {
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<Pt> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Pt> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

pub fn polygon_area(poly: &[Pt]) -> f64 {
    let n = poly.len();
    let mut a = 0.0;
    for i in 0..n {
        let j = (i + n - 1) % n;
        a += poly[j][0] * poly[i][1] - poly[i][0] * poly[j][1];
    }
    a / 2.0
}

/// Repeatedly drop the vertex contributing least (< tol px) to the outline.
fn simplify_closed(poly: &[Pt], tol: f64) -> Vec<Pt> {
    let mut pts = poly.to_vec();
    while pts.len() > 4 {
        let n = pts.len();
        let mut best = None;
        let mut best_d = tol;
        for i in 0..n {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let l = (c[0] - a[0]).hypot(c[1] - a[1]).max(1e-9);
            let d = cross(a, c, b).abs() / l;
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        match best {
            Some(i) => {
                pts.remove(i);
            }
            None => break,
        }
    }
    pts
}

/// Largest-area quadrilateral with vertices on a convex polygon.
pub fn max_area_quad(hull: &[Pt]) -> Option<[Pt; 4]> {
    let mut pts = hull.to_vec();
    let mut tol = 0.5;
    while pts.len() > 80 {
        pts = simplify_closed(hull, tol);
        tol *= 2.0;
    }
    let n = pts.len();
    if n < 4 {
        return None;
    }
    let tri = |a: Pt, b: Pt, c: Pt| cross(a, b, c).abs() / 2.0;
    let mut best = -1.0;
    let mut best_q = None;
    for i in 0..n {
        for k in i + 2..n {
            let (mut bj, mut bja) = (None, -1.0);
            for j in i + 1..k {
                let a = tri(pts[i], pts[j], pts[k]);
                if a > bja {
                    bja = a;
                    bj = Some(j);
                }
            }
            let (mut bl, mut bla) = (None, -1.0);
            for l in k + 1..n + i {
                let l = l % n;
                if l == i {
                    continue;
                }
                let a = tri(pts[k], pts[l], pts[i]);
                if a > bla {
                    bla = a;
                    bl = Some(l);
                }
            }
            if let (Some(j), Some(l)) = (bj, bl) {
                if bja + bla > best {
                    best = bja + bla;
                    best_q = Some([pts[i], pts[j], pts[k], pts[l]]);
                }
            }
        }
    }
    best_q
}

/// Order as [top-left, top-right, bottom-right, bottom-left] (y down).
pub fn order_corners(pts: &[Pt; 4]) -> [Pt; 4] {
    let cx = pts.iter().map(|p| p[0]).sum::<f64>() / 4.0;
    let cy = pts.iter().map(|p| p[1]).sum::<f64>() / 4.0;
    let mut s = *pts;
    s.sort_by(|a, b| {
        let aa = (a[1] - cy).atan2(a[0] - cx);
        let bb = (b[1] - cy).atan2(b[0] - cx);
        aa.partial_cmp(&bb).unwrap()
    });
    let start = (0..4)
        .min_by(|&i, &j| {
            (s[i][0] + s[i][1])
                .partial_cmp(&(s[j][0] + s[j][1]))
                .unwrap()
        })
        .unwrap();
    [
        s[start],
        s[(start + 1) % 4],
        s[(start + 2) % 4],
        s[(start + 3) % 4],
    ]
}

pub fn dist(a: Pt, b: Pt) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
