use std::io::{Cursor, Read};

use crate::{ParseError, ParseErrorKind};

/// Build the "buffer underrun" error returned by `read_bytes` and
/// `skip_bytes` when the caller asked for more than `remaining()`
/// bytes. The kind is `Io` so `Bitreader::eof()` still recognises
/// it as end-of-stream rather than treating a normal stream
/// boundary as a malformed-input error.
fn eof_error(requested: usize, available: usize) -> ParseError {
    ParseError::new(
        format!("requested {requested} bytes but only {available} remain"),
        ParseErrorKind::Io,
    )
}

pub trait Readable: Read + AsRef<[u8]> {}
impl<T: Read + AsRef<[u8]>> Readable for T {}

/// A little endian binary reader
pub struct Bitreader<N: Readable> {
    cursor: Cursor<N>,
}

impl<N: Readable> Bitreader<N> {
    pub fn new(bits: N) -> Bitreader<N> {
        Bitreader {
            cursor: Cursor::new(bits),
        }
    }

    /// End Of File, returns true if not more bytes can be read.
    ///
    /// **Load-bearing invariant** (see also: `eof_error` at the top
    /// of this file and the `read_bytes_overshoot_is_io_kind_so_eof_idiom_works`
    /// regression test below): this function decides "no more bytes"
    /// by reading exactly one byte and checking whether the read
    /// failed with `ParseErrorKind::Io`. Every parser loop in this
    /// crate (`lib.rs` block-parse, the v6 sub-parsers, the v3-5
    /// page loop) calls `eof()` to know when to stop.
    ///
    /// History: a phase-4 audit added bounds checks to `read_bytes`
    /// / `skip_bytes` that returned `ParseErrorKind::InvalidInput`
    /// on overshoot. That broke `eof()` for every valid `.rm` file
    /// — the EOF check started treating a normal end-of-stream as a
    /// malformed-input error. Symptom: notebook OCR failed on every
    /// document; the render path's fallback wrote a blurry
    /// thumbnail-stitch PDF and cached it. Fix in `3f67ec3`
    /// restored the `Io` kind via `eof_error`; lesson: any change
    /// to the error kinds produced by `read_bytes`/`skip_bytes`
    /// MUST keep the "EOF means Io" contract or update both call
    /// sites here together.
    pub fn eof(&mut self) -> Result<bool, ParseError> {
        let pos = self.position();
        match self.read_bytes(1) {
            Ok(_) => {
                self.set_position(pos);
                Ok(false)
            }
            Err(e) => {
                // if an io error occurs we assume no more bytes can be read, aka eof
                if e.kind == ParseErrorKind::Io {
                    self.set_position(pos);
                    return Ok(true);
                }
                return Err(e);
            }
        }
    }

    pub fn position(&self) -> u64 {
        self.cursor.position()
    }

    pub fn set_position(&mut self, position: u64) {
        self.cursor.set_position(position);
        // self.cursor.seek(SeekFrom::Current(position)).unwrap();
    }

    /// Bytes left unread in the underlying buffer.
    ///
    /// Used to bound `read_bytes` allocations against the actual stream
    /// size. A hostile `.rm` file can declare a varuint-derived
    /// `string_length` of up to `u32::MAX`, which without this cap would
    /// pre-allocate ~4 GB before the underlying read discovered the
    /// stream was empty.
    pub fn remaining(&self) -> usize {
        let total = self.cursor.get_ref().as_ref().len();
        let pos = self.cursor.position() as usize;
        total.saturating_sub(pos)
    }

    // Read bytes first from inner buffer than from bits, will also update the offset
    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), ParseError> {
        self.cursor.read_exact(buffer)?;

        return Ok(());
    }

    /// Skip `amount` bytes without allocating. Same bounds check as
    /// `read_bytes`; used when the caller only needs to advance the
    /// cursor (e.g. tolerated trailing bytes on a block).
    ///
    /// On overshoot, the error's kind is [`ParseErrorKind::Io`] — the
    /// same kind `read_exact` would have produced via
    /// `io::ErrorKind::UnexpectedEof`. Several callers (notably
    /// [`Bitreader::eof`]) treat `Io` as "no more bytes" rather than
    /// "the input was malformed"; returning `InvalidInput` here would
    /// have broken the EOF-detection idiom for legitimate truncated
    /// streams.
    pub fn skip_bytes(&mut self, amount: usize) -> Result<(), ParseError> {
        let avail = self.remaining();
        if amount > avail {
            return Err(eof_error(amount, avail));
        }
        let pos = self.cursor.position();
        self.cursor.set_position(pos + amount as u64);
        Ok(())
    }

    pub fn read_bytes(&mut self, amount: usize) -> Result<Vec<u8>, ParseError> {
        // Refuse to allocate more than the buffer can possibly
        // provide. Without this guard, a malformed file can declare
        // a length of ~4 GB and the parser allocates eagerly *before*
        // discovering the underlying stream is empty — the classic
        // length-prefix DOS shape (see audit H-2).
        //
        // The error kind here is deliberately `Io` rather than
        // `InvalidInput`. Several callers rely on the `read_bytes` →
        // `read_exact` chain producing an `Io` error at end-of-stream
        // (the underlying `io::Read` returns `UnexpectedEof`), and
        // [`Bitreader::eof`] specifically catches `Io` to mean "no
        // more bytes." Returning `InvalidInput` here turned every
        // legitimate end-of-buffer into a fatal parse error and
        // broke notebook OCR for any document where the parser
        // walked to EOF — which is basically every document.
        let avail = self.remaining();
        if amount > avail {
            return Err(eof_error(amount, avail));
        }
        let mut buffer = vec![0; amount];
        self.read_exact(&mut buffer)?;
        return Ok(buffer);
    }

    pub fn read_string(&mut self, length: usize) -> Result<String, ParseError> {
        return Ok(String::from_utf8(self.read_bytes(length)?)
            .map_err(|_| ParseError::invalid("String contains invalid utf-8"))?);
    }

    // https://en.wikipedia.org/wiki/Variable-length_quantity
    //
    // Bug-fix note: the operand must be widened to `u32` *before* shifting.
    // Shifting a `u8` by ≥ 8 panics in debug builds and silently wraps the
    // shift amount on release, which corrupted the decode for any varint
    // that occupied more than one byte.
    pub fn read_varuint(&mut self) -> Result<u32, ParseError> {
        let mut shift: u32 = 0;
        let mut result: u32 = 0;
        // A u32 fits in at most 5 base-128 bytes; refuse longer streams to
        // avoid silent overflow on a malformed/oversize input.
        for _ in 0..5 {
            let i = self.read_u8()?;
            let chunk = (i & 0x7F) as u32;
            // Final (5th) byte may only contribute the top 4 bits of u32.
            if shift == 28 && (chunk >> 4) != 0 {
                return Err(ParseError::invalid("varuint overflows u32"));
            }
            result |= chunk << shift;
            if i & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err(ParseError::invalid("varuint exceeds 5 bytes"))
    }

    pub fn read_bool(&mut self) -> Result<bool, ParseError> {
        return Ok(self.read_u8()? > 0);
    }

    pub fn read_f32(&mut self) -> Result<f32, ParseError> {
        let mut buffer = [0; 4];
        self.read_exact(&mut buffer)?;
        return Ok(f32::from_le_bytes(buffer));
    }

    pub fn read_f64(&mut self) -> Result<f64, ParseError> {
        let mut buffer = [0; 8];
        self.read_exact(&mut buffer)?;
        return Ok(f64::from_le_bytes(buffer));
    }

    pub fn read_u8(&mut self) -> Result<u8, ParseError> {
        let mut buffer = [0];
        self.read_exact(&mut buffer)?;
        return Ok(u8::from_le_bytes(buffer));
    }

    pub fn read_u16(&mut self) -> Result<u16, ParseError> {
        let mut buffer = [0; 2];
        self.read_exact(&mut buffer)?;
        return Ok(u16::from_le_bytes(buffer));
    }

    pub fn read_u32(&mut self) -> Result<u32, ParseError> {
        let mut buffer = [0; 4];
        self.read_exact(&mut buffer)?;
        return Ok(u32::from_le_bytes(buffer));
    }

    /// Parse uuid from data in little endian format
    /// Using Variant 2 UUID's with mixed endianess <https://en.wikipedia.org/wiki/Universally_unique_identifier#Encoding>
    pub fn read_uuid(&mut self) -> Result<String, ParseError> {
        let uuid_length = self.read_varuint()?;
        if uuid_length != 16 {
            return Err(ParseError::invalid("Expected UUID length to be 16 bytes"));
        }

        let mut uuid_bytes: Vec<u8> = self.read_bytes(uuid_length as usize)?;

        // Set first 3 uuid sections to big endianness
        uuid_bytes[..4].reverse();
        uuid_bytes[4..6].reverse();
        uuid_bytes[6..8].reverse();

        // put bytes in a single number
        let uuid_bytes = u128::from_be_bytes(
            uuid_bytes
                .try_into()
                .map_err(|_| ParseError::invalid("Failed to parse uuid bytes into integer"))?,
        );

        // turn hexidecimals into string
        let uuid = format!("{uuid_bytes:032x}");
        // add slashes
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &uuid[..8],
            &uuid[8..12],
            &uuid[12..16],
            &uuid[16..20],
            &uuid[20..],
        );

        Ok(uuid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn br(bytes: &'static [u8]) -> Bitreader<&'static [u8]> {
        Bitreader::new(bytes)
    }

    #[test]
    fn varuint_single_byte() {
        assert_eq!(br(&[0x00]).read_varuint().unwrap(), 0);
        assert_eq!(br(&[0x7F]).read_varuint().unwrap(), 127);
    }

    #[test]
    fn varuint_multi_byte_no_overflow() {
        // 300 = 0xAC 0x02 in LEB128.
        assert_eq!(br(&[0xAC, 0x02]).read_varuint().unwrap(), 300);
        // u32::MAX encoded as 5 bytes: FF FF FF FF 0F.
        assert_eq!(
            br(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).read_varuint().unwrap(),
            u32::MAX
        );
    }

    #[test]
    fn varuint_rejects_overflow() {
        // 5th byte's payload bits overflow u32 (top bits set above 0x0F).
        assert!(br(&[0xFF, 0xFF, 0xFF, 0xFF, 0x10]).read_varuint().is_err());
        // 6+ bytes worth of continuation also rejected.
        assert!(br(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01])
            .read_varuint()
            .is_err());
    }

    #[test]
    fn read_bytes_refuses_amount_exceeding_remaining() {
        // The reader has 4 bytes; asking for 1 GiB must fail
        // immediately — without allocating a 1 GiB Vec — because
        // amount > remaining(). This is the audit-H-2 bound.
        let r = br(&[0x01, 0x02, 0x03, 0x04]).read_bytes(1024 * 1024 * 1024);
        assert!(r.is_err());
    }

    #[test]
    fn read_bytes_overshoot_is_io_kind_so_eof_idiom_works() {
        // Regression guard: the bounds check in `read_bytes` must
        // return a `ParseErrorKind::Io` error, not `InvalidInput`.
        // `Bitreader::eof()` reads 1 byte and treats an `Io`
        // failure as "no more bytes" — that's how every parser
        // loop in this crate knows when to stop. Returning
        // `InvalidInput` here broke notebook OCR for every
        // document that walked to EOF (i.e. basically all of them).
        let mut r = br(&[]);
        let err = r.read_bytes(1).unwrap_err();
        assert_eq!(err.kind, ParseErrorKind::Io, "{err:?}");
        // And the high-level eof() recognises it.
        let mut r2 = br(&[]);
        assert_eq!(r2.eof().unwrap(), true);
    }

    #[test]
    fn skip_bytes_refuses_overshoot() {
        let mut r = br(&[0x01, 0x02, 0x03, 0x04]);
        let err = r.skip_bytes(usize::MAX / 2).unwrap_err();
        // Same `Io` kind contract as `read_bytes` — see the
        // regression test above for why.
        assert_eq!(err.kind, ParseErrorKind::Io);
        // Stream cursor must not have moved past EOF — a follow-up
        // read returns the original bytes.
        let kept = r.read_bytes(4).unwrap();
        assert_eq!(kept, vec![0x01, 0x02, 0x03, 0x04]);
    }
}
