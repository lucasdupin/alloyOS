//! Bounds checked GGUF v2/v3 reader. Only F32/F16 tensor encodings are executable.
use alloc::{collections::BTreeMap, string::String, vec::Vec};
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Uint(u64),
    Int(i64),
    Float(f64),
    Text(String),
    Array(Vec<Value>),
    Bool(bool),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    Truncated,
    InvalidFormat,
    UnsupportedEncoding(u32),
    MissingMetadata,
    InvalidShape,
}
impl LoadError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Truncated => "truncated_weights",
            Self::InvalidFormat => "invalid_gguf",
            Self::UnsupportedEncoding(_) => "unsupported_tensor_encoding",
            Self::MissingMetadata => "missing_model_metadata",
            Self::InvalidShape => "invalid_tensor_shape",
        }
    }
}
#[derive(Debug, Clone)]
pub struct Tensor {
    pub dimensions: Vec<usize>,
    pub encoding: u32,
    pub offset: usize,
    pub bytes: usize,
}
pub struct Gguf {
    pub metadata: BTreeMap<String, Value>,
    pub tensors: BTreeMap<String, Tensor>,
    pub data_offset: usize,
}
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    string_bytes: usize,
}
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], LoadError> {
        let end = self.pos.checked_add(n).ok_or(LoadError::InvalidFormat)?;
        let slice = self.data.get(self.pos..end).ok_or(LoadError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }
    fn uint(&mut self, n: usize) -> Result<u64, LoadError> {
        let b = self.bytes(n)?;
        let mut v = 0;
        for (i, x) in b.iter().enumerate() {
            v |= (*x as u64) << (8 * i)
        }
        Ok(v)
    }
    fn string(&mut self) -> Result<String, LoadError> {
        let n = usize::try_from(self.uint(8)?).map_err(|_| LoadError::InvalidFormat)?;
        self.string_bytes = self
            .string_bytes
            .checked_add(n)
            .ok_or(LoadError::InvalidFormat)?;
        if n > 1024 * 1024 || self.string_bytes > 32 * 1024 * 1024 {
            return Err(LoadError::InvalidFormat);
        }
        String::from_utf8(self.bytes(n)?.to_vec()).map_err(|_| LoadError::InvalidFormat)
    }
    fn value(&mut self, t: u32, depth: u8) -> Result<Value, LoadError> {
        if depth > 4 {
            return Err(LoadError::InvalidFormat);
        }
        Ok(match t {
            0 => Value::Uint(self.uint(1)?),
            1 => Value::Int(self.uint(1)? as i8 as i64),
            2 => Value::Uint(self.uint(2)?),
            3 => Value::Int(self.uint(2)? as i16 as i64),
            4 => Value::Uint(self.uint(4)?),
            5 => Value::Int(self.uint(4)? as i32 as i64),
            6 => Value::Float(f32::from_bits(self.uint(4)? as u32) as f64),
            7 => match self.uint(1)? {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return Err(LoadError::InvalidFormat),
            },
            8 => Value::Text(self.string()?),
            9 => {
                let kind = self.uint(4)? as u32;
                if kind == 9 {
                    return Err(LoadError::InvalidFormat);
                }
                let count = usize::try_from(self.uint(8)?).map_err(|_| LoadError::InvalidFormat)?;
                if count > 1_000_000 || count > self.data.len().saturating_sub(self.pos) {
                    return Err(LoadError::InvalidFormat);
                }
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.value(kind, depth + 1)?)
                }
                Value::Array(values)
            }
            10 => Value::Uint(self.uint(8)?),
            11 => Value::Int(self.uint(8)? as i64),
            12 => Value::Float(f64::from_bits(self.uint(8)?)),
            _ => return Err(LoadError::InvalidFormat),
        })
    }
}
impl Gguf {
    pub fn parse(data: &[u8]) -> Result<Self, LoadError> {
        let mut r = Reader {
            data,
            pos: 0,
            string_bytes: 0,
        };
        if r.bytes(4)? != b"GGUF" {
            return Err(LoadError::InvalidFormat);
        }
        if !matches!(r.uint(4)?, 2 | 3) {
            return Err(LoadError::InvalidFormat);
        }
        let tensor_count = r.uint(8)?;
        let kv_count = r.uint(8)?;
        if tensor_count > 10000 || kv_count > 10000 {
            return Err(LoadError::InvalidFormat);
        }
        let mut metadata = BTreeMap::new();
        for _ in 0..kv_count {
            let key = r.string()?;
            let ty = r.uint(4)? as u32;
            let v = r.value(ty, 0)?;
            if metadata.insert(key, v).is_some() {
                return Err(LoadError::InvalidFormat);
            }
        }
        let mut tensors = BTreeMap::new();
        for _ in 0..tensor_count {
            let name = r.string()?;
            let n = r.uint(4)?;
            if n == 0 || n > 4 {
                return Err(LoadError::InvalidShape);
            }
            let mut dimensions = Vec::new();
            let mut count = 1usize;
            for _ in 0..n {
                let dim = usize::try_from(r.uint(8)?).map_err(|_| LoadError::InvalidShape)?;
                if dim == 0 {
                    return Err(LoadError::InvalidShape);
                }
                count = count.checked_mul(dim).ok_or(LoadError::InvalidShape)?;
                dimensions.push(dim)
            }
            let encoding = r.uint(4)? as u32;
            let offset = usize::try_from(r.uint(8)?).map_err(|_| LoadError::InvalidFormat)?;
            let width = match encoding {
                0 => 4,
                1 => 2,
                _ => return Err(LoadError::UnsupportedEncoding(encoding)),
            };
            let bytes = count.checked_mul(width).ok_or(LoadError::InvalidShape)?;
            if tensors
                .insert(
                    name,
                    Tensor {
                        dimensions,
                        encoding,
                        offset,
                        bytes,
                    },
                )
                .is_some()
            {
                return Err(LoadError::InvalidFormat);
            }
        }
        let alignment = match metadata.get("general.alignment") {
            Some(Value::Uint(n)) => *n as usize,
            None => 32,
            _ => return Err(LoadError::InvalidFormat),
        };
        if alignment == 0 || alignment > 4096 || !alignment.is_power_of_two() {
            return Err(LoadError::InvalidFormat);
        }
        let data_offset = r
            .pos
            .checked_add(alignment - 1)
            .ok_or(LoadError::InvalidFormat)?
            & !(alignment - 1);
        for t in tensors.values() {
            let start = data_offset
                .checked_add(t.offset)
                .ok_or(LoadError::InvalidFormat)?;
            let end = start.checked_add(t.bytes).ok_or(LoadError::InvalidFormat)?;
            if t.offset % alignment != 0 || end > data.len() {
                return Err(LoadError::Truncated);
            }
        }
        Ok(Self {
            metadata,
            tensors,
            data_offset,
        })
    }
    pub fn uint(&self, key: &str) -> Result<usize, LoadError> {
        match self.metadata.get(key) {
            Some(Value::Uint(n)) => usize::try_from(*n).map_err(|_| LoadError::InvalidFormat),
            _ => Err(LoadError::MissingMetadata),
        }
    }
    pub fn float(&self, key: &str) -> Result<f32, LoadError> {
        match self.metadata.get(key) {
            Some(Value::Float(n)) => Ok(*n as f32),
            _ => Err(LoadError::MissingMetadata),
        }
    }
    pub fn text(&self, key: &str) -> Result<&str, LoadError> {
        match self.metadata.get(key) {
            Some(Value::Text(s)) => Ok(s),
            _ => Err(LoadError::MissingMetadata),
        }
    }
    pub fn tensor<'a>(&'a self, data: &'a [u8], key: &str) -> Result<TensorView<'a>, LoadError> {
        let t = self.tensors.get(key).ok_or(LoadError::MissingMetadata)?;
        Ok(TensorView {
            shape: &t.dimensions,
            encoding: t.encoding,
            data: &data[self.data_offset + t.offset..self.data_offset + t.offset + t.bytes],
        })
    }
}
pub struct TensorView<'a> {
    pub shape: &'a [usize],
    encoding: u32,
    data: &'a [u8],
}
impl TensorView<'_> {
    pub fn value(&self, i: usize) -> f32 {
        if self.encoding == 0 {
            let p = i * 4;
            f32::from_le_bytes(self.data[p..p + 4].try_into().unwrap())
        } else {
            let p = i * 2;
            half(u16::from_le_bytes(self.data[p..p + 2].try_into().unwrap()))
        }
    }
    pub fn row(&self, row: usize) -> Vec<f32> {
        (0..self.shape[0])
            .map(|i| self.value(row * self.shape[0] + i))
            .collect()
    }
    pub fn matvec(&self, x: &[f32]) -> Vec<f32> {
        (0..self.shape[1])
            .map(|row| {
                x.iter()
                    .enumerate()
                    .map(|(i, v)| self.value(row * self.shape[0] + i) * v)
                    .sum()
            })
            .collect()
    }
}
pub fn half(v: u16) -> f32 {
    let sign = ((v & 0x8000) as u32) << 16;
    let e = (v >> 10) & 31;
    let m = v & 1023;
    let bits = if e == 0 {
        if m == 0 {
            sign
        } else {
            let mut m = m as u32;
            let mut e = 113u32;
            while m & 1024 == 0 {
                m <<= 1;
                e -= 1
            }
            sign | (e << 23) | ((m & 1023) << 13)
        }
    } else if e == 31 {
        sign | 0x7f800000 | ((m as u32) << 13)
    } else {
        sign | (((e as u32) + 112) << 23) | ((m as u32) << 13)
    };
    f32::from_bits(bits)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn half_known_values() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert_eq!(half(1), libm::powf(2.0, -24.0));
        assert!(half(0x7c00).is_infinite());
    }
    #[test]
    fn truncated_never_panics() {
        for n in 0..24 {
            assert!(Gguf::parse(&b"GGUF\x03\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"[..n]).is_err())
        }
    }
}
