//! SBE [`View`]: read-only access into an SBE-encoded buffer with no
//! `DynamicMessage` allocation. Mirrors `protowire/encoding/sbe/view.go` and
//! the TS port's `sbe/view.ts`.
//!
//! Strings allocate (Rust has no zero-copy `&str` from arbitrary bytes), but
//! `bytes_field()` returns a borrowed slice into the underlying buffer, and
//! `composite()` / `group()` return new views over the same backing slice
//! without copying.

use crate::codec::{Codec, GROUP_HEADER_SIZE, HEADER_SIZE};
use crate::errors::SbeError;
use crate::template::{FieldTemplate, GroupTemplate, SbeEncoding};

/// Allocation-free SBE reader. The view borrows both the data slice and the
/// codec's templates — both must outlive the view.
#[derive(Debug)]
pub struct View<'a> {
    data: &'a [u8],
    block_start: usize,
    block_end: usize,
    fields: &'a [FieldTemplate],
    groups: &'a [GroupTemplate],
    groups_start: usize,
}

impl Codec {
    /// Construct a [`View`] over `data` by looking up its template ID.
    pub fn view<'a>(&'a self, data: &'a [u8]) -> Result<View<'a>, SbeError> {
        if data.len() < HEADER_SIZE {
            return Err(SbeError::new("sbe: data too short for header"));
        }
        let block_length = read_u16_le(data, 0) as usize;
        let template_id = read_u16_le(data, 2) as u32;
        let tmpl = self.template_by_id(template_id)?;
        let end = HEADER_SIZE + block_length;
        if data.len() < end {
            return Err(SbeError::new("sbe: data too short for root block"));
        }
        Ok(View {
            data,
            block_start: HEADER_SIZE,
            block_end: end,
            fields: &tmpl.fields,
            groups: &tmpl.groups,
            groups_start: end,
        })
    }
}

impl<'a> View<'a> {
    fn block(&self) -> &'a [u8] {
        &self.data[self.block_start..self.block_end]
    }

    fn field(&self, name: &str) -> Result<&'a FieldTemplate, SbeError> {
        self.fields
            .iter()
            .find(|f| f.fd.name() == name)
            .ok_or_else(|| SbeError::new(format!("sbe: unknown field: {}", name)))
    }

    pub fn int_field(&self, name: &str) -> Result<i64, SbeError> {
        let ft = self.field(name)?;
        let off = ft.offset;
        let block = self.block();
        Ok(match ft.encoding.expect("scalar field has encoding") {
            SbeEncoding::Int8 => (block[off] as i8) as i64,
            SbeEncoding::Int16 => i16::from_le_bytes([block[off], block[off + 1]]) as i64,
            SbeEncoding::Int32 => i32::from_le_bytes([
                block[off],
                block[off + 1],
                block[off + 2],
                block[off + 3],
            ]) as i64,
            SbeEncoding::Int64 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&block[off..off + 8]);
                i64::from_le_bytes(b)
            }
            other => {
                return Err(SbeError::new(format!(
                    "sbe: field {} is not a signed integer (encoding {})",
                    name,
                    other.name()
                )));
            }
        })
    }

    pub fn uint_field(&self, name: &str) -> Result<u64, SbeError> {
        let ft = self.field(name)?;
        let off = ft.offset;
        let block = self.block();
        Ok(match ft.encoding.expect("scalar field has encoding") {
            SbeEncoding::Uint8 => block[off] as u64,
            SbeEncoding::Uint16 => u16::from_le_bytes([block[off], block[off + 1]]) as u64,
            SbeEncoding::Uint32 => u32::from_le_bytes([
                block[off],
                block[off + 1],
                block[off + 2],
                block[off + 3],
            ]) as u64,
            SbeEncoding::Uint64 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&block[off..off + 8]);
                u64::from_le_bytes(b)
            }
            other => {
                return Err(SbeError::new(format!(
                    "sbe: field {} is not an unsigned integer (encoding {})",
                    name,
                    other.name()
                )));
            }
        })
    }

    pub fn float_field(&self, name: &str) -> Result<f64, SbeError> {
        let ft = self.field(name)?;
        let off = ft.offset;
        let block = self.block();
        Ok(match ft.encoding.expect("scalar field has encoding") {
            SbeEncoding::Float => f32::from_le_bytes([
                block[off],
                block[off + 1],
                block[off + 2],
                block[off + 3],
            ]) as f64,
            SbeEncoding::Double => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&block[off..off + 8]);
                f64::from_le_bytes(b)
            }
            other => {
                return Err(SbeError::new(format!(
                    "sbe: field {} is not a float (encoding {})",
                    name,
                    other.name()
                )));
            }
        })
    }

    pub fn bool_field(&self, name: &str) -> Result<bool, SbeError> {
        let ft = self.field(name)?;
        Ok(self.block()[ft.offset] != 0)
    }

    pub fn enum_field(&self, name: &str) -> Result<i32, SbeError> {
        let ft = self.field(name)?;
        let off = ft.offset;
        let block = self.block();
        Ok(match ft.encoding.expect("scalar field has encoding") {
            SbeEncoding::Uint8 => block[off] as i32,
            SbeEncoding::Uint16 => u16::from_le_bytes([block[off], block[off + 1]]) as i32,
            other => {
                return Err(SbeError::new(format!(
                    "sbe: field {} has unsupported enum encoding ({})",
                    name,
                    other.name()
                )));
            }
        })
    }

    /// Trims trailing NUL padding before UTF-8 decoding.
    pub fn string_field(&self, name: &str) -> Result<String, SbeError> {
        let ft = self.field(name)?;
        let slice = &self.block()[ft.offset..ft.offset + ft.size];
        let mut n = slice.len();
        while n > 0 && slice[n - 1] == 0 {
            n -= 1;
        }
        String::from_utf8(slice[..n].to_vec())
            .map_err(|e| SbeError::new(format!("sbe: invalid utf-8 in {}: {}", name, e)))
    }

    /// Borrowed slice over the field's bytes (no copy).
    pub fn bytes_field(&self, name: &str) -> Result<&'a [u8], SbeError> {
        let ft = self.field(name)?;
        Ok(&self.block()[ft.offset..ft.offset + ft.size])
    }

    pub fn composite(&self, name: &str) -> Result<View<'a>, SbeError> {
        let ft = self.field(name)?;
        if ft.composite.is_empty() {
            return Err(SbeError::new(format!(
                "sbe: field {} is not a composite",
                name
            )));
        }
        Ok(View {
            data: self.data,
            block_start: self.block_start + ft.offset,
            block_end: self.block_start + ft.offset + ft.size,
            fields: &ft.composite,
            groups: &[],
            groups_start: 0,
        })
    }

    pub fn group(&self, name: &str) -> Result<GroupView<'a>, SbeError> {
        let mut pos = self.groups_start;
        for gt in self.groups {
            if pos + GROUP_HEADER_SIZE > self.data.len() {
                return Err(SbeError::new("sbe: data too short for group header"));
            }
            let block_length = read_u16_le(self.data, pos) as usize;
            let count = read_u16_le(self.data, pos + 2) as usize;
            if gt.fd.name() == name {
                return Ok(GroupView {
                    data: self.data,
                    base: pos,
                    block_length,
                    count,
                    fields: &gt.fields,
                });
            }
            pos += GROUP_HEADER_SIZE + count * block_length;
        }
        Err(SbeError::new(format!("sbe: unknown group: {}", name)))
    }
}

#[derive(Debug)]
pub struct GroupView<'a> {
    data: &'a [u8],
    base: usize,
    block_length: usize,
    count: usize,
    fields: &'a [FieldTemplate],
}

impl<'a> GroupView<'a> {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn entry(&self, i: usize) -> Result<View<'a>, SbeError> {
        if i >= self.count {
            return Err(SbeError::new(format!(
                "sbe: group entry {} out of range [0, {})",
                i, self.count
            )));
        }
        let start = self.base + GROUP_HEADER_SIZE + i * self.block_length;
        Ok(View {
            data: self.data,
            block_start: start,
            block_end: start + self.block_length,
            fields: self.fields,
            groups: &[],
            groups_start: 0,
        })
    }
}

fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}
