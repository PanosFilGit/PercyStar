mod awc;
mod model3d;
mod tex;
use model3d::{
    Mesh3D, decode_rage9_model_from_rpf, extract_0drpz_mesh, extract_rage9_model_name,
    is_vehicle_virtual, is_zlib_head, parse_obj, probe_large_rage9_model_in_buf, render_mesh3d_viewport,
};
use tex::{
    DecodedTexture, build_dds_dx10, decode_0drpz_text, decode_navmesh_from_rpf, decode_rbf0,
    decode_ttpf_texture, decode_txd_from_rpf, export_texture_png, inspect_gfx, probe_txd_header,
    scan_ttpf_descriptors,
};
use eframe::egui::{self, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, RichText, Stroke, TextureHandle, TextureOptions};
use flate2::{Decompress, FlushDecompress, Status};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO_RELEASES_URL: &str = "https://github.com/rpatty-dev/PercyStar/releases";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// threshold in levels rpf where vehicle fragtypes begin
const VEH_ZONE_OFF: u64 = 0x10b2_0000_00;

// win32 docking + PlaySound; non-windows builds use the stubs in the other `plat` below
#[cfg(windows)]
mod plat {
use std::path::{Path, PathBuf};

#[link(name = "winmm")]
extern "system" {
    fn PlaySoundW(pszSound: *const u16, hmod: isize, fdwSound: u32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(lpEnumFunc: unsafe extern "system" fn(isize, isize) -> i32, lParam: isize) -> i32;
    fn GetWindowThreadProcessId(hWnd: isize, lpdwProcessId: *mut u32) -> u32;
    fn IsWindowVisible(hWnd: isize) -> i32;
    fn SetParent(hWndChild: isize, hWndNewParent: isize) -> isize;
    fn MoveWindow(hWnd: isize, X: i32, Y: i32, nWidth: i32, nHeight: i32, bRepaint: i32) -> i32;
    fn GetActiveWindow() -> isize;
    fn GetWindowLongW(hWnd: isize, nIndex: i32) -> i32;
    fn SetWindowLongW(hWnd: isize, nIndex: i32, dwNewLong: i32) -> i32;
}

struct EnumCtx {
    pid: u32,
    hwnd: isize,
}

unsafe extern "system" fn enum_cb(hwnd: isize, lparam: isize) -> i32 {
    let ctx = &mut *(lparam as *mut EnumCtx);
    let mut wpid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut wpid);
    if wpid == ctx.pid && IsWindowVisible(hwnd) != 0 {
        ctx.hwnd = hwnd;
        return 0;
    }
    1
}

pub fn find_hwnd_for_pid(pid: u32) -> Option<isize> {
    let mut ctx = EnumCtx { pid, hwnd: 0 };
    unsafe { EnumWindows(enum_cb, &mut ctx as *mut _ as isize); }
    (ctx.hwnd != 0).then_some(ctx.hwnd)
}

// strips popup frame so binkplay docks cleanly insid egui panel
pub fn dock_child_hwnd(child_hwnd: isize, parent_hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    const GWL_STYLE: i32 = -16;
    const WS_CHILD: i32 = 0x4000_0000;
    const WS_POPUP: i32 = -0x8000_0000_i32;
    const WS_CAPTION: i32 = 0x00C0_0000;
    const WS_THICKFRAME: i32 = 0x0004_0000;
    unsafe {
        let style = GetWindowLongW(child_hwnd, GWL_STYLE);
        SetWindowLongW(child_hwnd, GWL_STYLE, (style & !WS_POPUP & !WS_CAPTION & !WS_THICKFRAME) | WS_CHILD);
        SetParent(child_hwnd, parent_hwnd);
        MoveWindow(child_hwnd, x, y, w.max(64), h.max(64), 1);
    }
}

pub fn find_binkplay() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("binkplay.exe")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\RADVideo\binkplay.exe"))
}

pub fn play_wav_async(path: &Path) {
    let mut wide: Vec<u16> = path.to_string_lossy().encode_utf16().collect();
    wide.push(0);
    unsafe { PlaySoundW(wide.as_ptr(), 0, 0x00020000 | 0x0001); }
}

pub fn stop_wav() {
    unsafe { PlaySoundW(std::ptr::null(), 0, 0); }
}

pub fn active_window() -> isize {
    unsafe { GetActiveWindow() }
}

pub fn move_window(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    unsafe { MoveWindow(hwnd, x, y, w, h, 1); }
}
}

// macos/linux: no HWND docking, binkplay launched as a separate window, wav via afplay/aplay
#[cfg(not(windows))]
mod plat {
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;

static WAV_PROC: Mutex<Option<Child>> = Mutex::new(None);

pub fn find_hwnd_for_pid(_pid: u32) -> Option<isize> { None }
pub fn dock_child_hwnd(_c: isize, _p: isize, _x: i32, _y: i32, _w: i32, _h: i32) {}
pub fn active_window() -> isize { 0 }
pub fn move_window(_hwnd: isize, _x: i32, _y: i32, _w: i32, _h: i32) {}

pub fn find_binkplay() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("binkplay")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("binkplay"))
}

pub fn play_wav_async(path: &Path) {
    stop_wav();
    let player = if cfg!(target_os = "macos") { "afplay" } else { "aplay" };
    if let Ok(c) = Command::new(player).arg(path).spawn() {
        *WAV_PROC.lock().unwrap() = Some(c);
    }
}

pub fn stop_wav() {
    if let Some(mut c) = WAV_PROC.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}
}
use plat::{active_window, dock_child_hwnd, find_binkplay, find_hwnd_for_pid, move_window, play_wav_async, stop_wav};

#[derive(Clone, PartialEq, Eq)]
enum Category {
    All,
    Vehicles,
    Models,
    Textures,
    Ui,
    NavMesh,
    Videos,
    Audio,
    Xml,
    Resources,
    Binary,
}

#[derive(Clone, Debug)]
struct RpfEntry {
    source_rpf: PathBuf,
    rpf_name: String,
    name: String,
    kind: &'static str,
    offset: u64,
    comp_size: u64,
    raw_size: u64,
    tracks: u32,
    chunks: Vec<(u64, u64)>,
}

#[derive(Default)]
struct ScanState {
    scanning: bool,
    header_info: String,
    status: String,
    entries: Vec<RpfEntry>,
    rpf_names: Vec<String>,
    file_sig: HashMap<PathBuf, (u64, Option<SystemTime>)>,
    ver: u64,
}

#[derive(Default)]
struct MediaState {
    loading: bool,
    status: String,
    awc_streams: Vec<(u32, PathBuf)>,
    bink_child_pid: Option<u32>,
    bink_hwnd: Option<isize>,
    bink_clip: Option<PathBuf>,
    loaded_mesh: Option<Mesh3D>,
    loaded_textures: Option<Vec<DecodedTexture>>,
}

#[derive(Default)]
struct UpdateState {
    available: Option<(String, String)>,
    checking: bool,
    installing: bool,
    show_modal: bool,
    status: String,
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let trimmed = s.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let maj = parts.next()?.parse::<u32>().ok()?;
    let min = parts.next()?.parse::<u32>().ok()?;
    let pat = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    Some((maj, min, pat))
}

fn silent_cmd(prog: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new(prog);
        c.creation_flags(CREATE_NO_WINDOW);
        c
    }
    #[cfg(not(windows))]
    Command::new(prog)
}

// checks github latest release redirect without api rate limits
fn spawn_update_check(upd: Arc<Mutex<UpdateState>>, ctx: egui::Context) {
    {
        let mut g = upd.lock().unwrap();
        if g.checking || g.installing { return; }
        g.checking = true;
    }
    std::thread::spawn(move || {
        let (curl, null) = if cfg!(windows) { ("curl.exe", "NUL") } else { ("curl", "/dev/null") };
        let out = silent_cmd(curl)
            .args(["-fsSL", "-o", null, "-w", "%{url_effective}", &format!("{REPO_RELEASES_URL}/latest")])
            .output();
        let mut found = None;
        if let Ok(res) = out {
            if res.status.success() {
                let eff = String::from_utf8_lossy(&res.stdout).trim().to_string();
                if let Some((_, tag)) = eff.rsplit_once("/tag/") {
                    if let (Some(rem), Some(cur)) = (parse_semver(tag), parse_semver(APP_VERSION)) {
                        if rem > cur {
                            let ver = tag.trim_start_matches(['v', 'V']).to_string();
                            found = Some((ver, eff));
                        }
                    }
                }
            }
        }
        {
            let mut g = upd.lock().unwrap();
            g.checking = false;
            g.available = found;
        }
        ctx.request_repaint();
    });
}

// downloads new release zip and swaps running windows binary cleanly
fn spawn_install_update(ver: String, upd: Arc<Mutex<UpdateState>>, ctx: egui::Context) {
    {
        let mut g = upd.lock().unwrap();
        if g.installing { return; }
        g.installing = true;
        g.status = format!("Downloading v{ver}...");
    }
    ctx.request_repaint();
    std::thread::spawn(move || {
        let res: Result<(), String> = (|| {
            let tmp = std::env::temp_dir().join("percystar_self_update");
            let _ = fs::remove_dir_all(&tmp);
            fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
            let zip_p = tmp.join("update.zip");
            let dl_url = format!("{REPO_RELEASES_URL}/download/v{ver}/PercyStar_v{ver}_Win64.zip");
            let dl = silent_cmd("curl.exe")
                .args(["-fsSL", "-o", zip_p.to_str().ok_or("bad zip path")?, &dl_url])
                .output()
                .map_err(|e| format!("curl error: {e}"))?;
            if !dl.status.success() {
                return Err(format!("Download failed ({})", dl.status));
            }

            {
                let mut g = upd.lock().unwrap();
                g.status = format!("Extracting v{ver}...");
            }
            ctx.request_repaint();
            let ps_arg = format!(
                "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                zip_p.display(),
                tmp.display()
            );
            let ex = silent_cmd("powershell")
                .args(["-NoProfile", "-Command", &ps_arg])
                .output()
                .map_err(|e| format!("Expand error: {e}"))?;
            if !ex.status.success() {
                return Err("Archive extraction failed".into());
            }

            let new_exe = tmp.join("PercyStar.exe");
            let head = fs::read(&new_exe).map_err(|_| "PercyStar.exe missing in release zip".to_string())?;
            if head.len() < 100_000 || !head.starts_with(b"MZ") {
                return Err("Downloaded binary looks invalid".into());
            }

            let cur_exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let old_exe = cur_exe.with_extension("exe.old");
            let _ = fs::remove_file(&old_exe);
            fs::rename(&cur_exe, &old_exe).map_err(|e| format!("Rename failed: {e}"))?;
            if let Err(e) = fs::copy(&new_exe, &cur_exe) {
                let _ = fs::rename(&old_exe, &cur_exe);
                return Err(format!("Replace failed: {e}"));
            }
            let _ = fs::remove_dir_all(&tmp);
            Command::new(&cur_exe).spawn().map_err(|e| format!("Relaunch failed: {e}"))?;
            std::process::exit(0);
        })();
        if let Err(err) = res {
            let mut g = upd.lock().unwrap();
            g.installing = false;
            g.status = err;
            ctx.request_repaint();
        }
    });
}

struct Rpf9App {
    target_path: Option<PathBuf>,
    is_folder: bool,
    selected_rpf: String,
    state: Arc<Mutex<ScanState>>,
    media: Arc<Mutex<MediaState>>,
    category: Category,
    selected: Option<usize>,
    preview: String,
    filter: String,
    logo_tex: Option<TextureHandle>,
    watch_gen: Arc<Mutex<u64>>,
    bink_proc: Option<Child>,
    parent_hwnd: isize,
    last_dock_rect: [i32; 4],
    clip_seq: u64,
    mesh3d: Option<Mesh3D>,
    show_3d: bool,
    textures: Vec<DecodedTexture>,
    tex_sel: usize,
    tex_handle: Option<(usize, TextureHandle)>,
    tex_zoom: f32,
    vehicles_only: bool,
    // cachd ui list so huge scans never re filter or clone evry fram
    view: Vec<usize>,
    view_key: Option<(u64, String, Category, String, bool)>,
    hay: Vec<String>,
    hay_ver: u64,
    counts: [usize; 11],
    cached_rpf_names: Vec<String>,
    last_rebuild: Instant,
    update: Arc<Mutex<UpdateState>>,
}

impl Rpf9App {
    fn new(cc: &eframe::CreationContext<'_>, logo_rgba: Option<(Vec<u8>, u32, u32)>) -> Self {
        setup_style(&cc.egui_ctx);
        let logo_tex = logo_rgba.map(|(raw, w, h)| {
            let ci = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &raw);
            cc.egui_ctx.load_texture("percystar_logo", ci, TextureOptions::LINEAR)
        });
        let mut app = Self {
            target_path: None,
            is_folder: false,
            selected_rpf: "All RPFs".into(),
            state: Arc::new(Mutex::new(ScanState {
                header_info: "No archive or folder loaded".into(),
                status: "Open an .rpf/.ttpf or select a game folder".into(),
                ..Default::default()
            })),
            media: Arc::new(Mutex::new(MediaState::default())),
            category: Category::All,
            selected: None,
            preview: String::new(),
            filter: String::new(),
            logo_tex,
            watch_gen: Arc::new(Mutex::new(0)),
            bink_proc: None,
            parent_hwnd: 0,
            last_dock_rect: [0; 4],
            clip_seq: 0,
            mesh3d: None,
            show_3d: false,
            textures: Vec::new(),
            tex_sel: 0,
            tex_handle: None,
            tex_zoom: 1.0,
            vehicles_only: false,
            view: Vec::new(),
            view_key: None,
            hay: Vec::new(),
            hay_ver: u64::MAX,
            counts: [0; 11],
            cached_rpf_names: Vec::new(),
            last_rebuild: Instant::now(),
            update: Arc::new(Mutex::new(UpdateState::default())),
        };
        load_ui_state(&mut app);
        spawn_update_check(Arc::clone(&app.update), cc.egui_ctx.clone());
        if let Some(last) = load_last_folder() {
            if last.is_dir() {
                app.start_folder_scan(last);
            }
        }
        app
    }

    fn stop_bink_player(&mut self) {
        if let Some(mut c) = self.bink_proc.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let mut m = self.media.lock().unwrap();
        m.bink_child_pid = None;
        m.bink_hwnd = None;
        m.bink_clip = None;
    }

    fn reset_selection(&mut self) {
        self.selected = None;
        self.preview.clear();
        self.last_dock_rect = [0; 4];
        self.mesh3d = None;
        self.show_3d = false;
        self.textures.clear();
        self.tex_sel = 0;
        self.tex_handle = None;
        self.tex_zoom = 1.0;
        stop_wav();
        self.stop_bink_player();
        let mut m = self.media.lock().unwrap();
        m.awc_streams.clear();
        m.status.clear();
        m.loading = false;
        m.loaded_mesh = None;
        m.loaded_textures = None;
    }

    fn import_catalog_csv(&mut self, csv_path: &Path) {
        let base_dir = self.target_path.as_ref().map(|p| {
            if self.is_folder { p.clone() } else { p.parent().unwrap_or(p).to_path_buf() }
        }).or_else(load_last_folder).unwrap_or_else(|| PathBuf::from(r"D:\Grand Theft Auto VI Build"));

        let mut rpf_map: HashMap<String, PathBuf> = HashMap::new();
        let mut all_paths = Vec::new();
        collect_rpfs(&base_dir, &mut all_paths);
        for p in all_paths {
            if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                rpf_map.insert(fname.to_ascii_lowercase(), p);
            }
        }

        let Ok(f) = File::open(csv_path) else {
            self.state.lock().unwrap().status = format!("Cannot open catalog: {}", csv_path.display());
            return;
        };
        let mut added = 0usize;
        let mut st = self.state.lock().unwrap();
        let mut existing: std::collections::HashSet<(String, u64)> = st.entries.iter()
            .map(|e| (e.rpf_name.to_ascii_lowercase(), e.offset))
            .collect();

        for (li, line) in BufReader::new(f).lines().flatten().enumerate() {
            if li == 0 && line.starts_with("archive,") { continue; }
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() < 4 { continue; }
            let arch = cols[0].trim();
            let off_s = cols[1].trim().trim_start_matches("0x").trim_start_matches("0X");
            let Ok(off) = u64::from_str_radix(off_s, 16) else { continue };
            let raw_name = cols[2].trim();
            let kind_ext = cols[3].trim();
            if raw_name.is_empty() { continue; }

            let arch_file = Path::new(arch).file_name().and_then(|s| s.to_str()).unwrap_or(arch);
            let arch_lo = arch_file.to_ascii_lowercase();
            let src_path = rpf_map.get(&arch_lo).cloned().unwrap_or_else(|| base_dir.join(arch));
            if !existing.insert((arch_lo, off)) { continue; }

            let name = if raw_name.contains(".#") {
                raw_name.to_string()
            } else {
                format!("{raw_name}.#{kind_ext}")
            };
            st.entries.push(RpfEntry {
                source_rpf: src_path,
                rpf_name: arch_file.to_string(),
                name,
                kind: "MODEL",
                offset: off,
                comp_size: 0,
                raw_size: 0,
                tracks: 0,
                chunks: Vec::new(),
            });
            if !st.rpf_names.iter().any(|r| r.eq_ignore_ascii_case(arch_file)) {
                st.rpf_names.push(arch_file.to_string());
            }
            added += 1;
        }
        st.rpf_names.sort();
        st.ver += 1;
        st.status = format!("Imported {added} catalog models ({} total entries)", st.entries.len());
    }

    fn bump_gen(&self) -> u64 {
        let mut g = self.watch_gen.lock().unwrap();
        *g += 1;
        *g
    }

    fn start_single_scan(&mut self, path: PathBuf) {
        self.target_path = Some(path.clone());
        self.is_folder = false;
        self.selected_rpf = "All RPFs".into();
        self.reset_selection();
        let _gen = self.bump_gen();
        {
            let mut st = self.state.lock().unwrap();
            st.scanning = true;
            st.entries.clear();
            st.ver += 1;
            st.rpf_names.clear();
            st.file_sig.clear();
            st.status = "Scanning RPF9 archive...".into();
        }
        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            if path.file_name().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("levels.rpf")).unwrap_or(false) {
                let _ = scan_levels_vehicle_zone(&path, Some(&state));
            }
            match scan_single_rpf(&path, Some(&state)) {
                Ok((hdr, mut entries)) => {
                    let mut st = state.lock().unwrap();
                    st.header_info = hdr;
                    st.rpf_names = vec![path.file_name().unwrap_or_default().to_string_lossy().into_owned()];
                    let n = entries.len();
                    st.entries.append(&mut entries);
                    st.ver += 1;
                    st.scanning = false;
                    st.status = format!("{n} entries ready");
                }
                Err(e) => {
                    let mut st = state.lock().unwrap();
                    st.scanning = false;
                    st.status = format!("Error: {e}");
                }
            }
        });
    }

    fn start_folder_scan(&mut self, folder: PathBuf) {
        if load_last_folder().as_ref() != Some(&folder) {
            self.selected_rpf = "All RPFs".into();
        }
        save_last_folder(&folder);
        self.target_path = Some(folder.clone());
        self.is_folder = true;
        self.reset_selection();
        let gen = self.bump_gen();
        {
            let mut st = self.state.lock().unwrap();
            st.entries.clear();
            st.ver += 1;
            st.rpf_names.clear();
            st.file_sig.clear();
            st.scanning = true;
            st.header_info = format!("Folder: {}", folder.display());
            st.status = "Scanning folder for .rpf archives...".into();
        }

        let state = Arc::clone(&self.state);
        let watch_gen = Arc::clone(&self.watch_gen);
        std::thread::spawn(move || {
            let mut disk_cache = load_disk_cache();
            loop {
                if *watch_gen.lock().unwrap() != gen { break; }
                let mut paths = Vec::new();
                collect_rpfs(&folder, &mut paths);
                let mut rpfs: Vec<(PathBuf, fs::Metadata)> = paths.into_iter().filter_map(|p| fs::metadata(&p).ok().map(|m| (p, m))).collect();

                // scan levels veh zone first so cars show up fast
                if let Some((levels_p, meta)) = rpfs.iter().find(|(p, _)| {
                    p.file_name().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("levels.rpf")).unwrap_or(false)
                }).cloned() {
                    {
                        if meta.len() > VEH_ZONE_OFF {
                            let mtime_s = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                            let vkey = format!("VEHZONE|{}|{}|{}", levels_p.display(), meta.len(), mtime_s);
                            let already_loaded = state.lock().unwrap().entries.iter().any(|e| e.kind == "VEH" && e.source_rpf == levels_p);
                            if !already_loaded {
                                if let Some(cached_vehs) = disk_cache.get(&vkey) {
                                    let mut st = state.lock().unwrap();
                                    st.entries.extend(cached_vehs.iter().cloned());
                                    st.ver += 1;
                                    if !st.rpf_names.contains(&"levels.rpf".to_string()) {
                                        st.rpf_names.push("levels.rpf".into());
                                        st.rpf_names.sort();
                                    }
                                    st.status = format!("Loaded {} cached vehicles!", cached_vehs.len());
                                } else {
                                    state.lock().unwrap().status = "Fast-indexing Vehicles (levels.rpf)...".into();
                                    let veh_entries = scan_levels_vehicle_zone(&levels_p, Some(&state));
                                    if !veh_entries.is_empty() {
                                        {
                                            let mut st = state.lock().unwrap();
                                            if !st.rpf_names.contains(&"levels.rpf".to_string()) {
                                                st.rpf_names.push("levels.rpf".into());
                                                st.rpf_names.sort();
                                            }
                                            st.status = format!("Indexed {} vehicle/mod models!", veh_entries.len());
                                        }
                                        disk_cache.insert(vkey, veh_entries);
                                        save_disk_cache(&disk_cache);
                                    }
                                }
                            }
                        }
                    }
                }

                // smallest archives first so xmls and audio pop in under 2 secs
                rpfs.sort_by_key(|(_, m)| m.len());

                let mut changed = Vec::new();
                let mut cache_hits = 0usize;
                {
                    let mut st = state.lock().unwrap();
                    for (p, meta) in &rpfs {
                        {
                            let mtime_s = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                            let sig = (meta.len(), meta.modified().ok());
                            if st.file_sig.get(p) != Some(&sig) {
                                let key = format!("{}|{}|{}", p.display(), meta.len(), mtime_s);
                                if let Some(cached_entries) = disk_cache.get(&key) {
                                    let is_levels = p.file_name().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("levels.rpf")).unwrap_or(false);
                                    st.entries.retain(|e| &e.source_rpf != p || (is_levels && e.offset >= VEH_ZONE_OFF));
                                    st.entries.extend(cached_entries.iter().cloned());
                                    st.ver += 1;
                                    st.file_sig.insert(p.clone(), sig);
                                    let rname = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                                    if !st.rpf_names.contains(&rname) {
                                        st.rpf_names.push(rname);
                                    }
                                    cache_hits += 1;
                                } else {
                                    changed.push((p.clone(), sig, key));
                                }
                            }
                        }
                    }
                    if cache_hits > 0 {
                        st.rpf_names.sort();
                        st.status = format!("Loaded {cache_hits} cached RPFs ({} entries)...", st.entries.len());
                    }
                }

                if !changed.is_empty() {
                    state.lock().unwrap().scanning = true;
                    let total = changed.len();
                    let next_idx = Arc::new(Mutex::new(0usize));
                    let cache_arc = Arc::new(Mutex::new(disk_cache));
                    let last_save = Arc::new(Mutex::new(Instant::now()));
                    std::thread::scope(|s| {
                        for _ in 0..4 {
                            let state = Arc::clone(&state);
                            let watch_gen = Arc::clone(&watch_gen);
                            let next_idx = Arc::clone(&next_idx);
                            let cache_arc = Arc::clone(&cache_arc);
                            let last_save = Arc::clone(&last_save);
                            let changed = &changed;
                            s.spawn(move || loop {
                                if *watch_gen.lock().unwrap() != gen { return; }
                                let idx = {
                                    let mut g = next_idx.lock().unwrap();
                                    let i = *g;
                                    *g += 1;
                                    i
                                };
                                if idx >= total { return; }
                                let (p, sig, key) = &changed[idx];
                                let rname = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                                state.lock().unwrap().status = format!("Scanning [{}/{}] {}...", idx + 1, total, rname);
                                if let Ok((_, new_entries)) = scan_single_rpf(p, None) {
                                    {
                                        let mut st = state.lock().unwrap();
                                        let is_levels = rname.eq_ignore_ascii_case("levels.rpf");
                                        st.entries.retain(|e| &e.source_rpf != p || (is_levels && e.offset >= VEH_ZONE_OFF));
                                        st.entries.extend(new_entries.iter().cloned());
                                        st.ver += 1;
                                        st.file_sig.insert(p.clone(), *sig);
                                        if !st.rpf_names.contains(&rname) {
                                            st.rpf_names.push(rname);
                                            st.rpf_names.sort();
                                        }
                                    }
                                    let mut c = cache_arc.lock().unwrap();
                                    c.insert(key.clone(), new_entries);
                                    let mut ls = last_save.lock().unwrap();
                                    if ls.elapsed() > Duration::from_secs(5) {
                                        save_disk_cache(&c);
                                        *ls = Instant::now();
                                    }
                                }
                            });
                        }
                    });
                    disk_cache = Arc::try_unwrap(cache_arc).unwrap().into_inner().unwrap();
                    save_disk_cache(&disk_cache);
                    let mut st = state.lock().unwrap();
                    st.scanning = false;
                    st.status = format!("{} RPFs · {} entries (cached + auto-updating)", st.rpf_names.len(), st.entries.len());
                } else {
                    let mut st = state.lock().unwrap();
                    st.scanning = false;
                    if cache_hits > 0 {
                        st.status = format!("{} RPFs · {} entries (instant cache)", st.rpf_names.len(), st.entries.len());
                    }
                }

                for _ in 0..20 {
                    if *watch_gen.lock().unwrap() != gen { return; }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        });
    }

    fn select_entry(&mut self, idx: usize) {
        let Some(e) = self.state.lock().unwrap().entries.get(idx).cloned() else { return };
        self.reset_selection();
        self.selected = Some(idx);

        if e.kind == "MODEL" || e.kind == "VEH" {
            let media = Arc::clone(&self.media);
            {
                let mut m = media.lock().unwrap();
                m.loading = true;
                m.status = format!("Decoding RAGE9 meshlets: {}...", e.name);
            }
            let src = e.source_rpf.clone();
            let off = e.offset;
            let nm = e.name.clone();
            std::thread::spawn(move || {
                match decode_rage9_model_from_rpf(&src, off, &nm) {
                    Ok(mesh) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status.clear();
                        m.loaded_mesh = Some(mesh);
                    }
                    Err(err) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status = format!("Model decode error ({nm}): {err}");
                    }
                }
            });
            return;
        }

        if e.kind == "NAVM" {
            let media = Arc::clone(&self.media);
            {
                let mut m = media.lock().unwrap();
                m.loading = true;
                m.status = format!("Decoding CNavMesh tile: {}...", e.name);
            }
            let src = e.source_rpf.clone();
            let off = e.offset;
            let nm = e.name.clone();
            std::thread::spawn(move || {
                match decode_navmesh_from_rpf(&src, off, &nm) {
                    Ok(mesh) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status.clear();
                        m.loaded_mesh = Some(mesh);
                    }
                    Err(err) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status = format!("NavMesh decode error ({nm}): {err}");
                    }
                }
            });
            return;
        }

        if e.kind == "TXD" {
            let media = Arc::clone(&self.media);
            {
                let mut m = media.lock().unwrap();
                m.loading = true;
                m.status = format!("Deswizzling Xbox Scarlett TXD: {}...", e.name);
            }
            let src = e.source_rpf.clone();
            let off = e.offset;
            let nm = e.name.clone();
            std::thread::spawn(move || {
                match decode_txd_from_rpf(&src, off) {
                    Ok(texs) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status.clear();
                        m.loaded_textures = Some(texs);
                    }
                    Err(err) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status = format!("TXD decode error ({nm}): {err}");
                    }
                }
            });
            return;
        }

        if e.kind == "TTPF" {
            let media = Arc::clone(&self.media);
            {
                let mut m = media.lock().unwrap();
                m.loading = true;
                m.status = format!("Decoding RDS5/ETPF texture: {}...", e.name);
            }
            let src = e.source_rpf.clone();
            let tidx = e.tracks as usize;
            let nm = e.name.clone();
            std::thread::spawn(move || {
                match decode_ttpf_texture(&src, tidx) {
                    Ok(tex) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status.clear();
                        m.loaded_textures = Some(vec![tex]);
                    }
                    Err(err) => {
                        let mut m = media.lock().unwrap();
                        m.loading = false;
                        m.status = format!("TTPF decode error ({nm}): {err}");
                    }
                }
            });
            return;
        }

        if e.kind == "BK2" {
            let tmp = std::env::temp_dir().join("rpf9_preview_bk2");
            let _ = fs::create_dir_all(&tmp);
            if let Ok(rd) = fs::read_dir(&tmp) {
                for old in rd.flatten() {
                    let _ = fs::remove_file(old.path());
                }
            }
            // bump seq name so windows file locks never block switching vids
            self.clip_seq += 1;
            let bk2_p = tmp.join(format!("clip_{}.bk2", self.clip_seq));
            match read_entry_bytes(&e) {
                Ok(bytes) => {
                    if let Err(err) = fs::write(&bk2_p, &bytes) {
                        self.media.lock().unwrap().status = format!("Write error: {err}");
                        return;
                    }
                }
                Err(err) => {
                    self.media.lock().unwrap().status = format!("Read error: {err}");
                    return;
                }
            }
            match Command::new(find_binkplay())
                .args([bk2_p.to_str().unwrap(), "/NoBorder", "/DrawD3D11", "/L", "/W640", "/H360"])
                .spawn()
            {
                Ok(child) => {
                    let pid = child.id();
                    self.bink_proc = Some(child);
                    let mut m = self.media.lock().unwrap();
                    m.bink_child_pid = Some(pid);
                    m.bink_clip = Some(bk2_p);
                    m.status = format!("Playing {} (D3D11 BinkPlay)", e.name);
                }
                Err(err) => {
                    self.media.lock().unwrap().status = format!("binkplay.exe error: {err}");
                }
            }
        } else if e.kind == "AWC" {
            let media = Arc::clone(&self.media);
            {
                let mut m = media.lock().unwrap();
                m.loading = true;
                m.status = format!("Decoding {} Opus streams...", e.name);
            }
            std::thread::spawn(move || decode_awc_for_player(&e, &media));
        } else {
            match read_entry_bytes(&e) {
                Ok(data) => {
                    let sample = &data[..data.len().min(200)];
                    let is_text = e.kind == "XML"
                        || (!sample.is_empty()
                            && sample.iter().all(|&b| (32..127).contains(&b) || matches!(b, 9 | 10 | 13 | 0x80..)));
                    if e.kind == "PSIN" {
                        self.preview = parse_psin(&data);
                    } else if e.kind == "RBF0" {
                        self.preview = decode_rbf0(&data).unwrap_or_else(|| format_hex(&data[..data.len().min(2048)]));
                    } else if e.kind == "GFX" {
                        self.preview = inspect_gfx(&data);
                    } else if e.kind == "GXT" {
                        self.preview = decode_0drpz_text(&data).unwrap_or_else(|| format_hex(&data[..data.len().min(2048)]));
                    } else if is_text {
                        self.preview = String::from_utf8_lossy(&data[..data.len().min(250_000)]).into_owned();
                    } else {
                        if let Some(txt) = decode_0drpz_text(&data) {
                            self.preview = txt;
                        } else {
                            if data.len() >= 24 && &data[16..21] == b"0DRPz" {
                                self.mesh3d = extract_0drpz_mesh(&e.name, &data);
                            }
                            self.preview = format_hex(&data[..data.len().min(2048)]);
                        }
                    }
                }
                Err(err) => self.preview = format!("Read error: {err}"),
            }
        }
    }

    // throtled view rebuilder runs 4x a sec during live scans
    fn refresh_view(&mut self) {
        let ver = self.state.lock().unwrap().ver;
        let key = (ver, self.filter.clone(), self.category.clone(), self.selected_rpf.clone(), self.vehicles_only);
        if let Some(old) = &self.view_key {
            if *old == key { return; }
            let only_ver = old.1 == key.1 && old.2 == key.2 && old.3 == key.3 && old.4 == key.4;
            if only_ver && self.last_rebuild.elapsed() < Duration::from_millis(1000) {
                return;
            }
            if !only_ver {
                save_ui_state(self);
            }
        }
        self.rebuild_view(key);
    }

    fn rebuild_view(&mut self, key: (u64, String, Category, String, bool)) {
        let st = self.state.lock().unwrap();
        if self.hay_ver != st.ver {
            self.hay = st.entries.iter().map(|e| format!("{}|{}|{}", e.name, e.rpf_name, e.kind).to_lowercase()).collect();
            self.cached_rpf_names = st.rpf_names.clone();
            self.hay_ver = st.ver;
            let mut c = [0usize; 11];
            for e in &st.entries {
                c[0] += 1;
                c[match e.kind {
                    "VEH" => 1,
                    "MODEL" => 2,
                    "TXD" | "TTPF" => 3,
                    "RBF0" | "GFX" | "GXT" => 4,
                    "NAVM" => 5,
                    "BK2" => 6,
                    "AWC" => 7,
                    "XML" => 8,
                    "PSIN" => 9,
                    _ => 10,
                }] += 1;
            }
            self.counts = c;
        }
        let q = key.1.to_lowercase();
        let (inc, exc): (Vec<&str>, Vec<&str>) = q.split_whitespace().partition(|t| !t.starts_with('-'));
        let exc: Vec<&str> = exc.iter().map(|t| &t[1..]).filter(|t| !t.is_empty()).collect();
        let all_rpf = self.selected_rpf == "All RPFs";
        let mut view: Vec<usize> = st
            .entries
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                let cat_ok = match self.category {
                    Category::All => true,
                    Category::Vehicles => e.kind == "VEH",
                    Category::Models => e.kind == "VEH" || (!self.vehicles_only && e.kind == "MODEL"),
                    Category::Textures => e.kind == "TXD" || e.kind == "TTPF",
                    Category::Ui => e.kind == "RBF0" || e.kind == "GFX" || e.kind == "GXT",
                    Category::NavMesh => e.kind == "NAVM",
                    Category::Videos => e.kind == "BK2",
                    Category::Audio => e.kind == "AWC",
                    Category::Xml => e.kind == "XML",
                    Category::Resources => e.kind == "PSIN",
                    Category::Binary => e.kind == "ZLIB",
                };
                let h = &self.hay[*i];
                cat_ok
                    && (all_rpf || e.rpf_name == self.selected_rpf)
                    && inc.iter().all(|t| h.contains(t))
                    && !exc.iter().any(|t| h.contains(t))
            })
            .map(|(i, _)| i)
            .collect();
        drop(st);
        if let Some(first) = inc.first() {
            view.sort_by_key(|&i| (!self.hay[i].starts_with(first), self.hay[i].len()));
        }
        self.view = view;
        self.view_key = Some(key);
        self.last_rebuild = Instant::now();
    }

    fn export_entries(&self, indices: &[usize]) {
        let Some(out_dir) = rfd::FileDialog::new().pick_folder() else { return };
        let entries: Vec<RpfEntry> = {
            let st = self.state.lock().unwrap();
            indices.iter().filter_map(|&i| st.entries.get(i).cloned()).collect()
        };
        let mut ok = 0usize;
        for e in entries {
            if e.kind == "MODEL" || e.kind == "VEH" {
                if let Ok(mesh) = decode_rage9_model_from_rpf(&e.source_rpf, e.offset, &e.name) {
                    let fname = format!("{}.obj", e.name.replace('#', ""));
                    if mesh.export_obj(&out_dir.join(fname)).is_ok() {
                        ok += 1;
                    }
                }
            } else if e.kind == "NAVM" {
                if let Ok(mesh) = decode_navmesh_from_rpf(&e.source_rpf, e.offset, &e.name) {
                    let fname = format!("{}.obj", e.name.replace('#', ""));
                    if mesh.export_obj(&out_dir.join(fname)).is_ok() {
                        ok += 1;
                    }
                }
            } else if e.kind == "TXD" {
                if let Ok(texs) = decode_txd_from_rpf(&e.source_rpf, e.offset) {
                    for t in &texs {
                        let safe: String = t.name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') { c } else { '_' }).collect();
                        if export_texture_png(t, &out_dir.join(format!("{safe}.png"))).is_ok() {
                            ok += 1;
                        }
                    }
                }
            } else if e.kind == "TTPF" {
                if let Ok(t) = decode_ttpf_texture(&e.source_rpf, e.tracks as usize) {
                    if export_texture_png(&t, &out_dir.join(format!("{}.png", t.name))).is_ok() {
                        ok += 1;
                    }
                }
            } else if e.kind == "RBF0" {
                if let Ok(data) = read_entry_bytes(&e) {
                    let xml = decode_rbf0(&data).unwrap_or_else(|| String::from_utf8_lossy(&data).into_owned());
                    let fname = format!("{}.xml", e.name.trim_end_matches(".rbf"));
                    if fs::write(out_dir.join(fname), xml).is_ok() {
                        ok += 1;
                    }
                }
            } else if let Ok(data) = read_entry_bytes(&e) {
                let fname = if self.is_folder { format!("{}_{}", e.rpf_name.trim_end_matches(".rpf"), e.name) } else { e.name.clone() };
                if fs::write(out_dir.join(fname), &data).is_ok() {
                    ok += 1;
                }
            }
        }
        self.state.lock().unwrap().status = format!("Exported {ok} file(s) to {}", out_dir.display());
    }
}

fn app_data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var("LOCALAPPDATA").ok().map(|d| PathBuf::from(d).join("PercyStar"))
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME").ok().map(|d| PathBuf::from(d).join("Library/Application Support/PercyStar"))
    } else {
        std::env::var("HOME").ok().map(|d| PathBuf::from(d).join(".local/share/PercyStar"))
    }
}

fn cache_file_path() -> Option<PathBuf> {
    app_data_dir().map(|d| d.join("scan_cache_v8.tsv"))
}

fn load_last_folder() -> Option<PathBuf> {
    let s = fs::read_to_string(app_data_dir()?.join("last_folder.txt")).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| PathBuf::from(t))
}

fn save_last_folder(folder: &Path) {
    if let Some(dir) = app_data_dir() {
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(dir.join("last_folder.txt"), folder.display().to_string());
    }
}

const CATS: [Category; 11] = [
    Category::All, Category::Vehicles, Category::Models, Category::Textures,
    Category::Ui, Category::NavMesh, Category::Videos, Category::Audio,
    Category::Xml, Category::Resources, Category::Binary,
];

fn save_ui_state(app: &Rpf9App) {
    let Some(dir) = app_data_dir() else { return };
    let _ = fs::create_dir_all(&dir);
    let cat = CATS.iter().position(|c| *c == app.category).unwrap_or(0);
    let _ = fs::write(dir.join("ui_state.txt"), format!("{cat}\n{}\n{}\n{}", app.vehicles_only as u8, app.selected_rpf, app.filter));
}

fn load_ui_state(app: &mut Rpf9App) {
    let Some(s) = app_data_dir().and_then(|d| fs::read_to_string(d.join("ui_state.txt")).ok()) else { return };
    let mut l = s.splitn(4, '\n');
    if let Some(c) = l.next().and_then(|x| x.parse::<usize>().ok()).and_then(|i| CATS.get(i)) {
        app.category = c.clone();
    }
    app.vehicles_only = l.next() == Some("1");
    if let Some(r) = l.next().filter(|r| !r.is_empty()) {
        app.selected_rpf = r.into();
    }
    if let Some(f) = l.next() {
        app.filter = f.into();
    }
}

fn load_disk_cache() -> HashMap<String, Vec<RpfEntry>> {
    let mut map: HashMap<String, Vec<RpfEntry>> = HashMap::new();
    let Some(p) = cache_file_path() else { return map };
    let Ok(f) = File::open(p) else { return map };
    for line in BufReader::new(f).lines().flatten() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 9 { continue; }
        let kind: &'static str = match cols[4] {
            "VEH" => "VEH",
            "MODEL" => "MODEL",
            "TXD" => "TXD",
            "TTPF" => "TTPF",
            "RBF0" => "RBF0",
            "GFX" => "GFX",
            "GXT" => "GXT",
            "NAVM" => "NAVM",
            "BK2" => "BK2",
            "AWC" => "AWC",
            "XML" => "XML",
            "PSIN" => "PSIN",
            _ => "ZLIB",
        };
        // tolerate old 11 col cache rows and newer 10 col ones
        let tracks_col = if cols.len() >= 11 { 9 } else { 8 };
        let chunks_col = tracks_col + 1;
        let chunks = if cols.len() > chunks_col && !cols[chunks_col].is_empty() {
            cols[chunks_col].split(';').filter_map(|pair| {
                let mut it = pair.split(',');
                Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
            }).collect()
        } else {
            Vec::new()
        };
        map.entry(cols[0].to_string()).or_default().push(RpfEntry {
            source_rpf: PathBuf::from(cols[1]),
            rpf_name: cols[2].to_string(),
            name: cols[3].to_string(),
            kind,
            offset: cols[5].parse().unwrap_or(0),
            comp_size: cols[6].parse().unwrap_or(0),
            raw_size: cols[7].parse().unwrap_or(0),
            tracks: cols[tracks_col].parse().unwrap_or(0),
            chunks,
        });
    }
    map
}

// atomic write via tmp file so ctrl c never corrupts the cahce
fn save_disk_cache(map: &HashMap<String, Vec<RpfEntry>>) {
    let Some(p) = cache_file_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = p.with_extension("tmp");
    let Ok(f) = File::create(&tmp) else { return };
    let mut w = BufWriter::new(f);
    for (key, entries) in map {
        for e in entries {
            let chunks_s: String = e.chunks.iter().map(|(a, b)| format!("{a},{b}")).collect::<Vec<_>>().join(";");
            let _ = writeln!(
                w,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                key, e.source_rpf.display(), e.rpf_name, e.name, e.kind,
                e.offset, e.comp_size, e.raw_size, e.tracks, chunks_s
            );
        }
    }
    if w.flush().is_ok() {
        drop(w);
        let _ = fs::rename(&tmp, &p);
    }
}

fn collect_rpfs(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            // skip .Trashes/.Spotlight and SMB "._x.rpf" AppleDouble forks
            if entry.file_name().as_encoded_bytes().first() == Some(&b'.') { continue; }
            // d_type from readdir: no extra stat per entry over SMB, and symlinks are not followed
            let Ok(ft) = entry.file_type() else { continue };
            let p = entry.path();
            if ft.is_dir() {
                collect_rpfs(&p, out);
            } else if ft.is_file() && p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("rpf") || s.eq_ignore_ascii_case("ttpf")).unwrap_or(false) {
                out.push(p);
            }
        }
    }
}

fn decode_awc_for_player(e: &RpfEntry, media: &Arc<Mutex<MediaState>>) {
    let tmp = std::env::temp_dir().join("rpf9_preview_awc");
    let _ = fs::remove_dir_all(&tmp);
    let wavs = match read_entry_bytes(e) {
        Ok(bytes) => awc::decode_awc_bytes_to_wavs(&bytes, &tmp, "stream"),
        Err(_) => Vec::new(),
    };
    let mut m = media.lock().unwrap();
    m.loading = false;
    m.status = format!("Decoded {} playable .wav stream(s) natively", wavs.len());
    m.awc_streams = wavs;
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut font_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        font_dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
    }
    if let Ok(home) = std::env::var("HOME") {
        font_dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }
    font_dirs.push(PathBuf::from("/Library/Fonts"));
    'dirs: for base in font_dirs {
        for name in ["SF-Pro-Display-Regular.otf", "SF-Pro-Display-Medium.otf", "SF-Pro-Display-Semibold.otf"] {
            if let Ok(bytes) = fs::read(base.join(name)) {
                fonts.font_data.insert("sf_pro".into(), Arc::new(FontData::from_owned(bytes)));
                fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "sf_pro".into());
                break 'dirs;
            }
        }
    }
    ctx.set_fonts(fonts);

    // neon sunset theme matching dusk purple magenta and golden amber
    let mut vis = egui::Visuals::dark();
    vis.panel_fill = Color32::from_rgb(18, 12, 28);
    vis.window_fill = Color32::from_rgb(24, 16, 36);
    vis.extreme_bg_color = Color32::from_rgb(12, 8, 20);
    vis.widgets.inactive.bg_fill = Color32::from_rgb(34, 22, 50);
    vis.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(68, 42, 98));
    vis.widgets.inactive.corner_radius = CornerRadius::same(6);
    vis.widgets.hovered.bg_fill = Color32::from_rgb(56, 34, 82);
    vis.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(255, 72, 165));
    vis.widgets.hovered.corner_radius = CornerRadius::same(6);
    vis.widgets.active.bg_fill = Color32::from_rgb(255, 168, 28);
    vis.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::BLACK);
    vis.widgets.active.corner_radius = CornerRadius::same(6);
    vis.selection.bg_fill = Color32::from_rgba_unmultiplied(255, 72, 165, 58);
    vis.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(255, 168, 28));
    ctx.set_visuals(vis);
}

fn draw_percystar_spinner(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let p = ui.painter();
    let c = rect.center();
    p.circle_stroke(c, size * 0.44, Stroke::new(2.0_f32, Color32::from_rgb(255, 85, 175)));
    let mut pts = Vec::with_capacity(10);
    for i in 0..10 {
        let ang = t * 3.5 + (i as f32) * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
        let r = if i % 2 == 0 { size * 0.34 } else { size * 0.14 };
        pts.push(egui::pos2(c.x + r * ang.cos(), c.y + r * ang.sin()));
    }
    p.add(egui::Shape::convex_polygon(pts, Color32::from_rgb(252, 175, 23), Stroke::NONE));
    ui.ctx().request_repaint();
}

fn kind_color(kind: &str) -> Color32 {
    match kind {
        "VEH" => Color32::from_rgb(255, 85, 175),
        "MODEL" => Color32::from_rgb(130, 175, 255),
        "TXD" => Color32::from_rgb(64, 235, 225),
        "TTPF" => Color32::from_rgb(48, 205, 255),
        "RBF0" => Color32::from_rgb(255, 145, 72),
        "GFX" => Color32::from_rgb(255, 115, 130),
        "GXT" => Color32::from_rgb(255, 225, 95),
        "NAVM" => Color32::from_rgb(125, 235, 110),
        "XML" => Color32::from_rgb(72, 210, 255),
        "AWC" => Color32::from_rgb(90, 235, 165),
        "BK2" => Color32::from_rgb(252, 175, 23),
        "PSIN" => Color32::from_rgb(210, 120, 255),
        _ => Color32::from_rgb(145, 152, 168),
    }
}

fn parse_psin(data: &[u8]) -> String {
    if data.len() < 16 || &data[..4] != b"PSIN" {
        return format_hex(&data[..data.len().min(2048)]);
    }
    let mut out = String::new();
    let data_sz = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let root_hash = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let flags = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    out.push_str("=== RAGE PSIN (PSO Serialized Metadata) ===\n");
    out.push_str(&format!("Total Size: {} B | PSIN Block: {} B | RootHash: 0x{:08x} | Flags: 0x{:08x}\n\n", data.len(), data_sz, root_hash, flags));
    out.push_str("--- PSO Sections ---\n");
    for tag in [b"LSCH", b"LENM", b"LSNS", b"LGID", b"LRSC", b"CHKS", b"PSCH", b"PMAP", b"STRF", b"STRS"] {
        let mut pos = 0usize;
        while let Some(rel) = data[pos..].windows(4).position(|w| w == tag) {
            let off = pos + rel;
            let sz = if off + 8 <= data.len() { u32::from_be_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]]) } else { 0 };
            out.push_str(&format!("  {} @ 0x{:06x} (be_len={})\n", std::str::from_utf8(tag).unwrap_or("?"), off, sz));
            pos = off + 4;
        }
    }
    out.push_str("\n--- Readable Strings ---\n");
    let mut cur = Vec::new();
    let mut count = 0usize;
    for &b in data {
        if (32..127).contains(&b) {
            cur.push(b);
        } else if !cur.is_empty() {
            if cur.len() >= 4 && !cur.iter().all(|&c| c == b'p' || c == b'a') {
                out.push_str(&format!("  {}\n", String::from_utf8_lossy(&cur)));
                count += 1;
                if count >= 120 { break; }
            }
            cur.clear();
        }
    }
    out.push_str("\n--- Header Hex (first 512 B) ---\n");
    out.push_str(&format_hex(&data[..data.len().min(512)]));
    out
}

fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_073_741_824.0 {
        format!("{:.2} GB", b / 1_073_741_824.0)
    } else if b >= 1_048_576.0 {
        format!("{:.1} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_hex(data: &[u8]) -> String {
    let mut out = String::new();
    for (row, chunk) in data.chunks(16).enumerate() {
        out.push_str(&format!("{:04x}: ", row * 16));
        for b in chunk { out.push_str(&format!("{b:02x} ")); }
        for _ in chunk.len()..16 { out.push_str("   "); }
        out.push(' ');
        for &b in chunk {
            out.push(if (32..127).contains(&b) { b as char } else { '.' });
        }
        out.push('\n');
    }
    out
}

fn read_entry_bytes(e: &RpfEntry) -> std::io::Result<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    let mut f = File::open(&e.source_rpf)?;
    if !e.chunks.is_empty() {
        let mut combined = Vec::new();
        for &(coff, clen) in &e.chunks {
            f.seek(SeekFrom::Start(coff))?;
            let mut raw = vec![0u8; clen as usize];
            f.read_exact(&mut raw)?;
            ZlibDecoder::new(&raw[..]).read_to_end(&mut combined)?;
        }
        Ok(combined)
    } else {
        f.seek(SeekFrom::Start(e.offset))?;
        let mut raw = vec![0u8; e.comp_size as usize];
        f.read_exact(&mut raw)?;
        if matches!(e.kind, "XML" | "PSIN" | "RBF0" | "GFX" | "GXT" | "ZLIB") {
            let mut out = Vec::new();
            ZlibDecoder::new(&raw[..]).read_to_end(&mut out)?;
            Ok(out)
        } else {
            Ok(raw)
        }
    }
}

fn extract_xml_tag(bytes: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(&bytes[..bytes.len().min(300)]).ok()?;
    for part in s.split('<').skip(1) {
        let tag = part.split(|c: char| c.is_whitespace() || c == '>').next()?;
        if !tag.starts_with('?') && !tag.starts_with('!') && !tag.starts_with('/') && tag.len() >= 3 && tag != "Item" {
            return Some(tag.chars().filter(|c| c.is_alphanumeric() || *c == '_').take(36).collect());
        }
    }
    None
}

// shared rage9 page detector used by both veh scanner and full archive pass
fn probe_rage9_page(
    buf: &[u8],
    pos: usize,
    read_n: usize,
    off: u64,
    path: &Path,
    rname: &str,
    probe_dec: &mut Decompress,
    probe_out: &mut [u8],
) -> Option<RpfEntry> {
    let p = pos + 0x10;
    if p + 8 > read_n || !is_zlib_head(&buf[p..read_n]) {
        return None;
    }
    let end = (p + 32768).min(read_n);
    probe_dec.reset(true);
    let _ = probe_dec.decompress(&buf[p..end], probe_out, FlushDecompress::None);
    let olen = probe_dec.total_out() as usize;
    if olen < 0x80 { return None; }
    let vft = u64::from_le_bytes(probe_out[0..8].try_into().ok()?);
    let bmap = u64::from_le_bytes(probe_out[8..16].try_into().ok()?);
    if vft != 0 || !(0x5000_0000..0x5800_0000).contains(&bmap) || &probe_out[0x10..0x15] == b"0DRPz" {
        return None;
    }
    // 1. GTA VI CNavMesh tile (version 0x00010015)
    let ver = u32::from_le_bytes(probe_out[0x14..0x18].try_into().ok()?);
    if ver == 0x0001_0015 && olen >= 0xD0 {
        let n_polys = u32::from_le_bytes(probe_out[0xC4..0xC8].try_into().ok()?);
        let midx = u32::from_le_bytes(probe_out[0xC8..0xCC].try_into().ok()?);
        let nm = if midx < 512 * 512 {
            let gx = (midx % 512) as i32;
            let gy = (midx / 512) as i32;
            let wx = (gx - 256) * 128;
            let wy = (gy - 256) * 128;
            format!("nav_{gx:03}_{gy:03}_({wx},{wy}).ynv")
        } else {
            format!("nav_dyn_{midx:06x}.ynv")
        };
        return Some(RpfEntry {
            source_rpf: path.to_path_buf(),
            rpf_name: rname.to_string(),
            name: nm,
            kind: "NAVM",
            offset: off + p as u64,
            comp_size: probe_dec.total_in(),
            raw_size: olen as u64,
            tracks: n_polys,
            chunks: Vec::new(),
        });
    }
    // 2. pgDictionary<grcTexture> (.#td texture dictionary)
    if let Some((txd_name, tex_cnt)) = probe_txd_header(&probe_out[..olen]) {
        return Some(RpfEntry {
            source_rpf: path.to_path_buf(),
            rpf_name: rname.to_string(),
            name: txd_name,
            kind: "TXD",
            offset: off + p as u64,
            comp_size: probe_dec.total_in(),
            raw_size: olen as u64,
            tracks: tex_cnt as u32,
            chunks: Vec::new(),
        });
    }
    // 3. Drawable / FragType 3D Model (.#dr / .#ft / .#dd)
    let (mname, is_veh, vlen) = if (bmap - 0x5000_0000) >= 32768 {
        probe_large_rage9_model_in_buf(&buf[p..read_n], path, off + p as u64)?
    } else {
        let (mname, _) = extract_rage9_model_name(&probe_out[..olen])?;
        (mname.clone(), is_vehicle_virtual(&probe_out[..olen], &mname), olen)
    };
    Some(RpfEntry {
        source_rpf: path.to_path_buf(),
        rpf_name: rname.to_string(),
        name: mname,
        kind: if is_veh { "VEH" } else { "MODEL" },
        offset: off + p as u64,
        comp_size: probe_dec.total_in(),
        raw_size: vlen as u64,
        tracks: 0,
        chunks: Vec::new(),
    })
}

// single 16mb sequntial stream avoids hdd seek thrash while indexing vehicles
fn scan_levels_vehicle_zone(levels_rpf: &PathBuf, live_state: Option<&Arc<Mutex<ScanState>>>) -> Vec<RpfEntry> {
    let sz = fs::metadata(levels_rpf).map(|m| m.len()).unwrap_or(0);
    let start_off = VEH_ZONE_OFF.min(sz);
    let total_len = sz.saturating_sub(start_off);
    if total_len == 0 { return Vec::new(); }
    let rname = levels_rpf.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let Ok(mut f) = File::open(levels_rpf) else { return Vec::new() };
    if f.seek(SeekFrom::Start(start_off)).is_err() { return Vec::new(); }

    let mut all_entries: Vec<RpfEntry> = Vec::new();
    let mut local_batch: Vec<RpfEntry> = Vec::with_capacity(32);
    let mut buf = vec![0u8; 16 * 1024 * 1024];
    let mut probe_dec = Decompress::new(true);
    let mut probe_out = vec![0u8; 32768];
    let mut off = start_off;
    let mut last_pct = 999u64;

    while off < sz {
        let Ok(n) = f.read(&mut buf) else { break };
        if n == 0 { break; }
        let mut pos = 0usize;
        while pos + 44 <= n {
            if let Some(entry) = probe_rage9_page(&buf, pos, n, off, levels_rpf, &rname, &mut probe_dec, &mut probe_out) {
                local_batch.push(entry);
                if local_batch.len() >= 15 {
                    if let Some(st) = live_state {
                        let mut g = st.lock().unwrap();
                        g.entries.extend(local_batch.iter().cloned());
                        g.ver += 1;
                    }
                    all_entries.extend(local_batch.drain(..));
                }
            }
            pos += 0x1000;
        }
        off += n as u64;
        if let Some(st) = live_state {
            let pct = ((off - start_off) * 100 / total_len).min(100);
            if pct != last_pct {
                last_pct = pct;
                let mut g = st.lock().unwrap();
                let vcnt = g.entries.iter().filter(|e| e.kind == "VEH").count();
                g.status = format!("Fast-indexing Vehicles: {pct}% ({vcnt} vehicles found)...");
            }
        }
    }
    if !local_batch.is_empty() {
        if let Some(st) = live_state {
            let mut g = st.lock().unwrap();
            g.entries.extend(local_batch.iter().cloned());
            g.ver += 1;
        }
        all_entries.extend(local_batch);
    }
    all_entries
}

fn scan_single_rpf(path: &PathBuf, progress_state: Option<&Arc<Mutex<ScanState>>>) -> std::io::Result<(String, Vec<RpfEntry>)> {
    if path.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("ttpf")).unwrap_or(false) {
        let rname = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let (info, descs) = scan_ttpf_descriptors(path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(st) = progress_state {
            st.lock().unwrap().header_info = info.clone();
        }
        let entries: Vec<RpfEntry> = descs.into_iter().map(|d| {
            let fnm = tex::dxgi_info(d.dxgi).map(|x| x.0).unwrap_or("UNK");
            RpfEntry {
                source_rpf: path.clone(),
                rpf_name: rname.clone(),
                name: format!("tex_{:05}_{}x{}_{fnm}{}.ttpf", d.idx, d.w, d.h, if d.has_mips { "_mip" } else { "" }),
                kind: "TTPF",
                offset: d.byte_off,
                comp_size: d.alloc_sz,
                raw_size: d.alloc_sz,
                tracks: d.idx as u32,
                chunks: Vec::new(),
            }
        }).collect();
        return Ok((info, entries));
    }

    let mut f = File::open(path)?;
    let sz = f.metadata()?.len();
    let rname = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut hdr = [0u8; 64];
    let n = f.read(&mut hdr)?;
    let magic = String::from_utf8_lossy(&hdr[..n.min(4)]).into_owned();
    let hdr_hex: String = hdr[4..n.min(16)].iter().map(|b| format!("{b:02x}")).collect();
    let hdr_info = format!("{magic}  ·  {sz} B  ·  {hdr_hex}");
    if let Some(st) = progress_state {
        st.lock().unwrap().header_info = hdr_info.clone();
    }

    // <=512mb: one sequential read serves both passes (was read twice, painful over smb)
    let whole: Option<Vec<u8>> = if sz <= 512 * 1024 * 1024 {
        f.seek(SeekFrom::Start(0))?;
        let mut d = Vec::with_capacity(sz as usize);
        f.read_to_end(&mut d)?;
        Some(d)
    } else {
        None
    };
    let mut found = Vec::new();
    let mut off = 0u64;
    let mut buf = if whole.is_some() { Vec::new() } else { vec![0u8; 16 * 1024 * 1024] };
    let mut probe_dec = Decompress::new(true);
    let mut probe_out = vec![0u8; 32768];
    let mut bk2_idx = 0usize;
    let mut awc_idx = 0usize;
    let mut last_ui_mb = 0u64;
    let mut need_seek = true;
    let scan_limit = if rname.eq_ignore_ascii_case("levels.rpf") && sz > VEH_ZONE_OFF { VEH_ZONE_OFF } else { sz };

    while off < scan_limit {
        if let Some(st) = progress_state {
            let cur_mb = off / (64 * 1024 * 1024);
            if cur_mb != last_ui_mb {
                last_ui_mb = cur_mb;
                st.lock().unwrap().status = format!("Scanning {:.2} / {:.2} GB ({} found)...", off as f64 / 1e9, sz as f64 / 1e9, found.len());
            }
        }
        let read_n;
        let buf: &[u8] = if let Some(w) = &whole {
            let s = &w[(off as usize).min(w.len())..];
            read_n = s.len();
            s
        } else {
            if need_seek {
                f.seek(SeekFrom::Start(off))?;
                need_seek = false;
            }
            read_n = f.read(&mut buf)?;
            &buf[..read_n]
        };
        if read_n == 0 { break; }
        let mut pos = 0usize;
        let mut jumped = false;
        while pos + 44 <= read_n {
            if let Some(entry) = probe_rage9_page(buf, pos, read_n, off, path, &rname, &mut probe_dec, &mut probe_out) {
                found.push(entry);
                pos += 0x1000;
                continue;
            }
            let tag = &buf[pos..pos + 4];
            if tag == b"ADAT" {
                awc_idx += 1;
                let sc = u32::from_le_bytes([buf[pos + 8], buf[pos + 9], buf[pos + 10], buf[pos + 11]]);
                found.push(RpfEntry {
                    source_rpf: path.clone(),
                    rpf_name: rname.clone(),
                    name: format!("sound_{awc_idx:04}_{sc}st.awc"),
                    kind: "AWC",
                    offset: off + pos as u64,
                    comp_size: 0,
                    raw_size: 0,
                    tracks: sc,
                    chunks: Vec::new(),
                });
            } else if &tag[..3] == b"KB2" && (b'a'..=b'z').contains(&tag[3]) {
                let fsize = u32::from_le_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]) as u64;
                let frames = u32::from_le_bytes([buf[pos + 8], buf[pos + 9], buf[pos + 10], buf[pos + 11]]);
                let frames2 = u32::from_le_bytes([buf[pos + 16], buf[pos + 17], buf[pos + 18], buf[pos + 19]]);
                let w = u32::from_le_bytes([buf[pos + 20], buf[pos + 21], buf[pos + 22], buf[pos + 23]]);
                let h = u32::from_le_bytes([buf[pos + 24], buf[pos + 25], buf[pos + 26], buf[pos + 27]]);
                let fn_ = u32::from_le_bytes([buf[pos + 28], buf[pos + 29], buf[pos + 30], buf[pos + 31]]);
                let fd_ = u32::from_le_bytes([buf[pos + 32], buf[pos + 33], buf[pos + 34], buf[pos + 35]]);
                let tracks = u32::from_le_bytes([buf[pos + 40], buf[pos + 41], buf[pos + 42], buf[pos + 43]]);
                let tot = fsize + 8;
                if (1..=500_000).contains(&frames) && frames == frames2 && (64..=8192).contains(&w) && (64..=8192).contains(&h) && fn_ > 0 && fd_ > 0 && tot >= 1024 && tot <= sz - (off + pos as u64) {
                    bk2_idx += 1;
                    let abs_off = off + pos as u64;
                    found.push(RpfEntry {
                        source_rpf: path.clone(),
                        rpf_name: rname.clone(),
                        name: format!("video_{bk2_idx:03}_{w}x{h}_{frames}f.bk2"),
                        kind: "BK2",
                        offset: abs_off,
                        comp_size: tot,
                        raw_size: tot,
                        tracks,
                        chunks: Vec::new(),
                    });
                    let next_page = (abs_off + tot + 0xfff) & !0xfff;
                    if next_page > off + read_n as u64 {
                        off = next_page;
                        jumped = true;
                        need_seek = true;
                        break;
                    } else {
                        pos = (next_page - off) as usize;
                        continue;
                    }
                }
            }
            pos += 0x1000;
        }
        if !jumped {
            off += read_n as u64;
        }
    }

    let awc_positions: Vec<usize> = found.iter().enumerate().filter(|(_, e)| e.kind == "AWC").map(|(i, _)| i).collect();
    for (k, &i) in awc_positions.iter().enumerate() {
        let nxt = if k + 1 < awc_positions.len() { found[awc_positions[k + 1]].offset } else { sz };
        let csz = (nxt - found[i].offset).min(32 * 1024 * 1024);
        found[i].comp_size = csz;
        found[i].raw_size = csz;
    }

    if let Some(data) = whole {
        let mut i = 0x40usize;
        let mut stitch_chunks: Vec<(u64, u64)> = Vec::new();
        let mut stitch_raw = 0u64;
        let mut stitch_head: Vec<u8> = Vec::new();
        let mut z_idx = 0usize;
        let mut decomp = Decompress::new(true);
        let mut out = vec![0u8; 131072];

        while i + 8 < data.len() {
            if is_zlib_head(&data[i..]) {
                decomp.reset(true);
                let end = (i + 1024 * 1024).min(data.len());
                if let Ok(Status::StreamEnd) = decomp.decompress(&data[i..end], &mut out, FlushDecompress::Finish) {
                    let out_len = decomp.total_out() as usize;
                    let clen = decomp.total_in() as usize;
                    if out_len >= 32 && clen > 0 {
                        if stitch_chunks.is_empty() {
                            stitch_head = out[..out_len.min(300)].to_vec();
                        }
                        stitch_chunks.push((i as u64, clen as u64));
                        stitch_raw += out_len as u64;

                        // rpf9 splits xml and config streams on exact 64kb pages
                        if out_len != 65536 {
                            z_idx += 1;
                            let trimmed = stitch_head.iter().position(|&b| !matches!(b, 0xef | 0xbb | 0xbf | b' ' | b'\t' | b'\r' | b'\n')).unwrap_or(0);
                            let head = &stitch_head[trimmed..stitch_head.len().min(trimmed + 32)];
                            let (kind, name) = if head.starts_with(b"<?xml") || head.starts_with(b"<") {
                                let tag = extract_xml_tag(&stitch_head[trimmed..]).unwrap_or_else(|| "config".into());
                                ("XML", format!("{z_idx:04}_{tag}.xml"))
                            } else if stitch_head.starts_with(b"RBF0") {
                                ("RBF0", format!("{z_idx:04}_ui.rbf"))
                            } else if stitch_head.starts_with(b"GFX") {
                                ("GFX", format!("{z_idx:04}_scaleform.gfx"))
                            } else if let Some(slug) = tex::probe_gxt_snippet(&stitch_head) {
                                ("GXT", format!("{z_idx:04}_{slug}.gxt"))
                            } else if stitch_head.starts_with(b"PSIN") {
                                ("PSIN", format!("{z_idx:04}_resource.psin"))
                            } else {
                                ("ZLIB", format!("{z_idx:04}_data.bin"))
                            };
                            found.push(RpfEntry {
                                source_rpf: path.clone(),
                                rpf_name: rname.clone(),
                                name,
                                kind,
                                offset: stitch_chunks[0].0,
                                comp_size: clen as u64,
                                raw_size: stitch_raw,
                                tracks: 0,
                                chunks: std::mem::take(&mut stitch_chunks),
                            });
                            stitch_raw = 0;
                        }
                        i = (i + clen + 15) & !15;
                        continue;
                    }
                }
            }
            i += 16;
        }
    }

    Ok((hdr_info, found))
}

impl eframe::App for Rpf9App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (media_loading, media_status, awc_streams) = {
            let mut m = self.media.lock().unwrap();
            if let Some(mesh) = m.loaded_mesh.take() {
                self.textures.clear();
                self.tex_handle = None;
                self.mesh3d = Some(mesh);
                self.show_3d = true;
            }
            if let Some(texs) = m.loaded_textures.take() {
                self.mesh3d = None;
                self.show_3d = false;
                self.textures = texs;
                self.tex_sel = 0;
                self.tex_handle = None;
                self.tex_zoom = 1.0;
            }
            (m.loading, m.status.clone(), m.awc_streams.clone())
        };

        egui::TopBottomPanel::top("top").frame(egui::Frame::new().fill(Color32::from_rgb(24, 14, 36)).inner_margin(10.0)).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(tex) = &self.logo_tex {
                    ui.image((tex.id(), egui::vec2(28.0, 28.0)));
                }
                ui.add_space(6.0);
                ui.label(RichText::new("PERCYSTAR").size(17.0).strong().color(Color32::from_rgb(255, 168, 28)));
                ui.label(RichText::new("RPF9").size(13.0).strong().color(Color32::from_rgb(255, 72, 165)));
                ui.label(RichText::new(format!("v{APP_VERSION}")).size(11.5).color(Color32::from_rgb(165, 145, 195)));
                let upd_avail = self.update.lock().unwrap().available.clone();
                if let Some((ref new_ver, _)) = upd_avail {
                    let pill = egui::Button::new(
                        RichText::new(format!("★ Update Available: v{new_ver}")).size(12.0).strong().color(Color32::BLACK),
                    )
                    .fill(Color32::from_rgb(255, 168, 28));
                    if ui.add(pill).clicked() {
                        self.update.lock().unwrap().show_modal = true;
                    }
                }
                ui.add_space(10.0);
                if ui.button("Open Folder...").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        self.start_folder_scan(folder);
                    }
                }
                if ui.button("Open Archive").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("RPF / TTPF Archives", &["rpf", "ttpf"]).pick_file() {
                        self.start_single_scan(p);
                    }
                }
                if ui.button("Open .OBJ").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("OBJ 3D Model", &["obj"]).pick_file() {
                        if let Some(m) = parse_obj(&p) {
                            self.stop_bink_player();
                            self.textures.clear();
                            self.tex_handle = None;
                            self.mesh3d = Some(m);
                            self.show_3d = true;
                        }
                    }
                }
                if ui.button("Import Catalog").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Models Catalog CSV", &["csv"]).pick_file() {
                        self.import_catalog_csv(&p);
                    }
                }
                if self.is_folder && ui.button("Rescan").clicked() {
                    if let Some(folder) = self.target_path.clone() {
                        self.start_folder_scan(folder);
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export All").clicked() {
                        let n = self.state.lock().unwrap().entries.len();
                        let all: Vec<usize> = (0..n).collect();
                        self.export_entries(&all);
                    }
                    if ui.button("Export Selected").clicked() {
                        if let Some(i) = self.selected {
                            self.export_entries(&[i]);
                        }
                    }
                    let r = ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(250.0).hint_text("Search (Ctrl+F) · words AND · -exclude"));
                    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                        r.request_focus();
                    }
                    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.filter.clear();
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("bottom").frame(egui::Frame::new().fill(Color32::from_rgb(14, 9, 22)).inner_margin(egui::Margin::symmetric(12, 5))).show(ctx, |ui| {
            let (hdr, status, scanning) = {
                let st = self.state.lock().unwrap();
                (st.header_info.clone(), st.status.clone(), st.scanning)
            };
            ui.horizontal(|ui| {
                if scanning { draw_percystar_spinner(ui, 18.0); }
                ui.label(RichText::new(status).size(12.0).color(Color32::from_rgb(185, 190, 204)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(hdr).size(11.5).color(Color32::from_rgb(115, 122, 136)));
                });
            });
        });

        egui::SidePanel::left("tree").default_width(195.0).frame(egui::Frame::new().fill(Color32::from_rgb(18, 12, 28)).inner_margin(8.0)).show(ctx, |ui| {
            self.refresh_view();
            let c = self.counts;
            ui.label(RichText::new("ARCHIVE TREE").size(11.0).color(Color32::from_rgb(110, 118, 134)));
            ui.add_space(4.0);
            for (cat, label, count) in [
                (Category::All, "All Files", c[0]),
                (Category::Vehicles, "Vehicles (.#ft)", c[1]),
                (Category::Models, "3D Models (.#dr/.#ft)", c[2] + c[1]),
                (Category::Textures, "Textures (.#td/.ttpf)", c[3]),
                (Category::Ui, "UI & Text (RBF0/GFX/GXT)", c[4]),
                (Category::NavMesh, "NavMesh (.ynv)", c[5]),
                (Category::Videos, "Videos (.bk2)", c[6]),
                (Category::Audio, "Audio (.awc)", c[7]),
                (Category::Xml, "Configs (.xml)", c[8]),
                (Category::Resources, "Resources (.psin)", c[9]),
                (Category::Binary, "Binary (.bin)", c[10]),
            ] {
                if ui.selectable_label(self.category == cat, format!("{label} ({count})")).clicked() {
                    self.category = cat;
                }
            }
            if self.cached_rpf_names.len() > 1 {
                ui.add_space(10.0);
                ui.label(RichText::new("ARCHIVES").size(11.0).color(Color32::from_rgb(110, 118, 134)));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if ui.selectable_label(self.selected_rpf == "All RPFs", "All RPFs").clicked() {
                        self.selected_rpf = "All RPFs".into();
                    }
                    for rn in &self.cached_rpf_names {
                        if ui.selectable_label(&self.selected_rpf == rn, rn).clicked() {
                            self.selected_rpf = rn.clone();
                        }
                    }
                });
            }
        });

        egui::SidePanel::left("filelist").default_width(390.0).frame(egui::Frame::new().fill(Color32::from_rgb(22, 15, 34)).inner_margin(8.0)).show(ctx, |ui| {
            if self.category == Category::Models {
                ui.horizontal(|ui| {
                    let btn = egui::Button::new(
                        RichText::new(if self.vehicles_only { "★ Vehicles Only: ON" } else { "Vehicles Only" })
                            .strong()
                            .color(if self.vehicles_only { Color32::BLACK } else { Color32::from_rgb(255, 72, 165) }),
                    )
                    .fill(if self.vehicles_only { Color32::from_rgb(255, 168, 28) } else { Color32::from_rgb(38, 24, 56) });
                    if ui.add(btn).clicked() {
                        self.vehicles_only = !self.vehicles_only;
                    }
                });
                ui.add_space(4.0);
            }
            self.refresh_view();
            ui.label(RichText::new(format!("{} results", self.view.len())).size(11.0).color(Color32::from_rgb(110, 118, 134)));
            let mut clicked = None;
            // Rows MUST be exactly ROW_H tall or show_rows miscomputes the total height (blank space / jumpy scroll)
            const ROW_H: f32 = 20.0;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, self.view.len(), |ui, range| {
                let rows: Vec<(usize, RpfEntry)> = {
                    let st = self.state.lock().unwrap();
                    self.view[range].iter().filter_map(|&i| st.entries.get(i).map(|e| (i, e.clone()))).collect()
                };
                for (idx, e) in &rows {
                    let w = ui.available_width();
                    ui.allocate_ui_with_layout(egui::vec2(w, ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_height(ROW_H);
                        ui.label(RichText::new(format!("{:<4}", e.kind)).strong().size(11.5).color(kind_color(e.kind)));
                        let sz_s = format_size(e.raw_size);
                        let label = if self.is_folder {
                            format!("{} / {}  ({})", e.rpf_name, e.name, sz_s)
                        } else {
                            format!("{}  ({})", e.name, sz_s)
                        };
                        if ui.selectable_label(self.selected == Some(*idx), RichText::new(label).size(12.5)).clicked() {
                            clicked = Some(*idx);
                        }
                    });
                }
            });
            if let Some(i) = clicked {
                self.select_entry(i);
            }
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_rgb(14, 10, 22)).inner_margin(12.0)).show(ctx, |ui| {
            if self.parent_hwnd == 0 {
                let h = active_window();
                if h != 0 { self.parent_hwnd = h; }
            }
            let (bink_pid, bink_hwnd_opt, bink_clip_opt) = {
                let m = self.media.lock().unwrap();
                (m.bink_child_pid, m.bink_hwnd, m.bink_clip.clone())
            };

            if let Some(pid) = bink_pid {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&media_status).strong().color(Color32::from_rgb(252, 175, 23)));
                    if ui.button("Stop").clicked() {
                        self.stop_bink_player();
                    }
                    if ui.button("Pop-out").clicked() {
                        if let Some(cp) = bink_clip_opt {
                            self.stop_bink_player();
                            let _ = Command::new(find_binkplay()).arg(&cp).spawn();
                        }
                    }
                });
                ui.add_space(6.0);
                let rect = ui.available_rect_before_wrap();
                let ppp = ctx.pixels_per_point();
                let (rx, ry, rw, rh) = ((rect.min.x * ppp) as i32, (rect.min.y * ppp) as i32, (rect.width() * ppp) as i32, (rect.height() * ppp) as i32);
                if let Some(bh) = bink_hwnd_opt {
                    let cur = [rx, ry, rw, rh];
                    if cur != self.last_dock_rect {
                        self.last_dock_rect = cur;
                        move_window(bh, rx, ry, rw.max(64), rh.max(64));
                    }
                } else if self.parent_hwnd != 0 {
                    if let Some(bh) = find_hwnd_for_pid(pid) {
                        dock_child_hwnd(bh, self.parent_hwnd, rx, ry, rw, rh);
                        self.media.lock().unwrap().bink_hwnd = Some(bh);
                        self.last_dock_rect = [rx, ry, rw, rh];
                    } else {
                        ctx.request_repaint();
                    }
                }
            } else if media_loading {
                ui.horizontal(|ui| {
                    draw_percystar_spinner(ui, 24.0);
                    ui.label(RichText::new(&media_status).color(Color32::from_rgb(252, 175, 23)));
                });
            } else if !awc_streams.is_empty() {
                ui.label(RichText::new(&media_status).strong().color(Color32::WHITE));
                ui.add_space(8.0);
                if ui.button("Stop Audio").clicked() { stop_wav(); }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (idx, wp) in &awc_streams {
                        ui.horizontal(|ui| {
                            if ui.button("Play").clicked() { play_wav_async(wp); }
                            ui.label(format!("Stream #{idx}: {}", wp.file_name().unwrap_or_default().to_string_lossy()));
                        });
                    }
                });
            } else if !media_status.is_empty() {
                ui.label(RichText::new(&media_status).color(Color32::from_rgb(255, 140, 120)));
            } else if !self.textures.is_empty() {
                if self.tex_sel >= self.textures.len() {
                    self.tex_sel = 0;
                }
                let t = &self.textures[self.tex_sel];
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&t.name).strong().color(Color32::from_rgb(255, 168, 28)));
                    ui.label(RichText::new(format!("{}x{} · {} · mips={}", t.w, t.h, t.fmt_name, t.mips)).color(Color32::from_rgb(170, 210, 235)));
                    ui.add(egui::Slider::new(&mut self.tex_zoom, 0.25..=4.0).text("Zoom").fixed_decimals(2));
                    if ui.button("Export PNG").clicked() {
                        let safe: String = t.name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') { c } else { '_' }).collect();
                        if let Some(p) = rfd::FileDialog::new().set_file_name(format!("{safe}.png")).add_filter("PNG Image", &["png"]).save_file() {
                            if let Err(err) = export_texture_png(t, &p) {
                                self.state.lock().unwrap().status = err;
                            } else {
                                self.state.lock().unwrap().status = format!("Saved {}", p.display());
                            }
                        }
                    }
                    if ui.button("Export DDS").clicked() {
                        let safe: String = t.name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') { c } else { '_' }).collect();
                        if let Some(p) = rfd::FileDialog::new().set_file_name(format!("{safe}.dds")).add_filter("DirectDraw Surface", &["dds"]).save_file() {
                            let dds = build_dds_dx10(t.w, t.h, t.dxgi, &t.raw_dds_payload);
                            if let Err(err) = fs::write(&p, &dds) {
                                self.state.lock().unwrap().status = format!("DDS write error: {err}");
                            } else {
                                self.state.lock().unwrap().status = format!("Saved {}", p.display());
                            }
                        }
                    }
                });
                if self.textures.len() > 1 {
                    ui.horizontal(|ui| {
                        if ui.button("◀").clicked() {
                            self.tex_sel = if self.tex_sel == 0 { self.textures.len() - 1 } else { self.tex_sel - 1 };
                        }
                        ui.label(RichText::new(format!("{}/{}", self.tex_sel + 1, self.textures.len())).color(Color32::from_rgb(190, 180, 215)));
                        if ui.button("▶").clicked() {
                            self.tex_sel = (self.tex_sel + 1) % self.textures.len();
                        }
                        egui::ScrollArea::horizontal().id_salt("txd_strip").show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (i, item) in self.textures.iter().enumerate() {
                                    if ui.selectable_label(self.tex_sel == i, format!("{} ({}x{})", item.name, item.w, item.h)).clicked() {
                                        self.tex_sel = i;
                                    }
                                }
                            });
                        });
                    });
                }
                ui.separator();
                let need_upload = self.tex_handle.as_ref().map(|(idx, _)| *idx != self.tex_sel).unwrap_or(true);
                if need_upload {
                    let cur = &self.textures[self.tex_sel];
                    let ci = ColorImage { size: [cur.w, cur.h], pixels: cur.pixels.clone() };
                    let h = ctx.load_texture(format!("percystar_tex_{}", self.tex_sel), ci, TextureOptions::LINEAR);
                    self.tex_handle = Some((self.tex_sel, h));
                }
                if let Some((_, handle)) = &self.tex_handle {
                    let cur = &self.textures[self.tex_sel];
                    let avail = ui.available_size();
                    let fit = (avail.x / cur.w as f32).min(avail.y / cur.h as f32).min(1.0).max(0.05);
                    let disp = egui::vec2(cur.w as f32 * fit * self.tex_zoom, cur.h as f32 * fit * self.tex_zoom);
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.image((handle.id(), disp));
                    });
                }
            } else if let Some(mesh) = self.mesh3d.as_mut() {
                let mut toggle_3d = self.show_3d;
                ui.horizontal(|ui| {
                    if ui.selectable_label(!toggle_3d, "Hex / Info").clicked() { toggle_3d = false; }
                    if ui.selectable_label(toggle_3d, "3D Model View").clicked() { toggle_3d = true; }
                });
                self.show_3d = toggle_3d;
                ui.separator();
                if self.show_3d {
                    render_mesh3d_viewport(ui, mesh);
                } else {
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut self.preview).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                    });
                }
            } else {
                egui::ScrollArea::both().show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut self.preview).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                });
            }
        });

        let (show_upd, upd_avail, upd_inst, upd_status) = {
            let g = self.update.lock().unwrap();
            (g.show_modal, g.available.clone(), g.installing, g.status.clone())
        };
        if show_upd {
            if let Some((ref new_ver, ref rel_url)) = upd_avail {
                let mut open = true;
                egui::Window::new("Update Available")
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.label(RichText::new(format!("PercyStar v{new_ver} is ready!")).size(15.0).strong().color(Color32::from_rgb(255, 168, 28)));
                        ui.label(RichText::new(format!("Installed: v{APP_VERSION}  ->  Latest: v{new_ver}")).color(Color32::from_rgb(200, 190, 220)));
                        ui.add_space(8.0);
                        if !upd_status.is_empty() {
                            ui.label(RichText::new(&upd_status).color(Color32::from_rgb(255, 72, 165)));
                            ui.add_space(4.0);
                        }
                        ui.horizontal(|ui| {
                            let btn = egui::Button::new(
                                RichText::new(if upd_inst { "Updating..." } else { "Download & Install" }).strong().color(Color32::BLACK),
                            )
                            .fill(Color32::from_rgb(255, 168, 28));
                            // releases ship a Win64 zip only; other platforms just get "View Release"
                            if cfg!(windows) && ui.add_enabled(!upd_inst, btn).clicked() {
                                spawn_install_update(new_ver.clone(), Arc::clone(&self.update), ctx.clone());
                            }
                            if ui.button("View Release").clicked() {
                                let _ = if cfg!(windows) {
                                    silent_cmd("cmd").args(["/C", "start", "", rel_url]).spawn()
                                } else {
                                    Command::new("open").arg(rel_url).spawn()
                                };
                            }
                            if ui.add_enabled(!upd_inst, egui::Button::new("Later")).clicked() {
                                self.update.lock().unwrap().show_modal = false;
                            }
                        });
                    });
                if !open {
                    self.update.lock().unwrap().show_modal = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_comparison_works() {
        assert_eq!(parse_semver("1.0.3"), Some((1, 0, 3)));
        assert_eq!(parse_semver("v1.0.4"), Some((1, 0, 4)));
        assert!(parse_semver("v1.0.4") > parse_semver("1.0.3"));
        assert!(parse_semver("1.1.0") > parse_semver("1.0.99"));
        assert!(parse_semver("v1.0.3") == parse_semver("1.0.3"));
        assert!(parse_semver("bad").is_none());
    }

    #[test]
    fn deswizzle_and_ui_decoders_work() {
        // 4KB SW_4KB_D bijective permutation test on 64x64 BC7 (16x16 blocks, 1 tile)
        let mut raw = vec![0u8; 4096];
        for blk in 0..256 {
            raw[blk * 16] = blk as u8;
        }
        let lin = tex::deswizzle_4kb_d(&raw, 64, 64, true, 16);
        assert_eq!(lin.len(), 4096);
        let mut seen = [false; 256];
        for blk in 0..256 {
            seen[lin[blk * 16] as usize] = true;
        }
        assert!(seen.iter().all(|&b| b), "SW_4KB_D 128bpp must be a full bijection");

        // 64KB TTPF bijective permutation test on 256x256 BC7 (64x64 blocks, 1 tile)
        let mut raw64 = vec![0u8; 65536];
        for blk in 0..4096 {
            raw64[blk * 16] = (blk & 0xFF) as u8;
            raw64[blk * 16 + 1] = ((blk >> 8) & 0xFF) as u8;
        }
        let lin64 = tex::deswizzle_64kb_ttpf(&raw64, 256, 256, true, 16);
        assert_eq!(lin64.len(), 65536);
        let mut seen64 = vec![false; 4096];
        for blk in 0..4096 {
            let v = (lin64[blk * 16] as usize) | ((lin64[blk * 16 + 1] as usize) << 8);
            seen64[v] = true;
        }
        assert!(seen64.iter().all(|&b| b), "TTPF 64KB 128bpp must be a full bijection");

        // RBF0 GTA6 header (0xFB 0xFF 0x01 0x00) decode test
        let mut rbf = Vec::new();
        rbf.extend_from_slice(b"RBF0");
        rbf.extend_from_slice(&[0xFB, 0xFF, 0x01, 0x00]); // GTA6 header
        rbf.extend_from_slice(&[0x00, 0x00, 0x04, 0x00]); // idx=0, struct, name len=4
        rbf.extend_from_slice(b"Root");
        rbf.extend_from_slice(&[0u8; 6]); // struct header
        rbf.extend_from_slice(&[0x01, 0x10, 0x02, 0x00]); // idx=1, u32 attr, len=2
        rbf.extend_from_slice(b"id");
        rbf.extend_from_slice(&42u32.to_le_bytes());
        rbf.extend_from_slice(&[0xFF, 0xFF]); // close Root
        let xml = decode_rbf0(&rbf).expect("RBF0 should decode");
        assert!(xml.contains("<Root id=\"42\" />"), "got: {xml}");

        // Scaleform GFX inspector test
        let mut gfx = Vec::new();
        gfx.extend_from_slice(b"GFX\x2a");
        gfx.extend_from_slice(&128u32.to_le_bytes());
        gfx.extend_from_slice(b"\0com.rockstargames.gtav.HudMap\0");
        let dump = inspect_gfx(&gfx);
        assert!(dump.contains("Version: 42"));
        assert!(dump.contains("com.rockstargames.gtav.HudMap"));
    }

    /// Writes sample PNGs from a big .ttpf so decode quality can be eyeballed:
    /// PERCY_TTPF=<file> PERCY_OUT=<dir> cargo test --release dump_ttpf_samples -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_ttpf_samples() {
        let (Ok(src), Ok(out)) = (std::env::var("PERCY_TTPF"), std::env::var("PERCY_OUT")) else { return };
        let src = PathBuf::from(src);
        let t = Instant::now();
        let ix = tex::ttpf_index(&src).expect("index");
        eprintln!("index: {} textures in {:.1}s", ix.descs.len(), t.elapsed().as_secs_f32());
        let _ = fs::create_dir_all(&out);
        let (mut ok, mut bcpack, mut fail) = (0, 0, 0);
        let step = (ix.descs.len() / 60).max(1);
        for d in ix.descs.iter().step_by(step) {
            let t1 = Instant::now();
            match decode_ttpf_texture(&src, d.idx) {
                Ok(tx) => {
                    ok += 1;
                    let _ = export_texture_png(&tx, &Path::new(&out).join(format!("{}.png", tx.name)));
                    if ok <= 5 { eprintln!("  {} in {} ms", tx.name, t1.elapsed().as_millis()); }
                }
                Err(e) if e.contains("BCPack") => bcpack += 1,
                Err(e) => { fail += 1; eprintln!("  #{} fail: {e}", d.idx); }
            }
        }
        eprintln!("ok={ok} bcpack={bcpack} fail={fail}");
        // raw virtual dumps (whole allocation) for offline layout analysis
        if let Ok(ids) = std::env::var("PERCY_RAW") {
            for id in ids.split(',').filter_map(|x| x.parse::<usize>().ok()) {
                let d = &ix.descs[id];
                if let Ok(raw) = tex::ttpf_read_virtual(&src, &ix, d.byte_off, d.alloc_sz as usize) {
                    let nm = format!("raw_{}_{}x{}_f{:02x}_m{}_a{}.bin", id, d.w, d.h, d.fmt_byte, d.has_mips as u8, d.alloc_sz);
                    let _ = fs::write(Path::new(&out).join(nm), raw);
                }
            }
        }
    }

    /// PERCY_RPF=<textures.rpf> PERCY_OUT=<dir>: dump every TXD texture as PNG
    #[test]
    #[ignore]
    fn dump_txd_samples() {
        let (Ok(src), Ok(out)) = (std::env::var("PERCY_RPF"), std::env::var("PERCY_OUT")) else { return };
        let src = PathBuf::from(src);
        let _ = fs::create_dir_all(&out);
        let t = Instant::now();
        let (_, entries) = scan_single_rpf(&src, None).unwrap();
        let txds: Vec<_> = entries.iter().filter(|e| e.kind == "TXD").collect();
        eprintln!("{} TXD in {:.1}s", txds.len(), t.elapsed().as_secs_f32());
        let (mut ok, mut fail) = (0, 0);
        for e in txds.iter().take(40) {
            let t1 = Instant::now();
            match decode_txd_from_rpf(&src, e.offset) {
                Ok(v) => for (k, tx) in v.iter().enumerate().take(3) {
                    ok += 1;
                    let _ = export_texture_png(tx, &Path::new(&out).join(format!("{}_{k}_{}.png", e.offset, tx.name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_"))));
                    if ok < 4 { eprintln!("  {} {}x{} {} in {} ms", tx.name, tx.w, tx.h, tx.fmt_name, t1.elapsed().as_millis()); }
                },
                Err(er) => { fail += 1; eprintln!("  fail {}: {er}", e.name); }
            }
        }
        eprintln!("ok={ok} fail={fail}");
    }

    /// Brute-force layout finder: PERCY_TTPF, PERCY_OUT, PERCY_IDS=1,2,3
    #[test]
    #[ignore]
    fn ttpf_layout_search() {
        let (Ok(src), Ok(out), Ok(ids)) = (std::env::var("PERCY_TTPF"), std::env::var("PERCY_OUT"), std::env::var("PERCY_IDS")) else { return };
        let src = PathBuf::from(src);
        let ix = tex::ttpf_index(&src).unwrap();
        let _ = fs::create_dir_all(&out);
        for id in ids.split(',').filter_map(|x| x.parse::<usize>().ok()) {
            let d = &ix.descs[id];
            let Ok(raw) = tex::ttpf_read_virtual(&src, &ix, d.byte_off, d.alloc_sz as usize) else { eprintln!("{id}: bcpack"); continue };
            let mut res = Vec::new();
            for (dxgi, bpe, is_bc) in [(98u8, 16usize, true), (71, 8, true), (77, 16, true), (80, 8, true), (83, 16, true), (95, 16, true), (28, 4, false), (61, 1, false), (49, 2, false)] {
                let (bw, bh) = if is_bc { ((d.w + 3) / 4, (d.h + 3) / 4) } else { (d.w, d.h) };
                for t64 in [false, true] {
                    if t64 && !is_bc { continue; }
                    let pad = if t64 {
                        let (tw, th) = if bpe == 16 { (64, 64) } else { (128, 64) };
                        ((bw + tw - 1) / tw) * tw * ((bh + th - 1) / th) * th * bpe
                    } else { tex::padded_4kb_size(d.w, d.h, is_bc, bpe) };
                    let mut offs = vec![0usize];
                    if raw.len() > pad { offs.push(raw.len() - pad); }
                    let mut o = 0x1000; while o + pad <= raw.len() && offs.len() < 40 { offs.push(o); o += 0x1000; }
                    offs.sort(); offs.dedup();
                    for off in offs {
                        if off + pad > raw.len() && off != 0 { continue; }
                        let sl = &raw[off..raw.len().min(off + pad)];
                        let lin = if t64 { tex::deswizzle_64kb_ttpf(sl, d.w, d.h, is_bc, bpe) } else { tex::deswizzle_4kb_d(sl, d.w, d.h, is_bc, bpe) };
                        let Some(px) = tex::decode_linear_to_rgba(&lin, d.w, d.h, dxgi) else { continue };
                        let mut diff = 0u64;
                        for y in 0..d.h { for x in 1..d.w {
                            let (a, b) = (px[y * d.w + x - 1], px[y * d.w + x]);
                            diff += (a.r() as i32 - b.r() as i32).unsigned_abs() as u64 + (a.g() as i32 - b.g() as i32).unsigned_abs() as u64;
                        }}
                        for y in 1..d.h { for x in 0..d.w {
                            let (a, b) = (px[(y - 1) * d.w + x], px[y * d.w + x]);
                            diff += (a.r() as i32 - b.r() as i32).unsigned_abs() as u64 + (a.g() as i32 - b.g() as i32).unsigned_abs() as u64;
                        }}
                        res.push((diff as f64 / (d.w * d.h) as f64, dxgi, t64, off, px));
                    }
                }
            }
            res.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            eprintln!("tex {id} {}x{} f{:02x} mips={} alloc={:#x}", d.w, d.h, d.fmt_byte, d.has_mips, d.alloc_sz);
            for (k, (sc, dx, t64, off, px)) in res.iter().take(4).enumerate() {
                eprintln!("   score {sc:.1} dxgi {dx} t64 {t64} off {off:#x}");
                let t = tex::DecodedTexture { name: String::new(), w: d.w, h: d.h, mips: 1, dxgi: *dx, fmt_name: "", pixels: px.clone(), raw_dds_payload: vec![] };
                let _ = export_texture_png(&t, &Path::new(&out).join(format!("{id}_{k}.png")));
            }
        }
    }

    #[test]
    fn real_gta6_archives_smoke_test() {
        let base = Path::new(r"D:\Grand Theft Auto VI Build");
        if !base.exists() { return; }

        // 1. TTPF native scan + decode
        let ttpf = base.join("data").join("ttdb").join("ui").join("platform.ttpf");
        if ttpf.exists() {
            let (info, descs) = scan_ttpf_descriptors(&ttpf).expect("scan platform.ttpf");
            assert!(!descs.is_empty(), "platform.ttpf descriptors empty: {info}");
            let mut decoded_ok = 0usize;
            for d in descs.iter().take(25) {
                if let Ok(t) = decode_ttpf_texture(&ttpf, d.idx) {
                    assert_eq!(t.pixels.len(), t.w * t.h);
                    decoded_ok += 1;
                    if decoded_ok >= 3 { break; }
                }
            }
            assert!(decoded_ok >= 1, "expected at least 1 decoded texture from platform.ttpf");
        }

        // 1b. Previously unsupported TTPF formats: DXGI 28 (RGBA8), 54 (R16F), 61 (R8), 65 (A8), 87 (BGRA8), 96 (BC6H_SF16)
        for (rel, idx, exp_dxgi) in [
            ("data\\ttdb\\graphics\\ttdb_graphics.ttpf", 1usize, 28u8),
            ("data\\ttdb\\graphics\\ttdb_graphics.ttpf", 563usize, 87u8),
            ("data\\ttdb\\maps\\ttdb_maps_0_7.ttpf", 605usize, 54u8),
            ("data\\ttdb\\interiors\\ttdb_interiors_0_3.ttpf", 1984usize, 65u8),
            ("data\\ttdb\\vfx\\ttdb_vfx.ttpf", 608usize, 61u8),
            ("data\\ttdb\\lighting\\lighting.ttpf", 334usize, 96u8),
        ] {
            let p = base.join(rel);
            if p.exists() {
                let t = decode_ttpf_texture(&p, idx).unwrap_or_else(|e| panic!("{rel} #{idx}: {e}"));
                assert_eq!(t.dxgi, exp_dxgi);
                assert_eq!(t.pixels.len(), t.w * t.h);
            }
        }

        // 2. TXD native scan + decode from textures.rpf
        let tex_rpf = base.join("textures.rpf");
        if tex_rpf.exists() {
            let (_, entries) = scan_single_rpf(&tex_rpf, None).expect("scan textures.rpf");
            let txds: Vec<&RpfEntry> = entries.iter().filter(|e| e.kind == "TXD").collect();
            assert!(!txds.is_empty(), "textures.rpf should yield TXD entries");
            let t0 = decode_txd_from_rpf(&tex_rpf, txds[0].offset).expect("decode first TXD");
            assert!(!t0.is_empty());
            assert_eq!(t0[0].pixels.len(), t0[0].w * t0[0].h);
        }

        // 3. CNavMesh 3D decode from navmeshes.rpf (first CNavMesh block starts at ~750MB after path regions)
        let nav_rpf = base.join("navmeshes.rpf");
        if nav_rpf.exists() {
            let mut f = File::open(&nav_rpf).unwrap();
            let base_off = 786_010_112u64; // page holding first CNavMesh tile (786010128 - 0x10)
            f.seek(SeekFrom::Start(base_off)).unwrap();
            let mut buf = vec![0u8; 256 * 1024];
            let n = f.read(&mut buf).unwrap();
            let mut dec = Decompress::new(true);
            let mut out = vec![0u8; 32768];
            let e = probe_rage9_page(&buf, 0, n, base_off, &nav_rpf, "navmeshes.rpf", &mut dec, &mut out)
                .expect("should detect NAVM in navmeshes.rpf");
            assert_eq!(e.kind, "NAVM");
            assert!(e.name.starts_with("nav_160_179_"), "got {}", e.name);
            let mesh = decode_navmesh_from_rpf(&nav_rpf, e.offset, &e.name).expect("decode NAVM mesh");
            assert!(!mesh.verts.is_empty() && !mesh.tris.is_empty());
        }

        // 4. packs.rpf catalog model decode (verifies +0xA8 name + SW_4KB_D embedded tex)
        let packs_rpf = base.join("packs.rpf");
        if packs_rpf.exists() {
            let mesh = decode_rage9_model_from_rpf(&packs_rpf, 0x47ab1010, "fallback").expect("decode packs.rpf model");
            assert!(!mesh.verts.is_empty() && !mesh.tris.is_empty());
        }
    }
}

fn export_models_cli(rpf_path: &PathBuf, out_dir: &PathBuf, filter: &str, vehicles_only: bool) {
    let _ = fs::create_dir_all(out_dir);
    let q = filter.to_ascii_lowercase();
    eprintln!("Scanning {}...", rpf_path.display());
    let entries = if vehicles_only {
        scan_levels_vehicle_zone(rpf_path, None)
    } else {
        match scan_single_rpf(rpf_path, None) {
            Ok((_, e)) => e,
            Err(err) => {
                eprintln!("Scan failed: {err}");
                return;
            }
        }
    };
    let mut best: HashMap<String, (u64, String, bool)> = HashMap::new();
    for e in entries {
        if vehicles_only {
            if e.kind != "VEH" || e.name.to_ascii_lowercase().contains("_mid.") { continue; }
            let stem = e.name.trim_end_matches(".#ft");
            let is_hi = stem.ends_with("_hi");
            let base = stem.trim_end_matches("_hi").to_ascii_lowercase();
            match best.get(&base) {
                Some((_, _, prev_hi)) if *prev_hi && !is_hi => {}
                _ => { best.insert(base, (e.offset, e.name, is_hi)); }
            }
        } else {
            if e.kind != "MODEL" && e.kind != "VEH" { continue; }
            let lo = e.name.to_ascii_lowercase();
            if !q.is_empty() && !q.split(',').any(|tok| !tok.is_empty() && lo.contains(tok)) { continue; }
            best.entry(lo).or_insert((e.offset, e.name, false));
        }
    }
    let mut list: Vec<(String, u64, String)> = best.into_iter().map(|(k, (o, n, _))| (k, o, n)).collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    eprintln!("Found {} models -> exporting .OBJs...", list.len());
    let mut ok = 0usize;
    for (idx, (key, off, name)) in list.iter().enumerate() {
        if let Ok(mesh) = decode_rage9_model_from_rpf(rpf_path, *off, name) {
            let fname = if vehicles_only { format!("{key}.obj") } else { format!("{}.obj", name.replace('#', "").replace('.', "_")) };
            if mesh.export_obj(&out_dir.join(fname)).is_ok() {
                ok += 1;
                if ok <= 25 || ok % 50 == 0 {
                    eprintln!("  [{}/{}] {} ({} v, {} f)", idx + 1, list.len(), key, mesh.verts.len(), mesh.tris.len());
                }
            }
        }
    }
    eprintln!("DONE: Exported {ok}/{} models to {}", list.len(), out_dir.display());
}

fn dump_textures_cli(src_path: &PathBuf, out_dir: &PathBuf, limit: usize) {
    let _ = fs::create_dir_all(out_dir);
    let is_ttpf = src_path.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("ttpf")).unwrap_or(false);
    let (mut ok, mut bcpk, mut fail) = (0usize, 0usize, 0usize);
    if is_ttpf {
        let ix = match tex::ttpf_index(src_path) {
            Ok(i) => i,
            Err(e) => { eprintln!("TTPF index error: {e}"); return; }
        };
        let n = if limit == 0 { ix.descs.len() } else { limit.min(ix.descs.len()) };
        eprintln!("Dumping up to {n}/{} textures from {}...", ix.descs.len(), src_path.display());
        for d in ix.descs.iter().take(n) {
            match decode_ttpf_texture(src_path, d.idx) {
                Ok(tx) => {
                    if export_texture_png(&tx, &out_dir.join(format!("{}.png", tx.name))).is_ok() {
                        ok += 1;
                    }
                }
                Err(e) if e.contains("BCPack") => bcpk += 1,
                Err(_) => fail += 1,
            }
            if (ok + bcpk + fail) % 250 == 0 {
                eprintln!("  progress: ok={ok} bcpack={bcpk} fail={fail}");
            }
        }
    } else {
        let entries = match scan_single_rpf(src_path, None) {
            Ok((_, e)) => e,
            Err(err) => { eprintln!("Scan failed: {err}"); return; }
        };
        let txds: Vec<&RpfEntry> = entries.iter().filter(|e| e.kind == "TXD").collect();
        eprintln!("Found {} TXDs in {} -> dumping PNGs...", txds.len(), src_path.display());
        let mut seen = std::collections::HashSet::new();
        'outer: for e in txds {
            match decode_txd_from_rpf(src_path, e.offset) {
                Ok(vec_t) => {
                    for tx in &vec_t {
                        let safe: String = tx.name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') { c } else { '_' }).collect();
                        let fname = if seen.insert(safe.clone()) { format!("{safe}.png") } else { format!("{safe}_{:x}.png", e.offset) };
                        if export_texture_png(tx, &out_dir.join(fname)).is_ok() {
                            ok += 1;
                            if limit > 0 && ok >= limit { break 'outer; }
                        }
                    }
                }
                Err(_) => fail += 1,
            }
        }
    }
    eprintln!("DONE: Exported {ok} PNGs (bcpack={bcpk}, fail={fail}) to {}", out_dir.display());
}

fn main() -> eframe::Result<()> {
    // remove leftover old exe from previous self update
    if let Ok(cur) = std::env::current_exe() {
        let _ = fs::remove_file(cur.with_extension("exe.old"));
    }
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "--dump-textures" {
        let lim = args.get(4).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
        dump_textures_cli(&PathBuf::from(&args[2]), &PathBuf::from(&args[3]), lim);
        return Ok(());
    }
    if args.len() >= 4 && args[1] == "--extract-vehicles" {
        export_models_cli(&PathBuf::from(&args[2]), &PathBuf::from(&args[3]), "", true);
        return Ok(());
    }
    if args.len() >= 4 && args[1] == "--extract-models" {
        let filt = args.get(4).map(|s| s.as_str()).unwrap_or("");
        export_models_cli(&PathBuf::from(&args[2]), &PathBuf::from(&args[3]), filt, false);
        return Ok(());
    }

    // decod embedded logo once and share bytes across window icon and topbar
    let logo_rgba = image::load_from_memory(include_bytes!("../assets/logo.png"))
        .ok()
        .map(|img| {
            let rgba = img.to_rgba8();
            (rgba.as_raw().to_vec(), rgba.width(), rgba.height())
        });
    let mut vb = egui::ViewportBuilder::default().with_inner_size([1320.0, 800.0]);
    if let Some((ref raw, w, h)) = logo_rgba {
        vb = vb.with_icon(egui::IconData { rgba: raw.clone(), width: w, height: h });
    }
    let opts = eframe::NativeOptions { viewport: vb, ..Default::default() };
    let title = format!("PercyStar v{APP_VERSION}");
    eframe::run_native(&title, opts, Box::new(move |cc| Ok(Box::new(Rpf9App::new(cc, logo_rgba)))))
}
