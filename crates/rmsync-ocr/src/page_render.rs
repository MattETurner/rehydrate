//! Rasterise one `.rm` v6 page's strokes onto an RGBA PNG suitable
//! for handing to a vision-LLM. We render strokes directly rather
//! than round-tripping through PDF + a PDF rasterizer — the
//! reMarkable canvas is small (1404×1872) and the visible content
//! is just black ink on white, so a simple anti-aliased line
//! drawing routine produces a clean image.
//!
//! Mirrors the per-tool width tuning in
//! `crates/rmsync-app/src/notebook_pdf.rs` so the OCR sees ink at
//! roughly the same proportions the user does.

use std::io::Cursor;

use image::{ImageBuffer, Rgba, RgbaImage};
use imageproc::drawing::draw_line_segment_mut;
use rm_parser::shared::tool::Tool;
use rm_parser::v6::block::Block;
use rm_parser::v6::scene_item::point::Point as RmPoint;
use rm_parser::RemarkableFile;

/// reMarkable canvas dimensions (px). We render at this size by
/// default — a 1024-px-tall PNG fits comfortably in Qwen2-VL's
/// preferred input range (its image encoder works in tiles of
/// 28×28, so any reasonable size is fine).
pub const RM_CANVAS_W: u32 = 1404;
pub const RM_CANVAS_H: u32 = 1872;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("parse: {0}")]
    Parse(String),
    #[error("encode: {0}")]
    Encode(#[from] image::ImageError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Render one `.rm` v6 page's strokes to PNG bytes at the canonical
/// canvas size. Returns the encoded PNG ready to ship to the OCR
/// backend.
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

    let mut img: RgbaImage = ImageBuffer::from_pixel(RM_CANVAS_W, RM_CANVAS_H, white());

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
            let pa = (a.x(), a.y());
            let pb = (b.x(), b.y());
            // Stroke width in pixels at the canvas's native resolution.
            let width_px = pixel_width_for(tool, a, thickness).max(1.0);
            // imageproc's `draw_line_segment_mut` is single-pixel; for
            // visible thickness we draw N parallel segments offset
            // perpendicular to the stroke — cheap and adequate for
            // OCR input where exact stroke width is decorative.
            stroke_segment(&mut img, pa, pb, width_px, colour);
        }
    }

    let mut out = Vec::with_capacity(64 * 1024);
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
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

/// Draw a thick anti-aliased line by walking parallel offsets. Not
/// the prettiest line drawing in the world, but it produces clean,
/// readable ink for a VLM that's looking at letterforms, not at
/// stroke aesthetics.
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
}
