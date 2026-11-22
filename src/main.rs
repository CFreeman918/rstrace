#![allow(clippy::unnecessary_map_or, clippy::manual_range_contains)]
use clap::Parser;
use rstrace::{FrameGeom, Options, Pipeline, SvgStyle, TRACE_CAPTURE_MM};
use std::path::PathBuf;

#[cfg(feature = "gui")]
mod gui;
mod install;

/// Offline photo -> real-scale SVG tracer for Shaper Trace style frames.
///
/// Run without arguments to open the graphical app. Pass a photo to trace it
/// from the command line.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Photo of a drawing inside the frame. Omit to start the GUI.
    input: Option<PathBuf>,

    /// Output path. A .dxf extension writes DXF (default: input name with
    /// the --format extension).
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Frame opening size in mm, LONGxSHORT.
    #[arg(long, default_value_t = format!("{}x{}", TRACE_CAPTURE_MM.0, TRACE_CAPTURE_MM.1))]
    frame: String,

    /// Manual opening corners in photo pixels: x1,y1,x2,y2,x3,y3,x4,y4
    /// (top-left, top-right, bottom-right, bottom-left). Skips auto-detection.
    #[arg(long)]
    corners: Option<String>,

    /// Rotate the output by 90° clockwise this many times.
    #[arg(long, default_value_t = 0)]
    rotate: u8,

    /// Fixed threshold 0-255 (default: automatic).
    #[arg(long)]
    threshold: Option<f32>,

    /// Trace light marks on a dark background.
    #[arg(long)]
    invert: bool,

    /// Disable lighting correction.
    #[arg(long)]
    no_flatten: bool,

    /// Outline smoothing in mm.
    #[arg(long)]
    smoothing: Option<f64>,

    /// Curve fitting tolerance in mm.
    #[arg(long)]
    tolerance: Option<f64>,

    /// Corner angle threshold in degrees (180 = no sharp corners).
    #[arg(long)]
    corner_angle: Option<f64>,

    /// Remove specks smaller than this (mm²).
    #[arg(long)]
    despeckle: Option<f64>,

    /// Ignore this margin (mm) along the inside of the frame.
    #[arg(long)]
    inset: Option<f64>,

    /// Processing resolution in pixels per mm.
    #[arg(long)]
    resolution: Option<f64>,

    /// Don't recover faint line segments (hysteresis thresholding).
    #[arg(long)]
    no_faint: bool,

    /// Bridge gaps in lines up to this size (mm).
    #[arg(long)]
    close_gaps: Option<f64>,

    /// How closed pen outlines become paths.
    #[arg(long, value_enum)]
    lines: Option<rstrace::LineMode>,

    /// Output format when not implied by the --output extension.
    #[arg(long, value_enum)]
    format: Option<rstrace::ExportFormat>,

    /// SVG style.
    #[arg(long, value_enum)]
    style: Option<SvgStyle>,

    /// Size the SVG to the traced shapes instead of the whole frame.
    #[arg(long)]
    crop: bool,

    /// Open the photo in the GUI instead of tracing from the command line.
    #[arg(long)]
    gui: bool,

    /// Linux: install for this user (program in ~/.local/bin, applications
    /// menu entry and icon), so the icon shows on Wayland desktops.
    #[arg(long, conflicts_with = "uninstall")]
    install: bool,

    /// Linux: remove what --install added.
    #[arg(long)]
    uninstall: bool,

    /// Also write the rectified photo (PNG) to this path.
    #[arg(long)]
    debug_rectified: Option<PathBuf>,

    /// Also write the threshold mask (PNG, black = ink) to this path.
    #[arg(long)]
    debug_mask: Option<PathBuf>,
}

fn parse_pair(s: &str) -> Result<(f64, f64), String> {
    let (a, b) = s
        .split_once(['x', 'X', ','])
        .ok_or("expected LONGxSHORT, e.g. 263x173")?;
    let a: f64 = a.trim().parse().map_err(|_| "bad number in --frame")?;
    let b: f64 = b.trim().parse().map_err(|_| "bad number in --frame")?;
    if a <= 0.0 || b <= 0.0 {
        return Err("frame size must be positive".into());
    }
    Ok((a, b))
}

fn run_cli(cli: Cli, input: PathBuf) -> Result<(), String> {
    let photo = rstrace::load::load_photo(&input)?;
    let mut pipe = Pipeline::new(photo.rgba, photo.width, photo.height);
    let (long, short) = parse_pair(&cli.frame)?;
    let corners = match &cli.corners {
        Some(s) => {
            let v: Vec<f64> = s
                .split(',')
                .map(|t| t.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .map_err(|_| "bad --corners")?;
            if v.len() != 8 {
                return Err("--corners needs 8 numbers".into());
            }
            [[v[0], v[1]], [v[2], v[3]], [v[4], v[5]], [v[6], v[7]]]
        }
        None => {
            let d = pipe.detect().ok_or(
                "Could not find the frame opening. Pass --corners, or use the GUI to place them.",
            )?;
            eprintln!(
                "Frame corners: {}",
                d.corners
                    .iter()
                    .map(|p| format!("{:.1},{:.1}", p[0], p[1]))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            d.corners
        }
    };
    let mut frame = FrameGeom::oriented(corners, long, short);
    for _ in 0..(cli.rotate % 4) {
        frame = frame.rotated();
    }

    let mut o = Options::default();
    if let Some(t) = cli.threshold {
        o.auto_threshold = false;
        o.threshold = t;
    }
    o.invert = cli.invert;
    o.flatten = !cli.no_flatten;
    if let Some(v) = cli.smoothing {
        o.smoothing_mm = v;
    }
    if let Some(v) = cli.tolerance {
        o.tolerance_mm = v;
    }
    if let Some(v) = cli.corner_angle {
        o.corner_angle = v;
    }
    if let Some(v) = cli.despeckle {
        o.despeckle_mm2 = v;
    }
    if let Some(v) = cli.inset {
        o.inset_mm = v;
    }
    if let Some(v) = cli.resolution {
        o.px_per_mm = v;
    }
    if let Some(v) = cli.style {
        o.style = v;
    }
    if let Some(v) = cli.lines {
        o.line_mode = v;
    }
    if let Some(v) = cli.close_gaps {
        o.close_gaps_mm = v;
    }
    o.catch_faint = !cli.no_faint;
    o.crop_to_content = cli.crop;

    if let Some(p) = &cli.debug_rectified {
        let (w, h, data) = rstrace::core::pipeline::rectify_rgba(
            &pipe.rgba,
            pipe.width,
            pipe.height,
            &frame,
            o.px_per_mm,
        )
        .ok_or("degenerate corners")?;
        image::save_buffer(
            p,
            &data,
            w as u32,
            h as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| e.to_string())?;
    }

    let result = pipe.run(&frame, &o).ok_or("degenerate corners")?;
    if let Some(p) = &cli.debug_mask {
        let px: Vec<u8> = result
            .mask
            .iter()
            .map(|&m| if m == 1 { 0 } else { 255 })
            .collect();
        image::save_buffer(
            p,
            &px,
            result.width as u32,
            result.height as u32,
            image::ExtendedColorType::L8,
        )
        .map_err(|e| e.to_string())?;
    }
    let format = cli
        .output
        .as_deref()
        .and_then(rstrace::ExportFormat::from_path)
        .or(cli.format)
        .unwrap_or_default();
    let data = result.export(format, &frame, &o, &[]);
    let out = cli
        .output
        .clone()
        .unwrap_or_else(|| input.with_extension(format.extension()));
    std::fs::write(&out, data).map_err(|e| format!("Cannot write {}: {e}", out.display()))?;
    eprintln!(
        "Wrote {} ({} shapes, {:.1} x {:.1} mm, threshold {:.0})",
        out.display(),
        result.shapes.len(),
        frame.width_mm,
        frame.height_mm,
        result.threshold
    );
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    if cli.install || cli.uninstall {
        let res = if cli.install {
            install::install()
        } else {
            install::uninstall()
        };
        if let Err(e) = res {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(feature = "gui")]
    if cli.gui || cli.input.is_none() {
        if let Err(e) = gui::run(cli.input.clone()) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    match cli.input.clone() {
        Some(input) => {
            if let Err(e) = run_cli(cli, input) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("Built without the GUI. Pass a photo path; see --help.");
            std::process::exit(2);
        }
    }
}
