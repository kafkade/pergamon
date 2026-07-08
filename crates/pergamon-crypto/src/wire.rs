// SPDX-License-Identifier: Apache-2.0

//! Minimal, dependency-free byte-cursor for parsing the crate's canonical wire
//! encodings (signed device records and attestations).
//!
//! The encoders build bytes by hand with length prefixes and fixed-width fields;
//! this reader is the symmetric decoder. It never panics: every out-of-bounds
//! read maps to [`CryptoError::Malformed`].

use crate::error::{CryptoError, Result};

/// A forward-only reader over a byte slice.
pub struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Wrap `bytes` for sequential reading.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Take the next `len` bytes, or fail if the buffer is too short.
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(CryptoError::Malformed("wire length overflow"))?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(CryptoError::Malformed("wire input truncated"))?;
        self.pos = end;
        Ok(slice)
    }

    /// Consume and check a fixed magic tag prefix.
    pub fn expect_tag(&mut self, tag: &[u8]) -> Result<()> {
        let got = self.take(tag.len())?;
        if got == tag {
            Ok(())
        } else {
            Err(CryptoError::Malformed("wire tag mismatch"))
        }
    }

    /// Read a big-endian `u32`.
    pub fn read_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a big-endian `i64`.
    pub fn read_i64(&mut self) -> Result<i64> {
        let b = self.take(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(i64::from_be_bytes(arr))
    }

    /// Read a fixed-size array of `N` bytes.
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let b = self.take(N)?;
        let mut arr = [0u8; N];
        arr.copy_from_slice(b);
        Ok(arr)
    }

    /// Read `len` bytes and decode them as UTF-8.
    pub fn read_string(&mut self, len: usize) -> Result<String> {
        let b = self.take(len)?;
        String::from_utf8(b.to_vec()).map_err(|_| CryptoError::Malformed("wire string not utf-8"))
    }

    /// Read a single wire byte.
    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Fail unless every byte has been consumed (rejects trailing garbage).
    pub const fn expect_end(&self) -> Result<()> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(CryptoError::Malformed("trailing bytes after wire record"))
        }
    }
}
