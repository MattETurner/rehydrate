use std::f32::consts::PI;

use crate::v6::block::BlockParse;

#[derive(Debug, Clone)]
pub struct Point {
    x: f32,
    y: f32,
    speed: f32,
    direction: f32,
    width: f32,
    pressure: f32,
}

impl Point {
    pub fn x(&self) -> f32 {
        self.x
    }
    pub fn y(&self) -> f32 {
        self.y
    }
    pub fn speed(&self) -> f32 {
        self.speed
    }
    pub fn direction(&self) -> f32 {
        self.direction
    }
    pub fn width(&self) -> f32 {
        self.width
    }
    pub fn pressure(&self) -> f32 {
        self.pressure
    }
}

impl BlockParse for Point {
    fn parse(
        info: &crate::v6::block::BlockInfo,
        reader: &mut crate::v6::tagged_bit_reader::TaggedBitreader<impl crate::bitreader::Readable>,
    ) -> Result<Self, crate::ParseError> {
        let x = reader.bit_reader.read_f32()?;
        let y = reader.bit_reader.read_f32()?;
        if info.current_version == 1 {
            let speed = reader.bit_reader.read_f32()? * 4.0;
            let direction = (255.0 * reader.bit_reader.read_f32()?) / (PI * 2.0);
            let width = reader.bit_reader.read_f32()? * 4.0;
            let pressure = reader.bit_reader.read_f32()? * 255.0;
            return Ok(Point {
                x,
                y,
                speed,
                direction,
                width,
                pressure,
            });
        } else {
            let speed = f32::from(reader.bit_reader.read_u16()?);
            let width = f32::from(reader.bit_reader.read_u16()?);
            let direction = f32::from(reader.bit_reader.read_u8()?);
            let pressure = f32::from(reader.bit_reader.read_u8()?);
            return Ok(Point {
                x,
                y,
                speed,
                direction,
                width,
                pressure,
            });
        }
    }
}
