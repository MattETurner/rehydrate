use crate::ParseError;

/// Data representation of an exported color in a reMarkable document line.
/// `Unknown` exists because newer firmware adds colour codes the parser
/// doesn't recognise; we keep the raw value so callers can decide how to
/// render rather than failing the whole file.
#[derive(Debug, Clone)]
pub enum PenColor {
    Black,
    Grey,
    White,
    Yellow,
    Green,
    Pink,
    Blue,
    Red,
    GreyOverlap,
    Unknown(u32),
}

impl TryFrom<u32> for PenColor {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(match value {
            0x00 => PenColor::Black,
            0x01 => PenColor::Grey,
            0x02 => PenColor::White,
            0x03 => PenColor::Yellow,
            0x04 => PenColor::Green,
            0x05 => PenColor::Pink,
            0x06 => PenColor::Blue,
            0x07 => PenColor::Red,
            0x08 => PenColor::GreyOverlap,
            other => PenColor::Unknown(other),
        })
    }
}
