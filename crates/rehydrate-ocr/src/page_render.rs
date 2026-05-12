//! Rasterise one `.rm` v6 page's strokes onto an RGBA PNG suitable
//! for handing to a vision-LLM. We render strokes directly rather
//! than round-tripping through PDF + a PDF rasterizer.
//!
//! Pipeline (each step driven by a quality lever for OCR accuracy):
//!
//! 1. **Supersample.** Render at `RENDER_SCALE`× the rM canvas size
//!    so the cheap parallel-offset stroke routine produces strokes
//!    whose perpendicular edges become anti-aliased after the
//!    downscale in step 2. Single-pixel `draw_line_segment_mut`
//!    output is fine at 2× when we know it'll get filtered.
//! 2. **Resize to the VLM's preferred long edge** with Lanczos3.
//!    Always preserves the rM canvas aspect ratio (3:4), which
//!    means the image arriving at mistral.rs always has the same
//!    shape. Stable shape matters: we previously tried a
//!    crop-to-ink step here, which bumped per-character resolution
//!    on sparse pages but produced variable aspect ratios that
//!    triggered an `index-select` OOB inside Qwen2.5-VL's
//!    windowed-attention path on Metal at otherwise-safe long-edge
//!    sizes (`vision.rs:488` and friends). Disabled for now;
//!    revisit when mistral.rs ships a fix.
//! 3. **Encode PNG.** Direct hand-off to the multimodal pipeline.
//!
//! Mirrors the per-tool width tuning in
//! `crates/rehydrate-app/src/notebook_pdf.rs` so the OCR sees ink at
//! roughly the same proportions the user does.

use std::io::Cursor;

use image::imageops::{resize, FilterType};
use image::{ImageBuffer, Rgba, RgbaImage};
use imageproc::drawing::draw_line_segment_mut;
use rm_parser::shared::tool::Tool;
use rm_parser::v6::block::Block;
use rm_parser::v6::scene_item::point::Point as RmPoint;
use rm_parser::RemarkableFile;

/// reMarkable canvas dimensions (px). Render space is this × `RENDER_SCALE`.
pub const RM_CANVAS_W: u32 = 1404;
pub const RM_CANVAS_H: u32 = 1872;

/// Render at this multiple of the rM canvas before downscaling. 2×
/// is enough to get clean anti-aliased strokes after the resize
/// pass — a 4× supersample gives a barely-visible quality bump for
/// 4× the rasterisation cost. Memory peak per page during render
/// is `RM_CANVAS_W * RM_CANVAS_H * 4 * RENDER_SCALE^2` bytes
/// (~42 MB at 2×).
const RENDER_SCALE: u32 = 2;

/// Target long-edge size handed to the VLM. 1280 = 40 × 32 (also a
/// clean multiple of mistral.rs's 28-px patch+merge factor: 28×46
/// = 1288, close enough that smart_resize lands here).
///
/// We tried 1568 (56 × 28, the natural "next size up") and it
/// triggered an out-of-bounds inside Qwen2.5-VL's m-RoPE path on
/// Metal — `index-select invalid index 1574 with dim size 1574`,
/// inside `mistralrs-core-0.8.1/src/vision_models/qwen2_5_vl/
/// vision.rs:395`. Couldn't reproduce the exact dimension chain
/// from the input arithmetic, so this looks like an upstream
/// edge-case at larger image sizes rather than something we can
/// patch around. 1280 is the largest value we've empirically
/// verified inferences cleanly.
///
/// Once mistral.rs ships a fix this can move back to 1568 — the
/// crop-to-ink + supersample pipeline above already gives most of
/// the per-character resolution gain we'd get from the bump.
const TARGET_LONG_EDGE: u32 = 1280;

// Crop-to-ink helpers are kept below for tests + future re-enable
// (see `crop_to_ink_bbox`). Disabled in the live pipeline because
// it produces variable image aspect ratios that occasionally
// trigger an upstream `index-select` OOB inside Qwen2.5-VL's
// windowed-attention path. Constants used by the helper:

/// Pixels of whitespace to keep around the cropped ink bbox at the
/// supersampled (`RENDER_SCALE`×) resolution. Generous margin so
/// stroke serifs and dot-of-i don't get clipped by the bbox
/// detector.
#[allow(dead_code)]
const INK_BBOX_MARGIN: u32 = 60 * RENDER_SCALE;

/// Brightness threshold (0–255) below which a pixel counts as ink
/// for the bbox computation.
const INK_BRIGHTNESS_THRESHOLD: u8 = 230;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("parse: {0}")]
    Parse(String),
    #[error("encode: {0}")]
    Encode(#[from] image::ImageError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Return true iff the page has any stroke content worth handing to
/// a vision-language model. We parse the `.rm` and look for at least
/// one `SceneLineItem` block with a non-empty `points` vector.
///
/// Why this matters: VLMs (qwen3.5:4b in particular) hallucinate
/// plausible essay-shaped prose when fed a pure-white canvas. The
/// user reported this on a brand-new notebook — completely empty
/// page → multi-paragraph generated text about "The Impact of
/// Artificial Intelligence on Healthcare". The model is
/// pattern-matching "page-in-a-paper" rather than transcribing.
/// No amount of prompt engineering reliably suppresses this on
/// blank input — the only robust fix is to never invoke the model
/// for a blank page in the first place.
///
/// Parse failures and non-v6 files are conservatively treated as
/// "no ink": the page renderer would also reject these, so passing
/// them to OCR would produce a confusing error or hallucinated
/// text either way.
pub fn rm_page_has_ink(rm_bytes: &[u8]) -> bool {
    let rm = match RemarkableFile::read(rm_bytes) {
        Ok(r) => r,
        Err(_) => return false,
    };
    match rm {
        RemarkableFile::V6 { blocks, .. } => blocks.iter().any(|b| {
            if let Block::SceneLineItem(item) = b {
                // A `SceneLineItem` whose inner value is None means
                // the line was removed in a later edit; only count
                // present-and-non-empty strokes as ink.
                item.item
                    .value
                    .as_ref()
                    .map(|line| !line.points().is_empty())
                    .unwrap_or(false)
            } else {
                false
            }
        }),
        RemarkableFile::Other { .. } => false,
    }
}

/// Render one `.rm` v6 page's strokes to PNG bytes, optimised for
/// vision-LLM consumption. See module docs for the pipeline.
pub fn render_rm_to_png(rm_bytes: &[u8]) -> Result<Vec<u8>, RenderError> {
    let rm = RemarkableFile::read(rm_bytes).map_err(|e| RenderError::Parse(e.to_string()))?;
    let lines: Vec<_> = match rm {
        RemarkableFile::V6 { blocks, .. } => blocks
            .into_iter()
            .filter_map(|b| {
                if let Block::SceneLineItem(item) = b {
                    item.item.value
                } else {
                    None
                }
            })
            .collect(),
        RemarkableFile::Other { .. } => {
            return Err(RenderError::Parse(
                "unsupported .rm version (only v6 strokes render today)".into(),
            ));
        }
    };

    // 1. Supersampled render space.
    let canvas_w = RM_CANVAS_W * RENDER_SCALE;
    let canvas_h = RM_CANVAS_H * RENDER_SCALE;
    let mut img: RgbaImage = ImageBuffer::from_pixel(canvas_w, canvas_h, white());

    // Compute the actual stroke bounding box and fit it into the
    // supersampled canvas with a small padding, exactly like
    // `notebook_pdf.rs` does for the PDF preview. The previous
    // version of this function assumed the .rm coordinates lived
    // inside the rM2 portrait canvas (1404×1872) and just multiplied
    // by `RENDER_SCALE`, which silently clipped:
    //   * landscape-orientation pages (x extends past 1404),
    //   * reMarkable Pro pages (different native canvas),
    //   * any v6 file whose authoring tool centres the origin
    //     differently from rM2 stock firmware.
    // The observed symptom was OCR transcribing only the right half
    // of the page — left-half strokes had x coordinates that
    // landed off the rendered canvas and were never drawn.
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for line in &lines {
        if !is_visible_tool(line.tool()) {
            continue;
        }
        for p in line.points() {
            min_x = min_x.min(p.x());
            max_x = max_x.max(p.x());
            min_y = min_y.min(p.y());
            max_y = max_y.max(p.y());
        }
    }
    if !min_x.is_finite() {
        // Blank page — encode the empty white canvas and return.
        let resized = resize_to_long_edge(&img);
        return encode_png(&resized);
    }
    // 2 % padding so strokes don't sit flush against the canvas
    // edge — the VLM sometimes treats edge pixels as page margin
    // and skips them.
    let pad = ((max_x - min_x).max(max_y - min_y)) * 0.02;
    min_x -= pad;
    max_x += pad;
    min_y -= pad;
    max_y += pad;
    let bbox_w = (max_x - min_x).max(1.0);
    let bbox_h = (max_y - min_y).max(1.0);

    // Aspect-preserving fit into the supersampled canvas. The
    // canvas is fixed at the rM2 portrait aspect; landscape /
    // wider-than-tall content gets letterboxed with white margins
    // (preferable to stretching, which would confuse the OCR
    // model on character glyphs).
    let xf = fit_transform(canvas_w, canvas_h, min_x, min_y, bbox_w, bbox_h);
    let fit = xf.fit;
    let map_xy = |x: f32, y: f32| -> (f32, f32) { xf.map(x, y) };

    for line in &lines {
        if !is_visible_tool(line.tool()) {
            continue;
        }
        let pts = line.points();
        if pts.len() < 2 {
            continue;
        }
        let thickness = (line.thickness_scale() as f32).clamp(0.3, 3.0);
        let tool = line.tool();
        let colour = stroke_colour(tool);
        for w in pts.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let pa = map_xy(a.x(), a.y());
            let pb = map_xy(b.x(), b.y());
            // `pixel_width_for` returns native-resolution px; scale
            // to the same fit factor so strokes keep their visual
            // proportion to the rasterised canvas. The upper clamp
            // protects against pathological cases: a notebook with
            // a single character produces a tiny bbox, which makes
            // `fit` huge and would otherwise turn a 3 px pen stroke
            // into a hundred-pixel-thick smear. `MAX_STROKE_PX` is
            // a generous ceiling — even a chunky highlighter at
            // canvas-fill scale tops out around 80 px.
            const MAX_STROKE_PX: f32 = 120.0;
            let width_px = (pixel_width_for(tool, a, thickness) * fit).clamp(1.0, MAX_STROKE_PX);
            stroke_segment(&mut img, pa, pb, width_px, colour);
        }
    }

    // 2. Resize so the long edge equals TARGET_LONG_EDGE,
    // preserving the canvas aspect ratio.
    let resized = resize_to_long_edge(&img);
    // 3. Encode PNG.
    encode_png(&resized)
}

/// Lanczos3 downscale to `TARGET_LONG_EDGE` on the longest side,
/// preserving the input's aspect ratio. Lanczos3 is the
/// highest-quality filter the `image` crate ships and is cheap on a
/// ~1500-px image; Triangle would also work; Nearest would
/// re-introduce the aliasing we paid the 2× supersample to remove.
/// Output shape is stable at the rM 3:4 portrait aspect, which is
/// useful for VLMs that prefer fixed input dimensions.
fn resize_to_long_edge(img: &RgbaImage) -> RgbaImage {
    let (cw, ch) = img.dimensions();
    let long_edge = cw.max(ch);
    let ratio = TARGET_LONG_EDGE as f32 / long_edge as f32;
    let new_w = ((cw as f32 * ratio).round() as u32).max(1);
    let new_h = ((ch as f32 * ratio).round() as u32).max(1);
    resize(img, new_w, new_h, FilterType::Lanczos3)
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, RenderError> {
    let mut out = Vec::with_capacity(64 * 1024);
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// Affine source-to-canvas transform: subtract `(min_x, min_y)`,
/// scale by `fit` (max possible while preserving aspect inside the
/// target), then offset to centre the letterbox.
#[derive(Debug, Clone, Copy)]
struct FitTransform {
    fit: f32,
    offset_x: f32,
    offset_y: f32,
    min_x: f32,
    min_y: f32,
}

impl FitTransform {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.min_x) * self.fit + self.offset_x,
            (y - self.min_y) * self.fit + self.offset_y,
        )
    }
}

fn fit_transform(
    canvas_w: u32,
    canvas_h: u32,
    min_x: f32,
    min_y: f32,
    bbox_w: f32,
    bbox_h: f32,
) -> FitTransform {
    let scale_x = canvas_w as f32 / bbox_w;
    let scale_y = canvas_h as f32 / bbox_h;
    let fit = scale_x.min(scale_y);
    let offset_x = (canvas_w as f32 - bbox_w * fit) * 0.5;
    let offset_y = (canvas_h as f32 - bbox_h * fit) * 0.5;
    FitTransform {
        fit,
        offset_x,
        offset_y,
        min_x,
        min_y,
    }
}

/// Crop the image to the bounding box of any pixel below the
/// brightness threshold, plus `margin` px of breathing room on
/// each side. Returns `None` for blank pages so the caller can
/// fall back to sending the original canvas.
///
/// Currently unused in the live pipeline (see module docs). Kept
/// behind `#[allow(dead_code)]` so the helper + tests stick around
/// for the day mistral.rs's Qwen2.5-VL windowed-attention bug
/// gets fixed and we can re-enable cropping for the per-character
/// resolution boost on sparse pages.
#[allow(dead_code)]
fn crop_to_ink_bbox(img: &RgbaImage, margin: u32) -> Option<RgbaImage> {
    let (w, h) = img.dimensions();
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0;
    let mut max_y = 0;
    let buf = img.as_raw();
    for y in 0..h {
        let row = (y * w * 4) as usize;
        for x in 0..w {
            let idx = row + (x as usize) * 4;
            // Min-channel brightness rather than mean — picks up
            // any dark colour channel, including the highlighter
            // yellow whose green/blue are near-white but red is too.
            // Actually for our palette (black/grey/yellow), the
            // mean of RGB is the simpler and adequate signal.
            let r = buf[idx] as u16;
            let g = buf[idx + 1] as u16;
            let b = buf[idx + 2] as u16;
            let brightness = ((r + g + b) / 3) as u8;
            if brightness < INK_BRIGHTNESS_THRESHOLD {
                if x < min_x {
                    min_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if x > max_x {
                    max_x = x;
                }
                if y > max_y {
                    max_y = y;
                }
            }
        }
    }
    if min_x > max_x {
        return None; // blank page
    }

    let cx0 = min_x.saturating_sub(margin);
    let cy0 = min_y.saturating_sub(margin);
    let cx1 = (max_x + margin + 1).min(w);
    let cy1 = (max_y + margin + 1).min(h);
    let cw = cx1 - cx0;
    let ch = cy1 - cy0;
    Some(image::imageops::crop_imm(img, cx0, cy0, cw, ch).to_image())
}

fn white() -> Rgba<u8> {
    Rgba([0xff, 0xff, 0xff, 0xff])
}

fn is_visible_tool(tool: &Tool) -> bool {
    !matches!(
        tool,
        Tool::Eraser | Tool::EraseArea | Tool::EraseAll | Tool::SelectionBrush
    )
}

fn stroke_colour(tool: &Tool) -> Rgba<u8> {
    match tool {
        Tool::Highlighter => Rgba([0xff, 0xea, 0x4a, 0x80]),
        Tool::Pencil | Tool::MechanicalPencil => Rgba([0x66, 0x66, 0x66, 0xff]),
        _ => Rgba([0x00, 0x00, 0x00, 0xff]),
    }
}

/// Per-point stroke thickness in pixels at the rM native canvas
/// resolution. Mirrors `notebook_pdf.rs::width_pt_for` but in pixel
/// units instead of PDF points so the rasterised image lines up
/// with the user's visual expectation.
fn pixel_width_for(tool: &Tool, p: &RmPoint, thickness: f32) -> f32 {
    let pressure_norm = (p.pressure() / 255.0).clamp(0.0, 1.0);
    let speed = p.speed().max(0.0);
    let raw_width = p.width();
    let width_mult = if raw_width > 0.0 {
        (raw_width / 300.0).clamp(0.3, 3.0)
    } else {
        1.0
    };

    // Base widths are in millimetres; the rM canvas is ≈ 156 mm wide
    // and 1404 px wide → 9 px / mm.
    const PX_PER_MM: f32 = 9.0;
    let base_mm = match tool {
        Tool::BallPoint => 0.40,
        Tool::FineLiner => 0.35,
        Tool::Marker => 1.00,
        Tool::Brush => 0.85,
        Tool::Pencil => 0.32,
        Tool::MechanicalPencil => 0.22,
        Tool::Calligraphy => 1.00,
        Tool::Highlighter => 4.50,
        _ => 0.40,
    };
    let pressure_curve = match tool {
        Tool::BallPoint => pressure_norm.powf(1.4) * 0.7 + 0.3,
        Tool::Brush => pressure_norm.powf(1.5) * 0.8 + 0.2,
        Tool::Pencil => pressure_norm.sqrt() * 0.6 + 0.4,
        Tool::MechanicalPencil => pressure_norm * 0.3 + 0.7,
        Tool::Calligraphy => pressure_norm * 0.6 + 0.4,
        Tool::FineLiner | Tool::Marker | Tool::Highlighter => 1.0,
        _ => 1.0,
    };
    let speed_factor = match tool {
        Tool::BallPoint => 1.0 / (1.0 + speed * 0.005),
        Tool::Brush => 1.0 / (1.0 + speed * 0.003),
        _ => 1.0,
    };
    base_mm * thickness * pressure_curve * speed_factor * width_mult * PX_PER_MM
}

/// Draw a thick line by walking parallel offsets. Single-pixel
/// drawing per offset; we lean on the supersample-then-Lanczos
/// pass downstream to anti-alias the perpendicular edges. Cheaper
/// than a proper covered-area rasteriser and good enough for a
/// VLM that's looking at letterforms, not at stroke aesthetics.
fn stroke_segment(
    img: &mut RgbaImage,
    a: (f32, f32),
    b: (f32, f32),
    width_px: f32,
    colour: Rgba<u8>,
) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        return;
    }
    let (nx, ny) = (-dy / len, dx / len);
    let half = (width_px / 2.0).max(0.5);
    let steps = (width_px.ceil() as i32).max(1);
    for s in -steps..=steps {
        let t = s as f32 / steps as f32; // -1 .. 1
        let off = t * half;
        let pa = (a.0 + nx * off, a.1 + ny * off);
        let pb = (b.0 + nx * off, b.1 + ny * off);
        draw_line_segment_mut(img, pa, pb, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_rm_bytes_fail_with_parse_error() {
        let r = render_rm_to_png(b"");
        assert!(matches!(r, Err(RenderError::Parse(_))));
    }

    #[test]
    fn rm_page_has_ink_is_false_for_header_only_v6() {
        // 43-byte v6 header with no blocks — the shape of a freshly
        // created notebook page. Pre-fix this fell through to the
        // renderer, produced a pure-white PNG, and the VLM
        // hallucinated essay text. The predicate must catch this.
        let mut bytes = Vec::with_capacity(43);
        bytes.extend_from_slice(b"reMarkable .lines file, version=6");
        bytes.resize(43, b' ');
        assert!(
            !rm_page_has_ink(&bytes),
            "header-only v6 file must report no ink",
        );
    }

    #[test]
    fn rm_page_has_ink_is_false_for_unparseable_bytes() {
        // Defensive: parse failures count as "no ink" so we never
        // hand garbage bytes to the model. The renderer would also
        // error on these, but doing it here lets the OCR dispatcher
        // skip the model call cleanly instead of erroring per-page.
        assert!(!rm_page_has_ink(b""));
        assert!(!rm_page_has_ink(b"not a remarkable file"));
    }

    #[test]
    fn crop_to_ink_bbox_blank_returns_none() {
        let img = ImageBuffer::from_pixel(100, 100, white());
        assert!(crop_to_ink_bbox(&img, 10).is_none());
    }

    #[test]
    fn crop_to_ink_bbox_finds_tight_box_with_margin() {
        // Black pixel at (30, 40); margin 5 should give us a box
        // from (25, 35) inclusive to (36, 46) inclusive — i.e. 11x11.
        let mut img = ImageBuffer::from_pixel(100, 100, white());
        img.put_pixel(30, 40, Rgba([0, 0, 0, 255]));
        let cropped = crop_to_ink_bbox(&img, 5).expect("non-blank");
        let (w, h) = cropped.dimensions();
        assert_eq!((w, h), (11, 11));
    }

    #[test]
    fn crop_to_ink_bbox_clamps_margin_to_image_edges() {
        // Ink right at (0, 0); margin would underflow.
        let mut img = ImageBuffer::from_pixel(20, 20, white());
        img.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        let cropped = crop_to_ink_bbox(&img, 5).expect("non-blank");
        let (w, h) = cropped.dimensions();
        // saturating_sub clamps the start to 0; end is 0+5+1=6.
        assert_eq!((w, h), (6, 6));
    }

    #[test]
    fn fit_transform_centres_portrait_bbox_in_portrait_canvas() {
        // Native rM2 portrait bbox into the same-shape canvas: no
        // letterbox, scale ratio ≈ 1.0, both axes filled.
        let xf = fit_transform(2808, 3744, 0.0, 0.0, 1404.0, 1872.0);
        assert!((xf.fit - 2.0).abs() < 1e-3);
        assert!((xf.offset_x).abs() < 1e-3);
        assert!((xf.offset_y).abs() < 1e-3);
        let (px, py) = xf.map(1404.0, 1872.0);
        assert!((px - 2808.0).abs() < 1.0);
        assert!((py - 3744.0).abs() < 1.0);
    }

    #[test]
    fn fit_transform_brings_landscape_bbox_inside_canvas() {
        // Regression: previously, strokes at x > RM_CANVAS_W (e.g.
        // landscape orientation, rM Pro, or any wider canvas) were
        // multiplied straight by `RENDER_SCALE`, landing them off
        // the rasterisation buffer — half the page never got
        // rendered and the OCR only saw the right half.
        //
        // The fit transform must map the full landscape bbox
        // (1872 × 1404) into the portrait canvas (2808 × 3744)
        // with letterbox top/bottom and *every* x within
        // [0, canvas_w].
        let xf = fit_transform(2808, 3744, 0.0, 0.0, 1872.0, 1404.0);
        // Far-right of the source landscape page lands inside the
        // canvas, not past it.
        let (px, _) = xf.map(1872.0, 0.0);
        assert!((0.0..=2808.0).contains(&px), "x out of canvas: {px}");
        let (px0, _) = xf.map(0.0, 0.0);
        assert!((0.0..2808.0).contains(&px0), "x0 out of canvas: {px0}");
    }

    #[test]
    fn fit_transform_handles_offset_origin() {
        // Some .rm authoring tools centre the origin or use a
        // negative-x range. The fit transform must subtract the
        // minimum before scaling, so negative-x strokes still land
        // inside the canvas rather than getting clipped at 0.
        let xf = fit_transform(2808, 3744, -500.0, -100.0, 2000.0, 1500.0);
        let (px, py) = xf.map(-500.0, -100.0);
        assert!(
            (0.0..2808.0).contains(&px),
            "negative-x stroke not mapped in: {px}"
        );
        assert!(
            (0.0..3744.0).contains(&py),
            "negative-y stroke not mapped in: {py}"
        );
    }
}
