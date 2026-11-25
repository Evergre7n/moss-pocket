//! 应用内的文件选择器。选择完成之前不读取文件正文，也不写入目标。

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use eframe::egui;
use mp_core::util::valid_file_name;

pub(crate) use mp_core::util::is_windows_device_name;

use crate::i18n::{self, Tr};
use crate::theme::{self, Palette};

pub(crate) enum PickerOutcome {
    Pending,
    Cancel,
    Selected(Vec<PathBuf>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Open,
    Save,
    Folder,
}

struct Entry {
    name: OsString,
    path: PathBuf,
    directory: bool,
}

struct DirectoryView {
    directory: PathBuf,
    entries: Vec<Entry>,
    partial: bool,
}

struct DirectoryRequest {
    path: PathBuf,
    mode: Mode,
    extensions: Vec<String>,
    cancel: Arc<AtomicBool>,
    result: Sender<io::Result<DirectoryView>>,
    ctx: egui::Context,
}

struct DirectoryLoad {
    path: PathBuf,
    allow_fallback: bool,
    cancel: Arc<AtomicBool>,
    result: Receiver<io::Result<DirectoryView>>,
    waiting: Option<DirectoryRequest>,
}

impl Drop for DirectoryLoad {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl DirectoryLoad {
    fn start(
        path: PathBuf,
        mode: Mode,
        extensions: Vec<String>,
        allow_fallback: bool,
        ctx: &egui::Context,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, result) = mpsc::channel();
        let waiting = Some(DirectoryRequest {
            path: path.clone(), mode, extensions, cancel: cancel.clone(), result: sender, ctx: ctx.clone(),
        });
        Self { path, allow_fallback, cancel, result, waiting }
    }

    fn poll(&mut self, ctx: &egui::Context) -> Option<io::Result<DirectoryView>> {
        if let Some(request) = self.waiting.take() {
            if let Some(worker) = directory_worker() {
                if let Err(TrySendError::Full(request)) = worker.try_send(request) {
                    self.waiting = Some(request);
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return None;
                }
            }
        }
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(io::Error::from(io::ErrorKind::BrokenPipe))),
        }
    }
}

fn directory_worker() -> Option<&'static SyncSender<DirectoryRequest>> {
    static WORKER: OnceLock<Option<SyncSender<DirectoryRequest>>> = OnceLock::new();
    WORKER.get_or_init(start_directory_workers).as_ref()
}

fn start_directory_workers() -> Option<SyncSender<DirectoryRequest>> {
    let (sender, receiver) = mpsc::sync_channel::<DirectoryRequest>(2);
    let receiver = Arc::new(Mutex::new(receiver));
    let mut started = false;
    for index in 0..2 {
        let receiver = receiver.clone();
        started |= std::thread::Builder::new().name(format!("file-picker-reader-{index}")).spawn(move || {
            loop {
                let request = receiver.lock().unwrap().recv();
                let Ok(request) = request else { break };
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let result = read_directory(&request.path, request.mode, &request.extensions, &request.cancel);
                if request.result.send(result).is_ok() {
                    request.ctx.request_repaint();
                }
            }
        }).is_ok();
    }
    started.then_some(sender)
}

enum PickerError {
    Folder(PathBuf, io::Error),
    PartialFolder,
    FileName,
    Target(PathBuf, Option<io::Error>),
    Missing(PathBuf),
}

impl PickerError {
    fn message(&self, tr: Tr) -> String {
        match self {
            Self::Folder(path, error) => {
                format!("{} · {} ({error})", tr.picker_bad_folder(), display_path(path))
            }
            Self::PartialFolder => tr.picker_partial_folder().to_owned(),
            Self::FileName => tr.picker_bad_file_name().to_owned(),
            Self::Target(path, None) => format!("{} · {}", tr.picker_bad_target(), display_path(path)),
            Self::Target(path, Some(error)) => {
                format!("{} · {} ({error})", tr.picker_bad_target(), display_path(path))
            }
            Self::Missing(path) => format!("{} · {}", tr.picker_missing_file(), display_path(path)),
        }
    }
}

pub(crate) struct FilePicker {
    mode: Mode,
    title: String,
    filter: String,
    extensions: Vec<String>,
    directory: PathBuf,
    location: String,
    entries: Vec<Entry>,
    initial_dir: Option<PathBuf>,
    reading: Option<DirectoryLoad>,
    drives: Vec<PathBuf>,
    selected: Vec<PathBuf>,
    focused_row: Option<(usize, egui::Id)>,
    file_name: String,
    overwrite: Option<PathBuf>,
    error: Option<PickerError>,
}

impl FilePicker {
    pub(crate) fn open(title: &str, filter: &str, extensions: &[&str], initial_dir: PathBuf) -> Self {
        Self::new(Mode::Open, title, filter, extensions, String::new(), initial_dir)
    }

    pub(crate) fn save(
        title: &str,
        filter: &str,
        extension: Option<&str>,
        file_name: String,
        initial_dir: PathBuf,
    ) -> Self {
        Self::new(Mode::Save, title, filter, extension.as_slice(), file_name, initial_dir)
    }

    /// Markdown 的文件名由笔记生成，用户选择放置正文和媒体的文件夹。
    pub(crate) fn folder(title: &str, target_name: String, initial_dir: PathBuf) -> Self {
        Self::new(Mode::Folder, title, "", &[], target_name, initial_dir)
    }

    fn new(
        mode: Mode,
        title: &str,
        filter: &str,
        extensions: &[&str],
        file_name: String,
        initial_dir: PathBuf,
    ) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let initial_dir = cwd.join(initial_dir);
        Self {
            mode,
            title: title.to_owned(),
            filter: filter.to_owned(),
            extensions: extensions.iter().map(|extension| extension.to_ascii_lowercase()).collect(),
            directory: cwd,
            location: display_path(&initial_dir),
            entries: Vec::new(),
            initial_dir: Some(initial_dir),
            reading: None,
            drives: available_drives(),
            selected: Vec::new(),
            focused_row: None,
            file_name,
            overwrite: None,
            error: None,
        }
    }

    fn navigate(&mut self, path: PathBuf, allow_fallback: bool, ctx: &egui::Context) {
        let path = self.directory.join(path);
        self.location = display_path(&path);
        self.error = None;
        self.focused_row = None;
        self.reading = Some(DirectoryLoad::start(path, self.mode, self.extensions.clone(), allow_fallback, ctx));
        ctx.request_repaint();
    }

    fn poll_navigation(&mut self, ctx: &egui::Context) {
        if let Some(path) = self.initial_dir.take() {
            self.navigate(path, true, ctx);
        }
        let Some(load) = self.reading.as_mut() else { return };
        let Some(result) = load.poll(ctx) else { return };
        let load = self.reading.take().unwrap();
        let same_location = self.location == display_path(&load.path);
        match result {
            Ok(view) => {
                if same_location {
                    self.location = display_path(&view.directory);
                }
                self.directory = view.directory;
                self.entries = view.entries;
                if self.error.is_none() && view.partial {
                    self.error = Some(PickerError::PartialFolder);
                }
            }
            Err(error) => {
                if same_location {
                    self.location = display_path(&self.directory);
                }
                if same_location && load.allow_fallback && load.path != self.directory {
                    self.navigate(self.directory.clone(), false, ctx);
                }
                self.error = Some(PickerError::Folder(load.path.clone(), error));
            }
        }
    }

    fn toggle_selection(&mut self, path: PathBuf) {
        if let Some(position) = self.selected.iter().position(|selected| *selected == path) {
            self.selected.remove(position);
        } else {
            self.selected.push(path);
        }
        self.error = None;
    }

    fn save_target(&self) -> Result<PathBuf, PickerError> {
        let name = self.file_name.trim();
        let name_path = Path::new(name);
        if name.ends_with(['/', '\\', '.']) || !name_path.file_name().and_then(OsStr::to_str).is_some_and(valid_file_name) {
            return Err(PickerError::FileName);
        }
        let mut target = self.directory.join(name_path);
        if target.is_dir() {
            return Err(PickerError::Target(target, None));
        }
        if target.extension().is_none() {
            if let Some(extension) = self.extensions.first() {
                target.set_extension(extension);
            }
        }
        let parent = target.parent().unwrap_or(&self.directory);
        match fs::metadata(parent) {
            Ok(metadata) if metadata.is_dir() => Ok(target),
            Ok(_) => Err(PickerError::Target(target, None)),
            Err(error) => Err(PickerError::Target(target, Some(error))),
        }
    }

    fn accept(&mut self) -> PickerOutcome {
        if self.mode == Mode::Open {
            if self.selected.is_empty() {
                return PickerOutcome::Pending;
            }
            if let Some(path) = self.selected.iter().find(|path| !path.is_file()) {
                self.error = Some(PickerError::Missing(path.clone()));
                return PickerOutcome::Pending;
            }
            return PickerOutcome::Selected(std::mem::take(&mut self.selected));
        }
        let target = match self.save_target() {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error);
                return PickerOutcome::Pending;
            }
        };
        match fs::symlink_metadata(&target) {
            Ok(_) if target.is_file() => {
                self.overwrite = Some(target);
                self.error = None;
            }
            Ok(_) => self.error = Some(PickerError::Target(target, None)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return PickerOutcome::Selected(vec![target]);
            }
            Err(error) => self.error = Some(PickerError::Target(target, Some(error))),
        }
        PickerOutcome::Pending
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, tr: Tr) -> PickerOutcome {
        self.poll_navigation(ui.ctx());
        let palette = Palette::for_ui(ui);
        let screen = ui.ctx().content_rect();
        let confirming = self.overwrite.is_some();
        let id = egui::Id::unique("moss-file-picker").with(confirming);
        let frame = egui::Frame::window(ui.style())
            .fill(palette.surface)
            .stroke(egui::Stroke::new(1.0, palette.line))
            .inner_margin(16.0)
            .corner_radius(8);
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).fade_in(false))
            .frame(frame)
            .show(ui.ctx(), |ui| {
                let width = if confirming { 460.0 } else { 610.0 };
                ui.set_width((screen.width() - 48.0).clamp(220.0, width));
                ui.spacing_mut().item_spacing.y = 6.0;
                if self.overwrite.is_some() {
                    return self.confirm_overwrite(ui, tr, palette);
                }
                let list_height = (screen.height() - 236.0).clamp(56.0, 340.0);
                self.show_picker(ui, tr, palette, list_height)
            });
        if matches!(modal.inner, PickerOutcome::Pending)
            && modal.is_top_modal
            && !modal.any_popup_open
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            ui.ctx().request_repaint();
            if self.overwrite.take().is_some() {
                return PickerOutcome::Pending;
            }
            return PickerOutcome::Cancel;
        }
        if confirming != self.overwrite.is_some() || !matches!(modal.inner, PickerOutcome::Pending) {
            ui.ctx().request_repaint();
        }
        modal.inner
    }

    fn show_picker(&mut self, ui: &mut egui::Ui, tr: Tr, palette: Palette, list_height: f32) -> PickerOutcome {
        ui.label(egui::RichText::new(&self.title).font(theme::bold(18.0)).color(palette.ink));
        self.location_row(ui, tr);
        if !self.filter.is_empty() {
            ui.label(egui::RichText::new(&self.filter).small().color(palette.muted));
        }
        self.entry_list(ui, tr, palette, list_height);
        let accept_name = match self.mode {
            Mode::Open => {
                ui.horizontal(|ui| {
                    ui.label(i18n::picker_selected(tr, self.selected.len()));
                    if ui.add_enabled(!self.selected.is_empty(), theme::quiet_button(tr.picker_clear())).clicked() {
                        self.selected.clear();
                        self.error = None;
                    }
                });
                false
            }
            Mode::Save => {
                let label = ui.label(tr.picker_file_name());
                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.file_name)
                        .id(egui::Id::unique("picker-filename"))
                        .desired_width(f32::INFINITY),
                ).labelled_by(label.id);
                if edit.changed() {
                    self.error = None;
                }
                edit.lost_focus() && enter
            }
            Mode::Folder => {
                ui.horizontal(|ui| {
                    ui.label(tr.picker_file_name());
                    ui.add(egui::Label::new(&self.file_name).truncate()).on_hover_text(&self.file_name);
                });
                false
            }
        };
        if let Some(error) = &self.error {
            let message = error.message(tr);
            ui.add(egui::Label::new(egui::RichText::new(&message).color(palette.danger)).truncate())
                .on_hover_text(message);
        }
        let (action, enabled) = if self.mode == Mode::Open {
            (tr.picker_open(), !self.selected.is_empty())
        } else {
            (tr.picker_save(), !self.file_name.trim().is_empty())
        };
        let enabled = enabled && self.reading.is_none();
        let actions = egui::vec2(ui.available_width(), 30.0);
        ui.allocate_ui_with_layout(actions, egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let accepted = ui.add_enabled(enabled, theme::primary_button(action, palette)).clicked();
            if ui.add(theme::quiet_button(tr.picker_cancel())).clicked() {
                PickerOutcome::Cancel
            } else if accepted || (enabled && accept_name) {
                self.accept()
            } else {
                PickerOutcome::Pending
            }
        }).inner
    }

    fn location_row(&mut self, ui: &mut egui::Ui, tr: Tr) {
        let label = ui.label(tr.picker_location());
        let parent = self.directory.parent().map(Path::to_path_buf);
        let mut navigate = None;
        ui.horizontal(|ui| {
            let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
            let go_width = ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap(tr.picker_go().to_owned(), egui::TextStyle::Button.resolve(ui.style()), egui::Color32::WHITE).size().x
            }) + ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x + 12.0;
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.location)
                    .id(egui::Id::unique("picker-location"))
                    .desired_width((ui.available_width() - go_width).max(30.0)),
            ).labelled_by(label.id);
            if ui.button(tr.picker_go()).clicked() || (edit.lost_focus() && enter) {
                let location = self.location.trim();
                if !location.is_empty() {
                    navigate = Some(PathBuf::from(location));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(parent.is_some(), theme::quiet_button(tr.picker_parent())).clicked() {
                navigate = parent;
            }
            if !self.drives.is_empty() {
                egui::ComboBox::from_id_salt("picker-drives")
                    .selected_text(tr.picker_drives())
                    .show_ui(ui, |ui| {
                        for drive in &self.drives {
                            if ui.selectable_label(false, display_path(drive)).clicked() {
                                navigate = Some(drive.clone());
                            }
                        }
                    });
            }
        });
        if let Some(path) = navigate {
            self.navigate(path, false, ui.ctx());
        }
    }

    fn entry_list(&mut self, ui: &mut egui::Ui, tr: Tr, palette: Palette, height: f32) {
        const ROW_HEIGHT: f32 = 26.0;
        const ROW_GAP: f32 = 2.0;
        let mut action = None;
        let mut focused_row = None;
        egui::Frame::new()
            .fill(palette.rail)
            .stroke(egui::Stroke::new(1.0, palette.line))
            .corner_radius(5)
            .inner_margin(4.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                if self.reading.is_some() {
                    ui.set_min_height(height);
                    ui.label(egui::RichText::new(tr.picker_loading()).color(palette.muted));
                    return;
                }
                if self.entries.is_empty() {
                    ui.set_min_height(height);
                    ui.label(egui::RichText::new(tr.picker_empty()).color(palette.muted));
                    return;
                }
                ui.spacing_mut().item_spacing.y = ROW_GAP;
                ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0, palette.muted);
                let focus_next = self.next_focused_row(ui);
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt(("picker-files", &self.directory))
                    .max_height(height)
                    .auto_shrink([false, false]);
                if let Some(index) = focus_next {
                    let offset = index as f32 * (ROW_HEIGHT + ROW_GAP) - (height - ROW_HEIGHT) * 0.5;
                    scroll = scroll.vertical_scroll_offset(offset.max(0.0));
                }
                scroll.show_rows(ui, ROW_HEIGHT, self.entries.len(), |ui, rows| {
                        for index in rows {
                            let entry = &self.entries[index];
                            let name = entry.name.to_string_lossy();
                            let (response, activated) = ui.push_id(&entry.path, |ui| {
                                if self.mode == Mode::Open && !entry.directory {
                                    let mut checked = self.selected.contains(&entry.path);
                                    let response = ui.allocate_ui_with_layout(
                                        egui::vec2(ui.available_width(), ROW_HEIGHT),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| ui.checkbox(&mut checked, name.as_ref()),
                                    ).inner;
                                    let activated = response.changed();
                                    (response, activated)
                                } else {
                                    let button = if entry.directory {
                                        egui::Button::new(("›", name.as_ref())).frame_when_inactive(false)
                                    } else {
                                        theme::quiet_button(name.as_ref())
                                    };
                                    let response = ui.add(button.truncate().min_size(egui::vec2(0.0, ROW_HEIGHT)));
                                    let activated = response.clicked();
                                    (response, activated)
                                }
                            }).inner;
                            let response = response.on_hover_text(name.as_ref());
                            if focus_next == Some(index) {
                                response.request_focus();
                                ui.ctx().request_repaint();
                            }
                            if response.has_focus() {
                                focused_row = Some((index, response.id));
                            }
                            if activated {
                                action = Some(index);
                            }
                        }
                    });
            });
        self.focused_row = focused_row;
        if let Some(index) = action {
            let entry = &self.entries[index];
            if entry.directory {
                self.navigate(entry.path.clone(), false, ui.ctx());
            } else if self.mode == Mode::Open {
                self.toggle_selection(entry.path.clone());
            } else {
                self.file_name = entry.name.to_string_lossy().into_owned();
                self.error = None;
            }
        }
    }

    fn next_focused_row(&self, ui: &mut egui::Ui) -> Option<usize> {
        let (index, id) = self.focused_row?;
        if !ui.memory(|memory| memory.has_focus(id)) {
            return None;
        }
        ui.input_mut(|input| {
            let last = self.entries.len() - 1;
            let mut pressed = |key| input.consume_key(egui::Modifiers::NONE, key);
            if pressed(egui::Key::ArrowDown) {
                Some((index + 1).min(last))
            } else if pressed(egui::Key::ArrowUp) {
                Some(index.saturating_sub(1))
            } else if pressed(egui::Key::Home) {
                Some(0)
            } else if pressed(egui::Key::End) {
                Some(last)
            } else {
                None
            }
        })
    }

    fn confirm_overwrite(&mut self, ui: &mut egui::Ui, tr: Tr, palette: Palette) -> PickerOutcome {
        ui.label(egui::RichText::new(tr.picker_replace_question()).font(theme::bold(18.0)).color(palette.ink));
        let target = display_path(self.overwrite.as_ref().unwrap());
        ui.add(egui::Label::new(target).wrap());
        ui.add_space(8.0);
        let actions = egui::vec2(ui.available_width(), 30.0);
        ui.allocate_ui_with_layout(actions, egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(theme::primary_button(tr.picker_replace(), palette)).clicked() {
                return PickerOutcome::Selected(vec![self.overwrite.take().unwrap()]);
            }
            if ui.add(theme::quiet_button(tr.picker_keep_editing())).clicked() {
                self.overwrite = None;
            }
            PickerOutcome::Pending
        }).inner
    }
}

fn read_directory(path: &Path, mode: Mode, extensions: &[String], cancel: &AtomicBool) -> io::Result<DirectoryView> {
    let directory = fs::canonicalize(path)?;
    let mut entries = Vec::new();
    let mut partial = false;
    for entry in fs::read_dir(&directory)? {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            partial = true;
            continue;
        };
        let (directory, file) = if kind.is_symlink() {
            match fs::metadata(&path) {
                Ok(target) => (target.is_dir(), target.is_file()),
                Err(_) => {
                    partial = true;
                    continue;
                }
            }
        } else {
            (kind.is_dir(), kind.is_file())
        };
        let matches_filter = extensions.is_empty() || path.extension().and_then(OsStr::to_str)
            .is_some_and(|ext| extensions.iter().any(|allowed| allowed.eq_ignore_ascii_case(ext)));
        if directory || (file && mode != Mode::Folder && matches_filter) {
            entries.push(Entry { name: entry.file_name(), path, directory });
        }
    }
    entries.sort_by_cached_key(|entry| {
        (!entry.directory, entry.name.to_string_lossy().to_lowercase(), entry.name.clone())
    });
    Ok(DirectoryView { directory, entries, partial })
}

#[cfg(windows)]
fn available_drives() -> Vec<PathBuf> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    // 返回的位掩码代表系统分配的盘符。这里不读取磁盘或连接网络盘。
    let mask = unsafe { GetLogicalDrives() };
    (0..26).filter(|index| mask & (1 << index) != 0)
        .map(|index| PathBuf::from(format!("{}:\\", char::from(b'A' + index))))
        .collect()
}

#[cfg(not(windows))]
fn available_drives() -> Vec<PathBuf> {
    Vec::new()
}

fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    text.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or_else(|| text.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let directory = std::env::temp_dir().join(format!(
                "moss-picker-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&directory).unwrap();
            Self(fs::canonicalize(directory).unwrap())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for_folder(picker: &mut FilePicker, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            picker.poll_navigation(ctx);
            if picker.reading.is_none() {
                return;
            }
            assert!(Instant::now() < deadline, "folder load exceeded test deadline");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn open_filters_files_and_preserves_selection_order_across_folders() {
        let Scratch(ref root) = Scratch::new();
        let child = root.join("child");
        let first = root.join("z.PNG");
        let second = child.join("a.png");
        let third = root.join("b.png");
        fs::create_dir(&child).unwrap();
        for file in [&first, &second, &third, &root.join("ignored.txt")] {
            fs::write(file, b"test").unwrap();
        }

        let ctx = egui::Context::default();
        let mut picker = FilePicker::open("Open", "Images", &["png"], root.clone());
        wait_for_folder(&mut picker, &ctx);
        let entries: Vec<_> = picker.entries.iter().map(|entry| entry.name.as_os_str()).collect();
        assert_eq!(entries, ["child", "b.png", "z.PNG"].map(OsStr::new));
        picker.toggle_selection(first.clone());
        picker.navigate(child.clone(), false, &ctx);
        wait_for_folder(&mut picker, &ctx);
        assert_eq!(picker.entries[0].path, second);
        picker.toggle_selection(second.clone());
        picker.navigate(root.clone(), false, &ctx);
        wait_for_folder(&mut picker, &ctx);
        picker.toggle_selection(third.clone());
        picker.toggle_selection(first.clone());
        picker.toggle_selection(first.clone());
        let PickerOutcome::Selected(paths) = picker.accept() else { panic!("missing selection") };
        assert_eq!(paths, [second, third, first]);
    }

    #[test]
    fn only_latest_navigation_applies_and_unavailable_folder_keeps_the_current_list() {
        let Scratch(ref root) = Scratch::new();
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(second.join("present.png"), b"example").unwrap();

        let ctx = egui::Context::default();
        let mut picker = FilePicker::open("Open", "Images", &["png"], root.clone());
        wait_for_folder(&mut picker, &ctx);
        picker.navigate(first, false, &ctx);
        let cancel = picker.reading.as_ref().unwrap().cancel.clone();
        picker.poll_navigation(&ctx);
        picker.navigate(second.clone(), false, &ctx);
        assert!(cancel.load(Ordering::Relaxed));
        wait_for_folder(&mut picker, &ctx);
        assert_eq!(picker.directory, second);

        picker.navigate(root.join("missing"), false, &ctx);
        wait_for_folder(&mut picker, &ctx);
        assert!(matches!(picker.error, Some(PickerError::Folder(..))));
        assert_eq!(picker.directory, second);
        assert_eq!(picker.entries[0].path, second.join("present.png"));
    }

    #[test]
    fn exporting_suggests_an_extension_and_prompts_for_existing_targets() {
        let Scratch(ref root) = Scratch::new();
        let ctx = egui::Context::default();
        let name = "笔记";
        let target = root.join(format!("{name}.docx"));
        let mut picker = FilePicker::save("Export", "Word", Some("docx"), name.to_owned(), root.clone());
        wait_for_folder(&mut picker, &ctx);
        let PickerOutcome::Selected(paths) = picker.accept() else { panic!("missing save target") };
        assert_eq!(paths, [target.clone()]);
        assert!(!target.exists());

        fs::write(&target, b"existing document").unwrap();
        assert!(matches!(picker.accept(), PickerOutcome::Pending));
        assert_eq!(picker.overwrite.as_deref(), Some(target.as_path()));
        assert_eq!(fs::read(&target).unwrap(), b"existing document");

        let markdown_target = root.join(format!("{name}.md"));
        fs::write(&markdown_target, b"existing Markdown").unwrap();
        let mut markdown = FilePicker::folder("Export", format!("{name}.md"), root.clone());
        wait_for_folder(&mut markdown, &ctx);
        assert!(markdown.entries.is_empty());
        assert!(matches!(markdown.accept(), PickerOutcome::Pending));
        assert_eq!(markdown.overwrite.as_deref(), Some(markdown_target.as_path()));
        assert_eq!(fs::read(&markdown_target).unwrap(), b"existing Markdown");
    }

    #[test]
    fn export_suggestions_avoid_windows_device_names() {
        for name in ["CONIN$", "conout$.backup", "COM¹", "lpt² .old", "PRN .txt"] {
            assert!(!valid_file_name(&format!("{name}.md")));
            let safe = crate::export::safe_file_name(name);
            assert_eq!(safe, format!("_{name}"));
            assert!(valid_file_name(&format!("{safe}.md")));
        }
        assert!(valid_file_name("COM10.md"));
    }
}
