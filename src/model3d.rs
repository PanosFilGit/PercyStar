use eframe::egui::{self, Color32, ColorImage, CornerRadius, RichText, TextureHandle, TextureOptions};
use flate2::{Decompress, FlushDecompress, Status};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ShadeMode {
    #[default]
    Textured,
    Solid,
    SolidWire,
    Wireframe,
}

pub struct Texture2D {
    pub w: usize,
    pub h: usize,
    pub pixels: Vec<Color32>,
}

impl Texture2D {
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> Color32 {
        if self.w == 0 || self.h == 0 {
            return Color32::WHITE;
        }
        let x = ((u.rem_euclid(1.0) * (self.w as f32)) as usize).min(self.w - 1);
        let y = ((v.rem_euclid(1.0) * (self.h as f32)) as usize).min(self.h - 1);
        self.pixels[y * self.w + x]
    }
}

const DEFAULT_YAW: f32 = 2.45;
const DEFAULT_PITCH: f32 = 0.38;

pub struct Mesh3D {
    pub name: String,
    pub verts: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tris: Vec<[u32; 3]>,
    pub tri_geom: Vec<u16>,
    pub normals: Vec<[f32; 3]>,
    pub edges: Vec<(u32, u32)>,
    pub texture: Option<Texture2D>,
    pub tex_status: String,
    pub center: [f32; 3],
    pub radius: f32,
    pub geom_count: usize,
    pub meshlet_count: usize,
    pub lod_label: String,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub shade_mode: ShadeMode,
    pub auto_spin: bool,
    // reused softwre rasterizer buffrs zero allocs per fram
    fb_w: usize,
    fb_h: usize,
    color_buf: Vec<Color32>,
    z_buf: Vec<f32>,
    proj_buf: Vec<[f32; 3]>,
    tex: Option<TextureHandle>,
    dirty: bool,
    last_dragging: bool,
}

impl Mesh3D {
    pub fn from_parts(
        name: String,
        verts: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        tris: Vec<[u32; 3]>,
        tri_geom: Vec<u16>,
        edges: Vec<(u32, u32)>,
        texture: Option<Texture2D>,
        tex_status: String,
        geom_count: usize,
        meshlet_count: usize,
        lod_label: String,
    ) -> Self {
        let (mut min_v, mut max_v) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &verts {
            for k in 0..3 {
                if v[k] < min_v[k] { min_v[k] = v[k]; }
                if v[k] > max_v[k] { max_v[k] = v[k]; }
            }
        }
        let center = if verts.is_empty() {
            [0.0; 3]
        } else {
            [
                (min_v[0] + max_v[0]) * 0.5,
                (min_v[1] + max_v[1]) * 0.5,
                (min_v[2] + max_v[2]) * 0.5,
            ]
        };
        let mut r2 = 0.0f32;
        for v in &verts {
            let dx = v[0] - center[0];
            let dy = v[1] - center[1];
            let dz = v[2] - center[2];
            r2 = r2.max(dx * dx + dy * dy + dz * dz);
        }
        let radius = r2.sqrt().max(1e-3);

        // precomput unit face normals once so drag stays fast
        let mut normals = Vec::with_capacity(tris.len());
        for t in &tris {
            let a = verts[t[0] as usize];
            let b = verts[t[1] as usize];
            let c = verts[t[2] as usize];
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let nx = ab[1] * ac[2] - ab[2] * ac[1];
            let ny = ab[2] * ac[0] - ab[0] * ac[2];
            let nz = ab[0] * ac[1] - ab[1] * ac[0];
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            if len > 1e-12 {
                normals.push([nx / len, ny / len, nz / len]);
            } else {
                normals.push([0.0, 0.0, 1.0]);
            }
        }

        Self {
            name,
            verts,
            uvs,
            tris,
            tri_geom,
            normals,
            edges,
            texture,
            tex_status,
            center,
            radius,
            geom_count,
            meshlet_count,
            lod_label,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            shade_mode: ShadeMode::Textured,
            auto_spin: false,
            fb_w: 0,
            fb_h: 0,
            color_buf: Vec::new(),
            z_buf: Vec::new(),
            proj_buf: Vec::new(),
            tex: None,
            dirty: true,
            last_dragging: false,
        }
    }

    pub fn export_obj(&self, path: &Path) -> std::io::Result<()> {
        let mut w = BufWriter::new(File::create(path)?);
        writeln!(w, "# PercyStar RAGE9 Model Export: {}", self.name)?;
        writeln!(w, "o {}", self.name)?;
        for v in &self.verts {
            writeln!(w, "v {:.6} {:.6} {:.6}", v[0], v[1], v[2])?;
        }
        let has_uv = self.uvs.len() == self.verts.len() && !self.uvs.is_empty();
        if has_uv {
            for uv in &self.uvs {
                writeln!(w, "vt {:.6} {:.6}", uv[0], uv[1])?;
            }
            for t in &self.tris {
                let (a, b, c) = (t[0] + 1, t[1] + 1, t[2] + 1);
                writeln!(w, "f {a}/{a} {b}/{b} {c}/{c}")?;
            }
        } else {
            for t in &self.tris {
                writeln!(w, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1)?;
            }
        }
        Ok(())
    }
}

pub fn parse_obj(path: &Path) -> Option<Mesh3D> {
    let txt = fs::read_to_string(path).ok()?;
    let mut verts = Vec::new();
    let mut tris = Vec::new();
    let mut edges = Vec::new();
    for line in txt.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                if let (Some(x), Some(y), Some(z)) = (
                    it.next().and_then(|s| s.parse::<f32>().ok()),
                    it.next().and_then(|s| s.parse::<f32>().ok()),
                    it.next().and_then(|s| s.parse::<f32>().ok()),
                ) {
                    verts.push([x, y, z]);
                }
            }
            Some("f") => {
                let idxs: Vec<u32> = it
                    .filter_map(|tok| tok.split('/').next()?.parse::<usize>().ok()?.checked_sub(1))
                    .filter(|&i| i < verts.len())
                    .map(|i| i as u32)
                    .collect();
                if idxs.len() >= 3 {
                    for k in 1..idxs.len() - 1 {
                        tris.push([idxs[0], idxs[k], idxs[k + 1]]);
                    }
                } else if idxs.len() == 2 {
                    edges.push((idxs[0], idxs[1]));
                }
            }
            _ => {}
        }
    }
    if verts.is_empty() { return None; }
    // flip external y up obj files into z up turntabl space
    if !txt.starts_with("# PercyStar") {
        for v in &mut verts {
            *v = [v[0], -v[2], v[1]];
        }
    }
    let tri_geom = vec![0u16; tris.len()];
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Some(Mesh3D::from_parts(name, verts, Vec::new(), tris, tri_geom, edges, None, "OBJ".into(), 1, 0, "OBJ".into()))
}

pub fn extract_0drpz_mesh(name: &str, data: &[u8]) -> Option<Mesh3D> {
    let mut verts = Vec::new();
    let mut off = 0x40usize;
    while off + 12 <= data.len() && verts.len() < 12000 {
        let x = rf32(data, off);
        let y = rf32(data, off + 4);
        let z = rf32(data, off + 8);
        if x.is_finite() && y.is_finite() && z.is_finite()
            && x.abs() < 250.0 && y.abs() < 250.0 && z.abs() < 250.0
            && (x.abs() > 1e-4 || y.abs() > 1e-4 || z.abs() > 1e-4)
        {
            verts.push([x, y, z]);
            off += 12;
        } else {
            off += 4;
        }
    }
    if verts.len() < 12 { return None; }
    Some(Mesh3D::from_parts(name.to_string(), verts, Vec::new(), Vec::new(), Vec::new(), Vec::new(), None, "Points".into(), 1, 0, "Points".into()))
}

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
pub fn is_zlib_head(b: &[u8]) -> bool {
    b.len() >= 3 && b[0] == 0x78 && matches!(b[1], 0x01 | 0x5e | 0x9c | 0xda) && (b[2] & 0x06) != 0x06
}

#[inline]
fn vptr(q: u64) -> Option<usize> {
    (0x5000_0000..0x5800_0000).contains(&q).then(|| (q - 0x5000_0000) as usize)
}

#[inline]
fn pptr(q: u64) -> Option<usize> {
    (0x6000_0000..0x7000_0000).contains(&q).then(|| (q - 0x6000_0000) as usize)
}

// decomprss one zlib stream from memory slice returns false if incomplete
fn decompress_zlib_slice(src: &[u8], dec: &mut Decompress, out: &mut Vec<u8>) -> Option<usize> {
    dec.reset(true);
    let mut tmp = [0u8; 65536];
    let mut pos = 0usize;
    loop {
        let bi = dec.total_in();
        let bo = dec.total_out();
        let st = dec.decompress(&src[pos..], &mut tmp, FlushDecompress::None).ok()?;
        let cin = (dec.total_in() - bi) as usize;
        let cout = (dec.total_out() - bo) as usize;
        pos += cin;
        if cout > 0 {
            out.extend_from_slice(&tmp[..cout]);
        }
        if st == Status::StreamEnd {
            return Some(pos);
        }
        if cin == 0 && cout == 0 {
            return None;
        }
    }
}

fn decompress_zlib_at(f: &mut File, file_off: u64, max_out: usize) -> std::io::Result<(Vec<u8>, u64)> {
    f.seek(SeekFrom::Start(file_off))?;
    let mut dec = Decompress::new(true);
    let mut in_buf = [0u8; 65536];
    let mut out_chunk = [0u8; 65536];
    let mut out = Vec::with_capacity(65536);
    let mut total_consumed = 0u64;
    loop {
        let n = f.read(&mut in_buf)?;
        if n == 0 { break; }
        let mut pos = 0usize;
        while pos < n {
            let bi = dec.total_in();
            let bo = dec.total_out();
            let status = dec
                .decompress(&in_buf[pos..n], &mut out_chunk, FlushDecompress::None)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let cin = (dec.total_in() - bi) as usize;
            let cout = (dec.total_out() - bo) as usize;
            pos += cin;
            total_consumed += cin as u64;
            if cout > 0 {
                out.extend_from_slice(&out_chunk[..cout]);
                if out.len() > max_out {
                    return Ok((out, file_off + total_consumed));
                }
            }
            if status == Status::StreamEnd {
                return Ok((out, file_off + total_consumed));
            }
            if cin == 0 && cout == 0 {
                break;
            }
        }
    }
    Ok((out, file_off + total_consumed))
}

fn find_next_zlib(f: &mut File, start_off: u64) -> std::io::Result<Option<u64>> {
    f.seek(SeekFrom::Start(start_off))?;
    let mut buf = [0u8; 4096];
    let n = f.read(&mut buf)?;
    for i in 0..n.saturating_sub(3) {
        if is_zlib_head(&buf[i..n]) {
            return Ok(Some(start_off + i as u64));
        }
    }
    Ok(None)
}

// reads all virtual chnks from a rage9 resource stopping right after blockmap
fn read_virtual_from_file(f: &mut File, file_off: u64) -> Result<(Vec<u8>, u64, usize), String> {
    let (mut v, mut next_off) = decompress_zlib_at(f, file_off, 16 * 1024 * 1024)
        .map_err(|e| format!("Virtual chunk 0 error: {e}"))?;
    if v.len() < 0x80 {
        return Err("Virtual chunk too small".into());
    }
    let Some(bmap_off) = vptr(ru64(&v, 8)) else {
        return Err("Not a valid RAGE9 resource (missing BlockMap)".into());
    };
    let mut v_chunks_read = 1usize;
    while v.len() <= bmap_off + 10 && v_chunks_read < 32 {
        let Some(zoff) = find_next_zlib(f, next_off).map_err(|e| e.to_string())? else { break };
        let (chunk, nxt) = decompress_zlib_at(f, zoff, 16 * 1024 * 1024).map_err(|e| e.to_string())?;
        v.extend_from_slice(&chunk);
        next_off = nxt;
        v_chunks_read += 1;
    }
    let (v_cnt, p_cnt) = if bmap_off + 10 <= v.len() {
        ((v[bmap_off + 8] as usize).clamp(1, 32), (v[bmap_off + 9] as usize).clamp(1, 64))
    } else {
        (v_chunks_read, 1)
    };
    for _ in v_chunks_read..v_cnt {
        let Some(zoff) = find_next_zlib(f, next_off).map_err(|e| e.to_string())? else { break };
        let (chunk, nxt) = decompress_zlib_at(f, zoff, 16 * 1024 * 1024).map_err(|e| e.to_string())?;
        v.extend_from_slice(&chunk);
        next_off = nxt;
    }
    Ok((v, next_off, p_cnt))
}

// finds dr ft dd modle string in virtual buffr (also checks drawable +0xa8 name ptr!)
pub fn extract_rage9_model_name(v: &[u8]) -> Option<(String, &'static str)> {
    let mut i = 1usize;
    while i + 2 < v.len() {
        if v[i] == b'#' && v[i - 1] == b'.' {
            let ext = match (v[i + 1], v[i + 2]) {
                (b'd', b'r') => ".#dr",
                (b'f', b't') => ".#ft",
                (b'd', b'd') => ".#dd",
                _ => {
                    i += 1;
                    continue;
                }
            };
            let dot = i - 1;
            let mut start = dot;
            while start > 0 {
                let c = v[start - 1];
                if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' {
                    start -= 1;
                } else {
                    break;
                }
            }
            if dot - start >= 2 && dot - start <= 96 {
                let stem = String::from_utf8_lossy(&v[start..dot]);
                return Some((format!("{stem}{ext}"), ext));
            }
        }
        i += 1;
    }
    // Fallback: RAGE9 gtaDrawable name pointer at +0xa8 (catches 3,000+ models without .#dr suffix!)
    if v.len() >= 0xb0 {
        if let Some(no) = vptr(ru64(v, 0xa8)) {
            if no < v.len() {
                if let Some(rel) = v[no..].iter().position(|&b| b == 0) {
                    if (2..=96).contains(&rel) {
                        let s = &v[no..no + rel];
                        if s.iter().all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'#'))
                            && s.iter().any(|b| b.is_ascii_alphabetic())
                        {
                            let nm = String::from_utf8_lossy(s).trim_end_matches('.').to_string();
                            if nm.len() >= 2 {
                                let ext = if nm.ends_with(".#ft") { ".#ft" } else { ".#dr" };
                                let full = if nm.contains(".#") { nm } else { format!("{nm}.#dr") };
                                return Some((full, ext));
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

// checks if virtual resource is an actual vehicle and not a small tuning part
pub fn is_vehicle_virtual(v: &[u8], name: &str) -> bool {
    let lo = name.to_ascii_lowercase();
    if !lo.ends_with(".#ft") || v.len() < 28_000 {
        return false;
    }
    for part in [
        "_dmg_", "_livery", "_roof_", "_roofext_", "_spoiler", "_exhaust",
        "_bumf_", "_bumr_", "_skirt_", "_bonnet", "_wing_", "_door_",
        "_mirror", "_lights_", "_intakes_", "_platemount", "_towbar",
        "_speakermount", "_deliverybag", "_antennaset", "_radar_", "_horn",
        "_outboard", "_inboard", "_mast", "_mount", "_turret", "_weap_",
        "_wheeliebar", "_induction_", "_whiplight", "_rack_", "_sign_mesh",
        "_armour_plate", "_fairing_", "_rescuehoist", "_footboards", "_callsign",
        "_shorad", "_badge", "_trim", "_overlay",
    ] {
        if lo.contains(part) {
            return false;
        }
    }
    [b"wheel_lf".as_slice(), b"door_dside_f", b"chassis", b"bodyshell", b"cockpit", b"rudder", b"rotor_main"]
        .iter()
        .any(|needle| v.windows(needle.len()).any(|w| w == *needle))
}

// zero seek in memory probe falls back to disk if stream crosses chunk boundry
pub fn probe_large_rage9_model_in_buf(buf: &[u8], rpf_path: &Path, abs_off: u64) -> Option<(String, bool, usize)> {
    let mut dec = Decompress::new(true);
    let mut v = Vec::with_capacity(131072);
    let mut pos = match decompress_zlib_slice(buf, &mut dec, &mut v) {
        Some(n) => n,
        None => return probe_large_rage9_model(rpf_path, abs_off),
    };
    if v.len() < 0x80 { return None; }
    let bmap_off = vptr(ru64(&v, 8))?;
    if bmap_off > 4 * 1024 * 1024 { return None; }

    for _ in 0..14 {
        if v.len() > bmap_off + 16 { break; }
        while pos + 3 < buf.len() && !is_zlib_head(&buf[pos..]) {
            pos += 1;
        }
        if pos + 3 >= buf.len() {
            return probe_large_rage9_model(rpf_path, abs_off);
        }
        match decompress_zlib_slice(&buf[pos..], &mut dec, &mut v) {
            Some(n) => pos += n,
            None => return probe_large_rage9_model(rpf_path, abs_off),
        }
    }
    let (name, _) = extract_rage9_model_name(&v)?;
    Some((name.clone(), is_vehicle_virtual(&v, &name), v.len()))
}

pub fn probe_large_rage9_model(rpf_path: &Path, file_off: u64) -> Option<(String, bool, usize)> {
    let mut f = File::open(rpf_path).ok()?;
    let (v, _, _) = read_virtual_from_file(&mut f, file_off).ok()?;
    let (name, _) = extract_rage9_model_name(&v)?;
    Some((name.clone(), is_vehicle_virtual(&v, &name), v.len()))
}

// shared meshlet link resolver so lod0 and fallback scanner stay dry
#[inline]
fn resolve_meshlet_buffer(v: &[u8], link_ptr: u64) -> Option<(usize, usize)> {
    let lo = vptr(link_ptr)?;
    if lo + 8 > v.len() { return None; }
    let mo = vptr(ru64(v, lo))?;
    if mo + 0x20 > v.len() { return None; }
    let mb_poff = pptr(ru64(v, mo + 0x18))?;
    let mb_words = ru32(v, mo + 0x08) as usize;
    Some((mb_poff, mb_words * 4))
}

fn collect_lod0_geoms(v: &[u8]) -> Vec<(usize, usize, usize, usize)> {
    let mut bases = vec![0usize];
    if v.len() >= 0x20 {
        if let Some(fo) = vptr(ru64(v, 0x18)) {
            if fo + 0x80 <= v.len() { bases.push(fo); }
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for base in bases {
        if base + 0x80 > v.len() { continue; }
        let Some(lod0_off) = vptr(ru64(v, base + 0x50)) else { continue };
        if lod0_off + 0x10 > v.len() { continue; }
        let Some(models_off) = vptr(ru64(v, lod0_off)) else { continue };
        let model_cnt = ru16(v, lod0_off + 8) as usize;
        if model_cnt == 0 || model_cnt > 128 || models_off + model_cnt * 8 > v.len() { continue; }

        for mi in 0..model_cnt {
            let Some(mp) = vptr(ru64(v, models_off + mi * 8)) else { continue };
            if mp + 0x18 > v.len() { continue; }
            let Some(garr) = vptr(ru64(v, mp + 0x08)) else { continue };
            let gcnt = ru16(v, mp + 0x10) as usize;
            if gcnt == 0 || gcnt > 256 || garr + gcnt * 8 > v.len() { continue; }

            for gi in 0..gcnt {
                let Some(gp) = vptr(ru64(v, garr + gi * 8)) else { continue };
                if gp + 0x70 > v.len() || ru64(v, gp + 0x60) != 0x1601 { continue; }
                let Some(vb_poff) = pptr(ru64(v, gp + 0x68)) else { continue };
                let Some((mb_poff, mb_bytes)) = resolve_meshlet_buffer(v, ru64(v, gp + 0x40)) else { continue };
                let vb_bytes = (ru32(v, gp + 0x58) as usize) * 4;
                if seen.insert((vb_poff, mb_poff)) {
                    out.push((vb_poff, vb_bytes, mb_poff, mb_bytes));
                }
            }
        }
        if !out.is_empty() { break; }
    }
    out
}

fn collect_all_geoms(v: &[u8]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut off = 0x28usize;
    while off + 0x10 <= v.len() {
        if ru64(v, off - 8) == 0x1601 {
            if let Some(vb_poff) = pptr(ru64(v, off)) {
                if let Some((mb_poff, mb_bytes)) = resolve_meshlet_buffer(v, ru64(v, off - 0x28)) {
                    let vb_bytes = (ru32(v, off - 16) as usize) * 4;
                    if seen.insert((vb_poff, mb_poff)) {
                        out.push((vb_poff, vb_bytes, mb_poff, mb_bytes));
                    }
                }
            }
        }
        off += 8;
    }
    out
}

fn try_extract_embedded_texture(v: &[u8], p: &[u8]) -> (Option<Texture2D>, String) {
    let mut bases = vec![0usize];
    if v.len() >= 0x20 {
        if let Some(fo) = vptr(ru64(v, 0x18)) {
            bases.push(fo);
        }
    }
    for base in bases {
        if base + 0x18 > v.len() { continue; }
        let Some(sg) = vptr(ru64(v, base + 0x10)) else { continue };
        if sg + 0x10 > v.len() { continue; }
        let Some(txd) = vptr(ru64(v, sg + 0x08)) else { continue };
        // Use our verified Xbox Scarlett SW_4KB_D + reverse-mip + pure-Rust BC7/BC3/BC1 decoder!
        let decoded = crate::tex::extract_txd_textures(v, p, txd);
        if !decoded.is_empty() {
            // Prefer diffuse/albedo texture over normal/spec/blank if multiple embedded
            let best = decoded
                .iter()
                .find(|t| {
                    let lo = t.name.to_ascii_lowercase();
                    !lo.contains("_n") && !lo.contains("_nm") && !lo.contains("_s") && !lo.contains("blank") && t.w >= 32
                })
                .unwrap_or(&decoded[0]);
            return (
                Some(crate::tex::to_model_tex(best)),
                format!("Embedded {} ({}x{} {}, {}/{})", best.name, best.w, best.h, best.fmt_name, 1, decoded.len()),
            );
        }
    }
    (None, "External .#td (Material Shading)".into())
}

pub fn decode_rage9_model_from_rpf(rpf_path: &Path, file_off: u64, fallback_name: &str) -> Result<Mesh3D, String> {
    let mut f = File::open(rpf_path).map_err(|e| format!("Open error: {e}"))?;
    let (v, mut next_off, p_cnt) = read_virtual_from_file(&mut f, file_off)?;

    let mut p = Vec::new();
    for _ in 0..p_cnt {
        let Some(zoff) = find_next_zlib(&mut f, next_off).map_err(|e| e.to_string())? else { break };
        let (chunk, nxt) = decompress_zlib_at(&mut f, zoff, 32 * 1024 * 1024).map_err(|e| e.to_string())?;
        p.extend_from_slice(&chunk);
        next_off = nxt;
    }
    if p.len() < 0x90 {
        return Err("Physical GPU buffer missing or empty".into());
    }

    let lod0_geoms = collect_lod0_geoms(&v);
    let (geoms, lod_label) = if !lod0_geoms.is_empty() {
        (lod0_geoms, "LOD0".to_string())
    } else {
        (collect_all_geoms(&v), "All".to_string())
    };
    if geoms.is_empty() {
        return Err("No RAGE9 meshlet geometries found in resource".into());
    }

    let model_name = extract_rage9_model_name(&v)
        .map(|(n, _)| n)
        .unwrap_or_else(|| fallback_name.to_string());

    let mut all_verts: Vec<[f32; 3]> = Vec::new();
    let mut all_uvs: Vec<[f32; 2]> = Vec::new();
    let mut all_tris: Vec<[u32; 3]> = Vec::new();
    let mut all_tri_geom: Vec<u16> = Vec::new();
    let mut total_meshlets = 0usize;

    for (gi_idx, &(vb_off, vb_size, mb_off, _mb_size)) in geoms.iter().enumerate() {
        if mb_off + 0x10 > p.len() || vb_off + 0x80 > p.len() {
            continue;
        }
        let num_meshlets = (ru32(&p, mb_off) & 0xFFFF) as usize;
        let tot_tris = ru32(&p, mb_off + 4) as usize;
        let tot_vrefs = ru32(&p, mb_off + 8) as usize;
        if num_meshlets == 0 || tot_tris == 0 || tot_vrefs == 0 {
            continue;
        }
        let tri_base = mb_off + 0x0c + num_meshlets * 16;
        let remap_base = tri_base + tot_tris * 4;
        if remap_base + tot_vrefs * 2 > p.len() {
            continue;
        }

        let mut hdrs = [0u32; 8];
        for k in 0..8 {
            if vb_off + k * 4 + 4 <= p.len() {
                hdrs[k] = ru32(&p, vb_off + k * 4);
            }
        }
        let stream0_off = ((hdrs[0] >> 8) << 1) as usize;
        let mut stream0_end = vb_size;
        let mut uv_stream_off: Option<usize> = None;
        for &h in &hdrs[1..] {
            if h == 0 { continue; }
            let so = ((h >> 8) << 1) as usize;
            let st = h & 0xff;
            if so > stream0_off && so < stream0_end {
                stream0_end = so;
            }
            if st == 0x28 && uv_stream_off.is_none() {
                uv_stream_off = Some(so);
            }
        }
        if vb_off + stream0_off >= p.len() {
            continue;
        }
        let max_vb_verts = ((stream0_end.saturating_sub(stream0_off)) / 8)
            .min((p.len() - (vb_off + stream0_off)) / 8);
        if max_vb_verts == 0 {
            continue;
        }

        let ox = rf32(&p, vb_off + 0x60);
        let oy = rf32(&p, vb_off + 0x64);
        let oz = rf32(&p, vb_off + 0x68);
        let sx = rf32(&p, vb_off + 0x70);
        let sy = rf32(&p, vb_off + 0x74);
        let sz = rf32(&p, vb_off + 0x78);

        let base_v = all_verts.len() as u32;
        let mut max_v_used = 0usize;
        let start_tri = all_tris.len();
        all_tris.reserve(tot_tris);

        for mi in 0..num_meshlets {
            let m_off = mb_off + 0x0c + mi * 16;
            let v_off = ru16(&p, m_off + 4) as usize;
            let t_off = ru16(&p, m_off + 6) as usize;
            let t_end = if mi + 1 < num_meshlets {
                ru16(&p, mb_off + 0x0c + (mi + 1) * 16 + 6) as usize
            } else {
                tot_tris
            };
            for ti in t_off..t_end.min(tot_tris) {
                let w = ru32(&p, tri_base + ti * 4);
                let r0 = v_off + (w & 0x7F) as usize;
                let r1 = v_off + ((w >> 7) & 0x7F) as usize;
                let r2 = v_off + ((w >> 14) & 0x7F) as usize;
                if r0 < tot_vrefs && r1 < tot_vrefs && r2 < tot_vrefs {
                    let i0 = ru16(&p, remap_base + r0 * 2) as usize;
                    let i1 = ru16(&p, remap_base + r1 * 2) as usize;
                    let i2 = ru16(&p, remap_base + r2 * 2) as usize;
                    if i0 < max_vb_verts && i1 < max_vb_verts && i2 < max_vb_verts && i0 != i1 && i1 != i2 && i0 != i2 {
                        all_tris.push([base_v + i0 as u32, base_v + i1 as u32, base_v + i2 as u32]);
                        max_v_used = max_v_used.max(i0).max(i1).max(i2);
                    }
                }
            }
        }

        let added_tris = all_tris.len() - start_tri;
        if added_tris > 0 {
            let v_cnt = (max_v_used + 1).min(max_vb_verts);
            let vbase = vb_off + stream0_off;
            all_verts.reserve(v_cnt);
            all_uvs.reserve(v_cnt);
            for i in 0..v_cnt {
                let o = vbase + i * 8;
                // packed rage9 24 bit xy and 16 bit z across 8 bytes
                let ix = (p[o + 1] as u32) | ((p[o + 2] as u32) << 8) | ((p[o + 3] as u32) << 16);
                let iy = (p[o] as u32) | ((p[o + 4] as u32) << 8) | ((p[o + 5] as u32) << 16);
                let iz = (p[o + 6] as u32) | ((p[o + 7] as u32) << 8);
                all_verts.push([ox + (ix as f32) * sx, oy + (iy as f32) * sy, oz + (iz as f32) * sz]);
                if let Some(uvo) = uv_stream_off {
                    let u_pos = vb_off + uvo + i * 4;
                    if u_pos + 4 <= p.len() {
                        all_uvs.push([ru16(&p, u_pos) as f32 / 8192.0, ru16(&p, u_pos + 2) as f32 / 8192.0]);
                        continue;
                    }
                }
                all_uvs.push([0.0, 0.0]);
            }
            all_tri_geom.resize(all_tri_geom.len() + added_tris, gi_idx as u16);
            total_meshlets += num_meshlets;
        }
    }

    if all_verts.is_empty() {
        return Err("Decoded 0 vertices from meshlets".into());
    }
    let (texture, tex_status) = try_extract_embedded_texture(&v, &p);
    Ok(Mesh3D::from_parts(
        model_name,
        all_verts,
        all_uvs,
        all_tris,
        all_tri_geom,
        Vec::new(),
        texture,
        tex_status,
        geoms.len(),
        total_meshlets,
        lod_label,
    ))
}

#[inline]
fn draw_line_z(
    color_buf: &mut [Color32],
    z_buf: &mut [f32],
    w: usize,
    h: usize,
    p0: [f32; 3],
    p1: [f32; 3],
    col: Color32,
    z_bias: f32,
) {
    let dx = p1[0] - p0[0];
    let dy = p1[1] - p0[1];
    let steps = dx.abs().max(dy.abs());
    if steps < 0.5 || steps > 4000.0 {
        return;
    }
    let n = steps.ceil() as i32;
    let inv = 1.0 / (n as f32);
    let sx = dx * inv;
    let sy = dy * inv;
    let sz = (p1[2] - p0[2]) * inv;
    let mut x = p0[0];
    let mut y = p0[1];
    let mut z = p0[2] + z_bias;
    for _ in 0..=n {
        let xi = x as i32;
        let yi = y as i32;
        if xi >= 0 && yi >= 0 && (xi as usize) < w && (yi as usize) < h {
            let idx = (yi as usize) * w + (xi as usize);
            if z >= z_buf[idx] {
                z_buf[idx] = z;
                color_buf[idx] = col;
            }
        }
        x += sx;
        y += sy;
        z += sz;
    }
}

fn rasterize_mesh(mesh: &mut Mesh3D, w: usize, h: usize) {
    let npix = w * h;
    if mesh.fb_w != w || mesh.fb_h != h {
        mesh.fb_w = w;
        mesh.fb_h = h;
        mesh.color_buf.resize(npix, Color32::from_rgb(13, 14, 18));
        mesh.z_buf.resize(npix, f32::NEG_INFINITY);
    }

    for y in 0..h {
        let t = (y as f32) / (h.max(1) as f32);
        let c = Color32::from_rgb(
            (11.0 + t * 9.0) as u8,
            (12.0 + t * 10.0) as u8,
            (17.0 + t * 14.0) as u8,
        );
        mesh.color_buf[y * w..(y + 1) * w].fill(c);
    }
    mesh.z_buf.fill(f32::NEG_INFINITY);

    let (sy, cy) = mesh.yaw.sin_cos();
    let (sp, cp) = mesh.pitch.sin_cos();
    let min_dim = (w.min(h) as f32) * 0.44;
    let scale = (min_dim / mesh.radius) * mesh.zoom;
    let cx = (w as f32) * 0.5 + mesh.pan_x * (w as f32);
    let cy_s = (h as f32) * 0.5 + mesh.pan_y * (h as f32);
    let ctr = mesh.center;
    let persp_k = 1.0 / (mesh.radius * 4.5);

    mesh.proj_buf.resize(mesh.verts.len(), [0.0; 3]);
    for (dst, v) in mesh.proj_buf.iter_mut().zip(&mesh.verts) {
        let x = v[0] - ctr[0];
        let y = v[1] - ctr[1];
        let z = v[2] - ctr[2];
        let x1 = x * cy - y * sy;
        let y1 = x * sy + y * cy;
        let y2 = -y1 * cp + z * sp;
        let z2 = z * cp + y1 * sp;
        let persp = 1.0 / (1.0 - (y2 * persp_k).clamp(-0.65, 0.65));
        *dst = [cx + x1 * scale * persp, cy_s - z2 * scale * persp, y2];
    }

    let wf = w as f32;
    let hf = h as f32;

    if !mesh.tris.is_empty() {
        let do_textured = mesh.shade_mode == ShadeMode::Textured;
        let do_solid = matches!(mesh.shade_mode, ShadeMode::Textured | ShadeMode::Solid | ShadeMode::SolidWire);
        let do_wire = matches!(mesh.shade_mode, ShadeMode::Wireframe | ShadeMode::SolidWire);
        let has_uvs = mesh.uvs.len() == mesh.verts.len() && !mesh.uvs.is_empty();

        const MAT_PALETTE: [[f32; 3]; 12] = [
            [0.82, 0.84, 0.88], [0.24, 0.26, 0.30], [0.18, 0.24, 0.32], [0.72, 0.75, 0.80],
            [0.34, 0.36, 0.40], [0.86, 0.24, 0.24], [0.48, 0.50, 0.56], [0.28, 0.24, 0.22],
            [0.78, 0.72, 0.62], [0.38, 0.44, 0.52], [0.64, 0.66, 0.70], [0.20, 0.21, 0.24],
        ];
        let l1 = [0.42f32, 0.65f32, 0.63f32];
        let l2 = [-0.60f32, 0.25f32, -0.45f32];

        if do_solid {
            for (ti, t) in mesh.tris.iter().enumerate() {
                let i0 = t[0] as usize;
                let i1 = t[1] as usize;
                let i2 = t[2] as usize;
                let p0 = mesh.proj_buf[i0];
                let p1 = mesh.proj_buf[i1];
                let p2 = mesh.proj_buf[i2];

                let min_x = p0[0].min(p1[0]).min(p2[0]);
                let max_x = p0[0].max(p1[0]).max(p2[0]);
                let min_y = p0[1].min(p1[1]).min(p2[1]);
                let max_y = p0[1].max(p1[1]).max(p2[1]);
                if max_x < 0.0 || min_x >= wf || max_y < 0.0 || min_y >= hf {
                    continue;
                }

                let area = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p1[1] - p0[1]) * (p2[0] - p0[0]);
                let abs_area = area.abs();
                if abs_area < 0.15 {
                    continue;
                }

                let n = mesh.normals[ti];
                let nx1 = n[0] * cy - n[1] * sy;
                let ny1 = n[0] * sy + n[1] * cy;
                let ny2 = -ny1 * cp + n[2] * sp;
                let nz2 = n[2] * cp + ny1 * sp;

                let ndl1 = (nx1 * l1[0] + ny2 * l1[1] + nz2 * l1[2]).abs();
                let ndl2 = (nx1 * l2[0] + ny2 * l2[1] + nz2 * l2[2]).max(0.0);
                let spec = ndl1.powi(10) * 0.35;
                let rim = (1.0 - ny2.abs()).powi(2) * 0.22;

                let shade_r = (0.18 + ndl1 * 0.72 + spec + rim * 0.45).min(1.15);
                let shade_g = (0.18 + ndl1 * 0.72 + ndl2 * 0.18 + spec + rim * 0.40).min(1.15);
                let shade_b = (0.20 + ndl1 * 0.70 + ndl2 * 0.28 + spec + rim * 0.55).min(1.15);

                let base_albedo = if do_textured && mesh.texture.is_none() {
                    let g = mesh.tri_geom.get(ti).copied().unwrap_or(0) as usize;
                    MAT_PALETTE[g % MAT_PALETTE.len()]
                } else {
                    [0.86, 0.88, 0.92]
                };
                let flat_col = Color32::from_rgb(
                    ((base_albedo[0] * shade_r).min(1.0) * 255.0) as u8,
                    ((base_albedo[1] * shade_g).min(1.0) * 255.0) as u8,
                    ((base_albedo[2] * shade_b).min(1.0) * 255.0) as u8,
                );

                let ix0 = (min_x.floor() as i32).max(0) as usize;
                let ix1 = (max_x.ceil() as i32).min(w as i32 - 1) as usize;
                let iy0 = (min_y.floor() as i32).max(0) as usize;
                let iy1 = (max_y.ceil() as i32).min(h as i32 - 1) as usize;

                // flip sign so edge funcs stay positive inside triangls
                let sign = if area > 0.0 { -1.0 } else { 1.0 };
                let inv_abs = 1.0 / abs_area;

                let e01_dx = (p1[1] - p0[1]) * sign;
                let e01_dy = (p0[0] - p1[0]) * sign;
                let e12_dx = (p2[1] - p1[1]) * sign;
                let e12_dy = (p1[0] - p2[0]) * sign;
                let e20_dx = (p0[1] - p2[1]) * sign;
                let e20_dy = (p2[0] - p0[0]) * sign;

                let px_start = ix0 as f32 + 0.5;
                let py_start = iy0 as f32 + 0.5;

                let mut w0_row = (px_start - p1[0]) * e12_dx + (py_start - p1[1]) * e12_dy;
                let mut w1_row = (px_start - p2[0]) * e20_dx + (py_start - p2[1]) * e20_dy;
                let mut w2_row = (px_start - p0[0]) * e01_dx + (py_start - p0[1]) * e01_dy;
                let span_len = ix1 - ix0 + 1;

                if do_textured && has_uvs {
                    if let Some(tex) = &mesh.texture {
                        let uv0 = mesh.uvs[i0];
                        let uv1 = mesh.uvs[i1];
                        let uv2 = mesh.uvs[i2];
                        for y in iy0..=iy1 {
                            let row_start = y * w + ix0;
                            let z_row = &mut mesh.z_buf[row_start..row_start + span_len];
                            let c_row = &mut mesh.color_buf[row_start..row_start + span_len];
                            let (mut w0, mut w1, mut w2) = (w0_row, w1_row, w2_row);
                            for x in 0..span_len {
                                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                                    let (b0, b1, b2) = (w0 * inv_abs, w1 * inv_abs, w2 * inv_abs);
                                    let z = b0 * p0[2] + b1 * p1[2] + b2 * p2[2];
                                    if z > z_row[x] {
                                        z_row[x] = z;
                                        let tc = tex.sample(
                                            b0 * uv0[0] + b1 * uv1[0] + b2 * uv2[0],
                                            b0 * uv0[1] + b1 * uv1[1] + b2 * uv2[1],
                                        );
                                        c_row[x] = Color32::from_rgb(
                                            ((tc.r() as f32 * shade_r).min(255.0)) as u8,
                                            ((tc.g() as f32 * shade_g).min(255.0)) as u8,
                                            ((tc.b() as f32 * shade_b).min(255.0)) as u8,
                                        );
                                    }
                                }
                                w0 += e12_dx;
                                w1 += e20_dx;
                                w2 += e01_dx;
                            }
                            w0_row += e12_dy;
                            w1_row += e20_dy;
                            w2_row += e01_dy;
                        }
                        continue;
                    }
                }

                for y in iy0..=iy1 {
                    let row_start = y * w + ix0;
                    let z_row = &mut mesh.z_buf[row_start..row_start + span_len];
                    let c_row = &mut mesh.color_buf[row_start..row_start + span_len];
                    let (mut w0, mut w1, mut w2) = (w0_row, w1_row, w2_row);
                    for x in 0..span_len {
                        if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                            let z = (w0 * p0[2] + w1 * p1[2] + w2 * p2[2]) * inv_abs;
                            if z > z_row[x] {
                                z_row[x] = z;
                                c_row[x] = flat_col;
                            }
                        }
                        w0 += e12_dx;
                        w1 += e20_dx;
                        w2 += e01_dx;
                    }
                    w0_row += e12_dy;
                    w1_row += e20_dy;
                    w2_row += e01_dy;
                }
            }
        }

        if do_wire {
            let wire_col = if do_solid { Color32::from_rgb(252, 175, 23) } else { Color32::from_rgb(72, 210, 255) };
            let z_bias = if do_solid { mesh.radius * 0.004 } else { 0.0 };
            let step = (mesh.tris.len() / 25_000).max(1);
            for t in mesh.tris.iter().step_by(step) {
                let p0 = mesh.proj_buf[t[0] as usize];
                let p1 = mesh.proj_buf[t[1] as usize];
                let p2 = mesh.proj_buf[t[2] as usize];
                draw_line_z(&mut mesh.color_buf, &mut mesh.z_buf, w, h, p0, p1, wire_col, z_bias);
                draw_line_z(&mut mesh.color_buf, &mut mesh.z_buf, w, h, p1, p2, wire_col, z_bias);
                draw_line_z(&mut mesh.color_buf, &mut mesh.z_buf, w, h, p2, p0, wire_col, z_bias);
            }
        }
    } else if !mesh.edges.is_empty() {
        let col = Color32::from_rgb(72, 210, 255);
        for &(a, b) in mesh.edges.iter().take(60_000) {
            draw_line_z(&mut mesh.color_buf, &mut mesh.z_buf, w, h, mesh.proj_buf[a as usize], mesh.proj_buf[b as usize], col, 0.0);
        }
    } else {
        let col = Color32::from_rgb(252, 175, 23);
        for p0 in &mesh.proj_buf {
            let xi = p0[0] as i32;
            let yi = p0[1] as i32;
            if xi >= 1 && yi >= 1 && (xi as usize) + 1 < w && (yi as usize) + 1 < h {
                let idx = (yi as usize) * w + (xi as usize);
                if p0[2] > mesh.z_buf[idx] {
                    mesh.z_buf[idx] = p0[2];
                    mesh.color_buf[idx] = col;
                }
            }
        }
    }
}

pub fn render_mesh3d_viewport(ui: &mut egui::Ui, mesh: &mut Mesh3D) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "{}  ·  {} verts  ·  {} tris  ·  {} geoms / {} meshlets ({})",
                mesh.name, mesh.verts.len(), mesh.tris.len(), mesh.geom_count, mesh.meshlet_count, mesh.lod_label
            ))
            .strong()
            .color(Color32::from_rgb(252, 175, 23)),
        );
        ui.add_space(6.0);
        for (mode, label) in [
            (ShadeMode::Textured, "Textured/Mat"),
            (ShadeMode::Solid, "Clay Solid"),
            (ShadeMode::SolidWire, "Solid+Wire"),
            (ShadeMode::Wireframe, "Wire"),
        ] {
            if ui.selectable_label(mesh.shade_mode == mode, label).clicked() {
                mesh.shade_mode = mode;
                mesh.dirty = true;
            }
        }
        if ui.selectable_label(mesh.auto_spin, "Spin").clicked() {
            mesh.auto_spin = !mesh.auto_spin;
            mesh.dirty = true;
        }
        if ui.button("Load Texture...").clicked() {
            if let Some(tp) = rfd::FileDialog::new().add_filter("Textures", &["png", "bmp"]).pick_file() {
                if let Ok(img) = image::open(&tp) {
                    let rgba = img.to_rgba8();
                    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                    let pixels: Vec<Color32> = rgba
                        .chunks_exact(4)
                        .map(|c| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]))
                        .collect();
                    mesh.texture = Some(Texture2D { w, h, pixels });
                    mesh.tex_status = format!("Loaded {} ({w}x{h})", tp.file_name().unwrap_or_default().to_string_lossy());
                    mesh.shade_mode = ShadeMode::Textured;
                    mesh.dirty = true;
                }
            }
        }
        if ui.button("Reset").clicked() {
            mesh.yaw = DEFAULT_YAW;
            mesh.pitch = DEFAULT_PITCH;
            mesh.zoom = 1.0;
            mesh.pan_x = 0.0;
            mesh.pan_y = 0.0;
            mesh.dirty = true;
        }
        if ui.button("Export .OBJ").clicked() {
            let safe_name = mesh.name.replace('#', "");
            if let Some(p) = rfd::FileDialog::new().set_file_name(format!("{safe_name}.obj")).save_file() {
                let _ = mesh.export_obj(&p);
            }
        }
        ui.label(RichText::new(&mesh.tex_status).size(11.0).color(Color32::from_rgb(130, 145, 168)));
    });
    ui.add_space(4.0);

    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());

    let mut dragging = false;
    if resp.dragged_by(egui::PointerButton::Primary) {
        let d = resp.drag_delta();
        mesh.yaw += d.x * 0.01;
        mesh.pitch = (mesh.pitch + d.y * 0.01).clamp(-1.5, 1.5);
        mesh.dirty = true;
        dragging = true;
    } else if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
        let d = resp.drag_delta();
        mesh.pan_x += d.x / rect.width().max(1.0);
        mesh.pan_y += d.y / rect.height().max(1.0);
        mesh.dirty = true;
        dragging = true;
    }
    if resp.double_clicked() {
        mesh.yaw = DEFAULT_YAW;
        mesh.pitch = DEFAULT_PITCH;
        mesh.zoom = 1.0;
        mesh.pan_x = 0.0;
        mesh.pan_y = 0.0;
        mesh.dirty = true;
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            mesh.zoom = (mesh.zoom * (1.0 + scroll * 0.0022)).clamp(0.15, 25.0);
            mesh.dirty = true;
        }
    }
    if mesh.auto_spin && !dragging {
        mesh.yaw += 0.014;
        mesh.dirty = true;
        ui.ctx().request_repaint();
    }
    if mesh.last_dragging && !dragging {
        mesh.dirty = true;
    }
    mesh.last_dragging = dragging;

    let heavy = mesh.tris.len() > 25_000;
    let ppp = ui.ctx().pixels_per_point();
    let res_scale = ppp * if dragging && heavy { 0.55 } else { 1.0 };
    let target_w = ((rect.width() * res_scale) as usize).clamp(160, 1920);
    let target_h = ((rect.height() * res_scale) as usize).clamp(120, 1200);

    if mesh.dirty || mesh.fb_w != target_w || mesh.fb_h != target_h || mesh.tex.is_none() {
        rasterize_mesh(mesh, target_w, target_h);
        let ci = ColorImage { size: [target_w, target_h], pixels: mesh.color_buf.clone() };
        if let Some(tex) = &mut mesh.tex {
            tex.set(ci, TextureOptions::LINEAR);
        } else {
            mesh.tex = Some(ui.ctx().load_texture("percystar_3d_fb", ci, TextureOptions::LINEAR));
        }
        mesh.dirty = false;
    }

    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(11, 12, 17));
    if let Some(tex) = &mesh.tex {
        p.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }
    p.text(
        egui::pos2(rect.min.x + 10.0, rect.max.y - 16.0),
        egui::Align2::LEFT_BOTTOM,
        "Left-drag: Orbit  ·  Right-drag: Pan  ·  Scroll: Zoom  ·  Double-click: Reset",
        egui::FontId::proportional(11.0),
        Color32::from_rgba_unmultiplied(180, 188, 204, 150),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upright_and_not_mirrored() {
        let verts = vec![[0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0]];
        let mut m = Mesh3D::from_parts("t".into(), verts, vec![], vec![], vec![], vec![], None, String::new(), 1, 0, String::new());
        m.yaw = 0.0;
        m.pitch = 0.4;
        rasterize_mesh(&mut m, 200, 200);
        let p = &m.proj_buf;
        assert!(p[1][1] < p[0][1], "+Z must be screen-up");
        assert!(p[2][0] > 100.0, "+X must be screen-right (no mirror)");
        assert!(p[3][1] < p[4][1], "positive pitch views from above");
        assert!(p[4][2] > p[3][2], "-Y side is closer to camera");
    }

    #[test]
    fn model_name_and_vehicle_filter() {
        let mut buf = vec![0u8; 30_000];
        buf[100..116].copy_from_slice(b"test_vehicle.#ft");
        buf[500..508].copy_from_slice(b"wheel_lf");
        let (nm, ext) = extract_rage9_model_name(&buf).unwrap();
        assert_eq!(nm, "test_vehicle.#ft");
        assert_eq!(ext, ".#ft");
        assert!(is_vehicle_virtual(&buf, &nm));
        assert!(!is_vehicle_virtual(&buf, "test_vehicle_spoiler_01.#ft"));
    }
}
