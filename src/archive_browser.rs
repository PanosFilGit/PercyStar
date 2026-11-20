use crate::archive::{Archive, Catalog, Codec, Member, Result, PREVIEW_BYTES};
use crate::tex;
use eframe::egui::{self, ColorImage, RichText, TextureHandle, Ui};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

type Task<T> = Box<dyn FnOnce() -> Result<T> + Send>;

/// Limit each operation to one worker and one replaceable pending request.
struct Work<T> {
    running: Option<mpsc::Receiver<Result<T>>>,
    next: Option<Task<T>>,
    obsolete: bool,
}
impl<T> Default for Work<T> {
    fn default() -> Self {
        Self {
            running: None,
            next: None,
            obsolete: false,
        }
    }
}
impl<T: Send + 'static> Work<T> {
    fn submit(&mut self, task: impl FnOnce() -> Result<T> + Send + 'static) {
        self.clear();
        self.next = Some(Box::new(task));
    }

    fn clear(&mut self) {
        self.next = None;
        self.obsolete = self.running.is_some();
    }

    fn busy(&self) -> bool {
        self.running.is_some() || self.next.is_some()
    }

    fn poll(&mut self, ctx: &egui::Context) -> Option<Result<T>> {
        let mut result = None;
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(r) => result = Some(r),
                Err(mpsc::TryRecvError::Disconnected) => {
                    result = Some(Err("Worker stopped unexpectedly".into()))
                }
                Err(mpsc::TryRecvError::Empty) => return None,
            }
            self.running = None;
            if self.obsolete {
                result = None;
            }
            self.obsolete = false;
        }
        if let Some(task) = self.next.take() {
            let (tx, rx) = mpsc::channel();
            self.running = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(task());
                ctx.request_repaint();
            });
        }
        result
    }
}

enum Opened {
    Folder(PathBuf, Vec<PathBuf>, usize),
    File(Arc<Archive>),
}

fn index_folder(root: &Path) -> Result<Opened> {
    let mut dirs = vec![root.to_owned()];
    let mut archives = vec![];
    let mut skipped = 0;
    let mut visited = 0;
    while let Some(dir) = dirs.pop() {
        let items = match fs::read_dir(&dir) {
            Ok(items) => items,
            Err(e) if dir == root => return Err(e.to_string()),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        for item in items {
            let Ok(item) = item else {
                skipped += 1;
                continue;
            };
            let Ok(ty) = item.file_type() else {
                skipped += 1;
                continue;
            };
            if ty.is_symlink() {
                continue;
            }
            let path = item.path();
            if ty.is_dir() {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    match item.metadata() {
                        Ok(m) if m.file_attributes() & 0x400 != 0 => continue,
                        Err(_) => {
                            skipped += 1;
                            continue;
                        }
                        _ => {}
                    }
                }
                visited += 1;
                if visited > 100_000 {
                    return Err("Folder exceeds 100,000 subdirectories".into());
                }
                dirs.push(path);
            } else if ty.is_file()
                && path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
                    s.eq_ignore_ascii_case("rpf") || s.eq_ignore_ascii_case("ttpf")
                })
            {
                if archives.len() >= 100_000 {
                    return Err("Folder exceeds 100,000 archives".into());
                }
                archives.push(path);
            }
        }
    }
    archives.sort();
    Ok(Opened::Folder(root.into(), archives, skipped))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Members,
    Records,
    Textures,
    Hex,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Record(usize),
    Texture(usize),
}

impl Item {
    fn name(self, a: &Archive) -> String {
        match (self, &a.catalog) {
            (Self::Texture(i), _) => format!("Texture {i}"),
            (Self::Record(i), Catalog::Rpf0(nodes)) => nodes[i].name.clone(),
            (Self::Record(i), _) => format!("Record {i}"),
        }
    }
}

enum Preview {
    Bytes(Vec<u8>, u64),
    Image(tex::DecodedTexture),
}
enum Display {
    Empty,
    Loading,
    Bytes(String),
    Image(TextureHandle),
    Error(String),
}

#[derive(Clone, Copy)]
enum Output {
    Payload,
    Png,
    Dds,
}

fn hex(bytes: &[u8], offset: u64) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (i, row) in bytes.chunks(16).enumerate() {
        let _ = write!(out, "{:08X}   ", offset + (i * 16) as u64);
        for j in 0..16 {
            if j == 8 {
                out.push(' ');
            }
            if let Some(b) = row.get(j) {
                let _ = write!(out, "{b:02X} ");
            } else {
                out.push_str("   ");
            }
        }
        out.push(' ');
        for b in row {
            out.push(if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    out
}

#[derive(Default)]
struct FolderTree {
    children: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    expanded: HashSet<PathBuf>,
    rows: Vec<(usize, PathBuf)>,
}

impl FolderTree {
    fn build(&mut self, root: &Path, files: &[PathBuf]) {
        self.children.clear();
        self.expanded.clear();
        for file in files {
            let mut child = file.as_path();
            while let Some(parent) = child.parent() {
                if !self
                    .children
                    .entry(parent.into())
                    .or_default()
                    .insert(child.into())
                    || parent == root
                {
                    break;
                }
                child = parent;
            }
        }
        self.flatten(root);
    }

    fn flatten(&mut self, root: &Path) {
        self.rows.clear();
        let mut stack = vec![(0, root.to_owned())];
        while let Some((depth, parent)) = stack.pop() {
            if parent != root {
                self.rows.push((depth, parent.clone()));
            }
            if parent == root || self.expanded.contains(&parent) {
                if let Some(children) = self.children.get(&parent) {
                    let child_depth = depth + usize::from(parent != root);
                    stack.extend(children.iter().rev().map(|p| (child_depth, p.clone())));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_navigation_preserves_duplicate_archive_basenames() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for folder in ["a", "b"] {
            fs::create_dir(root.join(folder)).unwrap();
            fs::write(root.join(folder).join("maps.TTPF"), b"").unwrap();
        }
        fs::write(root.join("other.txt"), b"").unwrap();
        let Opened::Folder(_, paths, 0) = index_folder(root).unwrap() else {
            panic!()
        };
        let mut tree = FolderTree::default();
        tree.build(root, &paths);
        assert_eq!(tree.rows, [(0, root.join("a")), (0, root.join("b"))]);
        tree.expanded.extend([root.join("a"), root.join("b")]);
        tree.flatten(root);
        assert_eq!(tree.rows[1], (1, root.join("a/maps.TTPF")));
        assert_eq!(tree.rows[3], (1, root.join("b/maps.TTPF")));
    }

    #[test]
    fn superseded_preview_cannot_reappear_after_selection_changes() {
        let (tx, rx) = mpsc::channel();
        let mut work = Work {
            running: Some(rx),
            next: None,
            obsolete: false,
        };
        work.submit(|| Ok(2));
        work.submit(|| Ok(3));
        tx.send(Ok(1)).unwrap();
        let ctx = egui::Context::default();
        assert!(work.poll(&ctx).is_none());
        assert_eq!(work.running.take().unwrap().recv().unwrap().unwrap(), 3);
    }
}

pub struct Browser {
    folder: Option<PathBuf>,
    files: Vec<PathBuf>,
    tree: FolderTree,
    archive: Option<Arc<Archive>>,
    open_work: Work<Opened>,
    preview_work: Work<Preview>,
    export_work: Work<PathBuf>,
    view: View,
    dir: usize,
    filter: String,
    visible: Vec<usize>,
    selected: Option<Item>,
    display: Display,
    hex_at: u64,
    address: String,
    pub status: String,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            folder: None,
            files: vec![],
            tree: Default::default(),
            archive: None,
            open_work: Work::default(),
            preview_work: Work::default(),
            export_work: Work::default(),
            view: View::Hex,
            dir: 0,
            filter: String::new(),
            visible: vec![],
            selected: None,
            display: Display::Empty,
            hex_at: 0,
            address: "0".into(),
            status: "Ready".into(),
        }
    }
}

impl Browser {
    pub fn open(&mut self, path: PathBuf) {
        self.status = format!("Opening {}", path.display());
        self.archive = None;
        self.reset();
        self.open_work.submit(move || {
            if path.is_dir() {
                index_folder(&path)
            } else {
                Archive::inspect(&path).map(|a| Opened::File(Arc::new(a)))
            }
        });
    }

    fn reset(&mut self) {
        self.preview_work.clear();
        self.selected = None;
        self.display = Display::Empty;
        self.visible.clear();
        self.filter.clear();
        self.hex_at = 0;
        self.address = "0".into();
        self.dir = 0;
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.open_work.poll(ctx) {
            match result {
                Err(e) => self.status = format!("Open failed · {e}"),
                Ok(Opened::Folder(root, files, skipped)) => {
                    self.tree.build(&root, &files);
                    self.folder = Some(root);
                    self.files = files;
                    self.status =
                        format!("{} archives · {skipped} unreadable items", self.files.len());
                }
                Ok(Opened::File(a)) => {
                    self.view = match &a.catalog {
                        Catalog::Rds(ix) if !ix.descs.is_empty() => View::Textures,
                        Catalog::Rds(_) => View::Records,
                        Catalog::Rpf0(_) => View::Members,
                        _ => View::Hex,
                    };
                    if !self.files.contains(&a.path) {
                        if !self.folder.as_ref().is_some_and(|p| a.path.starts_with(p)) {
                            self.folder = None;
                            self.files.clear();
                        }
                        self.files.push(a.path.clone());
                        if let Some(root) = &self.folder {
                            self.tree.build(root, &self.files);
                        }
                    }
                    self.status = format!("{} · {} bytes · read-only", a.format(), a.src.size);
                    self.archive = Some(a);
                    self.rebuild();
                    if self.view == View::Hex {
                        self.hex_page(0);
                    }
                    if self.view == View::Textures && !self.visible.is_empty() {
                        self.select(Item::Texture(0));
                    }
                }
            }
        }
        if let Some(result) = self.preview_work.poll(ctx) {
            self.display = match result {
                Err(e) => Display::Error(e),
                Ok(Preview::Bytes(bytes, base)) => Display::Bytes(hex(&bytes, base)),
                Ok(Preview::Image(t)) => {
                    let img = ColorImage {
                        size: [t.w, t.h],
                        pixels: t.pixels,
                    };
                    Display::Image(ctx.load_texture(
                        "selected-archive-texture",
                        img,
                        egui::TextureOptions::NEAREST,
                    ))
                }
            };
        }
        if let Some(result) = self.export_work.poll(ctx) {
            self.status = match result {
                Ok(path) => format!("Exported {}", path.display()),
                Err(e) => format!("Export failed · {e}"),
            };
        }
    }

    pub fn toolbar(&mut self, ui: &mut Ui) {
        let open = ui.button("Open archive…").on_hover_text("Ctrl+O").clicked()
            || ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O));
        if open {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("RAGE archives", &["rpf", "ttpf"])
                .pick_file()
            {
                self.open(p);
            }
        }
        let folder = ui
            .button("Open folder…")
            .on_hover_text("Ctrl+Shift+O")
            .clicked()
            || ui.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::O,
                )
            });
        if folder {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                self.open(p);
            }
        }
        if let Some(a) = &self.archive {
            ui.separator();
            ui.label(a.format());
            ui.label(format!("{} bytes", a.src.size));
        }
    }

    fn rebuild(&mut self) {
        self.visible.clear();
        let Some(a) = &self.archive else { return };
        let query = self.filter.trim().to_lowercase();
        match (&a.catalog, self.view) {
            (Catalog::Rds(ix), View::Textures) => {
                self.visible.extend(
                    ix.descs
                        .iter()
                        .filter(|d| {
                            query.is_empty()
                                || format!(
                                    "texture {} {}x{} {}",
                                    d.idx,
                                    d.w,
                                    d.h,
                                    tex::dxgi_info(d.dxgi).map(|x| x.0).unwrap_or("unknown")
                                )
                                .to_lowercase()
                                .contains(&query)
                        })
                        .map(|d| d.idx),
                );
            }
            (Catalog::Rds(ix), View::Records) => {
                self.visible.extend(
                    ix.chunks
                        .iter()
                        .enumerate()
                        .filter(|(i, c)| {
                            query.is_empty()
                                || format!(
                                    "Record {i} {} 0x{:X}",
                                    c.record.codec.label(),
                                    c.record.offset
                                )
                                .to_lowercase()
                                .contains(&query)
                        })
                        .map(|(i, _)| i),
                );
            }
            (Catalog::Rpf0(nodes), View::Members) => {
                if let Member::Folder(children) = &nodes[self.dir].member {
                    self.visible.extend(
                        children
                            .clone()
                            .filter(|i| nodes[*i].name.to_lowercase().contains(&query)),
                    );
                }
            }
            _ => {}
        }
    }

    fn set_view(&mut self, view: View) {
        self.reset();
        self.view = view;
        self.rebuild();
        if view == View::Hex {
            self.hex_page(0);
        }
    }

    fn select(&mut self, item: Item) {
        let Some(a) = self.archive.clone() else {
            return;
        };
        self.selected = Some(item);
        self.display = Display::Loading;
        self.preview_work.submit(move || match item {
            Item::Texture(i) => a.texture(i).map(Preview::Image),
            Item::Record(i) => Ok(Preview::Bytes(a.record(i)?.preview(&a.src)?, 0)),
        });
    }

    fn hex_page(&mut self, at: u64) {
        let Some(a) = self.archive.clone() else {
            return;
        };
        if at >= a.src.size && !(at == 0 && a.src.size == 0) {
            self.status = "Offset exceeds the archive".into();
            return;
        }
        self.hex_at = at;
        self.address = format!("{at:X}");
        self.display = Display::Loading;
        self.preview_work.submit(move || {
            Ok(Preview::Bytes(
                a.src
                    .read(at, (a.src.size - at).min(PREVIEW_BYTES as u64))?,
                at,
            ))
        });
    }

    fn export(&mut self, item: Item, archive: Arc<Archive>, output: Output) {
        let name = match (item, &archive.catalog) {
            (Item::Texture(i), _) => format!(
                "texture-{i}.{}",
                if matches!(output, Output::Dds) {
                    "dds"
                } else {
                    "png"
                }
            ),
            (Item::Record(i), Catalog::Rpf0(nodes)) => nodes[i].name.clone(),
            (Item::Record(i), _) => format!("record-{i}.bin"),
        };
        let mut dialog = rfd::FileDialog::new().set_file_name(name);
        if let Some(parent) = archive
            .path
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(Path::to_owned))
        {
            dialog = dialog.set_directory(parent);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        self.status = format!("Exporting {}", path.display());
        self.export_work.submit(move || {
            match item {
                Item::Record(i) => archive.record(i)?.export(&archive.src, &path)?,
                Item::Texture(i) => {
                    let t = archive.texture(i)?;
                    if matches!(output, Output::Dds) {
                        tex::export_texture_dds(&t, &path)?;
                    } else {
                        tex::export_texture_png(&t, &path)?;
                    }
                }
            }
            Ok(path)
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(path) = dropped {
            self.open(path);
        }
        let mut open = None;
        egui::SidePanel::left("archive_sources")
            .default_width(220.0)
            .min_width(130.0)
            .show(ctx, |ui| {
                ui.heading("Archives");
                if let Some(root) = &self.folder {
                    ui.label(
                        root.file_name()
                            .unwrap_or(root.as_os_str())
                            .to_string_lossy(),
                    )
                    .on_hover_text(root.display().to_string());
                }
                ui.separator();
                if let Some(root) = &self.folder {
                    let mut toggle = None;
                    egui::ScrollArea::vertical()
                        .id_salt("source_tree")
                        .show_rows(ui, 23.0, self.tree.rows.len(), |ui, range| {
                            for row in range {
                                let (depth, path) = &self.tree.rows[row];
                                let name = path.file_name().unwrap_or_default().to_string_lossy();
                                let dir = self.tree.children.contains_key(path);
                                let selected =
                                    self.archive.as_ref().is_some_and(|a| a.path == *path);
                                let label = if dir {
                                    format!(
                                        "{} {name}",
                                        if self.tree.expanded.contains(path) {
                                            "▾"
                                        } else {
                                            "▸"
                                        }
                                    )
                                } else {
                                    name.into_owned()
                                };
                                ui.horizontal(|ui| {
                                    ui.add_space((*depth).min(12) as f32 * 10.0);
                                    if ui
                                        .selectable_label(selected, label)
                                        .on_hover_text(path.display().to_string())
                                        .clicked()
                                    {
                                        if dir {
                                            toggle = Some(path.clone());
                                        } else {
                                            open = Some(path.clone());
                                        }
                                    }
                                });
                            }
                        });
                    if let Some(path) = toggle {
                        if !self.tree.expanded.remove(&path) {
                            self.tree.expanded.insert(path);
                        }
                        self.tree.flatten(root);
                    }
                    return;
                }
                egui::ScrollArea::both().show(ui, |ui| {
                    for path in &self.files {
                        let label = self
                            .folder
                            .as_ref()
                            .and_then(|r| path.strip_prefix(r).ok())
                            .or_else(|| path.file_name().map(Path::new))
                            .unwrap_or(path);
                        let selected = self.archive.as_ref().is_some_and(|a| a.path == *path);
                        if ui
                            .selectable_label(selected, label.display().to_string())
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            open = Some(path.clone());
                        }
                    }
                });
            });
        if let Some(path) = open {
            self.open(path);
        }
        let mut recovery = None;
        if let Some(a) = self.archive.clone() {
            if self.view != View::Hex {
                egui::SidePanel::left("archive_index")
                    .default_width(285.0)
                    .min_width(185.0)
                    .show(ctx, |ui| self.items(ui, &a));
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading(a.path.file_name().unwrap_or_default().to_string_lossy());
                ui.horizontal_wrapped(|ui| {
                    let mut choice = self.view;
                    match &a.catalog {
                        Catalog::Rds(ix) => {
                            if !ix.descs.is_empty() {
                                ui.selectable_value(&mut choice, View::Textures, "Textures");
                            }
                            ui.selectable_value(&mut choice, View::Records, "Data records");
                        }
                        Catalog::Rpf0(_) => {
                            ui.selectable_value(&mut choice, View::Members, "Files");
                        }
                        _ => {}
                    }
                    ui.selectable_value(&mut choice, View::Hex, "Archive hex");
                    if choice != self.view {
                        self.set_view(choice);
                    }
                });
                ui.separator();
                if let Catalog::Rds(ix) = &a.catalog {
                    if let Some(e) = &ix.descriptor_error {
                        ui.label(format!("Texture index unavailable · {e}"));
                    }
                }
                if let Catalog::Invalid(_, issue) = &a.catalog {
                    ui.label(format!("Archive index unavailable · {issue}"));
                }
                if matches!(a.catalog, Catalog::Rpf9) {
                    ui.label("RPF9 signature recognized. The directory layout is unverified.");
                    if ui.button("Search for embedded assets…").clicked() {
                        recovery = Some(a.path.clone());
                    }
                } else if matches!(a.catalog, Catalog::Unknown) {
                    ui.label("Unsupported archive signature.");
                }
                if self.view == View::Hex {
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(self.hex_at > 0, egui::Button::new("Previous"))
                            .clicked()
                        {
                            self.hex_page(self.hex_at.saturating_sub(PREVIEW_BYTES as u64));
                        }
                        if ui
                            .add_enabled(
                                a.src.size - self.hex_at > PREVIEW_BYTES as u64,
                                egui::Button::new("Next"),
                            )
                            .clicked()
                        {
                            self.hex_page(self.hex_at + PREVIEW_BYTES as u64);
                        }
                        let label = ui.label("Offset (hex)");
                        let field = ui
                            .add(egui::TextEdit::singleline(&mut self.address).desired_width(100.0))
                            .labelled_by(label.id);
                        if ui.button("Go").clicked()
                            || (field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            match u64::from_str_radix(
                                self.address.trim().trim_start_matches("0x"),
                                16,
                            ) {
                                Ok(at) => self.hex_page(at),
                                Err(_) => self.status = "Enter a hexadecimal offset".into(),
                            }
                        }
                    });
                } else if let Some(item) = self.selected {
                    self.details(ui, &a, item);
                }
                match &self.display {
                    Display::Empty => {
                        ui.label("Select an item to inspect it.");
                    }
                    Display::Loading => {
                        ui.label("Reading selection…");
                    }
                    Display::Error(e) => {
                        ui.label(format!("Preview unavailable · {e}"));
                    }
                    Display::Bytes(s) => {
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.monospace(s);
                        });
                    }
                    Display::Image(handle) => {
                        let original = handle.size_vec2();
                        let area = ui.available_size();
                        let scale = (area.x / original.x)
                            .min(area.y / original.y)
                            .clamp(0.01, 1.0);
                        ui.add(
                            egui::Image::new((handle.id(), original * scale))
                                .alt_text("Decoded archive texture"),
                        );
                    }
                }
            });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Archive browser");
                ui.label(if self.open_work.busy() {
                    "Reading archive index…"
                } else if self.files.is_empty() {
                    "Open an archive or drop a game folder here."
                } else {
                    "Choose an archive from the left."
                });
            });
        }
        if self.open_work.next.is_some()
            || self.preview_work.next.is_some()
            || self.export_work.next.is_some()
        {
            ctx.request_repaint();
        }
        recovery
    }

    fn items(&mut self, ui: &mut Ui, a: &Archive) {
        let title = match self.view {
            View::Textures => "Textures",
            View::Records => "Data records",
            _ => "Files",
        };
        ui.heading(title);
        if let Catalog::Rpf0(nodes) = &a.catalog {
            if self.dir != 0 {
                ui.horizontal(|ui| {
                    if ui.button("Up").clicked() {
                        self.dir = nodes[self.dir].parent;
                        self.filter.clear();
                        self.selected = None;
                        self.preview_work.clear();
                        self.display = Display::Empty;
                        self.rebuild();
                    }
                    ui.label(&nodes[self.dir].name);
                });
            }
        }
        let label = ui.label("Filter");
        let field = ui
            .add(egui::TextEdit::singleline(&mut self.filter).desired_width(f32::INFINITY))
            .labelled_by(label.id);
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            field.request_focus();
        }
        if field.changed() {
            self.rebuild();
        }
        ui.small(format!("{} items", self.visible.len()));
        ui.separator();
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt((self.dir, title))
            .show_rows(ui, 23.0, self.visible.len(), |ui, rows| {
                for row in rows {
                    let i = self.visible[row];
                    let (item, label) = match (&a.catalog, self.view) {
                        (Catalog::Rds(ix), View::Textures) => {
                            let d = &ix.descs[i];
                            (
                                Item::Texture(i),
                                format!("Texture {i}   ·   {} × {}", d.w, d.h),
                            )
                        }
                        (Catalog::Rpf0(nodes), _)
                            if matches!(nodes[i].member, Member::Folder(_)) =>
                        {
                            (Item::Record(i), format!("{}/", nodes[i].name))
                        }
                        _ => (Item::Record(i), Item::Record(i).name(a)),
                    };
                    if ui
                        .selectable_label(self.selected == Some(item), label)
                        .clicked()
                    {
                        chosen = Some((i, item));
                    }
                }
            });
        if let Some((i, item)) = chosen {
            if matches!(&a.catalog, Catalog::Rpf0(nodes) if matches!(nodes[i].member, Member::Folder(_)))
            {
                self.dir = i;
                self.selected = None;
                self.preview_work.clear();
                self.display = Display::Empty;
                self.filter.clear();
                self.rebuild();
            } else {
                self.select(item);
            }
        }
    }

    fn details(&mut self, ui: &mut Ui, a: &Arc<Archive>, item: Item) {
        ui.add_space(5.0);
        ui.label(
            RichText::new(item.name(a))
                .strong()
                .size(17.0)
                .color(egui::Color32::from_rgb(240, 233, 248)),
        );
        let (button, can_export, index_at) = match item {
            Item::Texture(i) => {
                let Catalog::Rds(ix) = &a.catalog else { return };
                let d = &ix.descs[i];
                ui.label(format!(
                    "{} × {}   ·   {}   ·   largest mip",
                    d.w,
                    d.h,
                    tex::dxgi_info(d.dxgi)
                        .map(|t| t.0)
                        .unwrap_or("Unknown DXGI format")
                ));
                ui.monospace(format!(
                    "Descriptor 0x{:X}   ·   virtual 0x{:X}",
                    ix.chunks[0].record.offset + 16 + i as u64 * 16,
                    d.byte_off
                ));
                let first = ix
                    .chunks
                    .partition_point(|c| c.start + c.record.size <= d.byte_off);
                let end = ix
                    .chunks
                    .partition_point(|c| c.start < d.byte_off + d.alloc_sz);
                ui.small(format!(
                    "Texture allocation · data records {first}–{}",
                    end - 1
                ));
                (
                    "Export PNG…",
                    matches!(self.display, Display::Image(_)),
                    ix.chunks[0].record.offset + 16 + i as u64 * 16,
                )
            }
            Item::Record(i) => {
                let Ok(r) = a.record(i) else { return };
                let at = match a.catalog {
                    Catalog::Rpf0(_) => 0x800 + i * 16,
                    _ => 16 + i * 8,
                };
                ui.label(format!(
                    "{}   ·   {} bytes stored   ·   {} bytes decoded",
                    r.codec.label(),
                    r.stored,
                    r.size
                ));
                ui.monospace(format!("Index row 0x{at:X}   ·   payload 0x{:X}", r.offset));
                ui.small(format!(
                    "Decoded hex preview · first {} bytes",
                    r.size.min(PREVIEW_BYTES as u64)
                ));
                let within_limit = r.codec == Codec::Stored
                    || r.size.max(r.stored) <= crate::archive::DECODE_LIMIT;
                (
                    "Extract file…",
                    within_limit && !matches!(r.codec, Codec::Unsupported(_)),
                    at as u64,
                )
            }
        };
        ui.horizontal(|ui| {
            let enabled = can_export && !self.export_work.busy();
            if ui.add_enabled(enabled, egui::Button::new(button)).clicked() {
                let output = if matches!(item, Item::Texture(_)) {
                    Output::Png
                } else {
                    Output::Payload
                };
                self.export(item, a.clone(), output);
            }
            if matches!(item, Item::Texture(_))
                && ui
                    .add_enabled(enabled, egui::Button::new("Export DDS…"))
                    .clicked()
            {
                self.export(item, a.clone(), Output::Dds);
            }
            if ui.button("View index bytes").clicked() {
                self.set_view(View::Hex);
                self.hex_page(index_at);
            }
        });
        ui.add_space(8.0);
    }
}
