//! Photo -> rectified image -> ink mask -> vector shapes, with per-stage
//! caching so that changing a later setting only recomputes what depends on it.

use super::dxf;
use super::frame;
use super::geometry::{self as g, Pt};
use super::imgproc::{self as ip, Gray};
use super::svg::{self, SvgShape, SvgStyle};
use super::trace::{self, Contour, FitOptions, Segment};
use serde::{Deserialize, Serialize};

/// Shaper Trace capture area (mm), from Shaper's published specs.
pub const TRACE_CAPTURE_MM: (f64, f64) = (263.0, 173.0);

/// How to turn drawn lines into cut paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum LineMode {
    /// Trace the ink itself: a closed pen line becomes two paths (both edges).
    Ink,
    /// Closed outlines become one path along the outside of the line.
    Outside,
    /// Closed outlines become one path along the middle of the line.
    Centre,
    /// Closed outlines become one path along the inside of the line
    /// (closest to an object you drew around).
    Inside,
}

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
    /// Also keep faint pixels that connect to clearly dark ink.
    pub catch_faint: bool,
    /// Bridge gaps in lines up to about this size.
    pub close_gaps_mm: f64,
    pub line_mode: LineMode,
    /// Ink thinner than this counts as a drawn line (for `line_mode`).
    pub max_line_width_mm: f64,
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
    pub format: ExportFormat,
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
            catch_faint: true,
            close_gaps_mm: 0.5,
            line_mode: LineMode::Centre,
            max_line_width_mm: 2.5,
            despeckle_mm2: 1.5,
            smoothing_mm: 0.25,
            tolerance_mm: 0.1,
            corner_angle: 60.0,
            min_area_mm2: 2.0,
            style: SvgStyle::Shaper,
            format: ExportFormat::Svg,
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
        FrameGeom {
            corners,
            width_mm,
            height_mm,
        }
    }

    /// Rotate the output by 90° clockwise.
    pub fn rotated(&self) -> Self {
        let c = self.corners;
        FrameGeom {
            corners: [c[3], c[0], c[1], c[2]],
            width_mm: self.height_mm,
            height_mm: self.width_mm,
        }
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
                p[0] >= b[0] as f64
                    && p[1] >= b[1] as f64
                    && p[0] <= b[2] as f64
                    && p[1] <= b[3] as f64
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

    /// Shapes not excluded, plus the exported region [x, y, w, h] in mm.
    fn export_shapes(
        &self,
        frame: &FrameGeom,
        opts: &Options,
        excluded: &[bool],
    ) -> (Vec<SvgShape<'_>>, [f64; 4]) {
        let shapes: Vec<SvgShape> = self
            .shapes
            .iter()
            .enumerate()
            .filter(|(i, _)| !excluded.get(*i).copied().unwrap_or(false))
            .map(|(_, s)| SvgShape {
                segs: &s.segs,
                hole: s.contour.hole,
                depth: s.contour.depth,
            })
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
            view = [
                b[0] * k - m,
                b[1] * k - m,
                (b[2] - b[0]) * k + 2.0 * m,
                (b[3] - b[1]) * k + 2.0 * m,
            ];
        }
        (shapes, view)
    }

    /// SVG for all shapes not excluded.
    pub fn to_svg(&self, frame: &FrameGeom, opts: &Options, excluded: &[bool]) -> String {
        let (shapes, view) = self.export_shapes(frame, opts, excluded);
        svg::build_svg(&shapes, self.px_per_mm, view, opts.style)
    }

    /// DXF (R12, mm) for all shapes not excluded. Curves become polylines
    /// within 0.01 mm of the traced curves.
    pub fn to_dxf(&self, frame: &FrameGeom, opts: &Options, excluded: &[bool]) -> String {
        let (shapes, view) = self.export_shapes(frame, opts, excluded);
        dxf::build_dxf(&shapes, self.px_per_mm, view, 0.01)
    }

    /// Export by format.
    pub fn export(
        &self,
        format: ExportFormat,
        frame: &FrameGeom,
        opts: &Options,
        excluded: &[bool],
    ) -> String {
        match format {
            ExportFormat::Svg => self.to_svg(frame, opts, excluded),
            ExportFormat::Dxf => self.to_dxf(frame, opts, excluded),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum, Default)]
pub enum ExportFormat {
    #[default]
    Svg,
    Dxf,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Svg => "svg",
            ExportFormat::Dxf => "dxf",
        }
    }

    /// Format implied by a file name's extension, if recognised.
    pub fn from_path(p: &std::path::Path) -> Option<Self> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "svg" => Some(ExportFormat::Svg),
            "dxf" => Some(ExportFormat::Dxf),
            _ => None,
        }
    }
}

pub fn rectify_gray(gray: &Gray, f: &FrameGeom, px_per_mm: f64) -> Option<Gray> {
    let (w, h) = out_size(f, px_per_mm);
    let hinv = g::homography(&dst_rect(w, h), &f.corners)?;
    Some(g::warp_gray(gray, &hinv, w, h))
}

pub fn rectify_rgba(
    rgba: &[u8],
    sw: usize,
    sh: usize,
    f: &FrameGeom,
    px_per_mm: f64,
) -> Option<(usize, usize, Vec<u8>)> {
    let (w, h) = out_size(f, px_per_mm);
    let hinv = g::homography(&dst_rect(w, h), &f.corners)?;
    Some((w, h, g::warp_rgba(rgba, sw, sh, &hinv, w, h)))
}

fn out_size(f: &FrameGeom, ppm: f64) -> (usize, usize) {
    (
        ((f.width_mm * ppm).round() as usize).max(1),
        ((f.height_mm * ppm).round() as usize).max(1),
    )
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
    let mut mask = if o.catch_faint {
        ip::hysteresis(&img, t, faint_threshold(&img, t, o.invert), o.invert)
    } else {
        ip::threshold(&img, t, o.invert)
    };
    ip::clear_border(&mut mask, w, h, (o.inset_mm * ppm).round() as usize);
    let mask = ip::close(&mask, w, h, (o.close_gaps_mm * ppm / 2.0).round() as usize);
    let mask = ip::despeckle(&mask, w, h, (o.despeckle_mm2 * ppm * ppm) as usize);
    (
        apply_line_mode(&mask, w, h, o.line_mode, o.max_line_width_mm * ppm),
        t,
    )
}

/// Weak threshold for hysteresis: just outside the paper's noise band.
fn faint_threshold(img: &Gray, t: f32, invert: bool) -> f32 {
    let stride = ((img.data.len() as f64 / 200_000.0).sqrt().round() as usize).max(1);
    let mut paper: Vec<f32> = img
        .data
        .iter()
        .step_by(stride)
        .copied()
        .filter(|&v| if invert { v < t } else { v >= t })
        .collect();
    if paper.len() < 100 {
        return t;
    }
    paper.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = paper[paper.len() / 2];
    let mut dev: Vec<f32> = paper.iter().map(|v| (v - med).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let sigma = 1.4826 * dev[dev.len() / 2];
    // Halfway between the main threshold and the paper, but never inside
    // the paper's noise band.
    let margin = (5.0 * sigma).max(8.0);
    if invert {
        (t - 0.5 * (t - med)).max(med + margin).min(t)
    } else {
        (t + 0.5 * (med - t)).min(med - margin).max(t)
    }
}

/// Convert closed pen outlines into single filled regions (see LineMode).
///
/// Only thin ink loops (estimated stroke width < `max_width_px`) are
/// converted; filled shapes and open strokes are left as drawn. Regions are
/// filled by nesting parity, so a washer drawn as two circles keeps its hole.
pub fn apply_line_mode(
    mask: &[u8],
    w: usize,
    h: usize,
    mode: LineMode,
    max_width_px: f64,
) -> Vec<u8> {
    if mode == LineMode::Ink {
        return mask.to_vec();
    }
    let ink = ip::label(mask, w, h, 1, true);
    let bg = ip::label(mask, w, h, 0, false);
    let (ni, nb) = (ink.areas.len(), bg.areas.len());
    if ni == 0 {
        return mask.to_vec();
    }
    // Stroke width estimate per ink component: 2 * area / boundary length.
    let mut boundary = vec![0usize; ni];
    let mut adj = std::collections::HashSet::new();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if mask[i] != 1 {
                continue;
            }
            let c = ink.labels[i] as usize;
            let mut edge = false;
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx >= w || ny >= h {
                    edge = true;
                    continue;
                }
                let j = ny * w + nx;
                if mask[j] == 0 {
                    edge = true;
                    adj.insert((c, bg.labels[j] as usize));
                }
            }
            if edge {
                boundary[c] += 1;
            }
        }
    }
    let thin: Vec<bool> = (0..ni)
        .map(|c| 2.0 * ink.areas[c] as f64 / boundary[c].max(1) as f64 <= max_width_px)
        .collect();
    let mut ink_nb: Vec<Vec<usize>> = vec![Vec::new(); ni];
    let mut bg_nb: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for &(c, r) in &adj {
        ink_nb[c].push(r);
        bg_nb[r].push(c);
    }
    // Breadth-first nesting levels: outside background 0, ink touching it 1,
    // regions enclosed by that ink 2, and so on.
    let mut bg_level = vec![usize::MAX; nb];
    let mut ink_level = vec![usize::MAX; ni];
    let mut queue = std::collections::VecDeque::new();
    for r in 0..nb {
        if bg.touches_border[r] {
            bg_level[r] = 0;
            queue.push_back((false, r));
        }
    }
    while let Some((is_ink, k)) = queue.pop_front() {
        if is_ink {
            for &r in &ink_nb[k] {
                if bg_level[r] == usize::MAX {
                    bg_level[r] = ink_level[k] + 1;
                    queue.push_back((false, r));
                }
            }
        } else {
            for &c in &bg_nb[k] {
                if ink_level[c] == usize::MAX {
                    ink_level[c] = bg_level[k] + 1;
                    queue.push_back((true, c));
                }
            }
        }
    }
    // Fill regions inside an odd number of loops whose enclosing ink is thin.
    let fill: Vec<bool> = (0..nb)
        .map(|r| {
            let l = bg_level[r];
            l != usize::MAX && l % 4 == 2 && {
                let enclosing: Vec<usize> = bg_nb[r]
                    .iter()
                    .copied()
                    .filter(|&c| ink_level[c] + 1 == l)
                    .collect();
                !enclosing.is_empty() && enclosing.iter().all(|&c| thin[c])
            }
        })
        .collect();
    let is_loop: Vec<bool> = (0..ni)
        .map(|c| thin[c] && ink_nb[c].iter().any(|&r| fill[r]))
        .collect();
    let f_px: Vec<bool> = (0..mask.len())
        .map(|i| mask[i] == 0 && fill[bg.labels[i] as usize])
        .collect();
    let loop_px: Vec<bool> = (0..mask.len())
        .map(|i| mask[i] == 1 && is_loop[ink.labels[i] as usize])
        .collect();
    let other_ink = |i: usize| mask[i] == 1 && !loop_px[i];
    match mode {
        LineMode::Ink => unreachable!(),
        LineMode::Outside => (0..mask.len())
            .map(|i| (mask[i] == 1 || f_px[i]) as u8)
            .collect(),
        LineMode::Inside => (0..mask.len())
            .map(|i| (other_ink(i) || f_px[i]) as u8)
            .collect(),
        LineMode::Centre => {
            let not_f: Vec<bool> = (0..mask.len()).map(|i| mask[i] == 0 && !f_px[i]).collect();
            let d_in = ip::distance_to(&f_px, w, h);
            let d_out = ip::distance_to(&not_f, w, h);
            (0..mask.len())
                .map(|i| (other_ink(i) || f_px[i] || (loop_px[i] && d_in[i] < d_out[i])) as u8)
                .collect()
        }
    }
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
        format: ExportFormat::Svg,
        crop_to_content: false,
        crop_margin_mm: 0.0,
        ..o.clone()
    }
}

fn trace_relevant(o: &Options) -> Options {
    Options {
        style: SvgStyle::Shaper,
        format: ExportFormat::Svg,
        crop_to_content: false,
        crop_margin_mm: 0.0,
        ..o.clone()
    }
}

impl Pipeline {
    pub fn new(rgba: Vec<u8>, width: usize, height: usize) -> Self {
        let gray = ip::to_gray(&rgba, width, height);
        Pipeline {
            width,
            height,
            rgba,
            gray,
            cache: Cache::default(),
        }
    }

    pub fn detect(&self) -> Option<frame::Detection> {
        frame::detect_frame(&self.gray)
    }

    pub fn run(&mut self, f: &FrameGeom, o: &Options) -> Option<&TraceResult> {
        let rect_key = (*f, o.px_per_mm.to_bits());
        if self.cache.rect_key != Some(rect_key) {
            self.cache = Cache {
                rect_key: Some(rect_key),
                gray: Some(rectify_gray(&self.gray, f, o.px_per_mm)?),
                ..Default::default()
            };
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
