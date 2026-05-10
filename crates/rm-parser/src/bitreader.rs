use std::io::{Cursor, Read};

use crate::{ParseError, ParseErrorKind};

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

    /// End Of File, returns true if not more bytes can be read
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

    // Read bytes first from inner buffer than from bits, will also update the offset
    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), ParseError> {
        self.cursor.read_exact(buffer)?;

        return Ok(());
    }

    pub fn read_bytes(&mut self, amount: usize) -> Result<Vec<u8>, ParseError> {
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
}
