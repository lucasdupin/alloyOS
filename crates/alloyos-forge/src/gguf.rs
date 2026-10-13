//! Bounds checked GGUF v2/v3 reader. F32/F16 and Q4_0 tensor encodings are executable.
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
            let bytes = match encoding {
                0 => count.checked_mul(4),
                1 => count.checked_mul(2),
                2 if dimensions[0] % 32 == 0 => (count / 32).checked_mul(18),
                2 => return Err(LoadError::InvalidShape),
                _ => return Err(LoadError::UnsupportedEncoding(encoding)),
            }.ok_or(LoadError::InvalidShape)?;
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
pub struct Q8Block {
    scale: f32,
    values: [i8; 32],
}
// These loads access checked tensor/activation ranges in Normal RAM. An
// explicit SIMD load avoids the bare-metal target scalarizing unaligned loads
// without changing alignment policy for the rest of the kernel or compiler flags.
// Safety: the pointer must refer to at least 16 readable bytes in Normal RAM;
// the guest must allow unaligned accesses (SCTLR_EL1.A = 0).
#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn load_u8x16(pointer: *const u8) -> core::arch::aarch64::uint8x16_t {
    let value;
    core::arch::asm!("ldr {value:q}, [{pointer}]",
        value = out(vreg) value, pointer = in(reg) pointer,
        options(nostack, readonly, preserves_flags));
    value
}
#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn load_i8x16(pointer: *const i8) -> core::arch::aarch64::int8x16_t {
    core::arch::aarch64::vreinterpretq_s8_u8(load_u8x16(pointer.cast()))
}
#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn load_f32x4(pointer: *const f32) -> core::arch::aarch64::float32x4_t {
    core::arch::aarch64::vreinterpretq_f32_u8(load_u8x16(pointer.cast()))
}
impl TensorView<'_> {
    /// Quantize activations once per projection, shared by all weight rows.
    pub fn quantize_input(&self, x: &[f32]) -> Option<Vec<Q8Block>> {
        if self.encoding != 2 { return None; }
        assert_eq!(x.len(), self.shape[0]);
        Some(x.chunks_exact(32).map(|chunk| {
            let max = chunk.iter().fold(0.0f32, |max, v| max.max(v.abs()));
            let scale = max / 127.0;
            let inverse = if scale > 0.0 { 1.0 / scale } else { 0.0 };
            let mut values = [0i8; 32];
            for (out, value) in values.iter_mut().zip(chunk) {
                *out = libm::roundf(value * inverse).clamp(-127.0, 127.0) as i8;
            }
            Q8Block { scale, values }
        }).collect())
    }
    pub fn dot_quantized(&self, row: usize, x: &[Q8Block]) -> f32 {
        assert!(self.encoding == 2 && row < self.shape[1] && x.len() * 32 == self.shape[0]);
        #[cfg(not(target_arch = "aarch64"))]
        let mut result = 0.0;
        #[cfg(target_arch = "aarch64")]
        let mut result = unsafe { core::arch::aarch64::vdupq_n_f32(0.0) };
        for (i, input) in x.iter().enumerate() {
            let block = (row * x.len() + i) * 18;
            let scale = half(u16::from_le_bytes(self.data[block..block + 2].try_into().unwrap()));
            #[cfg(target_arch = "aarch64")]
            unsafe {
                use core::arch::aarch64::*;
                let packed = load_u8x16(self.data.as_ptr().add(block + 2));
                let low = vreinterpretq_s8_u8(vsubq_u8(vandq_u8(packed, vdupq_n_u8(15)), vdupq_n_u8(8)));
                let high = vreinterpretq_s8_u8(vsubq_u8(vshrq_n_u8::<4>(packed), vdupq_n_u8(8)));
                let a = load_i8x16(input.values.as_ptr());
                let b = load_i8x16(input.values.as_ptr().add(16));
                let mut sum = vdupq_n_s32(0);
                sum = vpadalq_s16(sum, vmull_s8(vget_low_s8(low), vget_low_s8(a)));
                sum = vpadalq_s16(sum, vmull_s8(vget_high_s8(low), vget_high_s8(a)));
                sum = vpadalq_s16(sum, vmull_s8(vget_low_s8(high), vget_low_s8(b)));
                sum = vpadalq_s16(sum, vmull_s8(vget_high_s8(high), vget_high_s8(b)));
                // Keep four independent float accumulators across blocks instead
                // of horizontally reducing each block into a scalar dependency.
                result = vfmaq_n_f32(result, vcvtq_f32_s32(sum), scale * input.scale);
            }
            #[cfg(not(target_arch = "aarch64"))]
            let dot: i32 = (0..32).map(|j| {
                let packed = self.data[block + 2 + j % 16];
                let weight = if j < 16 { packed & 15 } else { packed >> 4 };
                (weight as i32 - 8) * input.values[j] as i32
            }).sum();
            #[cfg(not(target_arch = "aarch64"))]
            { result += dot as f32 * scale * input.scale; }
        }
        #[cfg(target_arch = "aarch64")]
        { unsafe { core::arch::aarch64::vaddvq_f32(result) } }
        #[cfg(not(target_arch = "aarch64"))]
        { result }
    }
    pub fn value(&self, i: usize) -> f32 {
        if self.encoding == 0 {
            let p = i * 4;
            f32::from_le_bytes(self.data[p..p + 4].try_into().unwrap())
        } else if self.encoding == 2 {
            let block = (i / 32) * 18;
            let scale = half(u16::from_le_bytes(self.data[block..block + 2].try_into().unwrap()));
            let index = i % 32;
            let packed = self.data[block + 2 + index % 16];
            let quant = if index < 16 { packed & 15 } else { packed >> 4 };
            scale * (quant as i32 - 8) as f32
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
    pub fn dot_range(&self, row: usize, x: &[f32], start: usize, end: usize) -> f32 {
        assert!(start <= end && end <= x.len() && x.len() == self.shape[0] && row < self.shape[1]);
        #[cfg(target_arch = "aarch64")]
        if self.encoding == 2 {
            unsafe {
                use core::arch::aarch64::*;
                let mut result = 0.0;
                let mut i = start;
                while i < end && i % 32 != 0 {
                    result += self.value(row * x.len() + i) * x[i];
                    i += 1;
                }
                let mut sum = vdupq_n_f32(0.0);
                while i + 32 <= end {
                    let block = (row * x.len() + i) / 32 * 18;
                    let scale = half(u16::from_le_bytes(self.data[block..block + 2].try_into().unwrap()));
                    let packed = load_u8x16(self.data.as_ptr().add(block + 2));
                    let low = vreinterpretq_s8_u8(vsubq_u8(vandq_u8(packed, vdupq_n_u8(15)), vdupq_n_u8(8)));
                    let high = vreinterpretq_s8_u8(vsubq_u8(vshrq_n_u8::<4>(packed), vdupq_n_u8(8)));
                    let mut dot = vdupq_n_f32(0.0);
                    for (part, quant) in [low, high].into_iter().enumerate() {
                        for (half_id, wide) in [vmovl_s8(vget_low_s8(quant)), vmovl_s8(vget_high_s8(quant))].into_iter().enumerate() {
                            let base = x.as_ptr().add(i + part * 16 + half_id * 8);
                            dot = vfmaq_f32(dot, vcvtq_f32_s32(vmovl_s16(vget_low_s16(wide))), load_f32x4(base));
                            dot = vfmaq_f32(dot, vcvtq_f32_s32(vmovl_s16(vget_high_s16(wide))), load_f32x4(base.add(4)));
                        }
                    }
                    sum = vfmaq_n_f32(sum, dot, scale);
                    i += 32;
                }
                result += vaddvq_f32(sum);
                while i < end {
                    result += self.value(row * x.len() + i) * x[i];
                    i += 1;
                }
                return result;
            }
        }
        #[cfg(target_arch = "aarch64")]
        if self.encoding == 0 {
            // Unaligned NEON loads are valid; ranges are checked above.
            unsafe {
                use core::arch::aarch64::*;
                // Independent accumulators let NEON overlap FMA instructions
                // instead of waiting on one dependency chain for each vector.
                let mut sum = vdupq_n_f32(0.0);
                let mut sum1 = sum;
                let mut sum2 = sum;
                let mut sum3 = sum;
                let mut i = start;
                while i + 16 <= end {
                    let weights = self.data.as_ptr().add((row * x.len() + i) * 4) as *const f32;
                    let input = x.as_ptr().add(i);
                    sum = vfmaq_f32(sum, load_f32x4(weights), load_f32x4(input));
                    sum1 = vfmaq_f32(sum1, load_f32x4(weights.add(4)), load_f32x4(input.add(4)));
                    sum2 = vfmaq_f32(sum2, load_f32x4(weights.add(8)), load_f32x4(input.add(8)));
                    sum3 = vfmaq_f32(sum3, load_f32x4(weights.add(12)), load_f32x4(input.add(12)));
                    i += 16;
                }
                sum = vaddq_f32(vaddq_f32(sum, sum1), vaddq_f32(sum2, sum3));
                while i + 4 <= end {
                    let weights = self.data.as_ptr().add((row * x.len() + i) * 4) as *const f32;
                    sum = vfmaq_f32(sum, load_f32x4(weights), load_f32x4(x.as_ptr().add(i)));
                    i += 4;
                }
                let mut result = vaddvq_f32(sum);
                for i in i..end {
                    result += self.value(row * x.len() + i) * x[i];
                }
                return result;
            }
        }
        (start..end)
            .map(|i| self.value(row * x.len() + i) * x[i])
            .sum()
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
    fn q4_loader_checks_block_width_and_payload_size() {
        fn fixture(width: u64) -> Vec<u8> {
            let mut bytes = b"GGUF".to_vec();
            bytes.extend(3u32.to_le_bytes());
            bytes.extend(1u64.to_le_bytes());
            bytes.extend(0u64.to_le_bytes());
            bytes.extend(1u64.to_le_bytes());
            bytes.push(b'w');
            bytes.extend(2u32.to_le_bytes());
            bytes.extend(width.to_le_bytes());
            bytes.extend(1u64.to_le_bytes());
            bytes.extend(2u32.to_le_bytes());
            bytes.extend(0u64.to_le_bytes());
            bytes.resize((bytes.len() + 31) / 32 * 32 + 18, 0);
            bytes
        }
        let bytes = fixture(32);
        assert_eq!(Gguf::parse(&bytes).unwrap().tensors["w"].bytes, 18);
        assert!(matches!(Gguf::parse(&fixture(31)), Err(LoadError::InvalidShape)));
        assert!(matches!(Gguf::parse(&bytes[..bytes.len() - 1]), Err(LoadError::Truncated)));
    }
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

#[cfg(test)]
mod dot_product_tests {
    use super::*;

    #[test]
    fn q4_blocks_decode_signed_nibbles_and_dot_ranges_match_reference() {
        let mut data = Vec::new();
        let mut reference = Vec::new();
        for block in 0..6 {
            let scale = if block % 2 == 0 { 0.5 } else { -0.25 };
            data.extend(if block % 2 == 0 { 0x3800u16 } else { 0xb400u16 }.to_le_bytes());
            for j in 0..16 { data.push(((j + block) % 16) as u8 | (((15 - j + block) % 16) as u8) << 4); }
            for j in 0..32 {
                let quant = if j < 16 { (j + block) % 16 } else { (15 - (j - 16) + block) % 16 };
                reference.push(scale * (quant as i32 - 8) as f32);
            }
        }
        let tensor = TensorView { shape: &[96, 2], encoding: 2, data: &data };
        for (i, expected) in reference.iter().enumerate() { assert_eq!(tensor.value(i), *expected); }
        let input: Vec<f32> = (0..96).map(|i| (i as f32 - 40.0) / 16.0).collect();
        let quantized = tensor.quantize_input(&input).unwrap();
        for row in 0..2 {
            let expected: f32 = (0..96).map(|i| {
                let block = &quantized[i / 32];
                reference[row * 96 + i] * block.values[i % 32] as f32 * block.scale
            }).sum();
            assert!((tensor.dot_quantized(row, &quantized) - expected).abs() < 0.001);
            for (start, end) in [(0, 96), (1, 95), (17, 70), (32, 64), (5, 8), (3, 3)] {
                let expected: f32 = (start..end).map(|i| reference[row * 96 + i] * input[i]).sum();
                assert!((tensor.dot_range(row, &input, start, end) - expected).abs() < 0.001);
            }
        }
        let zero = tensor.quantize_input(&[0.0; 96]).unwrap();
        assert_eq!(tensor.dot_quantized(0, &zero), 0.0);
    }

    #[test]
    fn f32_dot_ranges_match_reference_for_vector_blocks_and_tails() {
        let width = 65;
        let weights: Vec<f32> = (0..width * 2).map(|i| ((i % 19) as f32 - 9.0) / 8.0).collect();
        let data: Vec<u8> = weights.iter().flat_map(|value| value.to_le_bytes()).collect();
        let input: Vec<f32> = (0..width).map(|i| ((i % 13) as f32 - 6.0) / 4.0).collect();
        let tensor = TensorView { shape: &[65, 2], encoding: 0, data: &data };
        for row in 0..2 {
            for (start, end) in [(0, 65), (1, 64), (5, 8), (17, 65), (3, 3)] {
                let reference: f32 = (start..end).map(|i| weights[row * width + i] * input[i]).sum();
                let actual = tensor.dot_range(row, &input, start, end);
                assert!((actual - reference).abs() < 0.0001, "row={row}, range={start}..{end}: {actual} vs {reference}");
            }
        }
    }
}
