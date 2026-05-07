//! Render a notebook into a multi-page A4 PDF.
//!
//! Two paths exist:
//! - `build_pdf_from_rm_files`: parse the device's `.rm` ink files (v6
//!   format, what current reMarkable firmware writes) and render each
//!   stroke as a vector polyline. Sharp at any zoom, low file size.
//! - `build_pdf_from_pngs`: stitch the device's per-page thumbnail PNGs
//!   into a PDF. Used as a fallback when a page has no `.rm` data.

use image::ImageReader;
use printpdf::{
    Color, Line as PdfLine, LineDashPattern, LinePoint, Mm, Op, PdfDocument, PdfPage,
    PdfSaveOptions, Point, Pt, RawImage, Rgb, XObjectTransform,
};
use rm_parser::v6::block::Block;
use rm_parser::RemarkableFile;

const PAGE_W_MM: f32 = 210.0;
const PAGE_H_MM: f32 = 297.0;
const PT_PER_INCH: f32 = 72.0;
const MM_PER_INCH: f32 = 25.4;
/// Margin around the strokes inside the page.
const MARGIN_MM: f32 = 10.0;
/// reMarkable canvas in pixels (portrait). Used as the default coordinate
/// space when the file's bounding box is degenerate.
const RM_CANVAS_W: f32 = 1404.0;
const RM_CANVAS_H: f32 = 1872.0;

fn mm_to_pt(mm: f32) -> f32 {
    mm * PT_PER_INCH / MM_PER_INCH
}

/// Build a multi-page A4 PDF from the per-page byte buffers, in order.
/// Each page is parsed as a v6 `.rm` file and rendered as vectors. Returns
/// the encoded PDF bytes.
pub fn build_pdf_from_rm_files(title: &str, pages: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("notebook has no pages".to_string());
    }

    let mut doc = PdfDocument::new(title);
    let mut warnings = Vec::new();
    let mut pdf_pages = Vec::with_capacity(pages.len());

    for (i, bytes) in pages.iter().enumerate() {
        let rm = RemarkableFile::read(bytes.as_slice())
            .map_err(|e| format!("page {i} parse: {e}"))?;
        let ops = render_rm_to_ops(&rm)?;
        pdf_pages.push(PdfPage::new(Mm(PAGE_W_MM), Mm(PAGE_H_MM), ops));
    }

    Ok(doc
        .with_pages(pdf_pages)
        .save(&PdfSaveOptions::default(), &mut warnings))
}

fn render_rm_to_ops(rm: &RemarkableFile) -> Result<Vec<Op>, String> {
    use rm_parser::shared::tool::Tool;

    // Collect every stroke from every Block::SceneLineItem.
    let lines: Vec<&rm_parser::v6::scene_item::line::Line> = match rm {
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

    // bbox so we can fit the page regardless of the coord system used.
    // Skip points from invisible tools (erasers/selections) so they don't
    // pull the bbox out — those strokes are filtered later anyway.
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for line in &lines {
        if !is_visible_tool(line.tool()) {
            continue;
        }
        for p in line.points() {
            min_x = min_x.min(p.x());
            min_y = min_y.min(p.y());
            max_x = max_x.max(p.x());
            max_y = max_y.max(p.y());
        }
    }
    if !min_x.is_finite() {
        // Nothing visible on the page.
        return Ok(vec![]);
    }
    let bbox_w = (max_x - min_x).max(RM_CANVAS_W * 0.1);
    let bbox_h = (max_y - min_y).max(RM_CANVAS_H * 0.1);

    let printable_w_mm = PAGE_W_MM - 2.0 * MARGIN_MM;
    let printable_h_mm = PAGE_H_MM - 2.0 * MARGIN_MM;
    let scale_x = printable_w_mm / bbox_w;
    let scale_y = printable_h_mm / bbox_h;
    let scale = scale_x.min(scale_y);
    let drawn_w_mm = bbox_w * scale;
    let drawn_h_mm = bbox_h * scale;
    let offset_x_mm = MARGIN_MM + (printable_w_mm - drawn_w_mm) / 2.0;
    let offset_y_mm = MARGIN_MM + (printable_h_mm - drawn_h_mm) / 2.0;

    let map_pt = |x: f32, y: f32| -> Point {
        let mx = offset_x_mm + (x - min_x) * scale;
        let my = PAGE_H_MM - offset_y_mm - (y - min_y) * scale;
        Point {
            x: Pt(mm_to_pt(mx)),
            y: Pt(mm_to_pt(my)),
        }
    };

    let mut ops: Vec<Op> = Vec::new();
    ops.push(Op::SetLineDashPattern {
        dash: LineDashPattern::default(),
    });

    // Order strokes so highlighters render first (stay underneath ink).
    let mut ordered: Vec<&rm_parser::v6::scene_item::line::Line> = lines.clone();
    ordered.sort_by_key(|l| match l.tool() {
        Tool::Highlighter => 0,
        _ => 1,
    });

    let mut last_rgb: Option<(f32, f32, f32)> = None;
    let mut last_width_pt: Option<f32> = None;

    for line in &ordered {
        if !is_visible_tool(line.tool()) {
            continue;
        }
        if line.points().len() < 2 {
            continue;
        }

        let rgb = stroke_color_rgb(line.tool(), line.color());
        let width_mm = stroke_width_mm(line);
        let width_pt = mm_to_pt(width_mm);

        if last_rgb != Some(rgb) {
            ops.push(Op::SetOutlineColor {
                col: Color::Rgb(Rgb {
                    r: rgb.0,
                    g: rgb.1,
                    b: rgb.2,
                    icc_profile: None,
                }),
            });
            last_rgb = Some(rgb);
        }
        if last_width_pt != Some(width_pt) {
            ops.push(Op::SetOutlineThickness {
                pt: Pt(width_pt),
            });
            last_width_pt = Some(width_pt);
        }

        let pts: Vec<LinePoint> = line
            .points()
            .iter()
            .map(|p| LinePoint {
                p: map_pt(p.x(), p.y()),
                bezier: false,
            })
            .collect();
        ops.push(Op::DrawLine {
            line: PdfLine {
                points: pts,
                is_closed: false,
            },
        });
    }

    Ok(ops)
}

fn is_visible_tool(tool: &rm_parser::shared::tool::Tool) -> bool {
    use rm_parser::shared::tool::Tool;
    !matches!(
        tool,
        Tool::Eraser | Tool::EraseArea | Tool::EraseAll | Tool::SelectionBrush
    )
}

/// Map (Tool, PenColor) to a stroke colour. Highlighters get tinted toward
/// the pastel of the chosen ink so they read as overlay marks even without
/// an alpha channel.
fn stroke_color_rgb(
    tool: &rm_parser::shared::tool::Tool,
    color: &rm_parser::shared::pen_color::PenColor,
) -> (f32, f32, f32) {
    use rm_parser::shared::{pen_color::PenColor, tool::Tool};
    match tool {
        Tool::Highlighter => match color {
            PenColor::Yellow => (1.00, 0.94, 0.40),
            PenColor::Green => (0.55, 0.95, 0.55),
            PenColor::Pink => (1.00, 0.65, 0.82),
            PenColor::Blue => (0.55, 0.80, 1.00),
            PenColor::Red => (1.00, 0.55, 0.55),
            _ => (1.00, 0.94, 0.40), // default highlighter yellow
        },
        Tool::Pencil | Tool::MechanicalPencil => {
            // Pencils on the device read as graphite — much lighter than
            // ink, never quite black. Bias hard toward mid-grey regardless
            // of which "colour" the user picked, since on the device pencil
            // is essentially a single greyscale tool.
            let base = pen_color_rgb(color);
            // 70% mid-grey, 30% original colour, then lighten further.
            let r = base.0 * 0.30 + 0.55;
            let g = base.1 * 0.30 + 0.55;
            let b = base.2 * 0.30 + 0.55;
            (r.min(1.0), g.min(1.0), b.min(1.0))
        }
        _ => pen_color_rgb(color),
    }
}

fn pen_color_rgb(color: &rm_parser::shared::pen_color::PenColor) -> (f32, f32, f32) {
    use rm_parser::shared::pen_color::PenColor;
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
        // Unknown colour code — most likely a colour reMarkable added in
        // a firmware update we haven't seen. Render as black so the
        // stroke is at least visible.
        PenColor::Unknown(_) => (0.00, 0.00, 0.00),
    }
}

/// Stroke width in mm. Honours the line's thickness_scale (set on the
/// device when the user picks fine/medium/thick) and applies a per-tool
/// multiplier that reflects each pen's natural footprint.
fn stroke_width_mm(line: &rm_parser::v6::scene_item::line::Line) -> f32 {
    use rm_parser::shared::tool::Tool;
    let base_mm = match line.tool() {
        Tool::BallPoint => 0.40,
        Tool::FineLiner => 0.35,
        Tool::Marker => 1.00,
        Tool::Brush => 0.85,
        // Pencils on the device are noticeably thinner and lighter than
        // ink. Without alpha support we fake the "graphite" look with a
        // small width and a desaturated colour (see stroke_color_rgb).
        Tool::Pencil => 0.25,
        Tool::MechanicalPencil => 0.20,
        Tool::Calligraphy => 0.85,
        Tool::Highlighter => 4.50,
        // Unknown tools and erasers/selection brush (which is_visible_tool
        // filters out, but be defensive). Render unknown as a generic ink
        // line so the user sees their content.
        Tool::Unknown(_) => 0.40,
        _ => 0.30,
    };
    let thickness = (line.thickness_scale() as f32).clamp(0.5, 3.0);
    base_mm * thickness
}

/// Build a multi-page PDF from the given page PNG byte buffers, in order.
/// Used as a fallback when a notebook has no `.rm` ink files (rare —
/// typically only pre-firmware-3 notebooks). Pages are A4 portrait;
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
