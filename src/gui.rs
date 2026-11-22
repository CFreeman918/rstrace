//! Desktop GUI (egui). Two steps:
//!   1. Frame: check / drag the four corners of the frame opening.
//!   2. Trace: tune threshold & smoothing, click shapes to drop them, export.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use rstrace::core::geometry::Pt;
use rstrace::core::pipeline::rectify_rgba;
use rstrace::core::trace;
use rstrace::{FrameGeom, Options, Pipeline, SvgStyle, TraceResult, TRACE_CAPTURE_MM};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

const MAX_TEXTURE: usize = 4096;
const SETTINGS_KEY: &str = "rstrace-settings";

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    long_mm: f64,
    short_mm: f64,
    options: Options,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { long_mm: TRACE_CAPTURE_MM.0, short_mm: TRACE_CAPTURE_MM.1, options: Options::default() }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Step {
    Frame,
    Trace,
}

struct Photo {
    path: PathBuf,
    width: usize,
    height: usize,
    tex: TextureHandle,
    pipeline: Arc<Mutex<Pipeline>>,
}

struct Output {
    generation: u64,
    result: Arc<TraceResult>,
    frame: FrameGeom,
    color: Option<(egui::ColorImage, FrameGeom)>,
    mask: egui::ColorImage,
}

pub struct App {
    settings: Settings,
    photo: Option<Photo>,
    corners: [Pt; 4],
    rotation: u8,
    step: Step,
    dragging: Option<usize>,
    status: String,

    // Background processing.
    generation: Arc<AtomicU64>,
    tx: Sender<Result<Output, String>>,
    rx: Receiver<Result<Output, String>>,
    requested: Option<(FrameGeom, Options)>,
    busy: bool,

    // Latest results.
    result: Option<Arc<TraceResult>>,
    result_frame: Option<FrameGeom>,
    color_tex: Option<(TextureHandle, FrameGeom)>,
    mask_tex: Option<TextureHandle>,
    excluded_mm: Vec<Pt>,
    show_photo: bool,
    show_mask: bool,
}

pub fn run(initial: Option<PathBuf>) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]).with_title("rstrace"),
        ..Default::default()
    };
    eframe::run_native("rstrace", options, Box::new(move |cc| {
        let mut app = App::new(cc);
        if let Some(p) = initial {
            app.open(&cc.egui_ctx, &p);
            // Undocumented: jump straight to the trace step (used for screenshots).
            if std::env::var_os("RSTRACE_START_TRACE").is_some() {
                app.step = Step::Trace;
            }
        }
        Ok(Box::new(app))
    })).map_err(|e| e.to_string())
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        let settings = cc.storage.and_then(|s| eframe::get_value(s, SETTINGS_KEY)).unwrap_or_default();
        let (tx, rx) = channel();
        App {
            settings,
            photo: None,
            corners: [[0.0; 2]; 4],
            rotation: 0,
            step: Step::Frame,
            dragging: None,
            status: "Open a photo of a drawing in the frame (or drop one onto this window).".into(),
            generation: Arc::new(AtomicU64::new(0)),
            tx,
            rx,
            requested: None,
            busy: false,
            result: None,
            result_frame: None,
            color_tex: None,
            mask_tex: None,
            excluded_mm: Vec::new(),
            show_photo: true,
            show_mask: false,
        }
    }

    fn frame_geom(&self) -> FrameGeom {
        let mut f = FrameGeom::oriented(self.corners, self.settings.long_mm, self.settings.short_mm);
        for _ in 0..self.rotation % 4 {
            f = f.rotated();
        }
        f
    }

    fn open(&mut self, ctx: &egui::Context, path: &Path) {
        match rstrace::load::load_photo(path) {
            Ok(photo) => {
                let (w, h) = (photo.width, photo.height);
                let tex = ctx.load_texture("photo", texture_image(&photo.rgba, w, h), TextureOptions::LINEAR);
                let pipeline = Pipeline::new(photo.rgba, w, h);
                let det = pipeline.detect();
                self.status = match &det {
                    Some(_) => "Frame found. Check the corners (drag to adjust), then go to Trace.".into(),
                    None => "Couldn't find the frame automatically: drag the four corners onto the inside corners of the frame.".into(),
                };
                self.corners = det.map(|d| d.corners).unwrap_or_else(|| {
                    let (w, h) = (w as f64, h as f64);
                    [[w * 0.2, h * 0.2], [w * 0.8, h * 0.2], [w * 0.8, h * 0.8], [w * 0.2, h * 0.8]]
                });
                self.rotation = 0;
                self.photo = Some(Photo { path: path.to_path_buf(), width: w, height: h, tex, pipeline: Arc::new(Mutex::new(pipeline)) });
                self.result = None;
                self.result_frame = None;
                self.color_tex = None;
                self.mask_tex = None;
                self.requested = None;
                self.excluded_mm.clear();
                self.step = Step::Frame;
            }
            Err(e) => self.status = e,
        }
    }

    fn redetect(&mut self) {
        if let Some(p) = &self.photo {
            let det = p.pipeline.lock().unwrap().detect();
            match det {
                Some(d) => {
                    self.corners = d.corners;
                    self.status = "Frame re-detected.".into();
                }
                None => self.status = "Couldn't find the frame automatically; place the corners by hand.".into(),
            }
        }
    }

    /// Kick off processing if inputs changed since the last request.
    fn request(&mut self, ctx: &egui::Context) {
        let Some(photo) = &self.photo else { return };
        let frame = self.frame_geom();
        let opts = self.settings.options.clone();
        if self.requested.as_ref() == Some(&(frame, opts.clone())) {
            return;
        }
        self.requested = Some((frame, opts.clone()));
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = self.generation.clone();
        let need_color = self.color_tex.as_ref().map_or(true, |(_, f)| *f != frame);
        let pipeline = photo.pipeline.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.busy = true;
        std::thread::spawn(move || {
            let mut pipe = pipeline.lock().unwrap();
            if latest.load(Ordering::SeqCst) != generation {
                return; // superseded before we started
            }
            let out = match pipe.run(&frame, &opts) {
                None => Err("The corners don't form a valid quadrilateral.".to_string()),
                Some(r) => {
                    let result = Arc::new(r.clone());
                    let mask = mask_image(&result);
                    let color = need_color.then(|| {
                        let ppm = display_ppm(&frame, opts.px_per_mm);
                        let (w, h, rgba) = rectify_rgba(&pipe.rgba, pipe.width, pipe.height, &frame, ppm).unwrap();
                        (egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba), frame)
                    });
                    Ok(Output { generation, result, frame, color, mask })
                }
            };
            let _ = tx.send(out);
            ctx.request_repaint();
        });
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Ok(out) => {
                    if let Some((img, f)) = out.color {
                        self.color_tex = Some((ctx.load_texture("rectified", img, TextureOptions::LINEAR), f));
                    }
                    self.mask_tex = Some(ctx.load_texture("mask", out.mask, TextureOptions::NEAREST));
                    self.result = Some(out.result);
                    self.result_frame = Some(out.frame);
                    if out.generation == self.generation.load(Ordering::SeqCst) {
                        self.busy = false;
                    }
                }
                Err(e) => {
                    self.status = e;
                    self.busy = false;
                }
            }
        }
    }

    fn export(&mut self) {
        let (Some(r), Some(f), Some(photo)) = (&self.result, &self.result_frame, &self.photo) else { return };
        let excluded = r.excluded(&self.excluded_px(r));
        let svg = r.to_svg(f, &self.settings.options, &excluded);
        let name = photo.path.file_stem().map(|s| format!("{}.svg", s.to_string_lossy())).unwrap_or("trace.svg".into());
        let mut dlg = rfd::FileDialog::new().add_filter("SVG", &["svg"]).set_file_name(name);
        if let Some(dir) = photo.path.parent() {
            dlg = dlg.set_directory(dir);
        }
        if let Some(path) = dlg.save_file() {
            self.status = match std::fs::write(&path, svg) {
                Ok(()) => format!("Saved {}", path.display()),
                Err(e) => format!("Couldn't save: {e}"),
            };
        }
    }

    fn excluded_px(&self, r: &TraceResult) -> Vec<Pt> {
        self.excluded_mm.iter().map(|p| [p[0] * r.px_per_mm, p[1] * r.px_per_mm]).collect()
    }

    // ------------------------------------------------------------ panels

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Open photo…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Images", &["jpg", "jpeg", "png", "webp", "tif", "tiff", "bmp"])
                    .pick_file()
                {
                    self.open(ui.ctx(), &p);
                }
            }
            let can_export = self.result.is_some();
            if ui.add_enabled(can_export, egui::Button::new("Export SVG…")).clicked() {
                self.export();
            }
        });
        ui.separator();

        ui.horizontal(|ui| {
            let has = self.photo.is_some();
            ui.add_enabled_ui(has, |ui| {
                ui.selectable_value(&mut self.step, Step::Frame, "1. Frame");
                ui.selectable_value(&mut self.step, Step::Trace, "2. Trace");
            });
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            let s = &mut self.settings;
            ui.heading("Frame");
            ui.label("Inner opening size (mm):");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.long_mm).range(10.0..=2000.0).speed(0.1).suffix(" mm"));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut s.short_mm).range(10.0..=2000.0).speed(0.1).suffix(" mm"));
            });
            if ui.small_button("Shaper Trace (263 × 173)").clicked() {
                (s.long_mm, s.short_mm) = TRACE_CAPTURE_MM;
            }
            ui.horizontal(|ui| {
                if ui.button("Auto-detect").clicked() {
                    self.redetect();
                }
                if ui.button("Rotate 90°").clicked() {
                    self.rotation = (self.rotation + 1) % 4;
                }
            });
            let f = self.frame_geom();
            ui.label(format!("Output: {:.1} × {:.1} mm", f.width_mm, f.height_mm));

            let o = &mut self.settings.options;
            ui.add_space(8.0);
            ui.heading("Threshold");
            ui.checkbox(&mut o.auto_threshold, "Automatic");
            let auto_val = self.result.as_ref().map(|r| r.threshold);
            if o.auto_threshold {
                if let Some(t) = auto_val {
                    o.threshold = t;
                }
            }
            ui.add_enabled(!o.auto_threshold, egui::Slider::new(&mut o.threshold, 1.0..=254.0).text("level"));
            ui.checkbox(&mut o.flatten, "Even out lighting / shadows");
            ui.checkbox(&mut o.invert, "Light lines on dark background");
            ui.add(egui::Slider::new(&mut o.despeckle_mm2, 0.0..=20.0).text("despeckle mm²"));

            ui.add_space(8.0);
            ui.heading("Shape");
            ui.add(egui::Slider::new(&mut o.smoothing_mm, 0.0..=2.0).text("smoothing mm"));
            ui.add(egui::Slider::new(&mut o.tolerance_mm, 0.02..=1.0).logarithmic(true).text("accuracy mm"));
            ui.add(egui::Slider::new(&mut o.corner_angle, 30.0..=180.0).text("corner angle °"))
                .on_hover_text("Turns sharper than this become crisp corners. 180 = all smooth.");
            ui.add(egui::Slider::new(&mut o.min_area_mm2, 0.0..=100.0).logarithmic(true).text("min shape mm²"));
            ui.add(egui::Slider::new(&mut o.inset_mm, 0.0..=15.0).text("edge margin mm"))
                .on_hover_text("Ignore marks this close to the frame (shadows from the frame lip).");
            ui.add(egui::Slider::new(&mut o.px_per_mm, 3.0..=16.0).text("resolution px/mm"))
                .on_hover_text("Higher = finer detail, slower. Useful up to roughly the photo's own resolution.");

            ui.add_space(8.0);
            ui.heading("Export");
            egui::ComboBox::from_label("style")
                .selected_text(match o.style {
                    SvgStyle::Shaper => "Shaper Origin (cut types)",
                    SvgStyle::Filled => "Filled",
                    SvgStyle::Outline => "Outlines",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut o.style, SvgStyle::Shaper, "Shaper Origin (cut types)")
                        .on_hover_text("Outer boundaries: black fill (exterior cut). Holes: white fill + black stroke (interior cut).");
                    ui.selectable_value(&mut o.style, SvgStyle::Filled, "Filled");
                    ui.selectable_value(&mut o.style, SvgStyle::Outline, "Outlines");
                });
            ui.checkbox(&mut o.crop_to_content, "Crop to drawing");
            if o.crop_to_content {
                ui.add(egui::Slider::new(&mut o.crop_margin_mm, 0.0..=20.0).text("margin mm"));
            }
            if ui.button("Reset settings").clicked() {
                *o = Options::default();
            }

            ui.add_space(8.0);
            ui.heading("View");
            ui.checkbox(&mut self.show_photo, "Photo");
            ui.checkbox(&mut self.show_mask, "Threshold mask");
            if !self.excluded_mm.is_empty() && ui.button(format!("Restore {} removed shape(s)", self.excluded_mm.len())).clicked() {
                self.excluded_mm.clear();
            }
        });
    }

    fn frame_view(&mut self, ui: &mut egui::Ui) {
        let Some(photo) = &self.photo else { return };
        let avail = ui.available_rect_before_wrap();
        let (w, h) = (photo.width as f32, photo.height as f32);
        let scale = (avail.width() / w).min(avail.height() / h);
        let img_rect = Rect::from_center_size(avail.center(), Vec2::new(w * scale, h * scale));
        let to_screen = |p: Pt| Pos2::new(img_rect.min.x + p[0] as f32 * scale, img_rect.min.y + p[1] as f32 * scale);
        let to_img = |p: Pos2| [((p.x - img_rect.min.x) / scale) as f64, ((p.y - img_rect.min.y) / scale) as f64];

        let painter = ui.painter_at(avail);
        let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(photo.tex.id(), img_rect, full_uv, Color32::WHITE);
        let tex_id = photo.tex.id();

        // Quad, with the output's top edge emphasised.
        let f = self.frame_geom();
        let pts: Vec<Pos2> = self.corners.iter().map(|&p| to_screen(p)).collect();
        for i in 0..4 {
            painter.line_segment([pts[i], pts[(i + 1) % 4]], Stroke::new(2.0, Color32::from_rgb(0, 200, 255)));
        }
        let (a, b) = (to_screen(f.corners[0]), to_screen(f.corners[1]));
        painter.line_segment([a, b], Stroke::new(5.0, Color32::from_rgb(255, 170, 0)));
        painter.text(
            a + (b - a) * 0.5 + Vec2::new(0.0, -14.0),
            egui::Align2::CENTER_BOTTOM,
            "top",
            egui::FontId::proportional(14.0),
            Color32::from_rgb(255, 170, 0),
        );

        // Draggable handles.
        let pointer = ui.ctx().pointer_interact_pos();
        for i in 0..4 {
            let c = to_screen(self.corners[i]);
            let resp = ui.interact(Rect::from_center_size(c, Vec2::splat(28.0)), ui.scope_id().with(("corner", i)), Sense::drag());
            if resp.drag_started() {
                self.dragging = Some(i);
            }
            if resp.dragged() {
                if let Some(p) = pointer {
                    let q = to_img(p.clamp(img_rect.min, img_rect.max));
                    self.corners[i] = q;
                }
            }
            if resp.drag_stopped() {
                self.dragging = None;
            }
            let hot = resp.hovered() || resp.dragged();
            painter.circle_stroke(c, if hot { 12.0 } else { 9.0 }, Stroke::new(2.5, Color32::from_rgb(0, 200, 255)));
            painter.circle_filled(c, 2.0, Color32::from_rgb(255, 60, 60));
            if hot {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }

        // Loupe while dragging.
        if let Some(i) = self.dragging {
            let c = self.corners[i];
            let zoom_px = 60.0f64; // photo pixels shown across the loupe
            let uv = Rect::from_min_max(
                Pos2::new(((c[0] - zoom_px / 2.0) / photo.width as f64) as f32, ((c[1] - zoom_px / 2.0) / photo.height as f64) as f32),
                Pos2::new(((c[0] + zoom_px / 2.0) / photo.width as f64) as f32, ((c[1] + zoom_px / 2.0) / photo.height as f64) as f32),
            );
            let size = 200.0;
            let lr = Rect::from_min_size(avail.min + Vec2::new(12.0, 12.0), Vec2::splat(size));
            painter.rect_filled(lr, 4.0, Color32::BLACK);
            painter.image(tex_id, lr, uv, Color32::WHITE);
            let m = lr.center();
            painter.line_segment([Pos2::new(lr.min.x, m.y), Pos2::new(lr.max.x, m.y)], Stroke::new(1.0, Color32::RED));
            painter.line_segment([Pos2::new(m.x, lr.min.y), Pos2::new(m.x, lr.max.y)], Stroke::new(1.0, Color32::RED));
            painter.rect_stroke(lr, 4.0, Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Outside);
        }
        ui.allocate_rect(avail, Sense::hover());
    }

    fn trace_view(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();
        let Some(r) = self.result.clone() else {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        let f = self.result_frame.unwrap();
        let (w, h) = (r.width as f32, r.height as f32);
        let scale = (avail.width() / w).min(avail.height() / h) * 0.97;
        let rect = Rect::from_center_size(avail.center(), Vec2::new(w * scale, h * scale));
        let to_screen = |p: Pt| Pos2::new(rect.min.x + p[0] as f32 * scale, rect.min.y + p[1] as f32 * scale);
        let painter = ui.painter_at(avail);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.rect_filled(rect, 0.0, Color32::WHITE);
        if self.show_photo {
            if let Some((tex, _)) = &self.color_tex {
                painter.image(tex.id(), rect, uv, Color32::from_white_alpha(140));
            }
        }
        if self.show_mask {
            if let Some(tex) = &self.mask_tex {
                painter.image(tex.id(), rect, uv, Color32::WHITE);
            }
        }

        let resp = ui.allocate_rect(avail, Sense::click());
        let hover_px = resp.hover_pos().filter(|p| rect.contains(*p)).map(|p| [((p.x - rect.min.x) / scale) as f64, ((p.y - rect.min.y) / scale) as f64]);
        let hovered = hover_px.and_then(|p| r.shape_at(p));
        let excluded = r.excluded(&self.excluded_px(&r));

        for (i, s) in r.shapes.iter().enumerate() {
            let poly: Vec<Pos2> = trace::flatten(&s.segs, 12).into_iter().map(to_screen).collect();
            if poly.len() < 2 {
                continue;
            }
            let (color, width) = if excluded[i] {
                (Color32::from_gray(170), 1.0)
            } else if s.contour.hole {
                (Color32::from_rgb(235, 110, 20), 1.6)
            } else {
                (Color32::from_rgb(20, 100, 230), 1.6)
            };
            let width = if hovered == Some(i) { width + 1.5 } else { width };
            painter.add(egui::Shape::closed_line(poly, Stroke::new(width, color)));
        }
        if let Some(i) = hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            resp.clone().on_hover_text(if excluded[i] { "Click to include this shape" } else { "Click to remove this shape" });
        }
        if resp.clicked() {
            if let (Some(p), Some(i)) = (hover_px, hovered) {
                let ppm = r.px_per_mm;
                let before = self.excluded_mm.len();
                // Clicking a removed shape restores it (drop the click(s) that removed it or an ancestor).
                self.excluded_mm.retain(|q| {
                    let qp = [q[0] * ppm, q[1] * ppm];
                    match r.shape_at(qp) {
                        Some(j) => !is_ancestor_or_self(&r, j, i),
                        None => false,
                    }
                });
                if self.excluded_mm.len() == before {
                    self.excluded_mm.push([p[0] / ppm, p[1] / ppm]);
                }
            }
        }

        // Scale bar & stats.
        let kept = excluded.iter().filter(|e| !**e).count();
        painter.text(
            rect.left_bottom() + Vec2::new(4.0, 6.0),
            egui::Align2::LEFT_TOP,
            format!("{:.1} × {:.1} mm · {} shapes ({} removed)", f.width_mm, f.height_mm, kept, r.shapes.len() - kept),
            egui::FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
        let ten_mm = 10.0 * r.px_per_mm as f32 * scale;
        let start = rect.right_bottom() + Vec2::new(-ten_mm - 4.0, 12.0);
        painter.line_segment([start, start + Vec2::new(ten_mm, 0.0)], Stroke::new(3.0, ui.visuals().text_color()));
        painter.text(start + Vec2::new(ten_mm / 2.0, -3.0), egui::Align2::CENTER_BOTTOM, "10 mm", egui::FontId::proportional(11.0), ui.visuals().text_color());
    }
}

fn is_ancestor_or_self(r: &TraceResult, anc: usize, mut i: usize) -> bool {
    loop {
        if i == anc {
            return true;
        }
        match r.shapes[i].contour.parent {
            Some(p) => i = p,
            None => return false,
        }
    }
}

fn display_ppm(f: &FrameGeom, ppm: f64) -> f64 {
    let max_mm = f.width_mm.max(f.height_mm);
    ppm.min(MAX_TEXTURE as f64 / max_mm)
}

/// Photo texture, downscaled to fit GPU texture limits.
fn texture_image(rgba: &[u8], w: usize, h: usize) -> egui::ColorImage {
    if w.max(h) <= MAX_TEXTURE {
        return egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec()).unwrap();
    let s = MAX_TEXTURE as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * s) as u32).max(1), ((h as f64 * s) as u32).max(1));
    let small = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    egui::ColorImage::from_rgba_unmultiplied([nw as usize, nh as usize], small.as_raw())
}

fn mask_image(r: &TraceResult) -> egui::ColorImage {
    let mut rgba = vec![0u8; r.width * r.height * 4];
    for (i, &m) in r.mask.iter().enumerate() {
        if m == 1 {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[230, 40, 40, 110]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([r.width, r.height], &rgba)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);

        // Drag & drop.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(p) = dropped.first() {
            self.open(&ctx, p);
        }

        if self.step == Step::Trace && self.photo.is_some() {
            self.request(&ctx);
        }

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                ui.label(&self.status);
            });
        });
        egui::Panel::left("controls").resizable(true).default_size(300.0).show(ui, |ui| {
            self.side_panel(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if self.photo.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.heading("Click 'Open photo…' or drop a photo here.\n\nTake the photo with the whole frame visible,\nas square-on as is practical.");
                });
                return;
            }
            match self.step {
                Step::Frame => self.frame_view(ui),
                Step::Trace => self.trace_view(ui),
            }
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}
