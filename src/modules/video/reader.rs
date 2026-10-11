//! Bounds-checked readers over in-memory buffers. Every read returns `None` past the end, so a
//! truncated or corrupt header yields partial info instead of a panic.

use std::io::{Read, Seek, SeekFrom};

pub struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Bytes { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }

    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    pub fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.array::<1>()?[0])
    }

    pub fn u16be(&mut self) -> Option<u16> {
        self.array().map(u16::from_be_bytes)
    }

    pub fn u24be(&mut self) -> Option<u32> {
        let [a, b, c] = self.array()?;
        Some(u32::from_be_bytes([0, a, b, c]))
    }

    pub fn u32be(&mut self) -> Option<u32> {
        self.array().map(u32::from_be_bytes)
    }

    pub fn i32be(&mut self) -> Option<i32> {
        self.array().map(i32::from_be_bytes)
    }

    pub fn u64be(&mut self) -> Option<u64> {
        self.array().map(u64::from_be_bytes)
    }

    pub fn f64be(&mut self) -> Option<f64> {
        self.array().map(f64::from_be_bytes)
    }

    pub fn u16le(&mut self) -> Option<u16> {
        self.array().map(u16::from_le_bytes)
    }

    pub fn i32le(&mut self) -> Option<i32> {
        self.array().map(i32::from_le_bytes)
    }

    pub fn u32le(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }

    pub fn fourcc(&mut self) -> Option<[u8; 4]> {
        self.array()
    }
}

/// MSB-first bit reader for codec bitstreams (H.264 SPS, AAC AudioSpecificConfig).
pub struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0 }
    }

    pub fn read(&mut self, n: u32) -> Option<u32> {
        debug_assert!(n <= 32);
        let mut out = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            out = (out << 1) | bit as u32;
            self.pos += 1;
        }
        Some(out)
    }

    pub fn flag(&mut self) -> Option<bool> {
        Some(self.read(1)? == 1)
    }

    /// Unsigned Exp-Golomb code.
    pub fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.read(zeros)?)
    }
}

/// Strip H.264/HEVC emulation-prevention bytes (`00 00 03` → `00 00`).
pub fn unescape_rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Read `len` bytes at `pos`, refusing anything over `cap` so a corrupt size can't allocate gigabytes.
pub fn read_at<R: Read + Seek>(r: &mut R, pos: u64, len: u64, cap: u64) -> Option<Vec<u8>> {
    if len > cap {
        return None;
    }
    r.seek(SeekFrom::Start(pos)).ok()?;
    let mut buf = vec![0; len as usize];
    r.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// A fourcc as printable text, for `codec_tag`.
pub fn fourcc_str(tag: &[u8]) -> String {
    tag.iter()
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Text field that may be NUL-terminated or padded.
pub fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_stop_at_end() {
        let mut b = Bytes::new(&[1, 2, 3]);
        assert_eq!(b.u16be(), Some(0x0102));
        assert_eq!(b.u16be(), None);
        assert_eq!(b.u8(), Some(3));
        assert_eq!(b.take(usize::MAX), None);
    }

    #[test]
    fn exp_golomb() {
        // 1 | 010 | 011 | 00100 → 0, 1, 2, 3
        let mut b = Bits::new(&[0b1010_0110, 0b0100_0000]);
        assert_eq!(b.ue(), Some(0));
        assert_eq!(b.ue(), Some(1));
        assert_eq!(b.ue(), Some(2));
        assert_eq!(b.ue(), Some(3));
        assert_eq!(Bits::new(&[0, 0, 0, 0, 0]).ue(), None);
    }

    #[test]
    fn rbsp_unescape() {
        assert_eq!(unescape_rbsp(&[0, 0, 3, 1, 0, 0, 3]), vec![0, 0, 1, 0, 0]);
    }
}
