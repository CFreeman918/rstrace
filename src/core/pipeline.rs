//! Photo -> rectified image -> ink mask -> vector shapes, with per-stage
//! caching so that changing a later setting only recomputes what depends on it.

use super::frame;
use super::geometry::{self as g, Pt};
use super::imgproc::{self as ip, Gray};
use super::svg::{self, SvgShape, SvgStyle};
use super::trace::{self, Contour, FitOptions, Segment};
use serde::{Deserialize, Serialize};

/// Shaper Trace capture area (mm), from Shaper's published specs.
pub const TRACE_CAPTURE_MM: (f64, f64) = (263.0, 173.0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Processing resolution.
    pub px_per_mm: f64,
    /// Ignore this much next to the frame edge (shadows, frame lip).
    pub inset_mm: f64,
    /// Even out uneven lighting / shadows before thresholding.
    pub flatten: bool,
    pub auto_threshold: bool,
    /// 0..255 on the (flattened) image; used when `auto_threshold` is false.
    pub threshold: f32,
    /// Trace light marks on a dark background.
    pub invert: bool,
    /// Remove specks / fill pinholes smaller than this.
    pub despeckle_mm2: f64,
    /// Gaussian smoothing of outlines.
    pub smoothing_mm: f64,
    /// Max deviation of the fitted curves.
    pub tolerance_mm: f64,
    /// Degrees; 180 disables sharp-corner detection.
    pub corner_angle: f64,
    /// Drop shapes smaller than this.
    pub min_area_mm2: f64,
    pub style: SvgStyle,
    /// Size the SVG to the traced shapes instead of the whole frame.
    pub crop_to_content: bool,
    pub crop_margin_mm: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            px_per_mm: 8.0,
            inset_mm: 2.0,
            flatten: true,
            auto_threshold: true,
            threshold: 150.0,
            invert: false,
            despeckle_mm2: 1.5,
            smoothing_mm: 0.25,
            tolerance_mm: 0.1,
            corner_angle: 60.0,
            min_area_mm2: 2.0,
            style: SvgStyle::Shaper,
            crop_to_content: false,
            crop_margin_mm: 0.0,
        }
    }
}

/// Where the frame opening is in the photo and how big it really is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameGeom {
    /// [TL, TR, BR, BL] in source pixels; TL->TR becomes the SVG's top edge.
    pub corners: [Pt; 4],
    pub width_mm: f64,
    pub height_mm: f64,
}

impl FrameGeom {
    /// Pick width/height from the frame's long/short sides by matching the
    /// orientation of the quad in the photo.
    pub fn oriented(corners: [Pt; 4], long_mm: f64, short_mm: f64) -> Self {
        let horiz = g::dist(corners[0], corners[1]) + g::dist(corners[3], corners[2]);
        let vert = g::dist(corners[1], corners[2]) + g::dist(corners[0], corners[3]);
        let (l, s) = (long_mm.max(short_mm), long_mm.min(short_mm));
        let (width_mm, height_mm) = if horiz >= vert { (l, s) } else { (s, l) };
        FrameGeom { corners, width_mm, height_mm }
    }

    /// Rotate the output by 90° clockwise.
    pub fn rotated(&self) -> Self {
        let c = self.corners;
        FrameGeom { corners: [c[3], c[0], c[1], c[2]], width_mm: self.height_mm, height_mm: self.width_mm }
    }
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub contour: Contour,
    pub segs: Vec<Segment>,
}

#[derive(Clone, Debug)]
pub struct TraceResult {
    pub width: usize,
    pub height: usize,
    pub px_per_mm: f64,
    pub threshold: f32,
    pub mask: Vec<u8>,
    /// Kept shapes; `contour.parent` indexes into this list.
    pub shapes: Vec<Shape>,
}

impl TraceResult {
    /// Index of the innermost shape containing `p` (pixels).
    pub fn shape_at(&self, p: Pt) -> Option<usize> {
        self.shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let b = s.contour.bbox;
                p[0] >= b[0] as f64 && p[1] >= b[1] as f64 && p[0] <= b[2] as f64 && p[1] <= b[3] as f64
                    && trace::point_in_polygon(p, &s.contour.points)
            })
            .min_by(|a, b| a.1.contour.area.partial_cmp(&b.1.contour.area).unwrap())
            .map(|(i, _)| i)
    }

    /// Which shapes are excluded, given points (pixels) the user clicked to
    /// exclude. Excluding a shape excludes everything inside it too.
    pub fn excluded(&self, points: &[Pt]) -> Vec<bool> {
        let mut direct = vec![false; self.shapes.len()];
        for &p in points {
            if let Some(i) = self.shape_at(p) {
                direct[i] = true;
            }
        }
        (0..self.shapes.len())
            .map(|i| {
                let mut cur = Some(i);
                while let Some(c) = cur {
                    if direct[c] {
                        return true;
                    }
                    cur = self.shapes[c].contour.parent;
                }
                false
            })
            .collect()
    }

    /// SVG for all shapes not excluded.
    pub fn to_svg(&self, frame: &FrameGeom, opts: &Options, excluded: &[bool]) -> String {
        let shapes: Vec<SvgShape> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(i, _)| !excluded.get(*i).copied().unwrap_or(false))
            .map(|(_, s)| SvgShape { segs: &s.segs, hole: s.contour.hole, depth: s.contour.depth })
            .collect();
        let mut view = [0.0, 0.0, frame.width_mm, frame.height_mm];
        if opts.crop_to_content && !shapes.is_empty() {
            let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for s in &shapes {
                for p in trace::flatten(s.segs, 16) {
                    b[0] = b[0].min(p[0]);
                    b[1] = b[1].min(p[1]);
                    b[2] = b[2].max(p[0]);
                    b[3] = b[3].max(p[1]);
                }
            }
            let m = opts.crop_margin_mm;
            let k = 1.0 / self.px_per_mm;
            view = [b[0] * k - m, b[1] * k - m, (b[2] - b[0]) * k + 2.0 * m, (b[3] - b[1]) * k + 2.0 * m];
        }
        svg::build_svg(&shapes, self.px_per_mm, view, opts.style)
    }
}

pub fn rectify_gray(gray: &Gray, f: &FrameGeom, px_per_mm: f64) -> Option<Gray> {
    let (w, h) = out_size(f, px_per_mm);
    let hinv = g::homography(&dst_rect(w, h), &f.corners)?;
    Some(g::warp_gray(gray, &hinv, w, h))
}

pub fn rectify_rgba(rgba: &[u8], sw: usize, sh: usize, f: &FrameGeom, px_per_mm: f64) -> Option<(usize, usize, Vec<u8>)> {
    let (w, h) = out_size(f, px_per_mm);
    let hinv = g::homography(&dst_rect(w, h), &f.corners)?;
    Some((w, h, g::warp_rgba(rgba, sw, sh, &hinv, w, h)))
}

fn out_size(f: &FrameGeom, ppm: f64) -> (usize, usize) {
    (((f.width_mm * ppm).round() as usize).max(1), ((f.height_mm * ppm).round() as usize).max(1))
}

fn dst_rect(w: usize, h: usize) -> [Pt; 4] {
    let (w, h) = (w as f64, h as f64);
    [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
}

pub fn make_mask(gray: &Gray, o: &Options) -> (Vec<u8>, f32) {
    let (w, h) = (gray.width, gray.height);
    let ppm = o.px_per_mm;
    let flat;
    let img = if o.flatten {
        flat = ip::flatten_lighting(gray, (12.0 * ppm) as usize);
        &flat
    } else {
        gray
    };
    let img = ip::box_blur(img, ((ppm * 0.08).round() as usize).max(1));
    let mut t = o.threshold;
    if o.auto_threshold {
        let m = (o.inset_mm * ppm).round() as usize + 2;
        let stride = (((w * h) as f64 / 400_000.0).sqrt().round() as usize).max(1);
        let mut vals = Vec::new();
        let mut y = m;
        while y + m < h {
            let mut x = m;
            while x + m < w {
                vals.push(img.data[y * w + x]);
                x += stride;
            }
            y += stride;
        }
        t = ip::otsu(vals).clamp(60.0, if o.invert { 230.0 } else { 220.0 });
    }
    let mut mask = ip::threshold(&img, t, o.invert);
    ip::clear_border(&mut mask, w, h, (o.inset_mm * ppm).round() as usize);
    let mask = ip::despeckle(&mask, w, h, (o.despeckle_mm2 * ppm * ppm) as usize);
    (mask, t)
}

pub fn trace_mask(mask: &[u8], w: usize, h: usize, o: &Options) -> Vec<Shape> {
    let ppm = o.px_per_mm;
    let contours = trace::trace_contours(mask, w, h);
    let min_area = o.min_area_mm2 * ppm * ppm;
    let fit = FitOptions {
        smoothing: o.smoothing_mm * ppm,
        tolerance: o.tolerance_mm * ppm,
        corner_angle: o.corner_angle,
    };
    // A child is always smaller than its parent, so dropping by area keeps
    // the hierarchy closed; remap parent indices to the kept list.
    let mut remap = vec![None; contours.len()];
    let mut kept = Vec::new();
    for (i, c) in contours.iter().enumerate() {
        if c.area >= min_area {
            remap[i] = Some(kept.len());
            kept.push(i);
        }
    }
    kept.into_iter()
        .map(|i| {
            let mut c = contours[i].clone();
            c.parent = c.parent.and_then(|p| remap[p]);
            let segs = trace::fit_contour(&c.points, &fit);
            Shape { contour: c, segs }
        })
        .collect()
}

#[derive(Default)]
struct Cache {
    rect_key: Option<(FrameGeom, u64)>,
    gray: Option<Gray>,
    mask_key: Option<Options>,
    mask: Option<(Vec<u8>, f32)>,
    trace_key: Option<Options>,
    result: Option<TraceResult>,
}

/// Holds one source photo and caches intermediate stages.
pub struct Pipeline {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub gray: Gray,
    cache: Cache,
}

fn mask_relevant(o: &Options) -> Options {
    Options {
        smoothing_mm: 0.0,
        tolerance_mm: 0.0,
        corner_angle: 0.0,
        min_area_mm2: 0.0,
        style: SvgStyle::Shaper,
        crop_to_content: false,
        crop_margin_mm: 0.0,
        ..o.clone()
    }
}

fn trace_relevant(o: &Options) -> Options {
    Options { style: SvgStyle::Shaper, crop_to_content: false, crop_margin_mm: 0.0, ..o.clone() }
}

impl Pipeline {
    pub fn new(rgba: Vec<u8>, width: usize, height: usize) -> Self {
        let gray = ip::to_gray(&rgba, width, height);
        Pipeline { width, height, rgba, gray, cache: Cache::default() }
    }

    pub fn detect(&self) -> Option<frame::Detection> {
        frame::detect_frame(&self.gray)
    }

    pub fn run(&mut self, f: &FrameGeom, o: &Options) -> Option<&TraceResult> {
        let rect_key = (*f, o.px_per_mm.to_bits());
        if self.cache.rect_key != Some(rect_key) {
            self.cache = Cache { rect_key: Some(rect_key), gray: Some(rectify_gray(&self.gray, f, o.px_per_mm)?), ..Default::default() };
        }
        let gray = self.cache.gray.as_ref()?;
        let mk = mask_relevant(o);
        if self.cache.mask_key.as_ref() != Some(&mk) {
            self.cache.mask = Some(make_mask(gray, o));
            self.cache.mask_key = Some(mk);
            self.cache.trace_key = None;
        }
        let tk = trace_relevant(o);
        if self.cache.trace_key.as_ref() != Some(&tk) {
            let (mask, t) = self.cache.mask.as_ref()?;
            let shapes = trace_mask(mask, gray.width, gray.height, o);
            self.cache.result = Some(TraceResult {
                width: gray.width,
                height: gray.height,
                px_per_mm: o.px_per_mm,
                threshold: *t,
                mask: mask.clone(),
                shapes,
            });
            self.cache.trace_key = Some(tk);
        }
        self.cache.result.as_ref()
    }
}
