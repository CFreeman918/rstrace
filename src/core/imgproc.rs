//! Raster helpers on plain buffers. Gray images are `f32` in 0..=255
//! (0 = black). Masks are `u8` with 1 = ink, 0 = background.

#[derive(Clone, Debug)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn new(width: usize, height: usize) -> Self {
        Gray { width, height, data: vec![0.0; width * height] }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Bilinear sample at continuous pixel-centre coordinates (clamped).
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        let (w, h) = (self.width as f64, self.height as f64);
        let x = x.clamp(0.0, w - 1.0);
        let y = y.clamp(0.0, h - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let (tx, ty) = (x - x0 as f64, y - y0 as f64);
        let g = |xx: usize, yy: usize| self.get(xx, yy) as f64;
        (g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx) * (1.0 - ty) + (g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx) * ty
    }
}

/// RGBA8 -> luma.
pub fn to_gray(rgba: &[u8], width: usize, height: usize) -> Gray {
    let data = rgba
        .chunks_exact(4)
        .map(|p| p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114)
        .collect();
    Gray { width, height, data }
}

/// Area-average downscale so the long edge is <= max_size. Returns (image, scale).
pub fn downscale(img: &Gray, max_size: usize) -> (Gray, f64) {
    let (w, h) = (img.width, img.height);
    let s = w.max(h) as f64 / max_size as f64;
    if s <= 1.0 {
        return (img.clone(), 1.0);
    }
    let nw = ((w as f64 / s).round() as usize).max(1);
    let nh = ((h as f64 / s).round() as usize).max(1);
    let (sx, sy) = (w as f64 / nw as f64, h as f64 / nh as f64);
    let mut out = Gray::new(nw, nh);
    for y in 0..nh {
        let y0 = (y as f64 * sy) as usize;
        let y1 = (((y + 1) as f64 * sy) as usize).max(y0 + 1).min(h);
        for x in 0..nw {
            let x0 = (x as f64 * sx) as usize;
            let x1 = (((x + 1) as f64 * sx) as usize).max(x0 + 1).min(w);
            let mut sum = 0.0f32;
            for yy in y0..y1 {
                let row = &img.data[yy * w + x0..yy * w + x1];
                sum += row.iter().sum::<f32>();
            }
            out.data[y * nw + x] = sum / ((x1 - x0) * (y1 - y0)) as f32;
        }
    }
    (out, w as f64 / nw as f64)
}

/// Summed-area table with a zero first row/column: (w+1)*(h+1).
fn integral<T: Copy + Into<f64>>(data: &[T], w: usize, h: usize) -> Vec<f64> {
    let ww = w + 1;
    let mut sat = vec![0.0f64; ww * (h + 1)];
    for y in 0..h {
        let mut row = 0.0;
        for x in 0..w {
            row += data[y * w + x].into();
            sat[(y + 1) * ww + x + 1] = sat[y * ww + x + 1] + row;
        }
    }
    sat
}

#[inline]
fn window(i: usize, r: usize, n: usize) -> (usize, usize) {
    (i.saturating_sub(r), (i + r + 1).min(n))
}

pub fn box_blur(img: &Gray, r: usize) -> Gray {
    if r == 0 {
        return img.clone();
    }
    let (w, h) = (img.width, img.height);
    let sat = integral(&img.data, w, h);
    let ww = w + 1;
    let mut out = Gray::new(w, h);
    for y in 0..h {
        let (y0, y1) = window(y, r, h);
        for x in 0..w {
            let (x0, x1) = window(x, r, w);
            let s = sat[y1 * ww + x1] - sat[y0 * ww + x1] - sat[y1 * ww + x0] + sat[y0 * ww + x0];
            out.data[y * w + x] = (s / ((x1 - x0) * (y1 - y0)) as f64) as f32;
        }
    }
    out
}

/// Otsu threshold. Values < threshold form the dark class.
pub fn otsu<I: IntoIterator<Item = f32>>(values: I) -> f32 {
    let mut hist = [0f64; 256];
    let mut total = 0.0;
    for v in values {
        hist[v.clamp(0.0, 255.0) as usize] += 1.0;
        total += 1.0;
    }
    let sum_all: f64 = (0..256).map(|i| i as f64 * hist[i]).sum();
    let (mut wb, mut sumb, mut best, mut thr) = (0.0, 0.0, -1.0, 128.0f32);
    for t in 0..256 {
        wb += hist[t];
        if wb == 0.0 {
            continue;
        }
        let wf = total - wb;
        if wf == 0.0 {
            break;
        }
        sumb += t as f64 * hist[t];
        let (mb, mf) = (sumb / wb, (sum_all - sumb) / wf);
        let between = wb * wf * (mb - mf) * (mb - mf);
        if between > best {
            best = between;
            thr = t as f32 + 1.0;
        }
    }
    thr
}

/// Divide out uneven lighting: estimate paper brightness per block (90th
/// percentile), fill blocks that are mostly ink from neighbours, upsample
/// bilinearly and normalise so paper ~ 255 everywhere.
pub fn flatten_lighting(img: &Gray, block: usize) -> Gray {
    let (w, h) = (img.width, img.height);
    let b = block.max(8);
    let (bw, bh) = (w.div_ceil(b), h.div_ceil(b));
    let mut bg = vec![0f32; bw * bh];
    let mut hist = [0u32; 256];
    for by in 0..bh {
        for bx in 0..bw {
            hist.fill(0);
            let (x0, y0) = (bx * b, by * b);
            let (x1, y1) = ((x0 + b).min(w), (y0 + b).min(h));
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    hist[img.data[y * w + x].clamp(0.0, 255.0) as usize] += 1;
                    n += 1;
                }
            }
            let target = (n as f32 * 0.9) as u32;
            let (mut acc, mut v) = (0u32, 255usize);
            for (i, c) in hist.iter().enumerate() {
                acc += c;
                if acc >= target.max(1) {
                    v = i;
                    break;
                }
            }
            bg[by * bw + bx] = v as f32;
        }
    }
    let mut sorted = bg.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2].max(1.0);
    let mut valid: Vec<bool> = bg.iter().map(|&v| v >= median * 0.65).collect();
    for _ in 0..(bw + bh) {
        let mut next = valid.clone();
        let mut changed = false;
        for by in 0..bh {
            for bx in 0..bw {
                let i = by * bw + bx;
                if valid[i] {
                    continue;
                }
                let (mut s, mut c) = (0.0, 0);
                for ny in by.saturating_sub(1)..(by + 2).min(bh) {
                    for nx in bx.saturating_sub(1)..(bx + 2).min(bw) {
                        let j = ny * bw + nx;
                        if valid[j] {
                            s += bg[j];
                            c += 1;
                        }
                    }
                }
                if c > 0 {
                    bg[i] = s / c as f32;
                    next[i] = true;
                    changed = true;
                }
            }
        }
        valid = next;
        if !changed {
            break;
        }
    }
    for (v, ok) in bg.iter_mut().zip(&valid) {
        if !ok {
            *v = median;
        }
    }
    let mut sm = vec![0f32; bg.len()];
    for by in 0..bh {
        for bx in 0..bw {
            let (mut s, mut c) = (0.0, 0);
            for ny in by.saturating_sub(1)..(by + 2).min(bh) {
                for nx in bx.saturating_sub(1)..(bx + 2).min(bw) {
                    s += bg[ny * bw + nx];
                    c += 1;
                }
            }
            sm[by * bw + bx] = s / c as f32;
        }
    }
    let mut out = Gray::new(w, h);
    let bf = b as f32;
    for y in 0..h {
        let fy = ((y as f32 + 0.5) / bf - 0.5).clamp(0.0, (bh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(bh - 1);
        let ty = fy - y0 as f32;
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / bf - 0.5).clamp(0.0, (bw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(bw - 1);
            let tx = fx - x0 as f32;
            let bgv = (sm[y0 * bw + x0] * (1.0 - tx) + sm[y0 * bw + x1] * tx) * (1.0 - ty)
                + (sm[y1 * bw + x0] * (1.0 - tx) + sm[y1 * bw + x1] * tx) * ty;
            out.data[y * w + x] = (img.data[y * w + x] / bgv.max(1.0) * 255.0).min(255.0);
        }
    }
    out
}

pub fn threshold(img: &Gray, t: f32, invert: bool) -> Vec<u8> {
    img.data
        .iter()
        .map(|&v| if invert { (v >= t) as u8 } else { (v < t) as u8 })
        .collect()
}

pub struct Labels {
    pub labels: Vec<i32>,
    pub areas: Vec<usize>,
    pub touches_border: Vec<bool>,
}

/// Connected components of pixels equal to `value`.
pub fn label(mask: &[u8], w: usize, h: usize, value: u8, conn8: bool) -> Labels {
    let mut labels = vec![-1i32; w * h];
    let mut areas = Vec::new();
    let mut touches_border = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..mask.len() {
        if mask[start] != value || labels[start] != -1 {
            continue;
        }
        let id = areas.len() as i32;
        let (mut area, mut border) = (0usize, false);
        labels[start] = id;
        stack.push(start);
        while let Some(p) = stack.pop() {
            area += 1;
            let (x, y) = (p % w, p / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                border = true;
            }
            for dy in -1i32..=1 {
                let ny = y as i32 + dy;
                if ny < 0 || ny >= h as i32 {
                    continue;
                }
                for dx in -1i32..=1 {
                    if (dx == 0 && dy == 0) || (!conn8 && dx != 0 && dy != 0) {
                        continue;
                    }
                    let nx = x as i32 + dx;
                    if nx < 0 || nx >= w as i32 {
                        continue;
                    }
                    let q = ny as usize * w + nx as usize;
                    if mask[q] == value && labels[q] == -1 {
                        labels[q] = id;
                        stack.push(q);
                    }
                }
            }
        }
        areas.push(area);
        touches_border.push(border);
    }
    Labels { labels, areas, touches_border }
}

/// Remove ink specks and fill background pinholes smaller than `min_area` px.
pub fn despeckle(mask: &[u8], w: usize, h: usize, min_area: usize) -> Vec<u8> {
    let mut out = mask.to_vec();
    if min_area == 0 {
        return out;
    }
    let ink = label(&out, w, h, 1, true);
    for i in 0..out.len() {
        if out[i] == 1 && ink.areas[ink.labels[i] as usize] < min_area {
            out[i] = 0;
        }
    }
    let bg = label(&out, w, h, 0, false);
    for i in 0..out.len() {
        if out[i] == 0 {
            let l = bg.labels[i] as usize;
            if !bg.touches_border[l] && bg.areas[l] < min_area {
                out[i] = 1;
            }
        }
    }
    out
}

/// Binary dilation (`dilate`) or erosion with a square of radius r.
pub fn morph(mask: &[u8], w: usize, h: usize, r: usize, dilate: bool) -> Vec<u8> {
    if r == 0 {
        return mask.to_vec();
    }
    let sat = integral(mask, w, h);
    let ww = w + 1;
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let (y0, y1) = window(y, r, h);
        for x in 0..w {
            let (x0, x1) = window(x, r, w);
            let s = sat[y1 * ww + x1] - sat[y0 * ww + x1] - sat[y1 * ww + x0] + sat[y0 * ww + x0];
            let full = ((x1 - x0) * (y1 - y0)) as f64;
            out[y * w + x] = if dilate { (s > 0.0) as u8 } else { (s >= full) as u8 };
        }
    }
    out
}

pub fn clear_border(mask: &mut [u8], w: usize, h: usize, px: usize) {
    if px == 0 {
        return;
    }
    for y in 0..h {
        for x in 0..w {
            if x < px || y < px || x + px >= w || y + px >= h {
                mask[y * w + x] = 0;
            }
        }
    }
}

/// Hysteresis threshold: pixels passing the `weak` threshold are kept only if
/// their 8-connected component contains a pixel passing the `strong` one.
/// Recovers faint parts of lines that connect to clearly dark ink.
pub fn hysteresis(img: &Gray, strong: f32, weak: f32, invert: bool) -> Vec<u8> {
    let (w, h) = (img.width, img.height);
    let weak_mask = threshold(img, weak, invert);
    let lab = label(&weak_mask, w, h, 1, true);
    let mut keep = vec![false; lab.areas.len()];
    for (i, &v) in img.data.iter().enumerate() {
        let is_strong = if invert { v >= strong } else { v < strong };
        if is_strong && lab.labels[i] >= 0 {
            keep[lab.labels[i] as usize] = true;
        }
    }
    lab.labels.iter().map(|&l| (l >= 0 && keep[l as usize]) as u8).collect()
}

/// Morphological closing (dilate then erode): bridges gaps up to ~2r px.
pub fn close(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    if r == 0 {
        return mask.to_vec();
    }
    morph(&morph(mask, w, h, r, true), w, h, r, false)
}

/// Chamfer (3-4) distance in pixels from every pixel to the nearest seed.
pub fn distance_to(seed: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e9;
    let mut d: Vec<f32> = seed.iter().map(|&s| if s { 0.0 } else { INF }).collect();
    let (a, b) = (1.0f32, 1.4f32);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 { v = v.min(d[i - 1] + a); }
            if y > 0 {
                v = v.min(d[i - w] + a);
                if x > 0 { v = v.min(d[i - w - 1] + b); }
                if x + 1 < w { v = v.min(d[i - w + 1] + b); }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w { v = v.min(d[i + 1] + a); }
            if y + 1 < h {
                v = v.min(d[i + w] + a);
                if x + 1 < w { v = v.min(d[i + w + 1] + b); }
                if x > 0 { v = v.min(d[i + w - 1] + b); }
            }
            d[i] = v;
        }
    }
    d
}
