//! 块式编辑器的界面。编辑决策全部在 mp-core 的 `Editor` 里，这里只负责画出来、转发按键。
//!
//! 媒体的分工：导入和解码在后台线程（见 `media.rs`），这里每帧收一次结果；
//! 图片预览与音视频播放都留在笔记内，原始媒体原样保存在库里。

use crate::file_picker::{FilePicker, PickerOutcome};
use crate::i18n::{format_time, Tr};
use crate::media::{format_size, ImportSource};
use crate::media_view::{self, clock, MediaViewer};
use crate::session::Session;
use crate::settings::Settings;
use crate::theme::{self, Palette};
use eframe::egui::{self};
use mp_core::editor::audio_name_parts;
use mp_core::media::MediaKind;
use mp_core::model::{
    byte_to_char, char_count, char_to_byte, Block, BlockId, ImageBlock, InlineStyle, MediaBlock, Note, StyleFlag,
    StyleSpan, TextBlock, TextKind, TitleSource,
};
use mp_core::util::now_millis;
use std::path::PathBuf;
use std::sync::Arc;

/// 上一帧有焦点的文本块和它的光标。
#[derive(Clone, Copy)]
struct Focused {
    block: BlockId,
    caret: usize,
    anchor: usize,
    row: usize,
    column: usize,
    rows: usize,
}

impl Focused {
    /// 选区的字符范围；没有选区时返回 None。
    fn selection(&self) -> Option<(u32, u32)> {
        (self.caret != self.anchor)
            .then(|| (self.caret.min(self.anchor) as u32, self.caret.max(self.anchor) as u32))
    }
}

#[derive(Default)]
pub struct EditorUi {
    /// 待执行的聚焦请求：(文本块, 光标的字符位置, 选区另一头的字符位置)。
    focus: Option<(BlockId, usize, usize)>,
    focused: Option<Focused>,
    /// 工具栏拿到焦点以后，仍可在最近的正文位置插入媒体。
    media_anchor: Option<BlockId>,
    media_anchor_revision: u64,
    /// 打开插入菜单时的锚点，直到选中媒体类型后再使用。
    insert_menu_anchor: (Option<BlockId>, u64),
    /// 保留输入法的组字缓冲。确认前，笔记和自动保存仍用已提交的正文。
    ime_draft: Option<(BlockId, String)>,
    /// 正在拖动改变大小的图片：(块, 按下时的宽度, 当前宽度)。
    image_drag: Option<(BlockId, f32, f32)>,
    audio_rename: Option<AudioRename>,
    viewer: Option<MediaViewer>,
    file_picker: Option<(FilePicker, AttachmentAction)>,
}

struct AudioRename {
    block: BlockId,
    stem: String,
    preedit: Option<String>,
    focus_on_show: bool,
    invalid: bool,
}

struct ImeInput {
    composing: bool,
    committed: bool,
    canceled: bool,
}

impl ImeInput {
    fn read(ui: &egui::Ui, active: bool, had_draft: bool) -> Self {
        let mut state = Self { composing: had_draft, committed: false, canceled: false };
        if active {
            ui.input(|input| {
                for event in &input.events {
                    match event {
                        egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                            state.canceled |= state.composing && text.is_empty();
                            state.composing = !text.is_empty();
                        }
                        egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                            state.canceled |= state.composing && text.is_empty();
                            state.composing = false;
                            state.committed |= !text.is_empty();
                        }
                        _ => {}
                    }
                }
            });
        }
        state
    }

    /// 同一帧提交了一段文字后又开始组字时，留下已提交的内容。
    fn committed_prefix(&self, buffer: &str, cursor: Option<egui::text::CCursorRange>) -> Option<String> {
        if !self.committed {
            return None;
        }
        let range = cursor?.as_sorted_char_range();
        let start = char_to_byte(buffer, range.start.0);
        let end = char_to_byte(buffer, range.end.0);
        let mut accepted = buffer.to_owned();
        accepted.replace_range(start..end, "");
        Some(accepted)
    }
}

enum AttachmentAction {
    Import { anchor: Option<BlockId>, revision: u64 },
    Save { blob_id: String },
}

impl EditorUi {
    /// 切换笔记时清空界面状态，让尚未完成的旧导入无法接管新锚点。
    pub fn reset(&mut self) {
        *self = Self {
            media_anchor_revision: self.media_anchor_revision.wrapping_add(1),
            ..Self::default()
        };
    }

    pub fn request_focus(&mut self, block: BlockId, caret: usize) {
        self.request_selection(block, caret, caret);
    }

    /// 把焦点挪到某个文本块，并选中从 `start` 到 `end` 的字符。两头一样就是没有选区。
    pub fn request_selection(&mut self, block: BlockId, start: usize, end: usize) {
        self.focus = Some((block, start, end));
    }

    /// 有焦点的文本块和光标位置。
    pub fn caret_position(&self) -> Option<(BlockId, usize)> {
        self.focused.map(|focused| (focused.block, focused.caret))
    }

    /// 正文里选中的文字。没有选区、或者选区跨了行时返回 None。
    pub fn selected_text(&self, note: &Note) -> Option<String> {
        let focused = self.focused?;
        let text = note.blocks.iter().filter_map(Block::as_text).find(|text| text.id == focused.block)?;
        let (start, end) = (focused.caret.min(focused.anchor), focused.caret.max(focused.anchor));
        let picked: String = text.text.chars().skip(start).take(end - start).collect();
        (!picked.is_empty() && !picked.contains('\n')).then_some(picked)
    }

    fn caret_of(&self, block: BlockId) -> usize {
        self.focused.filter(|focused| focused.block == block).map(|focused| focused.caret).unwrap_or(0)
    }

    /// 有焦点的文本块；没有时返回 None。
    pub fn focused_block(&self) -> Option<BlockId> {
        self.focused.map(|focused| focused.block)
    }

    pub fn media_anchor(&self) -> Option<BlockId> {
        self.media_anchor
    }

    pub fn media_anchor_revision(&self) -> u64 {
        self.media_anchor_revision
    }

    fn select_media_anchor(&mut self, block: BlockId) {
        self.media_anchor = Some(block);
        self.media_anchor_revision = self.media_anchor_revision.wrapping_add(1);
    }

    /// 连续插入跟在上一项后面。用户重新选了正文位置时，保留其选择。
    pub fn follow_media_insertion(&mut self, expected_revision: u64, block: BlockId) {
        if self.media_anchor_revision == expected_revision {
            self.media_anchor = Some(block);
        }
    }

    pub(crate) fn overlay_open(&self) -> bool {
        self.viewer.is_some() || self.file_picker.is_some()
    }

    pub(crate) fn capture_audio_error(&mut self, message: &str) -> bool {
        self.viewer.as_mut().is_some_and(|viewer| viewer.capture_audio_error(message))
    }

    pub(crate) fn take_audio_name(&mut self) -> Option<(BlockId, String)> {
        self.audio_rename.take().map(|draft| (draft.block, draft.stem))
    }
}

/// 块的字号和是否加粗。第一条文本块当标题用。
fn kind_look(kind: TextKind, is_title: bool, base: f32) -> (f32, bool) {
    if is_title && kind == TextKind::Paragraph {
        return ((base * 2.78).min(60.0), false);
    }
    match kind {
        TextKind::Heading1 => (base * 1.76, false),
        TextKind::Heading2 => (base * 1.42, false),
        TextKind::Heading3 => (base * 1.1, true),
        _ => (base, false),
    }
}

pub fn show(ui: &mut egui::Ui, session: &mut Session, settings: &Settings, tr: &Tr) {
    let palette = Palette::for_ui(ui);
    process_dropped_media(ui, session);
    if session.current.is_none() {
        empty_editor(ui, palette, session.view.show_trash, tr);
        return;
    }
    if session.editor_ui.audio_rename.is_some() {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            session.editor_ui.audio_rename = None;
        } else if !ui.is_enabled() {
            session.finish_audio_rename();
        }
    }
    // 按键要在任何文本框处理之前接管，所以放在画正文之前。
    if ui.is_enabled() && !session.current_in_trash {
        // Ctrl+F 查找，Ctrl+H 查找并替换。正文里选中了文字就拿它当查找内容，从光标处往后找。
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            open_find(session, false);
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::H)) {
            open_find(session, true);
        }
        // 查找栏开着时，F3 和 Shift+F3 在哪儿按都好使。
        if session.find.open {
            for (modifiers, delta) in [(egui::Modifiers::SHIFT, -1), (egui::Modifiers::NONE, 1)] {
                if ui.input_mut(|i| i.consume_key(modifiers, egui::Key::F3)) {
                    session.find.step(session.editor.note(), session.editor.revision(), delta);
                }
            }
        }
        if session.editor_ui.audio_rename.is_none() {
            handle_keys(ui, session, tr);
        }
    }
    editor_toolbar(ui, session, palette, tr);

    let mut focused_now = None;
    egui::ScrollArea::vertical()
        .id_salt(("editor-scroll", session.current.clone()))
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            reading_column(ui, |ui| {
                ui.add_space(theme::SECTION_GAP + theme::SMALL_GAP);
                focused_now = show_blocks(ui, session, settings, tr);
                ui.add_space(56.0);
            });
        });
    session.editor_ui.focused = focused_now;
}

pub(crate) fn show_overlay(ui: &mut egui::Ui, session: &mut Session, tr: Tr) {
    if let Some(viewer) = &mut session.editor_ui.viewer {
        if !viewer.show(ui, &mut session.media, tr) {
            session.editor_ui.viewer = None;
        }
    }
    if let Some((picker, action)) = session.editor_ui.file_picker.take() {
        let mut picker = picker;
        match picker.show(ui, tr) {
            PickerOutcome::Pending => session.editor_ui.file_picker = Some((picker, action)),
            PickerOutcome::Cancel => ui.ctx().request_repaint(),
            PickerOutcome::Selected(paths) => {
                apply_attachment_action(session, action, paths);
                ui.ctx().request_repaint();
            }
        }
    }
}

pub(crate) fn hide_native_overlay(ui: &egui::Ui, session: &Session) {
    if let Some(viewer) = &session.editor_ui.viewer {
        viewer.hide_native(ui.ctx());
    }
}

fn reading_column(
    ui: &mut egui::Ui,
    content: impl FnOnce(&mut egui::Ui),
) {
    let available = ui.available_width();
    let margin = if available < 540.0 { 14 } else { 24 };
    let width = (available - 32.0).clamp(1.0, 714.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(((available - width) / 2.0).max(0.0));
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(margin, 0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width((width - 2.0 * f32::from(margin)).max(1.0));
                    ui.spacing_mut().item_spacing.x = theme::SMALL_GAP;
                    content(ui);
                });
            });
    });
}

fn empty_editor(ui: &mut egui::Ui, palette: Palette, trash: bool, tr: &Tr) {
    ui.add_space((ui.available_height() * 0.30).min(220.0));
    reading_column(ui, |ui| {
        let (heading, hint) = if trash {
            (tr.trash(), tr.trash_description())
        } else {
            (tr.no_note_open(), tr.no_note_selected())
        };
        ui.label(egui::RichText::new(heading).font(crate::fonts::note_font(32.0, true, false)).color(palette.ink));
        ui.label(egui::RichText::new(hint).color(palette.muted));
    });
}

fn open_find(session: &mut Session, replace: bool) {
    let selected = session.editor_ui.selected_text(session.editor.note());
    let origin = session.editor_ui.caret_position();
    session.find.open(replace, selected.as_deref(), origin);
}

fn editor_toolbar(ui: &mut egui::Ui, session: &mut Session, palette: Palette, tr: &Tr) {
    egui::Panel::top("document-toolbar")
        .show_separator_line(false)
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: 0, right: 0, top: crate::window_ui::HEIGHT as i8 + 12, bottom: 12,
        }))
        .show(ui, |ui| {
            reading_column(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    editor_status(ui, session, palette, tr);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if session.current_in_trash {
                            if ui.add(theme::secondary_button(tr.restore(), palette)).clicked() {
                                if let Some(id) = session.current.clone() {
                                    session.restore(&id);
                                }
                            }
                        } else {
                            crate::export_ui::export_button(ui, session, *tr);
                            if ui.add(theme::secondary_button(tr.find(), palette).selected(session.find.open)).on_hover_text("Ctrl+F").clicked() {
                                open_find(session, false);
                            }
                            if session.editor.note().can_add_title()
                                && ui.add(theme::secondary_button(tr.add_title(), palette)).clicked()
                            {
                                if let Some(id) = session.editor.add_title() {
                                    session.note_text_changed();
                                    session.editor_ui.request_focus(id, 0);
                                }
                            }
                        }
                    });
                });
                if !session.current_in_trash {
                    media_toolbar(ui, session, palette, tr);
                }
                crate::find_bar::show(ui, session, tr);
            });
        });
}

fn media_toolbar(ui: &mut egui::Ui, session: &mut Session, palette: Palette, tr: &Tr) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(30.0);
        if let Some(elapsed) = session.recording_elapsed() {
            ui.add(egui::Label::new(egui::RichText::new(tr.recording()).strong().color(palette.danger)).extend());
            ui.add(egui::Label::new(egui::RichText::new(clock(elapsed.as_secs_f32())).monospace()).extend());
            if ui.button(tr.finish_recording()).clicked() {
                session.finish_recording();
            }
            if ui.button(tr.cancel_recording()).clicked() {
                session.cancel_recording();
            }
        } else if ui.button(tr.record_audio()).clicked() {
            session.start_recording();
        }

        let button = ui.button(tr.insert());
        if button.clicked() {
            session.editor_ui.insert_menu_anchor = (
                session.editor_ui.media_anchor().or(session.selected_block),
                session.editor_ui.media_anchor_revision(),
            );
        }
        let (anchor, revision) = session.editor_ui.insert_menu_anchor;
        egui::Popup::menu(&button).show(|ui| {
            for (kind, label, title) in [
                (MediaKind::Image, tr.insert_image(), tr.picker_insert_image()),
                (MediaKind::Audio, tr.insert_audio(), tr.picker_insert_audio()),
                (MediaKind::Video, tr.insert_video(), tr.picker_insert_video()),
            ] {
                if ui.button(label).clicked() {
                    ui.close();
                    pick_media(session, title, "", kind.extensions(), anchor, revision);
                    break;
                }
            }
        });
    });
}

fn editor_status(ui: &mut egui::Ui, session: &Session, palette: Palette, tr: &Tr) {
    let (message, dot) = if session.current_in_trash {
        (tr.trash(), palette.muted)
    } else if session.editor.dirty() {
        (tr.unsaved(), palette.muted)
    } else if session.persisted {
        (tr.saved(), palette.accent)
    } else if session.editor.note().can_add_title() {
        (tr.new_note_status(), palette.muted)
    } else {
        (tr.untitled(), palette.muted)
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(6.0, 14.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 2.5, dot);
    let status = ui.label(egui::RichText::new(message).monospace().size(11.5).color(palette.muted));
    if session.current_in_trash {
        status.on_hover_text(tr.note_deleted_hint());
    }
}

fn process_dropped_media(ui: &mut egui::Ui, session: &mut Session) {
    if ui.is_enabled() {
        for file in ui.input(|i| i.raw.dropped_files.clone()) {
            session.import(ImportSource::File(file.path().to_path_buf()));
        }
    }
}

/// 修改时间、置顶、标签。
fn meta_row(ui: &mut egui::Ui, session: &mut Session, tr: &Tr) {
    let note = session.editor.note();
    let (updated, pinned, in_trash) = (note.updated_at, note.pinned, note.deleted_at.is_some());
    let palette = Palette::for_ui(ui);
    ui.horizontal_wrapped(|ui| {
        ui.add_space(30.0);
        ui.label(egui::RichText::new(format_time(tr, updated, now_millis())).weak().size(12.0));
        if in_trash {
            return;
        }
        let pin_button = ui.add(theme::secondary_button(if pinned { tr.unpin() } else { tr.pin() }, palette).small());
        if pin_button.clicked() {
            session.editor.note_meta_mut().pinned = !pinned;
            session.note_text_changed();
        }
        let tag_font = egui::FontId::proportional(12.0);
        let tags = egui::TextEdit::singleline(&mut session.tags_input)
            .hint_text(egui::RichText::new(tr.tags_hint()).font(tag_font.clone()))
            .font(tag_font)
            .desired_width(ui.available_width().clamp(112.0, 156.0))
            .margin(egui::Margin::symmetric(6, 3))
            .show(ui)
            .response;
        let tags_focused = tags.has_focus();
        if tags.changed() {
            let text = session.tags_input.clone();
            session.set_tags(&text);
        }
        // 标签框也是正文以外的输入框，编辑器据此让出撤销等快捷键。它在这一帧画得晚，
        // 所以记到 `late_text_focus`，下一帧开头才并进去。
        session.other_text_focus |= tags_focused;
        session.late_text_focus |= tags_focused;
    });
}

fn show_blocks(ui: &mut egui::Ui, session: &mut Session, settings: &Settings, tr: &Tr) -> Option<Focused> {
    let note = session.editor.note();
    let title_id = note.title_block().map(|block| block.id);
    let legacy_title = note.title_source == TitleSource::Legacy;
    let blocks = note.blocks.clone();
    let base = settings.base_font_size;
    let line_spacing = settings.line_spacing;
    let spacing = settings.paragraph_spacing;
    let text_color = ui.visuals().text_color();
    let dim = ui.visuals().weak_text_color();
    let locked = session.current_in_trash;
    let mut focused_now = None;
    let mut number = 0usize;
    let mut previous_was_numbered = false;
    // 查找栏的命中高亮，按块分好、按正文顺序排；还有这一帧要滚到眼前的块。栏没开时都是空的。
    let mut marks = session.find.marks(session.editor.note(), session.editor.revision()).into_iter().peekable();
    let mut scroll_to = session.find.take_scroll_target();

    if title_id.is_none() && (!legacy_title || blocks.is_empty()) {
        note_details(ui, session, tr);
    }

    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Text(text) => {
                if text.kind == TextKind::Numbered {
                    number = if previous_was_numbered { number + 1 } else { 1 };
                    previous_was_numbered = true;
                } else {
                    previous_was_numbered = false;
                }
                // 命中清单和块都按正文顺序排，所以看一眼下一组就知道是不是这一块的。
                let group = marks.next_if(|(block, _)| *block == text.id);
                let mine: &[(u32, u32, bool)] = group.as_ref().map_or(&[], |(_, list)| list.as_slice());
                let top = ui.cursor().top();
                if let Some(focused) = text_block_row(
                    ui,
                    session,
                    text,
                    mine,
                    title_id == Some(text.id),
                    number,
                    base,
                    line_spacing,
                    text_color,
                    dim,
                    locked,
                    tr,
                ) {
                    focused_now = Some(focused);
                }
                if scroll_to == Some(text.id) {
                    scroll_to = None;
                    let (left, right) = (ui.max_rect().left(), ui.max_rect().right());
                    let bounds = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, ui.cursor().top()));
                    ui.scroll_to_rect(bounds, Some(egui::Align::Center));
                }
            }
            Block::Divider { id } => divider_row(ui, session, *id, dim, locked, tr),
            Block::Image(image) => image_row(ui, session, settings, image, locked, tr),
            Block::Audio(media) => audio_row(ui, session, media, locked, tr),
            Block::Video(media) => video_row(ui, session, media, locked, tr),
        }
        if (legacy_title && index == 0) || title_id == Some(block.id()) {
            note_details(ui, session, tr);
        }
        ui.add_space(spacing);
    }
    focused_now
}

fn note_details(ui: &mut egui::Ui, session: &mut Session, tr: &Tr) {
    meta_row(ui, session, tr);
    ui.add_space(theme::SMALL_GAP);
    let (rule, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    let left = (rule.left() + 30.0).min(rule.right());
    ui.painter().hline(left..=rule.right(), rule.center().y, egui::Stroke::new(1.0, Palette::for_ui(ui).line));
    ui.add_space(theme::SMALL_GAP);
}

fn block_menu_button(ui: &mut egui::Ui, session: &mut Session, block: BlockId, locked: bool, tr: &Tr) {
    // 直接画三条横线，不依赖字体里有没有某个符号字符。
    let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
    if locked {
        return;
    }
    let button = ui.interact(rect, egui::Id::unique(("block-menu", block)), egui::Sense::click());
    button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tr.block_menu()));
    let row = egui::Rect::from_min_max(rect.min, egui::pos2(ui.max_rect().right(), rect.bottom() + 6.0));
    if button.has_focus() || ui.rect_contains_pointer(row) {
        let color = if button.hovered() || button.has_focus() {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        for y in [-4.0, 0.0, 4.0] {
            for x in [-2.0, 2.0] {
                ui.painter().circle_filled(rect.center() + egui::vec2(x, y), 1.1, color);
            }
        }
    }
    let button = button.on_hover_text(tr.block_menu()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if button.clicked() {
        session.editor_ui.select_media_anchor(block);
    }
    egui::Popup::menu(&button).show(|ui| block_menu(ui, session, block, tr));
}

fn divider_row(ui: &mut egui::Ui, session: &mut Session, id: BlockId, dim: egui::Color32, locked: bool, tr: &Tr) {
    ui.horizontal_top(|ui| {
        block_menu_button(ui, session, id, locked, tr);
        let rect = ui.available_rect_before_wrap();
        let line = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + 14.0));
        ui.allocate_rect(line, egui::Sense::hover());
        ui.painter().hline(line.x_range(), line.center().y, egui::Stroke::new(1.0, dim));
    });
}

/// 图片块：显示缩小后的预览图，右下角可以拖着改大小，双击恢复原大。
fn image_row(
    ui: &mut egui::Ui,
    session: &mut Session,
    settings: &Settings,
    image: &ImageBlock,
    locked: bool,
    tr: &Tr,
) {
    let id = image.id;
    ui.horizontal(|ui| {
        ui.add_space(22.0);
        ui.label(egui::RichText::new(format!("{} × {}", image.width, image.height)).weak().small());
        if ui.add(theme::secondary_button(tr.view_image(), Palette::for_ui(ui))).clicked() {
            session.editor_ui.viewer = Some(MediaViewer::Image(image.clone()));
        }
    });
    ui.horizontal_top(|ui| {
        block_menu_button(ui, session, id, locked, tr);
        let Some(texture) = session.media.preview(&image.blob_id) else {
            ui.add_space(6.0);
            let status = if session.media.preview_failed(&image.blob_id) { tr.image_view_failed() } else { tr.loading_image() };
            ui.add(egui::Label::new(egui::RichText::new(status).weak().small()));
            return;
        };
        let natural = texture.size_vec2();
        if natural.x <= 0.0 || natural.y <= 0.0 {
            return;
        }
        let available = ui.available_width().max(1.0);
        let fit = natural.x.min(available);
        let min_width = fit.min(80.0);
        let shown = match session.editor_ui.image_drag {
            Some((dragging, _, width)) if dragging == id => width,
            _ => image.display_width.unwrap_or(fit),
        }
        .clamp(min_width, available);
        let size = egui::vec2(shown, shown * natural.y / natural.x);
        let radius = settings.corner_radius.round().clamp(0.0, 40.0) as u8;
        let sense = if locked { egui::Sense::hover() } else { egui::Sense::click() };
        let widget = ui.add(egui::Image::new(&texture).fit_to_exact_size(size).corner_radius(radius).sense(sense));
        if locked {
            return;
        }
        let widget = widget.on_hover_text(tr.image_resize_hint());
        if widget.double_clicked() {
            session.editor.set_image_width(id, None);
            session.note_text_changed();
        }
        if widget.clicked() {
            session.selected_block = Some(id);
            session.editor_ui.select_media_anchor(id);
        }

        // 右下角的缩放手柄。
        let corner = egui::Rect::from_min_size(widget.rect.right_bottom() - egui::vec2(5.0, 5.0), egui::vec2(11.0, 11.0));
        let handle = ui.interact(corner, egui::Id::unique(("image-resize", id)), egui::Sense::drag());
        let grip = if handle.hovered() || handle.dragged() {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let (grip_x, grip_y) = corner.right_bottom().into();
        let stroke = egui::Stroke::new(2.0, grip);
        ui.painter().hline(grip_x - 8.0..=grip_x, grip_y, stroke);
        ui.painter().vline(grip_x, grip_y - 8.0..=grip_y, stroke);

        if handle.drag_started() {
            session.editor_ui.image_drag = Some((id, shown, shown));
        }
        if handle.dragged() {
            let step = handle.drag_delta().x;
            if let Some((_, _, width)) = session.editor_ui.image_drag.as_mut() {
                *width = (*width + step).clamp(min_width, available);
            }
        }
        if handle.drag_stopped() {
            if let Some((_, _, width)) = session.editor_ui.image_drag.take() {
                // 拖完才记一步撤销，拖动过程中的每一帧不进撤销栈。
                session.editor.set_image_width(id, Some(width));
                session.note_text_changed();
            }
        }
    });
}

/// 音频块：播放、暂停、停止、进度条。音频线程常驻，这里只是遥控器。
fn audio_row(ui: &mut egui::Ui, session: &mut Session, media: &MediaBlock, locked: bool, tr: &Tr) {
    ui.horizontal_wrapped(|ui| {
        block_menu_button(ui, session, media.id, locked, tr);
        media_view::audio_controls(ui, &mut session.media, media, *tr, false);
        if ui.add(theme::secondary_button(tr.open_audio_player(), Palette::for_ui(ui))).clicked() {
            session.finish_audio_rename();
            if let Some(Block::Audio(block)) = session.block_by_id(media.id) {
                session.editor_ui.viewer = Some(MediaViewer::Audio { block: block.clone(), error: None });
            }
        }
        audio_file_name(ui, session, media, locked, tr);
    });
}

fn audio_file_name(ui: &mut egui::Ui, session: &mut Session, media: &MediaBlock, locked: bool, tr: &Tr) {
    let size = format_size(media.size);
    if locked {
        ui.add(egui::Label::new(&media.file_name).wrap()).on_hover_text(size);
        return;
    }
    let (stem, extension) = audio_name_parts(&media.file_name);
    let Some(mut draft) = session.editor_ui.audio_rename.take_if(|draft| draft.block == media.id) else {
        let name = egui::RichText::new(&media.file_name).underline();
        let button = egui::Button::new(name).frame_when_inactive(false).wrap();
        if ui.add(button)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!("{} · {size}", tr.rename_audio()))
            .clicked()
        {
            session.finish_audio_rename();
            session.editor_ui.focus = None;
            session.editor_ui.audio_rename = Some(AudioRename {
                block: media.id,
                stem: stem.to_owned(),
                preedit: None,
                focus_on_show: true,
                invalid: false,
            });
            session.other_text_focus = true;
            session.late_text_focus = true;
            ui.ctx().request_repaint();
        }
        return;
    };

    let id = egui::Id::unique(("audio-name-edit", media.id));
    if std::mem::take(&mut draft.focus_on_show) {
        egui::text_edit::TextEditState::default().store(ui.ctx(), id);
        focus_block(ui.ctx(), id, char_count(&draft.stem), 0);
    }
    let active = ui.memory(|memory| memory.has_focus(id));
    let had_preedit = draft.preedit.is_some();
    let ime = ImeInput::read(ui, active, had_preedit);
    let submit = active && !had_preedit && !ime.composing && !ime.committed
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let mut buffer = draft.preedit.take().unwrap_or_else(|| draft.stem.clone());
    let suffix_width = if extension.is_empty() {
        0.0
    } else {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let width = ui.fonts_mut(|fonts| fonts.layout_no_wrap(extension.to_owned(), font, ui.visuals().weak_text_color()).size().x);
        width + ui.spacing().item_spacing.x
    };
    let width = (135.0 + suffix_width).min(ui.max_rect().width());
    let output = ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let output = egui::TextEdit::singleline(&mut buffer)
                .id(id)
                .return_key(None)
                .desired_width((width - suffix_width).max(1.0))
                .hint_text(tr.rename_audio())
                .show(ui);
            if !extension.is_empty() {
                ui.label(egui::RichText::new(extension).weak());
            }
            output
        },
    ).inner;
    if output.response.changed() {
        draft.invalid = false;
    }
    if ime.composing && output.response.has_focus() {
        if let Some(committed) = ime.committed_prefix(&buffer, output.cursor_range) {
            draft.stem = committed;
        }
        draft.preedit = Some(buffer);
    } else if ime.committed || (!ime.canceled && !had_preedit && !ime.composing) {
        draft.stem = buffer;
    }

    if submit || output.response.lost_focus() {
        if session.apply_audio_rename(draft.block, &draft.stem) {
            output.response.surrender_focus();
            ui.ctx().request_repaint();
            return;
        }
        if !submit {
            session.notice = Some(tr.picker_bad_file_name().into());
            ui.ctx().request_repaint();
            return;
        }
        draft.invalid = true;
        output.response.request_focus();
    }
    if draft.invalid {
        ui.label(egui::RichText::new(tr.picker_bad_file_name()).color(Palette::for_ui(ui).danger));
    }
    session.editor_ui.audio_rename = Some(draft);
    session.other_text_focus = true;
    session.late_text_focus = true;
}

/// 视频块：在笔记上方打开应用内播放器。
fn video_row(ui: &mut egui::Ui, session: &mut Session, media: &MediaBlock, locked: bool, tr: &Tr) {
    ui.horizontal(|ui| {
        block_menu_button(ui, session, media.id, locked, tr);
        if ui.add(egui::Button::new(tr.play_video())).clicked() {
            session.editor_ui.viewer = Some(MediaViewer::video(ui.ctx(), &mut session.media, media, *tr));
        }
        let caption = format!("{}   {}", media.file_name, format_size(media.size));
        ui.add(egui::Label::new(egui::RichText::new(caption).weak()).wrap());
    });
}

/// 画一个文本块。有焦点时返回它的光标信息。
#[allow(clippy::too_many_arguments)]
fn text_block_row(
    ui: &mut egui::Ui,
    session: &mut Session,
    text: &TextBlock,
    marks: &[(u32, u32, bool)],
    is_title: bool,
    number: usize,
    base: f32,
    line_spacing: f32,
    text_color: egui::Color32,
    dim: egui::Color32,
    locked: bool,
    tr: &Tr,
) -> Option<Focused> {
    let id = egui::Id::unique(("block", text.id));
    let mut focused = None;
    let mut jumped = false;

    ui.horizontal_top(|ui| {
        block_menu_button(ui, session, text.id, locked, tr);
        let (size, bold) = kind_look(text.kind, is_title, base);
        let heading = is_title || matches!(text.kind, TextKind::Heading1 | TextKind::Heading2 | TextKind::Heading3);
        let font = crate::fonts::note_font(size, heading, bold);
        let bold_font = crate::fonts::note_font(size, heading, true);
        let line_height = size * if is_title { 1.17 } else { line_spacing };
        match text.kind {
            TextKind::Bullet => {
                ui.add_sized([24.0, size * 1.6], egui::Label::new(egui::RichText::new("•").color(dim)));
            }
            TextKind::Numbered => {
                ui.add_sized([36.0, size * 1.6], egui::Label::new(egui::RichText::new(format!("{number}.")).color(dim)));
            }
            _ => {}
        }
        let width = ui.available_width();
        // 聚焦请求先在画之前登记光标位置，画完再确认：没拿到就留着，下一帧继续要。
        let requested = ui.is_enabled() && !locked && session.editor_ui.focus.is_some_and(|(block, _, _)| block == text.id);
        if requested {
            let (_, caret, anchor) = session.editor_ui.focus.unwrap_or((text.id, 0, 0));
            let total = char_count(&text.text);
            focus_block(ui.ctx(), id, caret.min(total), anchor.min(total));
            jumped = true;
        }

        let active = !locked && ui.is_enabled() && ui.memory(|memory| memory.has_focus(id));
        let draft = session.editor_ui.ime_draft.take_if(|(block, _)| *block == text.id);
        let had_draft = draft.is_some();
        let ime = ImeInput::read(ui, active, had_draft);
        let mut buffer = draft.filter(|_| active).map_or_else(|| text.text.clone(), |(_, draft)| draft);
        let spans = &text.spans;
        let kind = text.kind;
        let hint = egui::RichText::new(if is_title { tr.new_note_title() } else { tr.empty_block_hint() })
            .font(font.clone())
            .line_height(Some(line_height))
            .weak();
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| -> Arc<egui::Galley> {
            let highlight = theme::highlight_colors(ui.visuals().dark_mode);
            ui.painter().layout_job(styled_job(
                buffer.as_str(),
                spans,
                marks,
                &font,
                &bold_font,
                line_height,
                width,
                text_color,
                highlight,
            ))
        };
        let out = egui::TextEdit::multiline(&mut buffer)
            .id(id)
            .font(font.clone())
            .frame(egui::Frame::new())
            .desired_width(width)
            .desired_rows(1)
            .margin(egui::Margin::symmetric(2, 5))
            .hint_text(hint)
            .layouter(&mut layouter)
            .interactive(!locked)
            .show(ui);
        let changes = if ime.composing && active && out.response.has_focus() {
            // Windows 可以在同一帧提交上一段文字，然后开始下一段组字。
            let accepted = ime.committed_prefix(&buffer, out.cursor_range);
            session.editor_ui.ime_draft = Some((text.id, buffer));
            accepted
        } else if ime.committed || (!ime.canceled && !had_draft && !ime.composing) {
            Some(buffer)
        } else {
            if had_draft && !out.response.has_focus() {
                egui::text_edit::TextEditState::default().store(ui.ctx(), id);
            }
            None
        };
        if let Some(changes) = changes.filter(|changes| changes != &text.text) {
            if session.editor.text_changed(text.id, &changes) {
                session.note_text_changed();
            }
        }
        if kind == TextKind::Quote {
            let rect = out.response.rect;
            ui.painter().vline(rect.left() - 8.0, rect.y_range(), egui::Stroke::new(2.0, dim));
        }
        if out.response.has_focus() {
            if requested {
                session.editor_ui.focus = None;
            }
            let range = out.cursor_range.unwrap_or_default();
            let position = out.galley.layout_from_cursor(range.primary);
            let (caret, anchor) = (range.primary.index.0, range.secondary.index.0);
            let selection_changed = session.editor_ui.focused.is_none_or(|previous| {
                (previous.block, previous.caret, previous.anchor) != (text.id, caret, anchor)
            });
            if out.response.clicked() || selection_changed {
                session.editor_ui.select_media_anchor(text.id);
            }
            focused = Some(Focused {
                block: text.id,
                caret,
                anchor,
                row: position.row,
                column: position.column.0,
                rows: out.galley.rows.len(),
            });
            if jumped {
                ui.scroll_to_cursor(Some(egui::Align::Center));
            }
        }
    });
    focused
}

/// 块菜单：换类型、上下移、复制、插入媒体或分割线、删除。
fn block_menu(ui: &mut egui::Ui, session: &mut Session, block: BlockId, tr: &Tr) {
    let caret = session.editor_ui.caret_of(block);
    let kind = session.editor.text_block(block).map(|text| text.kind);
    if kind.is_some() {
        for (candidate, name) in [
            (TextKind::Paragraph, tr.paragraph()),
            (TextKind::Heading1, tr.heading1()),
            (TextKind::Heading2, tr.heading2()),
            (TextKind::Heading3, tr.heading3()),
            (TextKind::Bullet, tr.bullet()),
            (TextKind::Numbered, tr.numbered()),
            (TextKind::Quote, tr.quote()),
        ] {
            if ui.add(egui::Button::new(name).selected(kind == Some(candidate))).clicked() {
                session.editor.set_text_kind(block, candidate);
                session.editor_ui.request_focus(block, caret);
                session.note_text_changed();
                ui.close();
                return;
            }
        }
    }
    ui.separator();
    if ui.button(tr.move_up()).clicked() {
        if session.editor.move_block(block, -1) {
            session.editor_ui.request_focus(block, caret);
        }
        session.note_text_changed();
        ui.close();
        return;
    }
    if ui.button(tr.move_down()).clicked() {
        if session.editor.move_block(block, 1) {
            session.editor_ui.request_focus(block, caret);
        }
        session.note_text_changed();
        ui.close();
        return;
    }
    if ui.button(tr.duplicate()).clicked() {
        if let Some(copy) = session.editor.duplicate_block(block) {
            session.editor_ui.request_focus(copy, caret);
        }
        session.note_text_changed();
        ui.close();
        return;
    }
    if ui.button(tr.insert_divider()).clicked() {
        let divider_id = mp_core::util::random_block_id().unwrap_or_else(|_| now_millis());
        session.insert_block(Block::Divider { id: divider_id }, Some(block));
        ui.close();
        return;
    }
    ui.separator();
    if ui.button(tr.insert_media()).clicked() {
        ui.close();
        let extensions: Vec<_> = mp_core::media::extensions().collect();
        let revision = session.editor_ui.media_anchor_revision();
        pick_media(session, tr.insert_media(), tr.media_filter(), &extensions, Some(block), revision);
        return;
    }
    let is_media = session
        .editor
        .note()
        .blocks
        .iter()
        .find(|item| item.id() == block)
        .is_some_and(|item| matches!(item, Block::Image(_) | Block::Audio(_) | Block::Video(_)));
    if is_media {
        if ui.button(tr.copy_block()).clicked() {
            session.copy_block(block);
            ui.close();
            return;
        }
        if ui.button(tr.cut_block()).clicked() {
            session.cut_block(block);
            ui.close();
            return;
        }
        if ui.button(tr.save_as()).clicked() {
            ui.close();
            save_blob_as(session, block, *tr);
            return;
        }
    }
    // 媒体块同样可以删除，所以上面不用提前返回。
    if ui.button(tr.delete_block()).clicked() {
        if let Some(next) = session.editor.remove_block(block) {
            session.editor_ui.request_focus(next, 0);
        }
        session.note_text_changed();
        ui.close();
    }
}

/// 文件选择框一开始停在哪儿：有「图片」文件夹就去那儿，没有就停在用户主目录。
pub(crate) fn start_directory() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| {
            let pictures = home.join("Pictures");
            if pictures.is_dir() { pictures } else { home }
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 打开文件选择框，保留点开菜单时的插入位置。
fn pick_media(session: &mut Session, title: &str, filter: &str, extensions: &[&str], anchor: Option<BlockId>, revision: u64) {
    let action = AttachmentAction::Import { anchor, revision };
    #[cfg(test)]
    if let Some(paths) = crate::matrix::take_file_dialog(crate::matrix::Dialog::Import) {
        if let Some(paths) = paths {
            apply_attachment_action(session, action, paths);
        }
        return;
    }
    let picker = FilePicker::open(title, filter, extensions, start_directory());
    session.editor_ui.file_picker = Some((picker, action));
}

/// 把一个媒体块解密后存成普通文件。位置由用户在保存框里选，这等于用户主动导出一份明文。
fn save_blob_as(session: &mut Session, block_id: BlockId, tr: Tr) {
    session.finish_audio_rename();
    let Some(block) = session.editor.note().blocks.iter().find(|block| block.id() == block_id) else {
        return;
    };
    let Some(blob_id) = block.blob_id() else {
        return;
    };
    let name = match block {
        Block::Image(_) => "image.png".to_string(),
        Block::Audio(media) | Block::Video(media) => media.file_name.clone(),
        _ => return,
    };
    let action = AttachmentAction::Save { blob_id: blob_id.to_owned() };
    #[cfg(test)]
    if let Some(paths) = crate::matrix::take_file_dialog(crate::matrix::Dialog::SaveMedia) {
        if let Some(paths) = paths {
            apply_attachment_action(session, action, paths);
        }
        return;
    }
    let picker = FilePicker::save(tr.save_as(), tr.picker_all_files(), None, name, start_directory());
    session.editor_ui.file_picker = Some((picker, action));
}

fn apply_attachment_action(session: &mut Session, action: AttachmentAction, paths: Vec<PathBuf>) {
    match action {
        AttachmentAction::Import { anchor, revision } => {
            let sources = paths.into_iter().map(ImportSource::File).collect();
            session.import_batch_after(sources, anchor, revision);
        }
        AttachmentAction::Save { blob_id } => {
            if let Some(target) = paths.into_iter().next() {
                let saved = session.media.read_blob(&blob_id)
                    .and_then(|bytes| std::fs::write(target, bytes).map_err(|error| error.to_string()));
                if let Err(error) = saved {
                    session.notice = Some(error);
                }
            }
        }
    }
}

/// 让某个文本框获得焦点，光标放到 `caret`，选区的另一头在 `anchor`。
fn focus_block(ctx: &egui::Context, id: egui::Id, caret: usize, anchor: usize) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange {
        primary: egui::text::CCursor::new(egui::text::CharIndex(caret)),
        secondary: egui::text::CCursor::new(egui::text::CharIndex(anchor)),
        h_pos: None,
    }));
    state.store(ctx, id);
    ctx.memory_mut(|memory| memory.request_focus(id));
}

/// 把光标挪到上一块或下一块文本里，尽量保持原来的列。
fn jump(session: &mut Session, from: BlockId, step: i32, column: usize) {
    let Some(index) = session.editor.block_index(from) else {
        return;
    };
    let blocks = &session.editor.note().blocks;
    let mut position = index as i64 + step as i64;
    let mut target = None;
    while position >= 0 && (position as usize) < blocks.len() {
        if let Block::Text(text) = &blocks[position as usize] {
            target = Some(text);
            break;
        }
        position += step as i64;
    }
    let Some(target) = target else {
        return;
    };
    let caret = if step < 0 {
        let total = char_count(&target.text);
        let last_line_start = target.text.rfind('\n').map(|byte| byte_to_char(&target.text, byte + 1)).unwrap_or(0);
        (last_line_start + column).min(total)
    } else {
        let first_line_len = target
            .text
            .find('\n')
            .map(|byte| byte_to_char(&target.text, byte))
            .unwrap_or_else(|| char_count(&target.text));
        column.min(first_line_len)
    };
    session.editor_ui.request_focus(target.id, caret);
}

/// 在任何文本框处理按键之前，先接管会改变块结构的按键。
fn handle_keys(ui: &mut egui::Ui, session: &mut Session, tr: &Tr) {
    if session.editor_ui.ime_draft.is_some() {
        return;
    }
    let focused = session.editor_ui.focused;
    // 撤销和重做由编辑器统一管。焦点在搜索框、标签框里时不接管，让那里自己处理。
    if !session.other_text_focus {
        let redo = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y))
            || ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z));
        let undo = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z));
        if undo {
            session.editor.undo();
            session.note_text_changed();
        }
        if redo {
            session.editor.redo();
            session.note_text_changed();
        }
    }
    handle_paste(ui, session, tr);
    let Some(focused) = focused else {
        return;
    };
    let mut touched = false;

    for (flag, key) in [
        (StyleFlag::Bold, egui::Key::B),
        (StyleFlag::Italic, egui::Key::I),
        (StyleFlag::Underline, egui::Key::U),
    ] {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key)) {
            session.editor.toggle_style(focused.block, focused.selection(), flag);
            touched = true;
        }
    }
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::X)) {
        session.editor.toggle_style(focused.block, focused.selection(), StyleFlag::Strikethrough);
        touched = true;
    }
    for (key, offset) in [(egui::Key::ArrowUp, -1), (egui::Key::ArrowDown, 1)] {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::ALT, key)) {
            if session.editor.move_block(focused.block, offset) {
                session.editor_ui.request_focus(focused.block, focused.caret);
            }
            touched = true;
        }
    }

    // 在可选标题末尾按回车，直接回到已经存在的正文。
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        let note = session.editor.note();
        let existing_body = note
            .title_block()
            .filter(|title| note.title_source == TitleSource::Block(focused.block) && char_count(&title.text) == focused.caret)
            .and_then(|_| {
                note.blocks.iter().skip_while(|block| block.id() != focused.block).skip(1).find_map(Block::as_text)
            })
            .map(|body| body.id);
        if let Some(body) = existing_body.or_else(|| session.editor.split_block(focused.block, focused.caret)) {
            session.editor_ui.request_focus(body, 0);
        }
        touched = true;
    }

    // 行首按退格：并到上一块。其他位置的退格交给文本框自己删字。
    let no_selection = focused.caret == focused.anchor;
    let backspace = ui.input(|i| i.key_pressed(egui::Key::Backspace) && i.modifiers.is_none());
    if backspace && no_selection && focused.caret == 0 {
        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
        let previous = session.editor.block_index(focused.block).and_then(|index| index.checked_sub(1));
        if let Some(caret) = session.editor.merge_with_previous(focused.block) {
            if let Some(target) = previous.and_then(|index| session.editor.note().blocks.get(index)) {
                session.editor_ui.request_focus(target.id(), caret);
            }
        }
        touched = true;
    }

    // 在第一行按上、最后一行按下：跳到相邻的文本块。
    if no_selection {
        if focused.row == 0 && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            jump(session, focused.block, -1, focused.column);
        } else if focused.row + 1 >= focused.rows
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
        {
            jump(session, focused.block, 1, focused.column);
        }
    }
    if touched {
        session.note_text_changed();
    }
}

/// 粘贴文字和图片。整块的复制、剪切、粘贴不在这里，见 `clipboard.rs`。
fn handle_paste(ui: &mut egui::Ui, session: &mut Session, tr: &Tr) {
    // 快捷键本身在 `clipboard.rs` 里处理：它看过剪贴板之后，把内容变成下面这些事件。
    for event in ui.input(|i| i.events.clone()) {
        match event {
            // 截图，或别的软件里复制的图片。
            egui::Event::PasteImage(image) => match crate::media::encode_png(&image) {
                Ok(bytes) => session.import(ImportSource::Memory { name: "pasted-image.png".into(), bytes }),
                Err(_) => session.notice = Some(tr.err_pasted_image().to_string()),
            },
            // 没有正文聚焦、焦点也不在别的输入框里时，粘贴文字：追加一个段落。
            // 焦点在搜索框、标签框、查找栏、设置窗口里时，粘进去的是那些框，正文不动。
            egui::Event::Paste(text) if session.editor_ui.focused_block().is_none() && !session.other_text_focus => {
                session.editor.append_paragraph(&text);
                session.note_text_changed();
            }
            _ => {}
        }
    }
}

/// 把一段带样式区间的文字排成 egui 的版面任务。`marks` 是查找命中的范围（字符位置，第三项表示是不是
/// 当前停的那一处），`highlight` 是普通命中和当前命中各自的高亮色。
#[allow(clippy::too_many_arguments)]
fn styled_job(
    text: &str,
    spans: &[StyleSpan],
    marks: &[(u32, u32, bool)],
    font: &egui::FontId,
    bold_font: &egui::FontId,
    line_height: f32,
    width: f32,
    text_color: egui::Color32,
    highlight: (egui::Color32, egui::Color32),
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font.clone(), text_color, width);
    job.sections[0].format.line_height = Some(line_height);
    if text.is_empty() {
        return job;
    }
    let base_format = job.sections[0].format.clone();

    let total = char_count(text) as u32;
    let mut sections = Vec::new();
    // 起止先按字符位置记，最后一起换成字节位置。
    let emit = |start: u32,
                end: u32,
                style: InlineStyle,
                background: Option<egui::Color32>,
                out: &mut Vec<egui::text::LayoutSection>| {
        if end <= start {
            return;
        }
        let mut format = base_format.clone();
        if style.bold {
            format.font_id = bold_font.clone();
        }
        format.italics |= style.italic;
        if style.underline {
            format.underline = egui::Stroke::new(1.0, format.color);
        }
        if style.strikethrough {
            format.strikethrough = egui::Stroke::new(1.0, format.color);
        }
        if let Some(color) = style.color {
            format.color = theme::color(color);
        }
        if let Some(color) = background {
            format.background = color;
        }
        out.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: egui::text::ByteIndex(start as usize)..egui::text::ByteIndex(end as usize),
            format,
        });
    };

    // 样式区间已排好序且互不重叠：区间之间的空档用默认样式补上，得到一段段连续的样式。
    let mut runs: Vec<(u32, u32, InlineStyle)> = Vec::new();
    let mut cursor = 0u32;
    for span in spans {
        if span.start > cursor {
            runs.push((cursor, span.start.min(total), InlineStyle::default()));
        }
        if span.end > cursor {
            runs.push((span.start.max(cursor), span.end.min(total), span.style));
            cursor = span.end.min(total);
        }
    }
    if cursor < total {
        runs.push((cursor, total, InlineStyle::default()));
    }

    // 再让命中范围把每一小段切开：切出来的每一节里，样式和高亮都一样。
    // 两张清单都排好了序，所以各用一个只往前走的下标就够。
    let (mut run, mut mark, mut position) = (0usize, 0usize, 0u32);
    while position < total {
        while runs.get(run).is_some_and(|(_, end, _)| *end <= position) {
            run += 1;
        }
        while marks.get(mark).is_some_and(|(_, end, _)| *end <= position) {
            mark += 1;
        }
        let mut next = runs.get(run).map_or(total, |(_, end, _)| *end);
        if let Some((start, end, _)) = marks.get(mark) {
            next = next.min(*end);
            if *start > position {
                next = next.min(*start);
            }
        }
        let background = marks
            .get(mark)
            .filter(|(start, end, _)| *start <= position && position < *end)
            .map(|(_, _, current)| if *current { highlight.1 } else { highlight.0 });
        let style = runs.get(run).map_or(InlineStyle::default(), |(_, _, style)| *style);
        emit(position, next, style, background, &mut sections);
        position = next;
    }

    // 各节的起止从小到大排好了，顺着文本走一遍就能全部换成字节位置。
    let (mut chars, mut index, mut byte) = (text.char_indices(), 0usize, 0usize);
    for section in &mut sections {
        for slot in [&mut section.byte_range.start, &mut section.byte_range.end] {
            let target = slot.0;
            while index < target {
                match chars.next() {
                    // 第 `index` 个字符占 at..at+len，所以下一个字符位置的字节下标是 at+len。
                    Some((at, ch)) => {
                        index += 1;
                        byte = at + ch.len_utf8();
                    }
                    None => break,
                }
            }
            *slot = egui::text::ByteIndex(if index == target { byte } else { text.len() });
        }
    }
    job.sections = sections;
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_title_keeps_its_first_row_metrics_when_empty() {
        let color = egui::Color32::WHITE;
        let font = crate::fonts::note_font(41.7, true, false);
        let bold_font = crate::fonts::note_font(41.7, true, true);
        let layout = |text| styled_job(
            text, &[], &[], &font, &bold_font, 48.8, 400.0, color, (color, color),
        );
        let filled = layout("给笔记起个标题");
        let empty = layout("");
        let empty_format = &empty.sections.first().expect("empty row has its title style").format;
        assert_eq!(empty_format.font_id, filled.sections[0].format.font_id);
        assert_eq!(empty_format.line_height, filled.sections[0].format.line_height);
    }
}
