//! 笔记内查找与替换的界面，以及它记住的状态。怎么找、怎么换在 mp-core 的 `find` 里，这里只管交互。

use crate::i18n::{replaced, Tr};
use crate::session::Session;
use eframe::egui;
use mp_core::find::{self, Edit, Hit};
use mp_core::model::{BlockId, Note};

const QUERY_ID: &str = "find-query";
const REPLACEMENT_ID: &str = "find-replacement";
/// 查找框和替换框的宽度。
const FIELD_WIDTH: f32 = 260.0;

#[derive(Default)]
pub struct FindBar {
    /// 栏是否打开。
    pub open: bool,
    /// 是否显示替换那一行。
    pub replace: bool,
    pub query: String,
    pub replacement: String,
    /// 区分大小写。默认不区分。
    pub match_case: bool,
    /// 当前停在第几处（命中清单的下标）。
    pub current: usize,
    hits: Vec<Hit>,
    /// 命中清单是按什么算出来的：(笔记, 内容版本, 查找内容, 是否区分大小写)。对不上就重算。
    key: (String, u64, String, bool),
    /// 接着从哪里找：(块, 字符位置)。换过一处之后排在它后面，刚换上去的字不会再被当成命中。
    resume: Option<(BlockId, usize)>,
    /// 打开栏时光标在正文里的位置。关栏时没有命中可回，就把光标放回这里。
    origin: Option<(BlockId, usize)>,
    /// 下一帧把当前命中所在的块滚到眼前。
    scroll_wanted: bool,
    /// 焦点还没进查找框：继续要。
    focus_wanted: bool,
}

impl FindBar {
    /// 打开查找栏。`origin` 是光标在正文里的位置，第一处命中从那里往后找；
    /// 选中的文字（如果有）当作查找内容。栏已经开着时只把焦点放回查找框。
    pub fn open(&mut self, replace: bool, selected: Option<&str>, origin: Option<(BlockId, usize)>) {
        if !self.open {
            self.resume = origin;
            if let Some(text) = selected {
                self.query = text.to_string();
            }
        }
        // 栏已经开着时焦点在栏里、正文里没有光标，这时不要把记下的位置冲掉：关栏时还要靠它回去。
        if origin.is_some() {
            self.origin = origin;
        }
        self.open = true;
        self.replace |= replace;
        self.focus_wanted = true;
        self.scroll_wanted = true;
    }

    /// 关掉查找栏。
    pub fn close(&mut self) {
        self.open = false;
        self.focus_wanted = false;
        self.scroll_wanted = false;
    }

    /// 换了一条笔记：之前的命中都不作数。
    pub fn note_changed(&mut self) {
        self.hits.clear();
        self.key = (String::new(), 0, String::new(), false);
        self.resume = None;
        self.origin = None;
        self.current = 0;
    }

    /// 命中清单。内容、查找条件变了就重算，并按 `resume` 重新定位当前这一处。
    pub fn hits(&mut self, note: &Note, revision: u64) -> &[Hit] {
        let key = (note.id.clone(), revision, self.query.clone(), self.match_case);
        if key != self.key {
            // 只是查找条件变了（或换了笔记）：把新选中的这一处滚到眼前。
            // 正文改动引起的不滚，免得一边找一边在别处打字时画面乱跳。
            let view_changed = key.0 != self.key.0 || key.2 != self.key.2 || key.3 != self.key.3;
            self.hits = find::find_in_note(note, &self.query, self.match_case);
            self.key = key;
            let (block, position) = self.resume.unzip();
            self.current = find::hit_from(note, &self.hits, block, position.unwrap_or(0) as u32).unwrap_or(0);
            if view_changed {
                self.scroll_wanted = true;
            }
        }
        &self.hits
    }

    /// 查找内容或大小写设置改了：从当前这一处的位置重新定位。
    pub fn conditions_changed(&mut self) {
        if let Some(hit) = self.hits.get(self.current) {
            self.resume = Some((hit.block, hit.start as usize));
        }
    }

    /// 上一处、下一处。
    pub fn step(&mut self, note: &Note, revision: u64, delta: i32) {
        let total = self.hits(note, revision).len();
        if total == 0 {
            return;
        }
        self.current = (self.current as i64 + delta as i64).rem_euclid(total as i64) as usize;
        let hit = self.hits[self.current];
        self.resume = Some((hit.block, hit.start as usize));
        self.scroll_wanted = true;
    }

    /// 换掉了一处之后：接着从换上去的新文字后面找。
    fn advance_past(&mut self, hit: Hit, inserted: u32) {
        self.resume = Some((hit.block, hit.start as usize + inserted as usize));
        self.scroll_wanted = true;
    }

    /// 全部换掉之后回到第一处。
    fn back_to_start(&mut self) {
        self.resume = None;
        self.scroll_wanted = true;
    }

    /// 每个块的命中高亮：(块, [(起, 止, 是不是当前这一处)])，按正文顺序。栏没开时是空的。
    pub fn marks(&mut self, note: &Note, revision: u64) -> Vec<(BlockId, Vec<(u32, u32, bool)>)> {
        if !self.open {
            return Vec::new();
        }
        self.hits(note, revision);
        let current = self.current;
        let mut out: Vec<(BlockId, Vec<(u32, u32, bool)>)> = Vec::new();
        for (index, hit) in self.hits.iter().enumerate() {
            let mark = (hit.start, hit.end, index == current);
            match out.last_mut() {
                Some((block, list)) if *block == hit.block => list.push(mark),
                _ => out.push((hit.block, vec![mark])),
            }
        }
        out
    }

    /// 下一帧要滚到眼前的块。
    pub fn take_scroll_target(&mut self) -> Option<BlockId> {
        if !std::mem::take(&mut self.scroll_wanted) {
            return None;
        }
        self.hits.get(self.current).map(|hit| hit.block)
    }

    /// 关栏时把光标放到哪：有命中就选中当前这一处，没有就回到打开时的位置。
    fn where_to_land(&self) -> Option<(BlockId, usize, usize)> {
        if let Some(hit) = self.hits.get(self.current) {
            return Some((hit.block, hit.start as usize, hit.end as usize));
        }
        self.origin.map(|(block, caret)| (block, caret, caret))
    }
}

/// 把当前这一处换掉，然后停到下一处。
fn replace_current(session: &mut Session) {
    let revision = session.editor.revision();
    session.find.hits(session.editor.note(), revision);
    let Some(hit) = session.find.hits.get(session.find.current).copied() else {
        return;
    };
    let edit = Edit {
        block: hit.block,
        start: hit.start,
        end: hit.end,
        replacement: session.find.replacement.clone(),
    };
    session.find.advance_past(hit, edit.replacement.chars().count() as u32);
    session.editor.replace_spans(&[edit]);
    session.note_text_changed();
}

/// 全部换掉。整批只占一步撤销。返回换了几处。「全部替换」按钮走的就是这条路。
pub(crate) fn replace_everywhere(session: &mut Session) -> usize {
    let revision = session.editor.revision();
    session.find.hits(session.editor.note(), revision);
    let edits = find::replace_all(&session.find.hits, &session.find.replacement);
    let count = session.editor.replace_spans(&edits);
    session.find.back_to_start();
    if count > 0 {
        session.note_text_changed();
    }
    count
}

fn query_controls(ui: &mut egui::Ui, session: &mut Session, tr: &Tr, width: f32) -> i32 {
    let query = egui::TextEdit::singleline(&mut session.find.query)
        .id(egui::Id::unique(QUERY_ID))
        .hint_text(tr.find_placeholder())
        .desired_width(width)
        .margin(egui::Margin::symmetric(6, 4))
        .return_key(None::<egui::KeyboardShortcut>)
        .show(ui)
        .response;
    if session.find.focus_wanted {
        if query.has_focus() {
            session.find.focus_wanted = false;
        } else {
            query.request_focus();
        }
    }
    let mut step = 0;
    if query.has_focus() {
        session.other_text_focus = true;
        session.late_text_focus = true;
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)) {
            step = -1;
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            step = 1;
        }
    }
    if query.changed() {
        session.find.conditions_changed();
    }
    let case = ui.add(crate::theme::quiet_button("Aa").small().selected(session.find.match_case));
    if case.on_hover_text(tr.find_match_case()).clicked() {
        session.find.match_case = !session.find.match_case;
        session.find.conditions_changed();
    }
    if ui.add(crate::theme::quiet_button(tr.find_show_replace()).small().selected(session.find.replace)).clicked() {
        session.find.replace = !session.find.replace;
    }
    step
}

fn navigate_controls(ui: &mut egui::Ui, session: &mut Session, tr: &Tr, mut step: i32) -> (i32, bool) {
    if ui.add(crate::theme::quiet_button(tr.find_prev()).small()).clicked() {
        step = -1;
    }
    if ui.add(crate::theme::quiet_button(tr.find_next()).small()).clicked() {
        step = 1;
    }
    let revision = session.editor.revision();
    let total = session.find.hits(session.editor.note(), revision).len();
    if !session.find.query.is_empty() {
        let count = if total == 0 { tr.find_none().to_string() } else { format!("{} / {total}", session.find.current + 1) };
        ui.label(egui::RichText::new(count).small().weak());
    }
    let close = ui.add(crate::theme::quiet_button(tr.close()).small()).clicked();
    (step, close)
}

/// 画查找栏。在正文之前调用。
pub fn show(ui: &mut egui::Ui, session: &mut Session, tr: &Tr) {
    if !session.find.open {
        return;
    }
    // Esc 关栏。egui 一看到 Esc 就先让控件失去焦点，所以这里不看焦点在不在栏里。
    // 有菜单或弹窗开着时，Esc 是留给它们的。
    let escape = ui.is_enabled() && !ui.any_popup_open()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut step = 0;
    let (mut land, mut replace_one, mut replace_all) = (false, false, false);
    let palette = crate::theme::Palette::for_ui(ui);
    ui.add_space(crate::theme::SMALL_GAP);
    egui::Frame::new()
        .fill(palette.rail)
        .stroke(egui::Stroke::new(1.0, palette.line))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            let available = ui.available_width();
            let field_width = FIELD_WIDTH.min(available * 0.44);
            (step, land) = if available < 600.0 {
                let step = ui.horizontal(|ui| query_controls(ui, session, tr, field_width)).inner;
                ui.horizontal(|ui| navigate_controls(ui, session, tr, step)).inner
            } else {
                // Leave room for English buttons and the range count even with a full-width query.
                ui.horizontal(|ui| {
                    let step = query_controls(ui, session, tr, field_width.min(200.0));
                    navigate_controls(ui, session, tr, step)
                }).inner
            };
            if session.find.replace {
                ui.horizontal(|ui| {
                    let replacement = egui::TextEdit::singleline(&mut session.find.replacement)
                        .id(egui::Id::unique(REPLACEMENT_ID))
                        .hint_text(tr.replace_placeholder())
                        .desired_width(field_width.min(available * 0.34))
                        .margin(egui::Margin::symmetric(6, 4))
                        .return_key(None::<egui::KeyboardShortcut>)
                        .show(ui)
                        .response;
                    if replacement.has_focus() {
                        session.other_text_focus = true;
                        session.late_text_focus = true;
                        // 回车：换掉当前这一处。
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                            replace_one = true;
                        }
                    }
                    if ui.add(crate::theme::quiet_button(tr.find_replace()).small()).clicked() {
                        replace_one = true;
                    }
                    if ui.add(crate::theme::quiet_button(tr.find_replace_all()).small()).clicked() {
                        replace_all = true;
                    }
                });
            }
        });
    if step != 0 {
        session.find.step(session.editor.note(), session.editor.revision(), step);
    }
    if replace_one {
        replace_current(session);
    }
    if replace_all {
        let count = replace_everywhere(session);
        session.notice = Some(replaced(*tr, count));
    }
    if land || escape {
        // 正文里本来有焦点时，Esc 已经让 egui 把它拿走了：关栏之后还给正文，光标留在原处。
        // 焦点在栏里时，选中当前这一处（没有命中就回到打开时光标的位置）。
        let caret = escape.then(|| session.editor_ui.caret_position()).flatten();
        let target = session.find.where_to_land();
        session.find.close();
        match caret {
            Some((block, at)) => session.editor_ui.request_focus(block, at),
            None => {
                if let Some((block, start, end)) = target {
                    session.editor_ui.request_selection(block, start, end);
                }
            }
        }
    }
    ui.add_space(4.0);
}
