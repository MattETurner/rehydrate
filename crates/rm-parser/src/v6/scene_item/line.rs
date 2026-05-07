use crate::{
    shared::{pen_color::PenColor, tool::Tool},
    v6::{
        block::{BlockInfo, BlockParse},
        scene_item::point::Point,
    },
    ParseError,
};

#[derive(Debug, Clone)]
pub struct Line {
    color: PenColor,
    tool: Tool,
    points: Vec<Point>,
    thickness_scale: f64,
    starting_length: f32,
}

impl Line {
    pub fn color(&self) -> &PenColor {
        &self.color
    }
    pub fn tool(&self) -> &Tool {
        &self.tool
    }
    pub fn points(&self) -> &[Point] {
        &self.points
    }
    pub fn thickness_scale(&self) -> f64 {
        self.thickness_scale
    }
    pub fn starting_length(&self) -> f32 {
        self.starting_length
    }
}

pub fn point_serialize_size(version: u8) -> Result<u32, ParseError> {
    match version {
        1 => return Ok(0x18),
        2 => return Ok(0x0E),
        _ => {
            return Err(ParseError::unsupported(format!(
                "Block unsupported version: {version}"
            )))
        }
    }
}

impl BlockParse for Line {
    fn parse(
        info: &BlockInfo,
        reader: &mut crate::v6::tagged_bit_reader::TaggedBitreader<impl crate::bitreader::Readable>,
    ) -> Result<Self, crate::ParseError> {
        let tool = Tool::try_from(reader.read_u32(1)?)?;
        let color = PenColor::try_from(reader.read_u32(2)?)?;
        let thickness_scale = reader.read_f64(3)?;
        let starting_length = reader.read_f32(4)?;

        let subblock = reader.read_subblock(5)?;
        let point_size = point_serialize_size(info.current_version)?;
        // Points live inside the subblock, not the outer Line block — the
        // upstream `remarkable_lines` 0.1.2 used `info.size` here, which
        // pointed at the parent Line block and broke parsing on every
        // page that mixed lines with other items. Use the subblock's
        // own size.
        if subblock.size % point_size != 0 {
            return Err(ParseError::invalid(format!(
                "Invalid point data size. {} is not multiple of {point_size}",
                subblock.size
            )));
        }
        let points = (0..subblock.size / point_size)
            .map(|_| Point::parse(info, reader))
            .collect::<Result<Vec<Point>, ParseError>>()?;
        subblock.validate_size(reader)?;

        // XXX unused
        let _timestamp = reader.read_id(6);

        return Ok(Line {
            tool,
            color,
            thickness_scale,
            starting_length,
            points,
        });
    }
}
