//! Integration tests against the public `RemarkableFile::read` API.
//!
//! These exist as a regression guard against the bitreader EOF
//! contract — the parser's loop in `lib.rs::read_impl` calls
//! `Bitreader::eof()` between blocks, and `eof()` decides
//! "no more bytes" by checking for `ParseErrorKind::Io` on a
//! 1-byte read. A previous audit broke that contract by returning
//! `ParseErrorKind::InvalidInput` from the bounds guard in
//! `read_bytes`; the symptom was every notebook OCR failing with
//! "Failed to read eof of bitreader when trying to add context."
//!
//! The crate-internal unit tests in `bitreader.rs` cover the
//! `read_bytes`/`skip_bytes` error-kind contract directly. This
//! file exists so the *end-to-end* `read()` API — which is what
//! the OCR + render pipelines call — has its own line of defence.
//! If both layers ever drift, both have to be re-broken before a
//! regression ships.

use rm_parser::RemarkableFile;

/// The minimum well-formed v6 `.rm` file: the 43-byte header
/// followed by zero blocks. The parser walks the header, enters the
/// v6 branch, calls `eof()`, gets `Ok(true)`, exits the loop, and
/// returns `RemarkableFile::V6 { tree, blocks: [] }`.
///
/// If `eof()` ever stops recognising end-of-stream — as it did
/// during the phase-4 regression — `read()` returns
/// `Err("Error while parsing remarkable file Failed to read eof
/// of bitreader …")` instead, and this test goes red.
#[test]
fn header_only_v6_file_parses_to_empty_blocks() {
    // Header layout: `read_bytes(43)` then `trim_end()` looks for
    // the literal `"reMarkable .lines file, version=6"`.
    let mut bytes = Vec::with_capacity(43);
    bytes.extend_from_slice(b"reMarkable .lines file, version=6");
    let pad = 43 - bytes.len();
    bytes.resize(bytes.len() + pad, b' ');
    assert_eq!(bytes.len(), 43);

    let rm = RemarkableFile::read(&bytes[..])
        .expect("header-only v6 file must parse cleanly — see file doc");
    match rm {
        RemarkableFile::V6 { blocks, .. } => {
            assert!(
                blocks.is_empty(),
                "header-only file should yield zero blocks, got {}",
                blocks.len()
            );
        }
        RemarkableFile::Other { version, .. } => {
            panic!("expected V6 variant, got Other(version={version})");
        }
    }
}

/// Truncated header (less than 43 bytes) is a real error — the
/// parser can't even read the version string. This locks the
/// "Io kind on overshoot" contract from the other direction: a
/// real truncation should surface as a parse error (which then
/// propagates up to the user-facing toast), not as silent EOF.
#[test]
fn truncated_header_errors() {
    let bytes = b"reMarkable .lines"; // 17 bytes, well short of 43.
    let r = RemarkableFile::read(&bytes[..]);
    assert!(r.is_err(), "truncated header must not parse");
}

/// Header announcing an unsupported version yields a typed
/// `Unsupported` error rather than an `Io` or `InvalidInput`
/// one. The `RemarkableFile::read` Display chain folds the
/// version number into the message, so a quick substring check
/// catches accidental message-shape drift.
#[test]
fn unknown_version_errors_as_unsupported() {
    let header = b"reMarkable .lines file, version=99         ";
    assert_eq!(header.len(), 43);
    let err = RemarkableFile::read(&header[..]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("99"),
        "error message should cite the unsupported version, got: {msg}"
    );
}
