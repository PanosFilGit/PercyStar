//! Native Xbox Series X (Scarlett / RDNA2) texture, TTPF (RDS5/ETPF), RBF0, Scaleform GFX,
//! and CNavMesh decoders for GTA VI (RAGE 9). Zero external runtime dependencies.

use crate::model3d::{is_zlib_head, Mesh3D, Texture2D};
use eframe::egui::Color32;
use flate2::{Decompress, FlushDecompress, Status};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[inline]
fn ru16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
#[inline]
fn ru32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
fn ru64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}
#[inline]
fn rf32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
fn vptr(q: u64) -> Option<usize> {
    (0x5000_0000..0x5800_0000).contains(&q).then(|| (q - 0x5000_0000) as usize)
}
#[inline]
fn pptr(q: u64) -> Option<usize> {
    (0x6000_0000..0x7000_0000).contains(&q).then(|| (q - 0x6000_0000) as usize)
}

#[derive(Clone)]
pub struct DecodedTexture {
    pub name: String,
    pub w: usize,
    pub h: usize,
    pub mips: u8,
    pub dxgi: u8,
    pub fmt_name: &'static str,
    pub pixels: Vec<Color32>,
    pub raw_dds_payload: Vec<u8>,
}

pub fn dxgi_info(dxgi: u8) -> Option<(&'static str, bool, usize)> {
    Some(match dxgi {
        28 => ("R8G8B8A8_UNORM", false, 4),
        29 => ("R8G8B8A8_UNORM_SRGB", false, 4),
        49 => ("R8G8_UNORM", false, 2),
        61 => ("R8_UNORM", false, 1),
        65 => ("A8_UNORM", false, 1),
        71 => ("BC1_UNORM", true, 8),
        72 => ("BC1_UNORM_SRGB", true, 8),
        74 => ("BC2_UNORM", true, 16),
        75 => ("BC2_UNORM_SRGB", true, 16),
        77 => ("BC3_UNORM", true, 16),
        78 => ("BC3_UNORM_SRGB", true, 16),
        80 => ("BC4_UNORM", true, 8),
        81 => ("BC4_SNORM", true, 8),
        83 => ("BC5_UNORM", true, 16),
        84 => ("BC5_SNORM", true, 16),
        87 => ("B8G8R8A8_UNORM", false, 4),
        95 => ("BC6H_UF16", true, 16),
        96 => ("BC6H_SF16", true, 16),
        98 => ("BC7_UNORM", true, 16),
        99 => ("BC7_UNORM_SRGB", true, 16),
        _ => return None,
    })
}

pub fn padded_4kb_size(w: usize, h: usize, is_bc: bool, bpe: usize) -> usize {
    let ew = if is_bc { (w + 3) / 4 } else { w }.max(1);
    let eh = if is_bc { (h + 3) / 4 } else { h }.max(1);
    let (tw, th) = match bpe {
        16 => (16usize, 16usize),
        8 => (32, 16),
        4 => (32, 32),
        2 => (64, 32),
        _ => (64, 64),
    };
    let pw = ((ew + tw - 1) / tw) * tw;
    let ph = ((eh + th - 1) / th) * th;
    pw * ph * bpe
}

/// Verified AMD RDNA2 / Xbox Scarlett SW_4KB_D deswizzle (128bpp: y0,y1,x0,x1,y2,x2,y3,x3).
pub fn deswizzle_4kb_d(raw: &[u8], w: usize, h: usize, is_bc: bool, bpe: usize) -> Vec<u8> {
    let ew = if is_bc { (w + 3) / 4 } else { w }.max(1);
    let eh = if is_bc { (h + 3) / 4 } else { h }.max(1);
    let (tw, th) = match bpe {
        16 => (16usize, 16usize),
        8 => (32, 16),
        4 => (32, 32),
        2 => (64, 32),
        _ => (64, 64),
    };
    let pw = ((ew + tw - 1) / tw) * tw;
    let ph = ((eh + th - 1) / th) * th;
    let needed = pw * ph * bpe;
    let lin_len = ew * eh * bpe;
    if raw.len() < needed {
        let mut out = vec![0u8; lin_len];
        let n = raw.len().min(lin_len);
        out[..n].copy_from_slice(&raw[..n]);
        return out;
    }

    let mut out = vec![0u8; lin_len];
    let tiles_x = pw / tw;
    let elems_per_tile = tw * th;

    for y in 0..eh {
        let ty = y / th;
        let ly = y % th;
        for x in 0..ew {
            let tx = x / tw;
            let lx = x % tw;
            let t_idx = ty * tiles_x + tx;
            let m = match bpe {
                16 => (ly & 3) | ((lx & 3) << 2) | (((ly >> 2) & 1) << 4) | (((lx >> 2) & 1) << 5) | (((ly >> 3) & 1) << 6) | (((lx >> 3) & 1) << 7),
                8 => (lx & 1) | ((ly & 3) << 1) | (((lx >> 1) & 3) << 3) | (((ly >> 2) & 1) << 5) | (((lx >> 3) & 1) << 6) | (((ly >> 3) & 1) << 7) | (((lx >> 4) & 1) << 8),
                4 => (lx & 3) | ((ly & 3) << 2) | (((lx >> 2) & 1) << 4) | (((ly >> 2) & 1) << 5) | (((lx >> 3) & 1) << 6) | (((ly >> 3) & 1) << 7) | (((lx >> 4) & 1) << 8) | (((ly >> 4) & 1) << 9),
                2 => (lx & 7) | ((ly & 3) << 3) | (((lx >> 3) & 1) << 5) | (((ly >> 2) & 1) << 6) | (((lx >> 4) & 1) << 7) | (((ly >> 3) & 1) << 8) | (((lx >> 5) & 1) << 9) | (((ly >> 4) & 1) << 10),
                _ => (lx & 7) | ((ly & 7) << 3) | (((lx >> 3) & 1) << 6) | (((ly >> 3) & 1) << 7) | (((lx >> 4) & 1) << 8) | (((ly >> 4) & 1) << 9) | (((lx >> 5) & 1) << 10) | (((ly >> 5) & 1) << 11),
            };
            let src_off = (t_idx * elems_per_tile + m) * bpe;
            let dst_off = (y * ew + x) * bpe;
            out[dst_off..dst_off + bpe].copy_from_slice(&raw[src_off..src_off + bpe]);
        }
    }
    out
}

/// Verified Xbox Scarlett 64KB TTPF tile deswizzle (BC7 128bpp & BC1 64bpp).
pub fn deswizzle_64kb_ttpf(raw: &[u8], w: usize, h: usize, bpe: usize) -> Vec<u8> {
    let bw = ((w + 3) / 4).max(1);
    let bh = ((h + 3) / 4).max(1);
    let (tw, th, shift) = if bpe == 16 { (64usize, 64usize, 12usize) } else { (128, 64, 13) };
    let pw = ((bw + tw - 1) / tw) * tw;
    let ph = ((bh + th - 1) / th) * th;
    let needed = pw * ph * bpe;
    let mut padded = Vec::new();
    let src: &[u8] = if raw.len() >= needed {
        &raw[..needed]
    } else {
        padded.resize(needed, 0);
        let n = raw.len().min(needed);
        padded[..n].copy_from_slice(&raw[..n]);
        &padded
    };
    let mut out = vec![0u8; bw * bh * bpe];
    let tiles_x = pw / tw;
    for y in 0..bh {
        let ty = y / th;
        let ly = y % th;
        for x in 0..bw {
            let tx = x / tw;
            let lx = x % tw;
            let t_idx = ty * tiles_x + tx;
            let m = if bpe == 16 {
                (lx & 1)
                    | ((ly & 1) << 1)
                    | (((lx >> 1) & 1) << 2)
                    | (((ly >> 1) & 1) << 3)
                    | (((ly >> 2) & 1) << 4)
                    | (((lx >> 2) & 1) << 5)
                    | (((ly >> 3) & 1) << 6)
                    | (((lx >> 3) & 1) << 7)
                    | (((ly >> 4) & 1) << 8)
                    | (((lx >> 4) & 1) << 9)
                    | (((ly >> 5) & 1) << 10)
                    | (((lx >> 5) & 1) << 11)
            } else {
                (lx & 1)
                    | ((ly & 1) << 1)
                    | (((lx >> 1) & 3) << 2)
                    | (((ly >> 1) & 1) << 4)
                    | (((ly >> 2) & 1) << 5)
                    | (((lx >> 3) & 1) << 6)
                    | (((ly >> 3) & 1) << 7)
                    | (((lx >> 4) & 1) << 8)
                    | (((ly >> 4) & 1) << 9)
                    | (((lx >> 5) & 1) << 10)
                    | (((ly >> 5) & 1) << 11)
                    | (((lx >> 6) & 1) << 12)
            };
            let src_off = ((t_idx << shift) | m) * bpe;
            let dst_off = (y * bw + x) * bpe;
            out[dst_off..dst_off + bpe].copy_from_slice(&src[src_off..src_off + bpe]);
        }
    }
    out
}

pub fn decode_linear_to_rgba(lin: &[u8], w: usize, h: usize, dxgi: u8) -> Option<Vec<Color32>> {
    if matches!(dxgi, 71 | 72 | 77 | 78 | 80 | 81 | 83 | 84 | 95 | 96 | 98 | 99) {
        let mut out_u32 = vec![0u32; w * h];
        let res = match dxgi {
            71 | 72 => texture2ddecoder::decode_bc1(lin, w, h, &mut out_u32),
            77 | 78 => texture2ddecoder::decode_bc3(lin, w, h, &mut out_u32),
            80 | 81 => texture2ddecoder::decode_bc4(lin, w, h, &mut out_u32),
            83 | 84 => texture2ddecoder::decode_bc5(lin, w, h, &mut out_u32),
            95 => texture2ddecoder::decode_bc6_unsigned(lin, w, h, &mut out_u32),
            96 => texture2ddecoder::decode_bc6_signed(lin, w, h, &mut out_u32),
            98 | 99 => texture2ddecoder::decode_bc7(lin, w, h, &mut out_u32),
            _ => unreachable!(),
        };
        res.ok()?;
        let mut pixels = Vec::with_capacity(w * h);
        for px in out_u32 {
            // texture2ddecoder packs u32 as 0xAARRGGBB (BGRA little-endian)
            let b = (px & 0xFF) as u8;
            let g = ((px >> 8) & 0xFF) as u8;
            let r = ((px >> 16) & 0xFF) as u8;
            let a = ((px >> 24) & 0xFF) as u8;
            pixels.push(Color32::from_rgba_unmultiplied(r, g, b, a));
        }
        return Some(pixels);
    }
    if matches!(dxgi, 74 | 75) {
        // BC2 = 8-byte 4-bit explicit alpha + 8-byte BC1 color per 4x4 block
        let bw = (w + 3) / 4;
        let bh = (h + 3) / 4;
        if lin.len() < bw * bh * 16 { return None; }
        let mut bc1_only = Vec::with_capacity(bw * bh * 8);
        for b in 0..bw * bh {
            bc1_only.extend_from_slice(&lin[b * 16 + 8..b * 16 + 16]);
        }
        let mut out_u32 = vec![0u32; w * h];
        texture2ddecoder::decode_bc1(&bc1_only, w, h, &mut out_u32).ok()?;
        let mut pixels = vec![Color32::TRANSPARENT; w * h];
        for by in 0..bh {
            for bx in 0..bw {
                let abits = ru64(lin, (by * bw + bx) * 16);
                for py in 0..4 {
                    let y = by * 4 + py;
                    if y >= h { continue; }
                    for px in 0..4 {
                        let x = bx * 4 + px;
                        if x >= w { continue; }
                        let c = out_u32[y * w + x];
                        let a4 = ((abits >> (4 * (py * 4 + px))) & 0xF) as u8;
                        pixels[y * w + x] = Color32::from_rgba_unmultiplied(
                            ((c >> 16) & 0xFF) as u8,
                            ((c >> 8) & 0xFF) as u8,
                            (c & 0xFF) as u8,
                            a4 | (a4 << 4),
                        );
                    }
                }
            }
        }
        return Some(pixels);
    }
    let mut pixels = Vec::with_capacity(w * h);
    match dxgi {
        28 | 29 => {
            if lin.len() < w * h * 4 { return None; }
            for px in lin[..w * h * 4].chunks_exact(4) {
                pixels.push(Color32::from_rgba_unmultiplied(px[0], px[1], px[2], px[3]));
            }
        }
        87 => {
            if lin.len() < w * h * 4 { return None; }
            for px in lin[..w * h * 4].chunks_exact(4) {
                pixels.push(Color32::from_rgba_unmultiplied(px[2], px[1], px[0], px[3]));
            }
        }
        49 => {
            if lin.len() < w * h * 2 { return None; }
            for px in lin[..w * h * 2].chunks_exact(2) {
                pixels.push(Color32::from_rgb(px[0], px[1], 0));
            }
        }
        61 | 65 => {
            if lin.len() < w * h { return None; }
            for &g in &lin[..w * h] {
                pixels.push(Color32::from_rgb(g, g, g));
            }
        }
        _ => return None,
    }
    Some(pixels)
}

pub fn build_dds_dx10(w: usize, h: usize, dxgi: u8, lin: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(148 + lin.len());
    out.extend_from_slice(b"DDS ");
    for v in [124u32, 0x1007, h as u32, w as u32, lin.len() as u32, 1, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[0u8; 44]);
    // pixelformat
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&0x4u32.to_le_bytes());
    out.extend_from_slice(b"DX10");
    out.extend_from_slice(&[0u8; 20]);
    // caps
    out.extend_from_slice(&0x1000u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    // dx10 header
    for v in [dxgi as u32, 3, 0, 1, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(lin);
    out
}

pub fn export_texture_png(tex: &DecodedTexture, path: &Path) -> Result<(), String> {
    let mut rgba = Vec::with_capacity(tex.w * tex.h * 4);
    for c in &tex.pixels {
        rgba.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    image::save_buffer(path, &rgba, tex.w as u32, tex.h as u32, image::ColorType::Rgba8)
        .map_err(|e| format!("PNG save error: {e}"))
}

/// Extract ALL textures from a pgDictionary<grcTexture> inside virtual `v` + physical `p`.
pub fn extract_txd_textures(v: &[u8], p: &[u8], dict_off: usize) -> Vec<DecodedTexture> {
    if dict_off + 0x3a > v.len() { return Vec::new(); }
    let Some(tarr) = vptr(ru64(v, dict_off + 0x30)) else { return Vec::new() };
    let tcnt = ru16(v, dict_off + 0x38) as usize;
    if tcnt == 0 || tcnt > 2048 || tarr + tcnt * 8 > v.len() { return Vec::new(); }

    let mut raw_list = Vec::new();
    for ti in 0..tcnt {
        let Some(tp) = vptr(ru64(v, tarr + ti * 8)) else { continue };
        if tp + 0x48 > v.len() { continue; }
        let Some(poff) = pptr(ru64(v, tp + 0x18)) else { continue };
        if poff >= p.len() { continue; }
        let w = ru16(v, tp + 0x38) as usize;
        let h = ru16(v, tp + 0x3a) as usize;
        let dxgi = v[tp + 0x3f];
        let swiz = v[tp + 0x40];
        let mips = v[tp + 0x42];
        if !(4..=8192).contains(&w) || !(4..=8192).contains(&h) || dxgi_info(dxgi).is_none() {
            continue;
        }
        let mut name = format!("tex_{ti:03}");
        if let Some(mut no) = vptr(ru64(v, tp + 0x30)) {
            if no + 8 < v.len() && v[no..no + 8] == [0u8; 8] {
                no += 8;
            }
            if let Some(rel) = v[no..].iter().position(|&b| b == 0) {
                if rel > 0 && rel < 128 {
                    let s = String::from_utf8_lossy(&v[no..no + rel]).into_owned();
                    if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                        name = s;
                    }
                }
            }
        }
        raw_list.push((poff, name, w, h, dxgi, swiz, mips));
    }

    let mut sorted_poffs: Vec<usize> = raw_list.iter().map(|t| t.0).collect();
    sorted_poffs.sort_unstable();
    sorted_poffs.dedup();
    let mut next_map: HashMap<usize, usize> = HashMap::new();
    for w2 in sorted_poffs.windows(2) {
        next_map.insert(w2[0], w2[1]);
    }

    let mut out = Vec::new();
    for (poff, name, w, h, dxgi, swiz, mips) in raw_list {
        let Some((fmt_name, is_bc, bpe)) = dxgi_info(dxgi) else { continue };
        let alloc_end = next_map.get(&poff).copied().unwrap_or(p.len());
        let alloc_sz = alloc_end.saturating_sub(poff);
        let mip0_sz = padded_4kb_size(w, h, is_bc, bpe);
        let mip0_off = if mips > 1 && alloc_sz >= mip0_sz {
            poff + (alloc_sz - mip0_sz)
        } else {
            poff
        };
        if mip0_off >= p.len() { continue; }
        let slice_end = (mip0_off + mip0_sz).min(p.len());
        let lin = if swiz == 0 {
            let ew = if is_bc { (w + 3) / 4 } else { w }.max(1);
            let eh = if is_bc { (h + 3) / 4 } else { h }.max(1);
            let need = ew * eh * bpe;
            let mut b = vec![0u8; need];
            let n = (p.len() - mip0_off).min(need);
            b[..n].copy_from_slice(&p[mip0_off..mip0_off + n]);
            b
        } else {
            deswizzle_4kb_d(&p[mip0_off..slice_end], w, h, is_bc, bpe)
        };
        if let Some(pixels) = decode_linear_to_rgba(&lin, w, h, dxgi) {
            out.push(DecodedTexture {
                name,
                w,
                h,
                mips,
                dxgi,
                fmt_name,
                pixels,
                raw_dds_payload: lin,
            });
        }
    }
    out
}

/// Convert DecodedTexture into model3d::Texture2D.
pub fn to_model_tex(t: &DecodedTexture) -> Texture2D {
    Texture2D { w: t.w, h: t.h, pixels: t.pixels.clone() }
}

/// Read RAGE9 resource virtual + physical buffers from RPF at `file_off` and decode all TXD textures.
pub fn decode_txd_from_rpf(rpf_path: &Path, file_off: u64) -> Result<Vec<DecodedTexture>, String> {
    let mut f = File::open(rpf_path).map_err(|e| format!("Open error: {e}"))?;
    let (v, p) = read_rage9_vp(&mut f, file_off)?;
    let texs = extract_txd_textures(&v, &p, 0);
    if texs.is_empty() {
        return Err("No decodable textures found in dictionary".into());
    }
    Ok(texs)
}

fn decompress_stream_at(f: &mut File, file_off: u64, max_out: usize) -> Result<(Vec<u8>, u64), String> {
    f.seek(SeekFrom::Start(file_off)).map_err(|e| e.to_string())?;
    let mut dec = Decompress::new(true);
    let mut in_buf = [0u8; 65536];
    let mut out_chunk = [0u8; 65536];
    let mut out = Vec::with_capacity(65536);
    let mut consumed = 0u64;
    loop {
        let n = f.read(&mut in_buf).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        let mut pos = 0usize;
        while pos < n {
            let bi = dec.total_in();
            let bo = dec.total_out();
            let st = dec.decompress(&in_buf[pos..n], &mut out_chunk, FlushDecompress::None).map_err(|e| e.to_string())?;
            let cin = (dec.total_in() - bi) as usize;
            let cout = (dec.total_out() - bo) as usize;
            pos += cin;
            consumed += cin as u64;
            if cout > 0 {
                out.extend_from_slice(&out_chunk[..cout]);
                if out.len() > max_out {
                    return Ok((out, file_off + consumed));
                }
            }
            if st == Status::StreamEnd {
                return Ok((out, file_off + consumed));
            }
            if cin == 0 && cout == 0 { break; }
        }
    }
    Ok((out, file_off + consumed))
}

fn find_next_zlib_near(f: &mut File, start_off: u64) -> Result<Option<u64>, String> {
    f.seek(SeekFrom::Start(start_off)).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 4096];
    let n = f.read(&mut buf).map_err(|e| e.to_string())?;
    for i in 0..n.saturating_sub(3) {
        if is_zlib_head(&buf[i..n]) {
            return Ok(Some(start_off + i as u64));
        }
    }
    Ok(None)
}

pub fn read_rage9_vp(f: &mut File, file_off: u64) -> Result<(Vec<u8>, Vec<u8>), String> {
    let (mut v, mut next_off) = decompress_stream_at(f, file_off, 16 * 1024 * 1024)?;
    if v.len() < 0x40 { return Err("Virtual chunk too small".into()); }
    let Some(bmap_off) = vptr(ru64(&v, 8)) else { return Err("Missing BlockMap".into()) };
    let mut v_read = 1usize;
    while v.len() <= bmap_off + 16 && v_read < 32 {
        let Some(zoff) = find_next_zlib_near(f, next_off)? else { break };
        let (c, nxt) = decompress_stream_at(f, zoff, 16 * 1024 * 1024)?;
        v.extend_from_slice(&c);
        next_off = nxt;
        v_read += 1;
    }
    let (v_cnt, p_cnt) = if bmap_off + 10 <= v.len() {
        ((v[bmap_off + 8] as usize).clamp(1, 32), (v[bmap_off + 9] as usize).clamp(0, 64))
    } else {
        (v_read, 1)
    };
    for _ in v_read..v_cnt {
        let Some(zoff) = find_next_zlib_near(f, next_off)? else { break };
        let (c, nxt) = decompress_stream_at(f, zoff, 16 * 1024 * 1024)?;
        v.extend_from_slice(&c);
        next_off = nxt;
    }
    let mut p = Vec::new();
    for _ in 0..p_cnt {
        let Some(zoff) = find_next_zlib_near(f, next_off)? else { break };
        let (c, nxt) = decompress_stream_at(f, zoff, 64 * 1024 * 1024)?;
        p.extend_from_slice(&c);
        next_off = nxt;
    }
    Ok((v, p))
}

/// Probe if `c0` (first chunk of a RAGE9 resource) is a pgDictionary<grcTexture>.
pub fn probe_txd_header(c0: &[u8]) -> Option<(String, usize)> {
    if c0.len() < 0x40 { return None; }
    let h_ptr = ru64(c0, 0x20);
    let h_cnt = ru16(c0, 0x28) as usize;
    let i_ptr = ru64(c0, 0x30);
    let i_cnt = ru16(c0, 0x38) as usize;
    let i_cap = ru16(c0, 0x3a) as usize;
    if vptr(h_ptr).is_none() || vptr(i_ptr).is_none() { return None; }
    if !(1..=2048).contains(&i_cnt) || i_cnt != h_cnt || i_cnt > i_cap { return None; }
    let arr_off = vptr(i_ptr)?;
    if arr_off + 8 > c0.len() { return None; }
    let tp = vptr(ru64(c0, arr_off))?;
    if tp + 0x44 > c0.len() { return None; }
    pptr(ru64(c0, tp + 0x18))?;
    let w = ru16(c0, tp + 0x38) as usize;
    let h = ru16(c0, tp + 0x3a) as usize;
    let dxgi = c0[tp + 0x3f];
    if !(4..=8192).contains(&w) || !(4..=8192).contains(&h) || dxgi_info(dxgi).is_none() {
        return None;
    }
    let mut first_nm = format!("txd_{i_cnt}tex");
    if let Some(mut no) = vptr(ru64(c0, tp + 0x30)) {
        if no + 8 < c0.len() && c0[no..no + 8] == [0u8; 8] { no += 8; }
        if let Some(rel) = c0[no..].iter().position(|&b| b == 0) {
            if rel >= 2 && rel < 96 {
                let s = String::from_utf8_lossy(&c0[no..no + rel]).into_owned();
                if s.chars().all(|c| c.is_ascii_graphic()) {
                    first_nm = if i_cnt > 1 { format!("{s} (+{}).#td", i_cnt - 1) } else { format!("{s}.#td") };
                }
            }
        }
    }
    Some((first_nm, i_cnt))
}

// =============================================================================
// TTPF (RDS5 / ETPF) Native Scanner & Decoder
// =============================================================================

#[derive(Clone, Debug)]
pub struct TtpfTexMeta {
    pub idx: usize,
    pub w: usize,
    pub h: usize,
    pub fmt_byte: u8,
    pub has_mips: bool,
    pub byte_off: u64,
    pub alloc_sz: u64,
}

pub fn scan_ttpf_descriptors(path: &Path) -> Result<(String, Vec<TtpfTexMeta>), String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let sz = f.metadata().map_err(|e| e.to_string())?.len();
    let mut hdr = [0u8; 24];
    f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    if &hdr[..4] != b"RDS5" {
        return Err("Not an RDS5 TTPF file".into());
    }
    let n_chunks = ru32(&hdr, 4);
    let tot_uncomp = ru32(&hdr, 8) as u64;
    let hdr_pages = ru32(&hdr, 16) as u64;
    let etpf_off = hdr_pages * 0x100;

    f.seek(SeekFrom::Start(etpf_off)).map_err(|e| e.to_string())?;
    let mut ehdr = [0u8; 16];
    f.read_exact(&mut ehdr).map_err(|e| e.to_string())?;
    if &ehdr[..4] != b"ETPF" {
        return Err(format!("Missing ETPF header at 0x{etpf_off:x}"));
    }
    let tex_count = ru32(&ehdr, 8) as usize;
    let mut recs = vec![0u8; tex_count * 16];
    f.read_exact(&mut recs).map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(tex_count);
    for i in 0..tex_count {
        let u0 = ru32(&recs, i * 16);
        let u1 = ru32(&recs, i * 16 + 4);
        let u2 = ru32(&recs, i * 16 + 8);
        let w = ((u0 & 0x3FFF) + 1) as usize;
        let h = (((u0 >> 14) & 0x3FFF) + 1) as usize;
        let fmt_byte = (u1 & 0xFF) as u8;
        let has_mips = (u1 >> 24) > 0;
        let page4k = (u2 & 0x000F_FFFF) as u64;
        out.push(TtpfTexMeta { idx: i, w, h, fmt_byte, has_mips, byte_off: page4k * 0x1000, alloc_sz: 0 });
    }
    let mut offs: Vec<u64> = out.iter().map(|d| d.byte_off).collect();
    offs.sort_unstable();
    offs.dedup();
    let mut nxt: HashMap<u64, u64> = HashMap::new();
    for w2 in offs.windows(2) {
        nxt.insert(w2[0], w2[1]);
    }
    for d in &mut out {
        let est = ((((d.w + 3) / 4) * ((d.h + 3) / 4) * 16) as u64).max(0x10000);
        d.alloc_sz = nxt.get(&d.byte_off).copied().unwrap_or((d.byte_off + est).min(tot_uncomp)).saturating_sub(d.byte_off).max(0x1000);
    }
    let info = format!("RDS5/ETPF · {sz} B · {n_chunks} chunks · {tex_count} textures");
    Ok((info, out))
}

fn etpf_fmt_to_dxgi(fmt_byte: u8) -> (u8, usize) {
    match fmt_byte {
        0x10 | 0x11 | 0x19 => (98, 16), // BC7
        0x20 | 0x21 => (77, 16),        // BC3
        0x38 | 0x39 | 0x68 | 0x69 => (71, 8), // BC1
        0x40 => (80, 8),                // BC4
        0x50 => (83, 16),               // BC5
        _ => (98, 16),
    }
}

/// Decode a single texture by descriptor index from a `.ttpf` archive.
pub fn decode_ttpf_texture(path: &Path, tex_idx: usize) -> Result<DecodedTexture, String> {
    let (_, descs) = scan_ttpf_descriptors(path)?;
    let d = descs.get(tex_idx).ok_or("Texture index out of range")?;

    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hdr = [0u8; 24];
    f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    let n_chunks = ru32(&hdr, 4) as usize;
    let hdr_pages = ru32(&hdr, 16) as u64;
    let etpf_off = hdr_pages * 0x100;

    let mut tbl = vec![0u8; n_chunks * 8];
    f.read_exact(&mut tbl).map_err(|e| e.to_string())?;

    let (mut dxgi, mut bpe) = etpf_fmt_to_dxgi(d.fmt_byte);
    let bw = ((d.w + 3) / 4).max(1);
    let bh = ((d.h + 3) / 4).max(1);
    let sz16 = (bw * bh * 16) as u64;
    let sz8 = (bw * bh * 8) as u64;
    if d.alloc_sz >= sz8 && d.alloc_sz < sz16 {
        dxgi = 71;
        bpe = 8;
    }
    let mip0_lin = bw * bh * bpe;
    let use_64k = mip0_lin >= 65536;
    let mip0_padded = if use_64k {
        let (tw, th) = if bpe == 16 { (64, 64) } else { (128, 64) };
        ((bw + tw - 1) / tw) * tw * ((bh + th - 1) / th) * th * bpe
    } else {
        padded_4kb_size(d.w, d.h, true, bpe)
    };
    let mip0_off = if d.has_mips && d.alloc_sz > mip0_padded as u64 && d.alloc_sz <= (mip0_padded as f64 * 1.65) as u64 {
        d.byte_off + (d.alloc_sz - mip0_padded as u64)
    } else {
        d.byte_off
    };
    let target_end = mip0_off + mip0_padded as u64;

    // Walk RDS5 chunks tracking exact virtual offset, decompressing only overlapping chunks!
    let mut prev_c = etpf_off;
    let mut prev_h = 0u64;
    let mut vpos = 0u64;
    let mut raw_tex = vec![0u8; mip0_padded];
    let mut covered = 0usize;
    let mut dec = Decompress::new(true);

    for i in 0..n_chunks {
        let lo = ru32(&tbl, i * 8);
        let hi = ru32(&tbl, i * 8 + 4);
        let cend = ((lo & 0x03FF_FFFF) as u64) * 0x100;
        let hi16k = ((hi & 0x000F_FFFF) as u64) * 0x4000;
        let cstart = prev_c;
        let h_delta = hi16k.saturating_sub(prev_h);
        prev_c = cend;
        prev_h = hi16k;
        if vpos >= target_end { break; }
        if cend <= cstart { continue; }

        let csize = (cend - cstart) as usize;
        // Quick peek if before target window: still need exact dec_len for sub-16KB zlib chunks
        f.seek(SeekFrom::Start(cstart)).map_err(|e| e.to_string())?;
        let mut head4 = [0u8; 4];
        let _ = f.read(&mut head4);
        let is_z = is_zlib_head(&head4);
        let is_raw_hdr = i == 0 || (h_delta > 0 && (csize as u64) >= h_delta && ((csize as u64) - h_delta) < 0x4000);

        // Estimate chunk virtual span before reading full bytes if well below mip0_off
        if !is_z && !is_raw_hdr {
            // Xbox Series X hardware BCPack chunk
            let adv = h_delta.max(0x1000);
            if vpos + adv > mip0_off && vpos < target_end {
                return Err("Texture uses Xbox Series X hardware BCPack compression (not zlib)".into());
            }
            vpos += adv;
            continue;
        }

        if is_raw_hdr {
            let adv = csize as u64;
            if vpos + adv > mip0_off && vpos < target_end {
                let mut ch = vec![0u8; csize];
                f.seek(SeekFrom::Start(cstart)).map_err(|e| e.to_string())?;
                f.read_exact(&mut ch).map_err(|e| e.to_string())?;
                let s0 = vpos.max(mip0_off);
                let s1 = (vpos + adv).min(target_end);
                raw_tex[(s0 - mip0_off) as usize..(s1 - mip0_off) as usize]
                    .copy_from_slice(&ch[(s0 - vpos) as usize..(s1 - vpos) as usize]);
                covered += (s1 - s0) as usize;
            }
            vpos += adv;
            continue;
        }

        // Zlib chunk: if h_delta >= 0x4000 and completely before mip0_off, check if we still need exact length (only if h_delta == 0!)
        if h_delta > 0 && vpos + h_delta + 0x4000 <= mip0_off {
            vpos += h_delta;
            continue;
        }

        let mut ch = vec![0u8; csize];
        f.seek(SeekFrom::Start(cstart)).map_err(|e| e.to_string())?;
        f.read_exact(&mut ch).map_err(|e| e.to_string())?;
        dec.reset(true);
        let mut out = Vec::with_capacity((h_delta as usize).max(65536));
        let mut tmp = [0u8; 65536];
        let mut ppos = 0usize;
        loop {
            let bi = dec.total_in();
            let bo = dec.total_out();
            let Ok(st) = dec.decompress(&ch[ppos..], &mut tmp, FlushDecompress::None) else { break };
            let cin = (dec.total_in() - bi) as usize;
            let cout = (dec.total_out() - bo) as usize;
            ppos += cin;
            if cout > 0 { out.extend_from_slice(&tmp[..cout]); }
            if st == Status::StreamEnd || (cin == 0 && cout == 0) { break; }
        }
        let adv = out.len() as u64;
        if adv == 0 {
            vpos += h_delta.max(0x1000);
            continue;
        }
        if vpos + adv > mip0_off && vpos < target_end {
            let s0 = vpos.max(mip0_off);
            let s1 = (vpos + adv).min(target_end);
            raw_tex[(s0 - mip0_off) as usize..(s1 - mip0_off) as usize]
                .copy_from_slice(&out[(s0 - vpos) as usize..(s1 - vpos) as usize]);
            covered += (s1 - s0) as usize;
        }
        vpos += adv;
    }

    if covered == 0 {
        return Err("Texture data not found in zlib chunks (likely BCPack)".into());
    }
    let lin = if use_64k {
        deswizzle_64kb_ttpf(&raw_tex, d.w, d.h, bpe)
    } else {
        deswizzle_4kb_d(&raw_tex, d.w, d.h, true, bpe)
    };
    let (fmt_name, _, _) = dxgi_info(dxgi).unwrap_or(("BC7_UNORM", true, 16));
    let pixels = decode_linear_to_rgba(&lin, d.w, d.h, dxgi).ok_or("BCn decode failed")?;
    Ok(DecodedTexture {
        name: format!("tex_{:05}_{}x{}_f{:02x}", d.idx, d.w, d.h, d.fmt_byte),
        w: d.w,
        h: d.h,
        mips: if d.has_mips { 2 } else { 1 },
        dxgi,
        fmt_name,
        pixels,
        raw_dds_payload: lin,
    })
}

// =============================================================================
// RBF0 (GTA 6 fwui UIObject) -> Readable XML Decoder
// =============================================================================

pub fn decode_rbf0(data: &[u8]) -> Option<String> {
    if data.len() < 8 || &data[..4] != b"RBF0" { return None; }
    let mut pos = 4usize;
    let n = data.len();
    let mut descs: Vec<(String, u8)> = Vec::new();
    let mut stack: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut root_lines: Vec<String> = Vec::new();

    let esc = |s: &str| -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    };

    while pos < n {
        let b0 = data[pos];
        if b0 == 0xFB && pos + 3 < n && data[pos + 1] == 0xFF {
            pos += 4;
            continue;
        }
        if b0 == 0xFF {
            if pos + 1 >= n { break; }
            let b1 = data[pos + 1];
            pos += 2;
            if b1 == 0xFF {
                if let Some((tag, attrs, kids)) = stack.pop() {
                    let indent = "  ".repeat(stack.len());
                    let attr_s = if attrs.is_empty() { String::new() } else { format!(" {}", attrs.join(" ")) };
                    let line = if kids.is_empty() {
                        format!("{indent}<{tag}{attr_s} />")
                    } else if kids.len() == 1 && !kids[0].starts_with('<') && !kids[0].contains('\n') {
                        format!("{indent}<{tag}{attr_s}>{}</{tag}>", kids[0])
                    } else {
                        format!("{indent}<{tag}{attr_s}>\n{}\n{indent}</{tag}>", kids.join("\n"))
                    };
                    if let Some(p) = stack.last_mut() { p.2.push(line); } else { root_lines.push(line); }
                }
            }
            continue;
        }
        if b0 == 0xFD && pos + 5 < n && data[pos + 1] == 0xFF {
            pos += 2;
            let dlen = ru32(data, pos) as usize;
            pos += 4;
            if pos + dlen > n { break; }
            let raw = &data[pos..pos + dlen];
            pos += dlen;
            let trimmed = raw.strip_suffix(&[0]).unwrap_or(raw);
            let txt = String::from_utf8_lossy(trimmed);
            if let Some(p) = stack.last_mut() { p.2.push(esc(&txt)); }
            continue;
        }
        if pos + 2 > n { break; }
        let idx = b0 as usize;
        let dtype = data[pos + 1];
        pos += 2;

        let name = if idx == descs.len() {
            if pos + 2 > n { break; }
            let nlen = ru16(data, pos) as usize;
            pos += 2;
            if pos + nlen > n { break; }
            let nm = String::from_utf8_lossy(&data[pos..pos + nlen]).into_owned();
            pos += nlen;
            descs.push((nm.clone(), dtype));
            nm
        } else if idx < descs.len() {
            descs[idx].0.clone()
        } else {
            break;
        };

        match dtype {
            0x00 => {
                if pos + 6 > n { break; }
                pos += 6;
                stack.push((name, Vec::new(), Vec::new()));
            }
            0x10 => {
                if pos + 4 > n { break; }
                let v = ru32(data, pos); pos += 4;
                if let Some(p) = stack.last_mut() { p.1.push(format!("{name}=\"{v}\"")); }
            }
            0x20 => {
                if pos + 1 > n { break; }
                let v = data[pos] != 0; pos += 1;
                if let Some(p) = stack.last_mut() { p.1.push(format!("{name}=\"{v}\"")); }
            }
            0x30 => {
                if pos + 1 > n { break; }
                let v = data[pos]; pos += 1;
                if let Some(p) = stack.last_mut() { p.1.push(format!("{name}=\"{v}\"")); }
            }
            0x40 => {
                if pos + 4 > n { break; }
                let v = rf32(data, pos); pos += 4;
                if let Some(p) = stack.last_mut() { p.1.push(format!("{name}=\"{v:.5}\"")); }
            }
            0x50 => {
                if pos + 12 > n { break; }
                let (x, y, z) = (rf32(data, pos), rf32(data, pos + 4), rf32(data, pos + 8));
                pos += 12;
                if let Some(p) = stack.last_mut() { p.1.push(format!("{name}=\"{x:.4},{y:.4},{z:.4}\"")); }
            }
            0x60 => {
                if pos + 2 > n { break; }
                let slen = ru16(data, pos) as usize; pos += 2;
                if pos + slen > n { break; }
                let sv = String::from_utf8_lossy(&data[pos..pos + slen]);
                pos += slen;
                if let Some(p) = stack.last_mut() { p.1.push(format!("{name}=\"{}\"", esc(&sv))); }
            }
            _ => break,
        }
    }
    while let Some((tag, attrs, kids)) = stack.pop() {
        let indent = "  ".repeat(stack.len());
        let attr_s = if attrs.is_empty() { String::new() } else { format!(" {}", attrs.join(" ")) };
        let line = format!("{indent}<{tag}{attr_s}>\n{}\n{indent}</{tag}>", kids.join("\n"));
        if let Some(p) = stack.last_mut() { p.2.push(line); } else { root_lines.push(line); }
    }
    (!root_lines.is_empty()).then(|| root_lines.join("\n"))
}

// =============================================================================
// Scaleform GFX (v42 AS3) Inspector
// =============================================================================

pub fn inspect_gfx(data: &[u8]) -> String {
    if data.len() < 8 || &data[..3] != b"GFX" {
        return "Not a Scaleform GFX binary".into();
    }
    let ver = data[3];
    let fsize = ru32(data, 4);
    let mut out = format!("Scaleform GFx Binary (ActionScript 3)\nVersion: {ver} (0x{ver:02x})  |  Header Size: {fsize} B  |  Chunk Size: {} B\n\nRecovered AS3 Classes / Packages / Symbols:\n", data.len());
    let mut seen = HashSet::new();
    let mut cur = Vec::new();
    let mut syms = Vec::new();
    for &b in data {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'/') {
            cur.push(b);
        } else {
            if cur.len() >= 6 && cur.len() <= 96 {
                if let Ok(s) = std::str::from_utf8(&cur) {
                    if (s.contains('.') || s.contains(':') || s.contains("fontlib") || s.contains("rockstar") || s.contains("scaleform") || s.contains("general"))
                        && s.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 4
                        && seen.insert(s.to_string())
                    {
                        syms.push(s.to_string());
                    }
                }
            }
            cur.clear();
        }
    }
    syms.sort();
    for s in syms.iter().take(400) {
        out.push_str("  ");
        out.push_str(s);
        out.push('\n');
    }
    out
}

// =============================================================================
// CNavMesh (ver 0x00010015) -> 3D Mesh Decoder for PercyStar 3D Viewport
// =============================================================================

pub fn decode_navmesh_from_rpf(rpf_path: &Path, file_off: u64, fallback_name: &str) -> Result<Mesh3D, String> {
    let mut f = File::open(rpf_path).map_err(|e| format!("Open error: {e}"))?;
    let (v, _) = read_rage9_vp(&mut f, file_off)?;
    if v.len() < 0xD0 || ru32(&v, 0x14) != 0x0001_0015 {
        return Err("Not a valid GTA VI CNavMesh (0x00010015)".into());
    }
    let ext = [rf32(&v, 0x60), rf32(&v, 0x64), rf32(&v, 0x68)];
    let qt = vptr(ru64(&v, 0xA0)).ok_or("Missing QuadTree")?;
    if qt + 32 > v.len() { return Err("Truncated QuadTree".into()); }
    let min_v = [rf32(&v, qt), rf32(&v, qt + 4), rf32(&v, qt + 8)];

    let read_split_blocks = |ptr_off: usize| -> Option<Vec<(usize, usize)>> {
        let sa = vptr(ru64(&v, ptr_off))?;
        if sa + 0x24 > v.len() { return None; }
        let p_blks = vptr(ru64(&v, sa + 0x10))?;
        let n_blks = ru32(&v, sa + 0x20) as usize;
        if n_blks == 0 || n_blks > 4096 || p_blks + n_blks * 16 > v.len() { return None; }
        let mut blks = Vec::with_capacity(n_blks);
        for bi in 0..n_blks {
            let bptr = vptr(ru64(&v, p_blks + bi * 16))?;
            let bcnt = ru32(&v, p_blks + bi * 16 + 8) as usize;
            blks.push((bptr, bcnt));
        }
        Some(blks)
    };

    let v_blks = read_split_blocks(0x70).ok_or("Missing vertex split array")?;
    let i_blks = read_split_blocks(0x80).ok_or("Missing index split array")?;
    let p_blks = read_split_blocks(0x98).ok_or("Missing poly split array")?;

    let mut verts = Vec::new();
    for (boff, cnt) in v_blks {
        for i in 0..cnt {
            let o = boff + i * 6;
            if o + 6 > v.len() { break; }
            let vx = min_v[0] + (ru16(&v, o) as f32 / 65535.0) * ext[0];
            let vy = min_v[1] + (ru16(&v, o + 2) as f32 / 65535.0) * ext[1];
            let vz = min_v[2] + (ru16(&v, o + 4) as f32 / 65535.0) * ext[2];
            verts.push([vx, vy, vz]);
        }
    }

    let mut indices = Vec::new();
    for (boff, cnt) in i_blks {
        for i in 0..cnt {
            let o = boff + i * 2;
            if o + 2 > v.len() { break; }
            indices.push(ru16(&v, o) as u32);
        }
    }

    let mut tris = Vec::new();
    let mut tri_geom = Vec::new();
    let mut poly_cnt = 0usize;
    for (boff, cnt) in p_blks {
        for i in 0..cnt {
            let o = boff + i * 44;
            if o + 44 > v.len() { break; }
            let vinfo = ru32(&v, o + 4);
            let f8 = ru32(&v, o + 8);
            let f18 = ru32(&v, o + 0x18);
            let nv = (vinfo & 0xF) as usize;
            let first = (vinfo >> 4) as usize;
            if nv < 3 || first + nv > indices.len() { continue; }
            // Group material ID by navmesh surface type: 0=Land, 1=Water, 2=Road, 3=Pavement, 4=Interior/Shelter
            let mat: u16 = if (f8 & 0x80) != 0 || (f18 & 0x80000) != 0 {
                1
            } else if (f8 & 0x08) != 0 {
                4
            } else if (f18 & 0x2000) != 0 {
                2
            } else if (f18 & 0x1000) != 0 {
                3
            } else {
                0
            };
            let i0 = indices[first];
            for k in 1..nv - 1 {
                let (i1, i2) = (indices[first + k], indices[first + k + 1]);
                if (i0 as usize) < verts.len() && (i1 as usize) < verts.len() && (i2 as usize) < verts.len() {
                    tris.push([i0, i1, i2]);
                    tri_geom.push(mat);
                }
            }
            poly_cnt += 1;
        }
    }
    if verts.is_empty() || tris.is_empty() {
        return Err("Empty CNavMesh geometry".into());
    }
    Ok(Mesh3D::from_parts(
        fallback_name.to_string(),
        verts,
        Vec::new(),
        tris,
        tri_geom,
        Vec::new(),
        None,
        format!("CNavMesh ({poly_cnt} polys, Z {:.0}..{:.0}m)", min_v[2], min_v[2] + ext[2]),
        5,
        poly_cnt,
        "NavMesh".into(),
    ))
}
