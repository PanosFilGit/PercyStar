use crate::archive::{word, Codec, Record, Result, Source};
use crate::tex::TtpfTexMeta;
use std::sync::Arc;

pub struct Chunk {
    pub start: u64,
    pub record: Record,
}

pub struct TtpfIndex {
    pub chunks: Vec<Chunk>,
    pub descs: Vec<TtpfTexMeta>,
    pub descriptor_error: Option<String>,
    pub source: Arc<Source>,
}

#[derive(Clone, Copy)]
struct Boundary {
    disk: u64,
    logical: u64,
    padding: u64,
    tag: u8,
}

impl Boundary {
    fn read(row: &[u8]) -> Self {
        let a = word(row, 0);
        let b = word(row, 4);
        Self {
            disk: u64::from(a & 0x03ff_ffff) << 8,
            logical: ((u64::from(b & 0xfffff) << 2) + u64::from(a >> 30)) << 12,
            padding: u64::from(b >> 20),
            tag: ((a >> 26) & 15) as u8,
        }
    }
}

impl TtpfIndex {
    pub fn read(source: Arc<Source>) -> Result<Self> {
        let hdr = source.read(0, 16)?;
        if &hdr[..4] != b"RDS5" {
            return Err("Missing RDS5 signature".into());
        }
        let count = word(&hdr, 4) as usize;
        if !(2..=262_144).contains(&count) {
            return Err(format!("Unsupported RDS5 boundary count {count}"));
        }
        let table = source.read(16, (count * 8) as u64)?;
        let mut rows = table.chunks_exact(8).map(Boundary::read);
        let mut current = rows.next().unwrap();
        if current.disk < 16 + table.len() as u64 || current.logical != 0 {
            return Err("Invalid first RDS5 boundary".into());
        }
        let mut chunks = Vec::with_capacity(count - 1);
        for next in rows {
            let i = chunks.len();
            if next.disk <= current.disk + current.padding
                || next.disk > source.size
                || next.logical <= current.logical
            {
                return Err(format!("Invalid RDS5 extents at record {i}"));
            }
            let size = next.logical - current.logical;
            let stored = next.disk - current.disk - current.padding;
            let codec = match current.tag {
                0 if stored == size => Codec::Stored,
                8 => Codec::Zlib,
                tag => Codec::Unsupported(tag),
            };
            chunks.push(Chunk {
                start: current.logical,
                record: Record {
                    offset: current.disk,
                    stored,
                    size,
                    codec,
                },
            });
            current = next;
        }
        if current.disk != source.size {
            return Err("RDS5 terminal boundary differs from file size".into());
        }
        let mut ix = Self {
            chunks,
            descs: vec![],
            descriptor_error: None,
            source,
        };
        match ix.descriptors(current.logical) {
            Ok(descs) => ix.descs = descs,
            Err(e) => ix.descriptor_error = Some(e),
        }
        Ok(ix)
    }

    fn descriptors(&self, virtual_end: u64) -> Result<Vec<TtpfTexMeta>> {
        let first = self.chunks[0].record;
        if first.codec != Codec::Stored || first.stored < 16 {
            return Ok(vec![]);
        }
        let hdr = self.source.read(first.offset, 16)?;
        if &hdr[..4] != b"ETPF" {
            return Ok(vec![]);
        }
        let count = word(&hdr, 8) as usize;
        if count > 100_000 || 16 + (count * 16) as u64 > first.stored {
            return Err("Invalid ETPF descriptor count".into());
        }
        let table = self.source.read(first.offset + 16, (count * 16) as u64)?;
        let mut descs: Vec<TtpfTexMeta> = Vec::with_capacity(count);
        for (idx, row) in table.chunks_exact(16).enumerate() {
            let dims = word(row, 0);
            let format = word(row, 4);
            let byte_off = u64::from(word(row, 8) & 0x001f_ffff) << 12;
            if byte_off < first.size || byte_off >= virtual_end {
                return Err(format!(
                    "ETPF address outside texture data at descriptor {idx}"
                ));
            }
            descs.push(TtpfTexMeta {
                idx,
                w: ((dims & 0x3fff) + 1) as usize,
                h: (((dims >> 14) & 0x3fff) + 1) as usize,
                fmt_byte: format as u8,
                dxgi: ((format >> 3) & 0x7f) as u8,
                has_mips: format >> 24 != 0,
                byte_off,
                alloc_sz: virtual_end - byte_off,
            });
        }
        // Aliased and out-of-order descriptors share the same allocation boundaries.
        let mut boundaries: Vec<_> = descs.iter().map(|d| d.byte_off).collect();
        boundaries.push(virtual_end);
        boundaries.sort_unstable();
        for desc in &mut descs {
            desc.alloc_sz =
                boundaries[boundaries.partition_point(|&at| at <= desc.byte_off)] - desc.byte_off;
        }
        Ok(descs)
    }

    pub fn read_virtual(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        if len > 64 << 20 {
            return Err("Texture input exceeds the 64 MiB limit".into());
        }
        let end = off
            .checked_add(len as u64)
            .ok_or("Virtual range overflow")?;
        let last = self.chunks.last().ok_or("Empty RDS5 index")?;
        if end > last.start + last.record.size {
            return Err("Virtual range exceeds the archive".into());
        }
        let first = self
            .chunks
            .partition_point(|chunk| chunk.start + chunk.record.size <= off);
        let mut out = Vec::with_capacity(len);
        for (i, chunk) in self.chunks[first..].iter().enumerate() {
            if chunk.start >= end {
                break;
            }
            let from = off.saturating_sub(chunk.start);
            let to = (end - chunk.start).min(chunk.record.size);
            let r = chunk.record;
            if r.codec == Codec::Stored {
                self.source.copy(r.offset + from, to - from, &mut out)?;
            } else {
                let data = r
                    .decode(&self.source)
                    .map_err(|e| format!("Record {}: {e}", first + i))?;
                out.extend_from_slice(&data[from as usize..to as usize]);
            }
        }
        if out.len() != len {
            return Err("Incomplete RDS5 virtual range".into());
        }
        Ok(out)
    }

    pub fn info(&self) -> String {
        format!(
            "RDS5 · {} bytes · {} records · {} textures",
            self.source.size,
            self.chunks.len(),
            self.descs.len()
        )
    }
}
