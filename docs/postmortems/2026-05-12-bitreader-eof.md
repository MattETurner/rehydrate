# Postmortem: notebook OCR + render regression (2026-05-12)

A phase-4 audit change to the v6 binary parser silently broke
notebook parsing for every document on the device. OCR failed
outright with a confusing message; the notebook-preview path
caught the parse error and quietly cached a blurry
thumbnail-stitch PDF in its place. Users reported "OCR failed for
…: page 0 render failed: parse: Error while parsing remarkable
file Failed to read eof of bitreader when trying to add context"
and "many of my notebooks render with the default blurry image."

The buggy commit shipped in `581de75` (audit phase 4); the parser
fix landed in `3f67ec3` and the cache-invalidation follow-up in
`0d78c9a`.

## What went wrong

The audit added a bounds check to `Bitreader::read_bytes` to
defend against `vec![0; ~4_000_000_000]` allocations driven by a
malformed varuint length prefix (audit H-2). The new code path
returned `ParseError::invalid(...)` — kind
`ParseErrorKind::InvalidInput` — when the requested byte count
exceeded the remaining stream.

`Bitreader::eof()` was already in the crate. Its body:

```rust
pub fn eof(&mut self) -> Result<bool, ParseError> {
    let pos = self.position();
    match self.read_bytes(1) {
        Ok(_)  => { self.set_position(pos); Ok(false) }
        Err(e) => {
            if e.kind == ParseErrorKind::Io {
                self.set_position(pos);
                return Ok(true);
            }
            return Err(e);
        }
    }
}
```

Every parser loop in the crate uses `eof()` to know when to stop.
Before the audit change, an empty buffer caused `read_exact` to
return `io::Error::UnexpectedEof`, which converts to `ParseError`
with kind `Io`. After: my bounds guard returned `InvalidInput`
instead, so `eof()` treated a normal end-of-stream as a malformed
input. The `loop { if !eof()? { Block::parse(...)? } }` shape in
`lib.rs::read_impl` consequently returned `Err` for every
valid `.rm` file.

Downstream:
- `rehydrate_render::build_pdf_from_rm_files` propagated the parse
  error → the Tauri command in `commands.rs::open_document`
  fell through to `build_pdf_from_pngs` (thumbnail fallback).
- The fallback PDF was written to the per-document cache at
  `<safe_name>-<document_id>-<manifest_hash>-ink-v16.pdf`.
- OCR went through `rehydrate-ocr::page_render::render_rm_to_png`
  which has no fallback — it just surfaced the parse error
  verbatim to the user as a failed OCR job.

## Why our gates didn't catch it

1. **Tests covered the wrong contract.** The new
   `read_bytes_refuses_amount_exceeding_remaining` asserted
   `.is_err()` without inspecting the error kind. The implicit
   "kind == Io ⇒ EOF" invariant lived in one line of `eof()` and
   was never tested.
2. **No end-to-end parser test existed.** Every test in `rm-parser`
   was a unit test against `Bitreader`; no test ran
   `RemarkableFile::read()` against real bytes. The bug was at
   the seam between `Bitreader::read_bytes` and
   `RemarkableFile::read`, exactly where there was no coverage.
3. **No end-to-end render test existed.** `rehydrate-render` had
   no integration tests at all. A test that called
   `build_pdf_from_rm_files` against a minimal valid `.rm` would
   have failed before merge.
4. **The graceful fallback hid the symptom.** The render path
   logged a `tracing::warn!` and emitted a Tauri
   `document:legacy-format-warning` event, but neither was wired
   into any test assertion. Users saw "blurry preview" and
   assumed it was an unrelated quirk.
5. **The cached fallback survived the parser fix.** Once a bad
   preview PDF was written to the per-document cache, the
   downstream `open_document` lookup returned the cached file
   directly. Fixing the parser without bumping the cache version
   left affected notebooks broken; users had to wait for the
   second fix or manually clear `target/cache/`.

## What's in place now

- **`crates/rm-parser/tests/v6_smoke.rs`** — three integration
  tests:
  - `header_only_v6_file_parses_to_empty_blocks` — the smallest
    valid v6 file (43-byte header, zero blocks) must round-trip
    through `RemarkableFile::read()`. This is the direct
    regression guard: under the old broken bounds guard, this
    file would return an error.
  - `truncated_header_errors` — a 17-byte input (less than the
    43-byte header) must surface as a real error, locking the
    "Io kind ⇒ EOF" contract from the other direction.
  - `unknown_version_errors_as_unsupported` — a typed error path
    that the parser-error display chain must keep intact.

- **`crates/rehydrate-render/tests/pipeline.rs`** — three
  end-to-end tests through `build_pdf_from_rm_files` so the
  parser ↔ renderer contract has its own line of defence:
  - `renders_minimal_v6_notebook_without_parse_error`
  - `renders_multi_page_v6_notebook`
  - `empty_pages_slice_is_typed_error_not_panic`

- **`Bitreader::eof()` postmortem comment** documenting the
  load-bearing `Io`-kind contract by reference to this file and
  the regression commit hash, so a future contributor stepping
  near the bounds-check code sees the trap before they spring it.

- **`rehydrate-render::PREVIEW_LAYOUT_VERSION`** — the cache-bust
  lever moved from `rehydrate-app::commands` into the renderer
  crate. A renderer change is now the same edit as the
  version-bump candidate. The commands layer reads via
  `rehydrate_render::PREVIEW_LAYOUT_VERSION`.

## Lessons applicable beyond this incident

- **A graceful-fallback path masks bugs.** Whenever the codebase
  catches an error and serves a degraded result, write an
  integration test that asserts the *good* path runs against a
  realistic input. The fallback is a comfort to users, not a
  signal to the test suite.
- **Cache invalidation lives with the code it caches.** Any
  constant that controls "is the cached blob still valid"
  belongs next to the code that produced the blob — not in the
  call site that reads from cache. Cross-crate co-location
  invites "I fixed the renderer but forgot to bump the version."
- **Error-kind branches are load-bearing contracts.** If any
  function dispatches on `error.kind == ...`, that's a contract.
  Document it inline and lock it with a test that builds the
  error via the natural code path (not by hand) and asserts the
  kind directly.
- **Test the seam, not just the unit.** Unit tests inside
  `Bitreader` couldn't have caught this. The bug lived in how
  `Bitreader` and `RemarkableFile::read` composed. The cheapest
  end-to-end test (minimal valid input through the public API)
  catches the entire class of seam regressions.
