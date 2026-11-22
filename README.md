# rstrace

An offline, cross-platform replacement for the Shaper Trace web app. It turns a
photo of a drawing inside a Shaper Trace frame (or any dark frame with a known
opening size) into a **real-scale SVG**. It is a single native executable with
no network access, no account and no runtime dependencies.

## How it works

1. **Find the frame.** The paper inside the frame is a bright region fully
   enclosed by the darker frame. rstrace closes thin dark lines (your drawing),
   finds that enclosed region, fits a quadrilateral to it, and then refines each
   side to sub-pixel accuracy on the full-resolution photo.
2. **Remove perspective.** A homography maps the four corners onto a rectangle
   of the frame's real size (default 263 × 173 mm, Shaper Trace's capture
   area), sampled at 8 px/mm.
3. **Threshold.** Uneven lighting and shadows are divided out by estimating the
   paper's brightness across the image. The result is thresholded (Otsu by
   default) with hysteresis: faint stretches of a line, such as the far side of
   an angled photo, are kept when they connect to clearly dark ink. Small gaps
   are then bridged, specks removed and pinholes filled.
4. **Lines vs. shapes.** A closed pen outline (for example, drawn around an
   object) has two edges. Thin closed loops are turned into one region whose
   edge follows the **centre** of the line (default), its **inside** edge
   (closest to the object you drew around) or its **outside** edge. Filled
   shapes and open strokes are traced as drawn, and nested loops keep their
   holes, so a washer drawn as two circles stays a washer.
5. **Trace.** Boundaries are followed along pixel edges. Sharp corners are
   detected and kept, and the rest is smoothed and fitted with cubic Béziers to
   a set tolerance (0.1 mm by default).
6. **Export.** The SVG is written in millimetres (`width="263mm"`). By default it
   uses Shaper Origin's cut encoding: outer boundaries get a black fill
   (exterior cut) and holes get a white fill with a black stroke (interior
   cut). Plain filled and outline-only styles are also available.

   **DXF** export (R12, the most widely readable flavour) is also available.
   It is in millimetres, with each shape as a closed polyline within 0.01 mm
   of the traced curve. Outer boundaries go on layer `OUTSIDE` and holes on
   layer `INSIDE`, ready for CAD, CAM, laser or CNC software.

## Using the app

Run `rstrace` with no arguments to open the window, or use
`rstrace --gui photo.jpg` to open straight onto a photo.

**Linux: add it to your applications menu (and get the icon).** Wayland
desktops only show an app's icon once it's installed with a desktop entry.
After building, run:

```sh
./target/release/rstrace --install     # just for you, no sudo needed
```

This copies the program to `~/.local/bin`, adds the icon and an
applications-menu entry, and lets you open images with rstrace from your file
manager. `rstrace --uninstall` removes it again. Re-run `--install` after
rebuilding to update the installed copy.

1. **Open photo…** (or drag a photo onto the window).
2. **1. Frame.** Check that the cyan quad sits on the *inside* edge of the
   frame. Drag the corners if needed; a magnifier appears while you drag. The
   orange edge becomes the top of the SVG, and **Rotate 90°** changes it.
3. **2. Trace.** Adjust the settings. Click a shape to remove it from the
   export (for example a smudge or a shadow) and click it again to bring it
   back. Turn on **Threshold mask** to see exactly which pixels count as ink.
4. Choose **SVG** or **DXF** under *Export*, then **Export…**

Settings (frame size, thresholds, etc.) are remembered between runs.

### Command line

```sh
rstrace photo.jpg                      # auto-detect, writes photo.svg
rstrace photo.jpg -o part.dxf          # DXF (format follows the extension)
rstrace photo.jpg --format dxf         # writes photo.dxf
rstrace photo.jpg -o part.svg --crop   # crop the SVG to the drawing
rstrace photo.jpg --frame 263x173 --smoothing 0.4 --style outline
rstrace photo.jpg --lines inside       # path on the inside of a drawn outline
rstrace photo.jpg --debug-mask m.png   # see which pixels counted as ink
rstrace photo.jpg --corners 812,604,3301,655,3260,2410,780,2350   # manual corners
rstrace --help
```

## Tips for good captures

- Keep the whole frame in the photo, with the frame's inside edge clearly
  visible against the paper.
- Shoot as square-on as practical. The frame lip sits a few millimetres above
  the paper, so very oblique photos introduce a small parallax error.
- Use even light and avoid harsh shadows from the frame. The **edge margin**
  setting ignores marks right next to the frame.
- Bold marker lines trace better than faint pencil.
- **Scale comes from the frame size.** The default 263 × 173 mm is Shaper's
  published capture area. On a real, fairly oblique test shot with a Shaper
  Trace frame, a 150 mm ruler outline measured 150.1 mm (pen-line centre to
  centre) with this default. If your results run consistently large or small,
  scale both numbers by the same factor.

## Building

You need a Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
cargo build --release        # binary at target/release/rstrace(.exe)
cargo test --release
cargo build --release --no-default-features   # CLI only, no GUI
```

- **Windows / macOS:** no extra dependencies.
- **Linux:** needs the usual desktop libraries at runtime (X11 or Wayland,
  OpenGL, `libxkbcommon`), which any desktop install already has.

On every push, CI (`.github/workflows/build.yml`) tests the project and builds
for Linux, Windows and macOS. Download them from the run's *Artifacts*:

- **Windows:** `rstrace.exe`, with the app icon embedded.
- **macOS:** `rstrace.app.zip`, an app bundle with its icon. It is unsigned,
  so after unzipping run `xattr -cr rstrace.app` once, or right-click it and
  choose *Open*.
- **Linux:** the `rstrace` binary. Run `./rstrace --install` once (see below).

The icon is drawn by `tools/make_icon.py` (Pillow), which writes
`assets/icon*.png` and `assets/icon.ico`.

To try it without a frame, `cargo test --release -- --ignored write_sample_photo`
writes a synthetic photo to `target/sample_photo.png`.

## Layout

```
src/core/frame.rs     frame detection + sub-pixel edge refinement
src/core/geometry.rs  homography, warping, line fitting, hulls
src/core/imgproc.rs   grayscale, lighting flattening, Otsu, labelling, morphology
src/core/trace.rs     contour following, corner detection, Bézier fitting
src/core/svg.rs       SVG writer (Shaper / filled / outline styles)
src/core/dxf.rs       DXF R12 writer (closed polylines, OUTSIDE/INSIDE layers)
src/core/pipeline.rs  stage-cached pipeline + shape selection
src/gui.rs            egui desktop app
src/main.rs           CLI + GUI entry point
tests/pipeline.rs     end-to-end test on a synthetic perspective photo
tests/real_photo.rs   regression test on a real Shaper Trace photo (150 mm ruler)
```

rstrace is an independent project and is not affiliated with Shaper Tools.
