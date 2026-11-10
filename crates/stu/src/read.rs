//! The STUv2 decoder, after TankLib's `teStructuredData` and field readers.

use crate::{
    Asset, Error, Object, Value,
    types::{FieldType, Kind, Types},
};

/// Decode an asset. Fields the asset doesn't store take their defaults.
pub fn read(bytes: &[u8]) -> Result<Asset, Error> {
    if bytes.get(..4) == Some(b"STUD") {
        return Err(Error("STUv1 assets are not supported".into()));
    }
    let header = bytes
        .get(..36)
        .ok_or_else(|| Error("truncated STU header".into()))?;
    let checksum = crc64(header);
    let mut head = Cursor::new(bytes);
    let instances = bag(&mut head, 16)?;
    let _inline_types = bag(&mut head, 8)?;
    let field_bags = bag(&mut head, 8)?
        .iter()
        .map(|entry| {
            let mut cursor = Cursor::new(bytes);
            cursor.position = entry.start;
            bag(&mut cursor, 8)
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| (u32_at(bytes, entry.start), i32_at(bytes, entry.start + 4)))
                .collect()
        })
        .collect();
    let dyn_size = head.i32()?;
    let dyn_offset = head.i32()?;
    let data_offset = head.i32()?;
    let dynamic = if dyn_size > 0 {
        let start = usize::try_from(dyn_offset).map_err(|_| corrupt("dynamic data offset"))?;
        bytes
            .get(start..start + dyn_size as usize)
            .ok_or_else(|| corrupt("dynamic data"))?
    } else {
        &[]
    };
    let data = bytes
        .get(usize::try_from(data_offset).map_err(|_| corrupt("data offset"))?..)
        .unwrap_or(&[]);
    let mut reader = Reader {
        types: Types::get(),
        data: Cursor::new(data),
        dynamic: Cursor::new(dynamic),
        field_bags,
        checksum,
        instance_count: instances.len(),
    };
    let mut objects = Vec::with_capacity(instances.len());
    for entry in &instances {
        let hash = u32_at(bytes, entry.start);
        let size = i32_at(bytes, entry.start + 12);
        let start = reader.data.position;
        let object = match reader.types.class(hash) {
            Some(_) => Some(reader.object(hash)?),
            None => None,
        };
        objects.push(object);
        reader.data.position = start + size.max(0) as usize;
    }
    Ok(Asset { instances: objects })
}

struct Reader<'a> {
    types: &'static Types,
    data: Cursor<'a>,
    dynamic: Cursor<'a>,
    /// Each bag: field hash and size (0: the size precedes the value).
    field_bags: Vec<Vec<(u32, i32)>>,
    checksum: u64,
    instance_count: usize,
}

impl Reader<'_> {
    /// An instance of `class` at the data position: its field bag, then the
    /// fields that bag lists.
    fn object(&mut self, class: u32) -> Result<Object, Error> {
        let bag = self.data.i32()?;
        let mut object = Object {
            class,
            fields: Vec::new(),
        };
        if bag == -1 {
            self.defaults(&mut object);
            return Ok(object);
        }
        let fields = self
            .field_bags
            .get(usize::try_from(bag).map_err(|_| corrupt("field bag index"))?)
            .ok_or_else(|| corrupt("field bag index"))?
            .clone();
        for (hash, size) in fields {
            let size = if size == 0 { self.data.i32()? } else { size };
            let start = self.data.position;
            if let Some(field) = self.types.field(class, hash) {
                let value = self.field(hash, field.ty, size)?;
                object.fields.push((hash, value));
            }
            self.data.position = start + size.max(0) as usize;
        }
        self.defaults(&mut object);
        Ok(object)
    }

    /// The fields the asset doesn't store, at their defaults, as DataTool
    /// shows them.
    fn defaults(&self, object: &mut Object) {
        for field in self.types.fields(object.class) {
            if !object.fields.iter().any(|(hash, _)| *hash == field.hash) {
                object.fields.push((field.hash, field.default.clone()));
            }
        }
    }

    fn field(&mut self, hash: u32, ty: FieldType, size: i32) -> Result<Value, Error> {
        if !ty.array {
            return self.single(hash, ty.kind);
        }
        if size == 0 || matches!(ty.kind, Kind::Inline(_)) {
            // In place: a count, then the elements in the data buffer.
            let count = self.data.i32()?;
            let mut items = Vec::with_capacity(count.max(0) as usize);
            for _ in 0..count {
                items.push(self.element(hash, ty.kind)?);
            }
            return Ok(Value::Array(items));
        }
        let offset = self.data.i32()?;
        if offset == -1 {
            return Ok(Value::Array(Vec::new()));
        }
        self.dynamic.seek(offset as usize)?;
        let count = self.dynamic.i32()?;
        if count == 0 {
            return Ok(Value::Array(Vec::new()));
        }
        let _unknown = self.dynamic.u32()?;
        let elements = self.dynamic.i64()?;
        self.dynamic.seek(elements as usize)?;
        let mut items = Vec::with_capacity(count.max(0) as usize);
        for _ in 0..count {
            items.push(self.element(hash, ty.kind)?);
        }
        Ok(Value::Array(items))
    }

    /// A value outside an array, from the data buffer.
    fn single(&mut self, hash: u32, kind: Kind) -> Result<Value, Error> {
        Ok(match kind {
            Kind::String => {
                let offset = self.data.i32()?;
                if offset == -1 {
                    return Ok(Value::Null);
                }
                self.dynamic.seek(offset as usize)?;
                self.string()?
            }
            Kind::Guid => {
                let stored = self.data.u64()?;
                Value::Guid(self.guid(hash, stored))
            }
            Kind::Embed => match self.data.i32()? {
                -1 => Value::Null,
                index if (index as usize) < self.instance_count => Value::Instance(index as usize),
                _ => Value::Null,
            },
            Kind::Inline(class) => Value::Object(self.object(class)?),
            Kind::Map => self.map()?,
            kind => scalar(&mut self.data, kind)?,
        })
    }

    /// An array element: inline instances from the data buffer, everything
    /// else from the dynamic buffer.
    fn element(&mut self, hash: u32, kind: Kind) -> Result<Value, Error> {
        Ok(match kind {
            Kind::Inline(class) => Value::Object(self.object(class)?),
            Kind::String => {
                let offset = self.dynamic.i64()?;
                let _mutability = self.dynamic.i64()?;
                let resume = self.dynamic.position;
                self.dynamic.seek(offset as usize)?;
                let value = self.string()?;
                self.dynamic.position = resume;
                value
            }
            Kind::Guid => {
                let _padding = self.dynamic.u64()?;
                let stored = self.dynamic.u64()?;
                Value::Guid(self.guid(hash, stored))
            }
            Kind::Embed => {
                let index = self.dynamic.i32()?;
                let _padding = self.dynamic.i32()?;
                match index {
                    -1 => Value::Null,
                    index if (index as usize) < self.instance_count => {
                        Value::Instance(index as usize)
                    }
                    _ => return Err(corrupt("embedded instance index")),
                }
            }
            Kind::Map => return Err(corrupt("an array of hash maps")),
            kind => scalar(&mut self.dynamic, kind)?,
        })
    }

    /// `{u32 size, u32 checksum, i64 offset}` at the dynamic position, then
    /// the bytes at that offset.
    fn string(&mut self) -> Result<Value, Error> {
        let size = self.dynamic.i32()?;
        if size == 0 {
            return Ok(Value::String(String::new()));
        }
        let _checksum = self.dynamic.u32()?;
        let offset = self.dynamic.i64()?;
        self.dynamic.seek(offset as usize)?;
        let bytes = self.dynamic.take(size as usize)?;
        Ok(Value::String(String::from_utf8_lossy(bytes).into_owned()))
    }

    /// `{i32 count, u32, i64 table, i64 entries}`; the table's last value is
    /// the number of `{u64 key, u64 instance}` entries.
    fn map(&mut self) -> Result<Value, Error> {
        let offset = self.data.i32()?;
        if offset == -1 {
            return Ok(Value::Map(Vec::new()));
        }
        self.dynamic.seek(offset as usize)?;
        let table_size = self.dynamic.i32()?;
        if table_size == 0 {
            return Ok(Value::Map(Vec::new()));
        }
        let _unknown = self.dynamic.u32()?;
        let table = self.dynamic.i64()?;
        let entries = self.dynamic.i64()?;
        self.dynamic
            .seek(table as usize + 4 * (table_size as usize - 1))?;
        let count = self.dynamic.u32()?;
        self.dynamic.seek(entries as usize)?;
        let mut map = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let key = self.dynamic.u64()?;
            let value = self.dynamic.u64()? as i32;
            let index = usize::try_from(value)
                .ok()
                .filter(|&index| index < self.instance_count);
            map.push((key, index));
        }
        Ok(Value::Map(map))
    }

    /// Asset references are stored xored with the field hash and the header
    /// checksum, with their bytes shuffled.
    fn guid(&self, field: u32, stored: u64) -> u64 {
        if stored == 0 {
            return 0;
        }
        let field = u64::from(field) | u64::from(field) << 32;
        let mut bytes = (stored ^ field ^ self.checksum).to_le_bytes();
        for (a, b) in [(0, 3), (7, 1), (2, 6), (4, 5)] {
            bytes.swap(a, b);
        }
        u64::from_le_bytes(bytes)
    }
}

fn scalar(cursor: &mut Cursor<'_>, kind: Kind) -> Result<Value, Error> {
    Ok(match kind {
        Kind::Int { bytes, signed } => {
            let raw = cursor.take(usize::from(bytes))?;
            let mut buffer = [0; 8];
            buffer[..raw.len()].copy_from_slice(raw);
            let unsigned = u64::from_le_bytes(buffer);
            if signed {
                let shift = 64 - 8 * u32::from(bytes);
                Value::Int(((unsigned << shift) as i64) >> shift)
            } else {
                Value::UInt(unsigned)
            }
        }
        Kind::F32 => Value::Float(f64::from(f32::from_le_bytes(take_array(cursor)?))),
        Kind::F64 => Value::Float(f64::from_le_bytes(take_array(cursor)?)),
        Kind::Floats(count) => Value::Floats(
            (0..count)
                .map(|_| take_array(cursor).map(f32::from_le_bytes))
                .collect::<Result<_, _>>()?,
        ),
        Kind::Bytes(count) => Value::Bytes(cursor.take(usize::from(count))?.to_vec()),
        Kind::String | Kind::Guid | Kind::Embed | Kind::Inline(_) | Kind::Map => {
            unreachable!("handled by the reader")
        }
    })
}

fn take_array<const N: usize>(cursor: &mut Cursor<'_>) -> Result<[u8; N], Error> {
    Ok(cursor.take(N)?.try_into().expect("N bytes"))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

struct Entry {
    start: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Cursor { bytes, position: 0 }
    }

    fn seek(&mut self, position: usize) -> Result<(), Error> {
        if position > self.bytes.len() {
            return Err(corrupt("an offset past the buffer"));
        }
        self.position = position;
        Ok(())
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let bytes = self
            .bytes
            .get(self.position..self.position + count)
            .ok_or_else(|| corrupt("a read past the buffer"))?;
        self.position += count;
        Ok(bytes)
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(take_array(self)?))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(take_array(self)?))
    }

    fn i64(&mut self) -> Result<i64, Error> {
        Ok(i64::from_le_bytes(take_array(self)?))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(take_array(self)?))
    }
}

/// A table `{i32 count, i32 offset}` at the cursor: the start of each of its
/// `count` entries of `width` bytes.
fn bag(cursor: &mut Cursor<'_>, width: usize) -> Result<Vec<Entry>, Error> {
    let count = cursor.i32()?;
    let offset = cursor.i32()?;
    let (Ok(count), Ok(offset)) = (usize::try_from(count), usize::try_from(offset)) else {
        return Err(corrupt("table header"));
    };
    if offset + count * width > cursor.bytes.len() {
        return Err(corrupt("table"));
    }
    Ok((0..count)
        .map(|index| Entry {
            start: offset + index * width,
        })
        .collect())
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("checked by bag"))
}

fn i32_at(bytes: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(bytes[at..at + 4].try_into().expect("checked by bag"))
}

fn corrupt(what: &str) -> Error {
    Error(format!("corrupt STU asset: {what}"))
}

/// The CRC-64 TankLib checksums the header with (reflected polynomial
/// `0x95AC9329AC4BC9B5`, initial value and output inverted).
const CRC64: [u64; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u64;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x95AC_9329_AC4B_C9B5
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
};

/// TankLib passes `0xFFFFFFFFFFFFFFFF` as the running value, which the
/// inversion turns into a zero start.
fn crc64(bytes: &[u8]) -> u64 {
    let mut crc = 0u64;
    for &byte in bytes {
        crc = (crc >> 8) ^ CRC64[((crc ^ u64::from(byte)) & 0xFF) as usize];
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc64_table_matches_tanklib() {
        // Entries 1, 128 and 255 of TankLib's CRC64Tab.
        assert_eq!(CRC64[1], 0x7ad8_70c8_3035_8979);
        assert_eq!(CRC64[128], 0x95ac_9329_ac4b_c9b5);
        assert_eq!(CRC64[255], 0x29b7_d047_efec_8728);
    }
}
