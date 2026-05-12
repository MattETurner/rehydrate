//! End-to-end smoke against a live local Ollama daemon.
//!
//! Reproduces the user's failure mode: a freshly-unloaded
//! vision model + a notebook-sized PNG hits a long
//! `prompt_eval` phase during which Ollama stays silent on
//! the socket. The previous 120s `timeout_read` fired in that
//! gap and surfaced as "Error encountered in the status line:
//! timed out reading response" — even though the daemon was
//! healthy and Settings → Test Connection said so.
//!
//! Run with:
//!
//! ```text
//! cargo run --example ocr_validate -p rehydrate-ocr
//! ```
//!
//! Prerequisites: Ollama running on `http://localhost:11434`
//! and `qwen3.5:4b` pulled. The example generates its own
//! test image so no fixtures are needed.
//!
//! Expected output on the post-fix code path (Apple Silicon
//! laptop, model cold): one page transcribed in ~200s with a
//! non-empty preview. A failure here is a regression of the
//! GENERATE_TIMEOUT ceiling — bump and re-validate.

use image::{ImageBuffer, Rgb, RgbImage};
use rehydrate_ocr::{OcrBackend, OcrCancel, OllamaBackend, TranscribeOptions};
use std::time::Instant;

fn synth_test_png() -> Vec<u8> {
    // 960×1280 — same long-edge order as page_render.rs's
    // TARGET_LONG_EDGE, the size the real OCR backend sends.
    let (w, h) = (960u32, 1280u32);
    let mut img: RgbImage = ImageBuffer::from_pixel(w, h, Rgb([255, 255, 255]));
    // Stamp a few rows of pixel "text" so the model has tokens
    // worth transcribing — the exact glyphs don't matter, only
    // that prompt_eval has to encode a non-trivial image.
    for y in (40..h - 40).step_by(40) {
        for x in (50..w - 50).step_by(8) {
            let pixel = img.get_pixel_mut(x, y);
            *pixel = Rgb([0, 0, 0]);
        }
    }
    let mut out = Vec::with_capacity(64 * 1024);
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("png encode");
    out
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let png = synth_test_png();
    let backend = OllamaBackend::new("http://localhost:11434", "qwen3.5:4b")
        .expect("local ollama URL parses");

    // Unload the model first to reproduce the cold-prompt_eval
    // path. Anything that takes the model out of the keep-alive
    // pool works; `keep_alive: 0` on a no-op generate evicts it.
    let _ = std::process::Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            "http://localhost:11434/api/generate",
            "-d",
            r#"{"model":"qwen3.5:4b","keep_alive":0}"#,
        ])
        .output();
    std::thread::sleep(std::time::Duration::from_secs(2));

    let t0 = Instant::now();
    eprintln!(
        "[{:>6.1}s] dispatching OCR request ({}KB png)",
        t0.elapsed().as_secs_f64(),
        png.len() / 1024
    );
    let opts = TranscribeOptions {
        language: None,
        markdown: false,
    };
    let cancel = OcrCancel::new();
    let res = backend
        .transcribe_pages(vec![png], &opts, None, cancel)
        .await;
    let elapsed = t0.elapsed().as_secs_f64();

    match res {
        Ok(pages) => {
            let chars = pages.first().map(|p| p.text.chars().count()).unwrap_or(0);
            eprintln!(
                "[{elapsed:>6.1}s] OK — {} page(s), {} chars first page",
                pages.len(),
                chars
            );
            if let Some(p) = pages.first() {
                let preview: String = p.text.chars().take(120).collect();
                eprintln!("    preview: {preview}");
            }
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("[{elapsed:>6.1}s] FAIL — {e}");
            std::process::exit(1);
        }
    }
}
