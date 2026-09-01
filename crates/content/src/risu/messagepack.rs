use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum MessagePackValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<MessagePackValue>),
    Map(Vec<(MessagePackValue, MessagePackValue)>),
}

impl MessagePackValue {
    pub(super) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub(super) fn as_slice(&self) -> Option<&[u8]> {
        match self {
            Self::Binary(value) => Some(value),
            _ => None,
        }
    }

    pub(super) fn as_map(&self) -> Option<&[(Self, Self)]> {
        match self {
            Self::Map(value) => Some(value),
            _ => None,
        }
    }

    pub(super) fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            Self::I64(value) => u64::try_from(*value).ok(),
            _ => None,
        }
    }
}

pub(super) fn decode(
    bytes: &[u8],
    label: &str,
    max_depth: usize,
    max_nodes: usize,
) -> CoreResult<MessagePackValue> {
    let mut parser = Parser {
        bytes,
        position: 0,
        nodes: 0,
        max_depth,
        max_nodes,
        label,
    };
    let value = parser.value(0)?;
    if parser.position != bytes.len() {
        return Err(parser.invalid("has trailing bytes"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
    nodes: usize,
    max_depth: usize,
    max_nodes: usize,
    label: &'a str,
}

impl Parser<'_> {
    fn value(&mut self, depth: usize) -> CoreResult<MessagePackValue> {
        if depth > self.max_depth {
            return Err(self.invalid("nesting exceeds the limit"));
        }
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > self.max_nodes {
            return Err(self.invalid("structure exceeds the node limit"));
        }
        let marker = self.byte()?;
        match marker {
            0x00..=0x7f => Ok(MessagePackValue::U64(u64::from(marker))),
            0x80..=0x8f => self.map(usize::from(marker & 0x0f), depth),
            0x90..=0x9f => self.array(usize::from(marker & 0x0f), depth),
            0xa0..=0xbf => self.string(usize::from(marker & 0x1f)),
            0xc0 => Ok(MessagePackValue::Null),
            0xc1 => Err(self.invalid("uses the reserved 0xc1 marker")),
            0xc2 => Ok(MessagePackValue::Bool(false)),
            0xc3 => Ok(MessagePackValue::Bool(true)),
            0xc4 => {
                let len = usize::from(self.byte()?);
                self.binary(len)
            }
            0xc5 => {
                let len = usize::from(self.u16()?);
                self.binary(len)
            }
            0xc6 => {
                let len = self.length_u32()?;
                self.binary(len)
            }
            0xc7 => {
                let len = usize::from(self.byte()?);
                self.extension(len)
            }
            0xc8 => {
                let len = usize::from(self.u16()?);
                self.extension(len)
            }
            0xc9 => {
                let len = self.length_u32()?;
                self.extension(len)
            }
            0xca => Ok(MessagePackValue::F64(f64::from(f32::from_bits(
                self.u32()?,
            )))),
            0xcb => Ok(MessagePackValue::F64(f64::from_bits(self.u64()?))),
            0xcc => Ok(MessagePackValue::U64(u64::from(self.byte()?))),
            0xcd => Ok(MessagePackValue::U64(u64::from(self.u16()?))),
            0xce => Ok(MessagePackValue::U64(u64::from(self.u32()?))),
            0xcf => Ok(MessagePackValue::U64(self.u64()?)),
            0xd0 => Ok(MessagePackValue::I64(i64::from(self.byte()? as i8))),
            0xd1 => Ok(MessagePackValue::I64(i64::from(self.u16()? as i16))),
            0xd2 => Ok(MessagePackValue::I64(i64::from(self.u32()? as i32))),
            0xd3 => Ok(MessagePackValue::I64(self.u64()? as i64)),
            0xd4 => self.extension(1),
            0xd5 => self.extension(2),
            0xd6 => self.extension(4),
            0xd7 => self.extension(8),
            0xd8 => self.extension(16),
            0xd9 => {
                let len = usize::from(self.byte()?);
                self.string(len)
            }
            0xda => {
                let len = usize::from(self.u16()?);
                self.string(len)
            }
            0xdb => {
                let len = self.length_u32()?;
                self.string(len)
            }
            0xdc => {
                let len = usize::from(self.u16()?);
                self.array(len, depth)
            }
            0xdd => {
                let len = self.length_u32()?;
                self.array(len, depth)
            }
            0xde => {
                let len = usize::from(self.u16()?);
                self.map(len, depth)
            }
            0xdf => {
                let len = self.length_u32()?;
                self.map(len, depth)
            }
            0xe0..=0xff => Ok(MessagePackValue::I64(i64::from(marker as i8))),
        }
    }

    fn array(&mut self, len: usize, depth: usize) -> CoreResult<MessagePackValue> {
        self.collection_length(len)?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(self.value(depth + 1)?);
        }
        Ok(MessagePackValue::Array(values))
    }

    fn map(&mut self, len: usize, depth: usize) -> CoreResult<MessagePackValue> {
        self.collection_length(len.saturating_mul(2))?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push((self.value(depth + 1)?, self.value(depth + 1)?));
        }
        Ok(MessagePackValue::Map(values))
    }

    fn collection_length(&self, additional_nodes: usize) -> CoreResult<()> {
        if additional_nodes > self.max_nodes.saturating_sub(self.nodes) {
            return Err(self.invalid("declares too many collection items"));
        }
        Ok(())
    }

    fn string(&mut self, len: usize) -> CoreResult<MessagePackValue> {
        let bytes = self.take(len)?.to_vec();
        let value = String::from_utf8(bytes).map_err(|_| self.invalid("contains invalid UTF-8"))?;
        Ok(MessagePackValue::String(value))
    }

    fn binary(&mut self, len: usize) -> CoreResult<MessagePackValue> {
        Ok(MessagePackValue::Binary(self.take(len)?.to_vec()))
    }

    fn extension(&mut self, len: usize) -> CoreResult<MessagePackValue> {
        self.byte()?;
        self.take(len)?;
        Ok(MessagePackValue::Null)
    }

    fn byte(&mut self) -> CoreResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> CoreResult<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("two-byte slice"),
        ))
    }

    fn u32(&mut self) -> CoreResult<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("four-byte slice"),
        ))
    }

    fn u64(&mut self) -> CoreResult<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("eight-byte slice"),
        ))
    }

    fn length_u32(&mut self) -> CoreResult<usize> {
        usize::try_from(self.u32()?).map_err(|_| self.invalid("length does not fit this device"))
    }

    fn take(&mut self, len: usize) -> CoreResult<&[u8]> {
        let end = self
            .position
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| self.invalid("is truncated"))?;
        let bytes = &self.bytes[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn invalid(&self, message: &str) -> CoreError {
        CoreError::new(
            CoreErrorCode::UnsupportedContent,
            format!("{} {message}", self.label),
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bounded_maps_arrays_and_binary_values() {
        let bytes = [
            0x82, 0xa4, b'n', b'a', b'm', b'e', 0xa4, b'R', b'i', b's', b'u', 0xa4, b'd', b'a',
            b't', b'a', 0x92, 0xc3, 0xc4, 0x02, 0x01, 0x02,
        ];
        let value = decode(&bytes, "fixture", 8, 32).expect("decode fixture");
        let map = value.as_map().expect("map");
        assert_eq!(map[0].0.as_str(), Some("name"));
        assert!(matches!(&map[1].1, MessagePackValue::Array(values) if values.len() == 2));
    }

    #[test]
    fn rejects_trailing_and_oversized_structures() {
        assert!(decode(&[0xc0, 0xc0], "fixture", 8, 8).is_err());
        assert!(decode(&[0x93, 0xc0, 0xc0, 0xc0], "fixture", 8, 2).is_err());
    }
}
