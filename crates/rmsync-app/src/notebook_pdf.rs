//! Stitch a notebook's per-page thumbnail PNGs into a single multi-page
//! PDF the user can scroll through in their default viewer. This is a
//! preview, not faithful ink rendering — the thumbnails come from xochitl
//! and are at the device's UI resolution. Real `.rm`-to-PDF rendering is
//! a v2 task.
//!
//! Pages are emitted at A4 portrait (210×297mm) so the result feels like
//! "a piece of paper" when opened in Preview/Acrobat. Each thumbnail is
//! scaled to fill the page width while preserving aspect ratio, and is
//! vertically centered on the page.

use image::ImageReader;
use printpdf::{Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, Pt, RawImage, XObjectTransform};

const PAGE_W_MM: f32 = 210.0;
const PAGE_H_MM: f32 = 297.0;
const PT_PER_INCH: f32 = 72.0;
const MM_PER_INCH: f32 = 25.4;

fn mm_to_pt(mm: f32) -> f32 {
    mm * PT_PER_INCH / MM_PER_INCH
}

/// Build a multi-page PDF from the given page PNG byte buffers, in order.
/// Returns the encoded PDF bytes.
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

        // printpdf renders an image at `dpi` (default 300) — i.e. its
        // physical size in inches is image_px / dpi. Pick the smallest
        // DPI that still fits both axes inside the page; that maximises
        // the rendered image without cropping.
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

    let bytes = doc
        .with_pages(pdf_pages)
        .save(&PdfSaveOptions::default(), &mut warnings);
    Ok(bytes)
}
