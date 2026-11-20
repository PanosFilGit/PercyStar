use flate2::{Decompress, FlushDecompress, Status};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::tex::{self, TtpfIndex};

pub type Result<T> = std::result::Result<T, String>;
pub const DECODE_LIMIT: u64 = 16 * 1024 * 1024;
pub const PREVIEW_BYTES: usize = 512;
const RPF0_TOC: u64 = 0x800;

pub fn word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

pub struct Source {
    file: Mutex<File>,
    pub size: u64,
}

impl Source {
    pub fn open(path: &Path) -> Result<Arc<Self>> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let file = options.open(path).map_err(|e| e.to_string())?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        Ok(Arc::new(Self {
            file: Mutex::new(file),
            size,
        }))
    }

    pub fn copy(&self, offset: u64, len: u64, to: &mut impl Write) -> Result<()> {
        if offset > self.size || len > self.size - offset {
            return Err(format!(
                "Range 0x{offset:X} + {len} exceeds archive size {}",
                self.size
            ));
        }
        let mut file = self.file.lock().map_err(|_| "Archive reader stopped")?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let count = io::copy(&mut (&mut *file).take(len), to).map_err(|e| e.to_string())?;
        if count != len {
            return Err("Archive ended during read".into());
        }
        Ok(())
    }

    pub fn read(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len > DECODE_LIMIT {
            return Err("Individual in-memory reads are limited to 16 MiB".into());
        }
        let mut result = Vec::new();
        self.copy(offset, len, &mut result)?;
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Stored,
    Deflate,
    Zlib,
    Unsupported(u8),
}

impl Codec {
    pub fn label(self) -> String {
        match self {
            Self::Stored => "Stored".into(),
            Self::Deflate => "DEFLATE".into(),
            Self::Zlib => "zlib".into(),
            Self::Unsupported(tag) => format!("Unsupported codec {tag}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Record {
    pub offset: u64,
    pub stored: u64,
    pub size: u64,
    pub codec: Codec,
}

impl Record {
    pub fn decode(self, src: &Source) -> Result<Vec<u8>> {
        if matches!(self.codec, Codec::Unsupported(_)) {
            return Err(self.codec.label());
        }
        if self.size > DECODE_LIMIT || self.stored > DECODE_LIMIT {
            return Err("Compressed and in-memory records are limited to 16 MiB".into());
        }
        let input = src.read(self.offset, self.stored)?;
        if self.codec == Codec::Stored {
            if self.stored != self.size {
                return Err("Stored and logical lengths disagree".into());
            }
            return Ok(input);
        }
        let mut decoder = Decompress::new(self.codec == Codec::Zlib);
        let mut output = vec![0; self.size as usize + 1];
        let status = decoder
            .decompress(&input, &mut output, FlushDecompress::Finish)
            .map_err(|e| format!("Invalid {} stream: {e}", self.codec.label()))?;
        if status != Status::StreamEnd
            || decoder.total_in() != self.stored
            || decoder.total_out() != self.size
        {
            return Err("Compressed stream end or length disagrees with its index record".into());
        }
        output.truncate(self.size as usize);
        Ok(output)
    }

    pub fn preview(self, src: &Source) -> Result<Vec<u8>> {
        if self.codec == Codec::Stored {
            return src.read(self.offset, self.size.min(PREVIEW_BYTES as u64));
        }
        let mut decoded = self.decode(src)?;
        decoded.truncate(PREVIEW_BYTES);
        Ok(decoded)
    }

    pub fn export(self, src: &Source, path: &Path) -> Result<()> {
        write_new(path, |out| {
            if self.codec == Codec::Stored && self.size == self.stored {
                src.copy(self.offset, self.stored, out)
            } else {
                out.write_all(&self.decode(src)?).map_err(|e| e.to_string())
            }
        })
    }
}

pub fn write_new(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    if !path
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(safe_component)
    {
        return Err("Invalid output filename".into());
    }
    if path.exists() {
        return Err(format!("File already exists: {}", path.display()));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    write(tmp.as_file_mut())?;
    tmp.as_file_mut().flush().map_err(|e| e.to_string())?;
    tmp.persist_noclobber(path)
        .map_err(|e| e.error.to_string())?;
    Ok(())
}

pub fn write_bytes_new(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    write_new(path, |out| {
        out.write_all(bytes.as_ref()).map_err(|e| e.to_string())
    })
}

pub enum Member {
    Folder(std::ops::Range<usize>),
    File(Record),
}

pub struct Node {
    pub name: String,
    pub parent: usize,
    pub member: Member,
}

pub enum Catalog {
    Rds(Arc<TtpfIndex>),
    Rpf0(Vec<Node>),
    Rpf9,
    Invalid(&'static str, String),
    Unknown,
}

pub struct Archive {
    pub path: PathBuf,
    pub src: Arc<Source>,
    pub catalog: Catalog,
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self> {
        let archive = Self::inspect(path)?;
        if let Catalog::Invalid(format, e) = &archive.catalog {
            return Err(format!("{format}: {e}"));
        }
        Ok(archive)
    }

    pub fn inspect(path: &Path) -> Result<Self> {
        let src = Source::open(path)?;
        let header = src.read(0, src.size.min(64))?;
        let catalog = match header.get(..4) {
            Some(b"RDS5") => TtpfIndex::read(src.clone())
                .map(|ix| Catalog::Rds(Arc::new(ix)))
                .unwrap_or_else(|e| Catalog::Invalid("RDS5", e)),
            Some(b"RPF0") => parse_rpf0(&src, &header)
                .map(Catalog::Rpf0)
                .unwrap_or_else(|e| Catalog::Invalid("RPF0", e)),
            Some(b"9FPR") => Catalog::Rpf9,
            _ => Catalog::Unknown,
        };
        Ok(Self {
            path: path.into(),
            src,
            catalog,
        })
    }

    pub fn format(&self) -> &'static str {
        match self.catalog {
            Catalog::Rds(_) => "RDS5",
            Catalog::Rpf0(_) => "RPF0",
            Catalog::Rpf9 => "RPF9",
            Catalog::Invalid(format, _) => format,
            Catalog::Unknown => "Unknown signature",
        }
    }

    pub fn record(&self, index: usize) -> Result<Record> {
        match &self.catalog {
            Catalog::Rds(ix) => ix.chunks.get(index).map(|ch| ch.record),
            Catalog::Rpf0(nodes) => nodes.get(index).and_then(|n| match n.member {
                Member::File(r) => Some(r),
                _ => None,
            }),
            _ => None,
        }
        .ok_or_else(|| format!("No file or data record at index {index}"))
    }

    pub fn texture(&self, index: usize) -> Result<tex::DecodedTexture> {
        match &self.catalog {
            Catalog::Rds(ix) => tex::decode_indexed_texture(ix, index),
            _ => Err("This archive has no texture descriptors".into()),
        }
    }
}

pub(crate) fn safe_component(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap()
        .trim_end()
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !(stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|s| {
            s.len() == 1 && s.as_bytes()[0].is_ascii_digit() || matches!(s, "¹" | "²" | "³")
        }))
}

fn parse_rpf0(src: &Source, hdr: &[u8]) -> Result<Vec<Node>> {
    if hdr.len() < 12 {
        return Err("Truncated RPF0 header".into());
    }
    let len = word(hdr, 4) as u64;
    let count = word(hdr, 8) as usize;
    if !(1..=100_000).contains(&count) || len <= (count * 16) as u64 || len > DECODE_LIMIT {
        return Err("Invalid RPF0 table length or count".into());
    }
    let table = src.read(RPF0_TOC, len)?;
    let mut nodes = Vec::with_capacity(count);
    let mut name_budget = DECODE_LIMIT as usize;
    for i in 0..count {
        let at = i * 16;
        let tag = word(&table, at);
        let name = if i == 0 {
            ""
        } else {
            let start = count * 16 + (tag & 0x7fff_ffff) as usize;
            let area = table.get(start..).ok_or("RPF0 name outside table")?;
            let end = area
                .iter()
                .take(1024)
                .position(|b| *b == 0)
                .ok_or("Unterminated RPF0 name")?;
            let name = std::str::from_utf8(&area[..end]).map_err(|_| "Invalid UTF-8 RPF0 name")?;
            if !safe_component(name) {
                return Err("Invalid RPF0 member name".into());
            }
            name
        };
        name_budget = name_budget
            .checked_sub(name.len())
            .ok_or("RPF0 name budget exceeded")?;
        let a = word(&table, at + 4) as u64;
        let b = word(&table, at + 8) as u64;
        let member = if tag >> 31 == 1 {
            if a + b > count as u64 {
                return Err("RPF0 children outside table".into());
            }
            Member::Folder(a as usize..(a + b) as usize)
        } else {
            let size = word(&table, at + 12) as u64;
            if a > src.size || b > src.size - a || (b != 0 && a < RPF0_TOC + len) {
                return Err("RPF0 payload outside data area".into());
            }
            Member::File(Record {
                offset: a,
                stored: b,
                size,
                codec: if size == b {
                    Codec::Stored
                } else {
                    Codec::Deflate
                },
            })
        };
        nodes.push(Node {
            name: name.into(),
            parent: usize::MAX,
            member,
        });
    }
    if !matches!(nodes[0].member, Member::Folder(_)) {
        return Err("Missing RPF0 root directory".into());
    }
    nodes[0].parent = 0;
    let mut pending = vec![0];
    while let Some(dir) = pending.pop() {
        if let Member::Folder(children) = &nodes[dir].member {
            let children = children.clone();
            for i in children {
                if nodes[i].parent != usize::MAX {
                    return Err("Repeated RPF0 child or directory cycle".into());
                }
                nodes[i].parent = dir;
                if matches!(nodes[i].member, Member::Folder(_)) {
                    pending.push(i);
                }
            }
        }
    }
    if nodes.iter().any(|n| n.parent == usize::MAX) {
        return Err("Unreachable RPF0 member".into());
    }
    Ok(nodes)
}
