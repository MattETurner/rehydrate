//! Render a notebook into a multi-page A4 PDF.
//!
//! Two paths exist:
//! - `build_pdf_from_rm_files`: parse the device's `.rm` ink files (v6
//!   format) and render each stroke as a filled, variable-width
//!   tessellated polygon. Sharp at any zoom, real pressure variation.
//! - `build_pdf_from_pngs`: stitch the device's per-page thumbnail PNGs
//!   into a PDF. Used as a fallback when a page has no `.rm` data.
//!
//! Rendering approach (research-backed; see commit message for sources):
//! PDF has no native variable-width stroke primitive, so for each stroke
//! we build a quad strip along the centerline. For each consecutive pair
//! of points (P_i, P_{i+1}) we compute the segment normal and offset
//! left/right by half the per-point width. The quads are emitted as
//! filled rings inside one Op::DrawPolygon per stroke. Joint gaps and
//! stroke tips are filled with a small disc at each point — gives natural
//! rounded caps and hides the seam where adjacent segments meet at
//! varying widths.
//!
//! Winding: every ring is wound clockwise in PDF (Y-up) space so
//! NonZero winding never cancels overlapping rings into holes. Mixing
//! orientations would punch disc-shaped voids through the stroke at
//! every point.

use image::ImageReader;
use printpdf::{
    Color, LineDashPattern, LinePoint, Mm, Op, PaintMode, PdfDocument, PdfPage, PdfSaveOptions,
    Point, Polygon, PolygonRing, Pt, RawImage, Rgb, WindingOrder, XObjectTransform,
};
use rm_parser::shared::{pen_color::PenColor, tool::Tool};
use rm_parser::v6::block::Block;
use rm_parser::v6::scene_item::line::Line;
use rm_parser::v6::scene_item::point::Point as RmPoint;
use rm_parser::RemarkableFile;

const PAGE_W_MM: f32 = 210.0;
const PAGE_H_MM: f32 = 297.0;
const PT_PER_INCH: f32 = 72.0;
const MM_PER_INCH: f32 = 25.4;

fn mm_to_pt(mm: f32) -> f32 {
    mm * PT_PER_INCH / MM_PER_INCH
}

/// Build a multi-page A4 PDF from `.rm` v6 byte buffers, in order.
pub fn build_pdf_from_rm_files(title: &str, pages: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("notebook has no pages".to_string());
    }

    let mut doc = PdfDocument::new(title);
    let mut warnings = Vec::new();
    let mut pdf_pages = Vec::with_capacity(pages.len());

    for (i, bytes) in pages.iter().enumerate() {
        let rm =
            RemarkableFile::read(bytes.as_slice()).map_err(|e| format!("page {i} parse: {e}"))?;
        let ops = render_rm_to_ops(&rm)?;
        pdf_pages.push(PdfPage::new(Mm(PAGE_W_MM), Mm(PAGE_H_MM), ops));
    }

    Ok(doc
        .with_pages(pdf_pages)
        .save(&PdfSaveOptions::default(), &mut warnings))
}

fn render_rm_to_ops(rm: &RemarkableFile) -> Result<Vec<Op>, String> {
    let lines: Vec<&Line> = match rm {
        RemarkableFile::V6 { blocks, .. } => blocks
            .iter()
            .filter_map(|b| {
                if let Block::SceneLineItem(item) = b {
                    item.item.value.as_ref()
                } else {
                    None
                }
            })
            .collect(),
        RemarkableFile::Other { .. } => {
            return Err("unsupported .rm version (only v6 strokes are rendered today)".into());
        }
    };

    if lines.is_empty() {
        return Ok(vec![]);
    }

    // Fit the actual stroke bbox to A4 (with a small breathing margin),
    // preserving aspect ratio. Avoids reproducing the empty canvas the
    // user didn't write on — a notebook page where the user only wrote
    // in the bottom third would otherwise leave 2/3 of A4 blank.
    const PAGE_MARGIN_MM: f32 = 8.0;

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
        return Ok(vec![]);
    }
    // Pad bbox by a fraction so strokes don't kiss the page edge.
    let pad = ((max_x - min_x).max(max_y - min_y)) * 0.02;
    min_x -= pad;
    max_x += pad;
    min_y -= pad;
    max_y += pad;
    let canvas_w = (max_x - min_x).max(1.0);
    let canvas_h = (max_y - min_y).max(1.0);

    // Aspect-preserving fit into the printable area.
    let printable_w_mm = PAGE_W_MM - 2.0 * PAGE_MARGIN_MM;
    let printable_h_mm = PAGE_H_MM - 2.0 * PAGE_MARGIN_MM;
    let scale_mm_per_px_w = printable_w_mm / canvas_w;
    let scale_mm_per_px_h = printable_h_mm / canvas_h;
    let scale_mm_per_px = scale_mm_per_px_w.min(scale_mm_per_px_h);
    let drawn_w_mm = canvas_w * scale_mm_per_px;

    // Horizontal: centre the bbox on the page (looks balanced for
    // notebook-style content that doesn't fill the width).
    // Vertical: top-anchor — the user's first written line should land
    // near the top of the A4, not floating in the middle. The flip
    // below means small canvas_y maps to high pdf_y_mm; we want
    // canvas_y=0 to land at PAGE_H_MM - PAGE_MARGIN_MM (near top).
    let offset_x_mm = (PAGE_W_MM - drawn_w_mm) / 2.0;
    let offset_y_mm = PAGE_MARGIN_MM;
    let _ = printable_h_mm; // referenced above for the height-bound branch

    // PDF y axis points up; ink y axis points down — flip on map.
    let map_xy = |x: f32, y: f32| -> Point {
        let canvas_x = x - min_x;
        let canvas_y = y - min_y;
        let pdf_x_mm = offset_x_mm + canvas_x * scale_mm_per_px;
        let pdf_y_mm = PAGE_H_MM - offset_y_mm - canvas_y * scale_mm_per_px;
        Point {
            x: Pt(mm_to_pt(pdf_x_mm)),
            y: Pt(mm_to_pt(pdf_y_mm)),
        }
    };

    let mut ops: Vec<Op> = Vec::new();
    ops.push(Op::SetLineDashPattern {
        dash: LineDashPattern::default(),
    });

    // Render highlighters first so they sit underneath ink.
    let mut ordered: Vec<&Line> = lines.clone();
    ordered.sort_by_key(|l| match l.tool() {
        Tool::Highlighter => 0,
        _ => 1,
    });

    let mut last_rgb: Option<(f32, f32, f32)> = None;

    for line in &ordered {
        if !is_visible_tool(line.tool()) {
            continue;
        }
        let pts = line.points();
        if pts.len() < 2 {
            continue;
        }

        let rgb = stroke_color_rgb(line.tool(), line.color());
        if last_rgb != Some(rgb) {
            // We render strokes as filled polygons, so colour goes on the
            // *fill* state. Outline ops are unused by this renderer.
            ops.push(Op::SetFillColor {
                col: Color::Rgb(Rgb {
                    r: rgb.0,
                    g: rgb.1,
                    b: rgb.2,
                    icc_profile: None,
                }),
            });
            last_rgb = Some(rgb);
        }

        let thickness = (line.thickness_scale() as f32).clamp(0.3, 3.0);
        let tool = line.tool();

        // Build one polygon with one ring per segment-quad plus one ring
        // per per-point disc. printpdf renders this as a single fill op.
        let mut rings: Vec<PolygonRing> = Vec::with_capacity(pts.len() * 2);

        // Pre-compute per-point widths (in pt) for reuse.
        let widths_pt: Vec<f32> = pts
            .iter()
            .map(|p| width_pt_for(tool, p, thickness))
            .collect();

        // Quad strip: one ring per segment.
        for i in 0..pts.len() - 1 {
            let p0 = &pts[i];
            let p1 = &pts[i + 1];
            let pos0 = map_xy(p0.x(), p0.y());
            let pos1 = map_xy(p1.x(), p1.y());
            let dx = pos1.x.0 - pos0.x.0;
            let dy = pos1.y.0 - pos0.y.0;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 0.05 {
                // Degenerate segment — the disc at the point covers it.
                continue;
            }
            let nx = -dy / len;
            let ny = dx / len;

            let half0 = (widths_pt[i] / 2.0).max(0.05);
            let half1 = (widths_pt[i + 1] / 2.0).max(0.05);

            let q = vec![
                LinePoint {
                    p: Point {
                        x: Pt(pos0.x.0 + nx * half0),
                        y: Pt(pos0.y.0 + ny * half0),
                    },
                    bezier: false,
                },
                LinePoint {
                    p: Point {
                        x: Pt(pos1.x.0 + nx * half1),
                        y: Pt(pos1.y.0 + ny * half1),
                    },
                    bezier: false,
                },
                LinePoint {
                    p: Point {
                        x: Pt(pos1.x.0 - nx * half1),
                        y: Pt(pos1.y.0 - ny * half1),
                    },
                    bezier: false,
                },
                LinePoint {
                    p: Point {
                        x: Pt(pos0.x.0 - nx * half0),
                        y: Pt(pos0.y.0 - ny * half0),
                    },
                    bezier: false,
                },
            ];
            rings.push(PolygonRing { points: q });
        }

        // Disc at every point — naturally rounds caps and fills joint gaps.
        for (i, p) in pts.iter().enumerate() {
            let pos = map_xy(p.x(), p.y());
            let r = (widths_pt[i] / 2.0).max(0.05);
            rings.push(PolygonRing {
                points: disc(pos, r),
            });
        }

        if !rings.is_empty() {
            ops.push(Op::DrawPolygon {
                polygon: Polygon {
                    rings,
                    mode: PaintMode::Fill,
                    winding_order: WindingOrder::NonZero,
                },
            });
        }
    }

    Ok(ops)
}

/// 16-vertex approximation of a circle, wound clockwise in PDF (Y-up)
/// space to match the segment-quad winding. Sixteen sides keep caps
/// looking round at the largest brush/highlighter widths.
fn disc(center: Point, radius_pt: f32) -> Vec<LinePoint> {
    use std::f32::consts::PI;
    const SIDES: usize = 16;
    (0..SIDES)
        .map(|i| {
            // Negative angle ⇒ clockwise traversal in Y-up coordinates.
            let a = -(i as f32) * PI * 2.0 / (SIDES as f32);
            LinePoint {
                p: Point {
                    x: Pt(center.x.0 + radius_pt * a.cos()),
                    y: Pt(center.y.0 + radius_pt * a.sin()),
                },
                bezier: false,
            }
        })
        .collect()
}

fn is_visible_tool(tool: &Tool) -> bool {
    !matches!(
        tool,
        Tool::Eraser | Tool::EraseArea | Tool::EraseAll | Tool::SelectionBrush
    )
}

/// Per-point rendered width, in PDF points (1 pt = 1/72 inch).
///
/// reMarkable point fields are roughly:
/// - `pressure`: 0..255 (u8 in v2 format, float×255 in v1 format)
/// - `speed`: pixel-distance per sample, typically 0..200
/// - `width`: tool-specific multiplier, u16 in v2 (typical 0..2000-ish)
///
/// Width formulas adapted from open-source rm renderers (rm2svg.py,
/// Nemoworld) and hand-tuned for the look of the reMarkable 2.
fn width_pt_for(tool: &Tool, p: &RmPoint, thickness: f32) -> f32 {
    let pressure_norm = (p.pressure() / 255.0).clamp(0.0, 1.0);
    let speed = p.speed().max(0.0);
    // The raw `width` field is a tool-internal multiplier. v1 stores it
    // as f32×4, v2 stores it as a u16. Empirically values cluster around
    // a few hundred; normalise toward 1.0 so the per-tool base width does
    // most of the work and the multiplier just adds variation.
    let raw_width = p.width();
    let width_mult = if raw_width > 0.0 {
        // log-soft normalisation: maps 30→0.5, 300→1.0, 3000→2.0 roughly.
        (raw_width / 300.0).clamp(0.3, 3.0)
    } else {
        1.0
    };

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
        Tool::BallPoint => pressure_norm.powf(1.4) * 0.7 + 0.3, // 30%..100%
        Tool::Brush => pressure_norm.powf(1.5) * 0.8 + 0.2,     // 20%..100%
        Tool::Pencil => pressure_norm.sqrt() * 0.6 + 0.4,       // 40%..100%, gentle
        Tool::MechanicalPencil => pressure_norm * 0.3 + 0.7,    // mostly fixed
        Tool::Calligraphy => pressure_norm * 0.6 + 0.4,
        Tool::FineLiner | Tool::Marker | Tool::Highlighter => 1.0,
        _ => 1.0,
    };

    // Speed thinning: ballpoints and brushes get noticeably narrower when
    // drawn fast (high speed). Other pens are speed-insensitive.
    let speed_factor = match tool {
        Tool::BallPoint => 1.0 / (1.0 + speed * 0.005),
        Tool::Brush => 1.0 / (1.0 + speed * 0.003),
        _ => 1.0,
    };

    let mm = base_mm * thickness * pressure_curve * speed_factor * width_mult;
    mm_to_pt(mm.max(0.04))
}

fn stroke_color_rgb(tool: &Tool, color: &PenColor) -> (f32, f32, f32) {
    match tool {
        Tool::Highlighter => match color {
            PenColor::Yellow => (1.00, 0.94, 0.40),
            PenColor::Green => (0.55, 0.95, 0.55),
            PenColor::Pink => (1.00, 0.65, 0.82),
            PenColor::Blue => (0.55, 0.80, 1.00),
            PenColor::Red => (1.00, 0.55, 0.55),
            _ => (1.00, 0.94, 0.40),
        },
        Tool::Pencil | Tool::MechanicalPencil => {
            // Bias hard toward graphite-grey regardless of nominal colour.
            let base = pen_color_rgb(color);
            let r = base.0 * 0.30 + 0.55;
            let g = base.1 * 0.30 + 0.55;
            let b = base.2 * 0.30 + 0.55;
            (r.min(1.0), g.min(1.0), b.min(1.0))
        }
        _ => pen_color_rgb(color),
    }
}

fn pen_color_rgb(color: &PenColor) -> (f32, f32, f32) {
    match color {
        PenColor::Black => (0.00, 0.00, 0.00),
        PenColor::Grey => (0.50, 0.50, 0.50),
        PenColor::GreyOverlap => (0.55, 0.55, 0.55),
        PenColor::White => (1.00, 1.00, 1.00),
        PenColor::Yellow => (0.95, 0.85, 0.00),
        PenColor::Green => (0.00, 0.65, 0.31),
        PenColor::Pink => (0.94, 0.42, 0.65),
        PenColor::Blue => (0.00, 0.40, 0.85),
        PenColor::Red => (0.85, 0.15, 0.15),
        PenColor::Unknown(_) => (0.00, 0.00, 0.00),
    }
}

/// Build a multi-page PDF from PNG byte buffers. Used as a fallback when
/// a notebook has no parseable `.rm` ink files. Pages are A4 portrait;
/// thumbnails are scaled to fit while preserving aspect ratio.
pub fn build_pdf_from_pngs(title: &str, pages: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("notebook has no thumbnail pages to render".to_string());
    }

    let mut doc = PdfDocument::new(title);
    let mut warnings = Vec::new();
    let mut pdf_pages = Vec::with_capacity(pages.len());

    let page_w_in = PAGE_W_MM / MM_PER_INCH;
    let page_h_in = PAGE_H_MM / MM_PER_INCH;
    let page_w_pt = mm_to_pt(PAGE_W_MM);
    let page_h_pt = mm_to_pt(PAGE_H_MM);

    for (i, png_bytes) in pages.iter().enumerate() {
        let dims = ImageReader::new(std::io::Cursor::new(png_bytes))
            .with_guessed_format()
            .map_err(|e| format!("page {i}: {e}"))?
            .into_dimensions()
            .map_err(|e| format!("page {i} dimensions: {e}"))?;

        let raw = RawImage::decode_from_bytes(png_bytes, &mut warnings)
            .map_err(|e| format!("page {i} decode: {e}"))?;
        let xobject_id = doc.add_image(&raw);

        let dpi_for_width = dims.0 as f32 / page_w_in;
        let dpi_for_height = dims.1 as f32 / page_h_in;
        let dpi = dpi_for_width.max(dpi_for_height);

        let drawn_w_in = dims.0 as f32 / dpi;
        let drawn_h_in = dims.1 as f32 / dpi;
        let drawn_w_pt = drawn_w_in * PT_PER_INCH;
        let drawn_h_pt = drawn_h_in * PT_PER_INCH;
        let translate_x_pt = (page_w_pt - drawn_w_pt) / 2.0;
        let translate_y_pt = (page_h_pt - drawn_h_pt) / 2.0;

        let page = PdfPage::new(
            Mm(PAGE_W_MM),
            Mm(PAGE_H_MM),
            vec![Op::UseXobject {
                id: xobject_id,
                transform: XObjectTransform {
                    translate_x: Some(Pt(translate_x_pt)),
                    translate_y: Some(Pt(translate_y_pt)),
                    scale_x: None,
                    scale_y: None,
                    rotate: None,
                    dpi: Some(dpi),
                },
            }],
        );
        pdf_pages.push(page);
    }

    Ok(doc
        .with_pages(pdf_pages)
        .save(&PdfSaveOptions::default(), &mut warnings))
}
