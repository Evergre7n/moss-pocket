//! 块式编辑器的纯逻辑。界面层只负责把按键和文本变化转交给这里，不自己改笔记。
//!
//! 撤销是整体快照：每次改动前把整条笔记存一份，撤销时换回去。连续打字在
//! [`COALESCE_WINDOW`] 内合并成一步，所以按一次撤销不会只退掉一个字。

use crate::find::Edit;
use crate::model::{
    byte_to_char, char_count, char_to_byte, Block, BlockId, InlineStyle, Note, StyleFlag, StyleSpan,
    TextBlock, TextKind, TitleSource,
};
use crate::spans::{apply_to_range, diff_prefix_suffix, normalize, patch, patch_range, style_at};
use crate::util::{now_millis, random_block_id, valid_file_name};
use std::time::{Duration, Instant};

/// 连续输入在这个时间窗内合并成一步撤销。
const COALESCE_WINDOW: Duration = Duration::from_millis(900);
/// 单次改动不超过这么多字符才算「打字」，才参与合并。粘贴大段文字不合并。
const COALESCE_MAX_CHARS: usize = 16;
/// 撤销栈最多保留的快照数。
const MAX_UNDO: usize = 200;

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidAudioName;

/// 音频文件名中可改的部分和保留的扩展名（包含点）。
pub fn audio_name_parts(file_name: &str) -> (&str, &str) {
    file_name.rsplit_once('.')
        .filter(|(_, extension)| !extension.is_empty())
        .map_or((file_name, ""), |(stem, _)| (stem, &file_name[stem.len()..]))
}

pub struct Editor {
    note: Note,
    undo: Vec<Note>,
    redo: Vec<Note>,
    last_typing: Option<(BlockId, Instant)>,
    /// 光标处按了样式键之后、还没打字时的「待用样式」：(块, 位置, 样式)。
    pending_style: Option<(BlockId, u32, InlineStyle)>,
    dirty: bool,
    /// 内容每变一次加一。界面用它判断按内容算出来的缓存（比如查找结果）还准不准。
    revision: u64,
}

/// 光标所在的行信息，给「按上下方向键跳出本块」用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaretLine {
    pub line: usize,
    pub column: usize,
    pub total: usize,
}

/// 光标在 `char_index` 处时，它位于文本的第几行、第几列。列按字符数计。
pub fn caret_line(text: &str, char_index: usize) -> CaretLine {
    let mut info = CaretLine {
        line: 0,
        column: 0,
        total: 1 + text.matches('\n').count(),
    };
    for (index, ch) in text.chars().enumerate() {
        if index == char_index {
            return info;
        }
        if ch == '\n' {
            info.line += 1;
            info.column = 0;
        } else {
            info.column += 1;
        }
    }
    info
}

/// 第 `line` 行第一个字符的下标。
pub fn line_start_char_index(text: &str, line: usize) -> usize {
    let mut current = 0;
    for (index, ch) in text.chars().enumerate() {
        if current == line {
            return index;
        }
        if ch == '\n' {
            current += 1;
        }
    }
    char_count(text)
}

impl Editor {
    pub fn new(note: Note) -> Self {
        Self {
            note,
            undo: Vec::new(),
            redo: Vec::new(),
            last_typing: None,
            pending_style: None,
            dirty: false,
            revision: 0,
        }
    }

    pub fn note(&self) -> &Note {
        &self.note
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// 内容变过多少次。换了笔记也会变。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 存盘之后调用：清脏标记并把修改时间对齐。
    pub fn mark_saved(&mut self, now: u64) {
        self.dirty = false;
        self.note.updated_at = now;
    }

    /// 元数据（置顶、标签）不走撤销栈，直接改。
    pub fn note_meta_mut(&mut self) -> &mut Note {
        self.touch();
        &mut self.note
    }

    /// 切换到另一条笔记：撤销栈清空。
    pub fn load(&mut self, note: Note) {
        self.note = note;
        self.undo.clear();
        self.redo.clear();
        self.reset_typing_state();
        self.dirty = false;
        self.revision = self.revision.wrapping_add(1);
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        self.note.updated_at = now_millis();
    }

    fn push_undo(&mut self) {
        if self.undo.len() >= MAX_UNDO {
            self.undo.remove(0);
        }
        self.undo.push(self.note.clone());
        self.redo.clear();
    }

    fn reset_typing_state(&mut self) {
        self.last_typing = None;
        self.pending_style = None;
    }

    pub fn undo(&mut self) -> bool {
        self.reset_typing_state();
        match self.undo.pop() {
            Some(previous) => {
                self.redo.push(self.note.clone());
                self.note.blocks = previous.blocks;
                self.note.title_source = previous.title_source;
                self.touch();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.reset_typing_state();
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(self.note.clone());
                self.note.blocks = next.blocks;
                self.note.title_source = next.title_source;
                self.touch();
                true
            }
            None => false,
        }
    }

    pub fn block_index(&self, block_id: BlockId) -> Option<usize> {
        self.note.blocks.iter().position(|block| block.id() == block_id)
    }

    pub fn text_block(&self, block_id: BlockId) -> Option<&TextBlock> {
        self.note.blocks.iter().find_map(|block| match block {
            Block::Text(text) if text.id == block_id => Some(text),
            _ => None,
        })
    }

    fn text_pair(&self, index: usize) -> Option<(&TextBlock, &TextBlock)> {
        let previous = self.note.blocks.get(index.wrapping_sub(1))?.as_text()?;
        let current = self.note.blocks.get(index)?.as_text()?;
        Some((previous, current))
    }

    /// 界面报告某文本块的内容变了。返回是否真的发生了改动。
    pub fn text_changed(&mut self, block_id: BlockId, new_text: &str) -> bool {
        let Some(index) = self.block_index(block_id) else {
            return false;
        };
        let Some(old) = self.note.blocks[index]
            .as_text()
            .filter(|block| block.text != new_text)
            .map(|block| block.text.clone())
        else {
            return false;
        };
        let (prefix, suffix) = diff_prefix_suffix(&old, new_text);
        let removed = char_count(&old) - prefix - suffix;
        let added = char_count(new_text) - prefix - suffix;
        let small = removed <= COALESCE_MAX_CHARS && added <= COALESCE_MAX_CHARS;
        let now = Instant::now();
        let coalesce = small
            && self
                .last_typing
                .is_some_and(|(id, at)| id == block_id && now.duration_since(at) < COALESCE_WINDOW);

        let pending_insert = match self.pending_style {
            Some((id, position, style)) if id == block_id && position as usize == prefix && removed == 0 && added > 0 => {
                Some((position, style))
            }
            _ => None,
        };
        // 光标挪到别处打字了：之前按出来的待用样式作废。
        if pending_insert.is_none() {
            self.pending_style = None;
        }
        if !coalesce {
            self.push_undo();
        }

        self.apply_text(index, &old, new_text, pending_insert);

        self.last_typing = if small { Some((block_id, now)) } else { None };
        if let Some((_, style)) = pending_insert {
            // 接着往下打字时沿用这个样式。
            self.pending_style = Some((block_id, (prefix + added) as u32, style));
        }
        self.touch();
        true
    }

    /// 把第 `index` 个文本块的内容换成 `new_text`，样式区间跟着挪。
    /// `pending_insert` 是「按了样式键之后接着打字」的情形：新插入的文字用那个样式。
    fn apply_text(&mut self, index: usize, old: &str, new_text: &str, pending_insert: Option<(u32, InlineStyle)>) {
        let Block::Text(block) = &mut self.note.blocks[index] else {
            return;
        };
        block.spans = patch(
            &block.spans,
            old,
            new_text,
            pending_insert.map(|(position, _)| position),
            pending_insert.map(|(_, style)| style),
        );
        block.text = new_text.to_string();
    }

    /// 查找替换：把若干处文字一次换掉，整体算一步撤销。范围越界的条目跳过，不影响其他的。
    /// 每个块里的多处要按先后顺序给出、互不重叠。返回实际替换的处数。
    pub fn replace_spans(&mut self, edits: &[Edit]) -> usize {
        if !edits.iter().any(|edit| valid_edit(&self.note, edit)) {
            return 0;
        }
        self.reset_typing_state();
        self.push_undo();
        // 从后往前换：前面位置不受影响，每一处的样式也只跟着自己原来那个字走。
        let mut count = 0;
        for edit in edits.iter().rev() {
            if !valid_edit(&self.note, edit) {
                continue;
            }
            let index = self.block_index(edit.block).unwrap_or_default();
            let old = self.note.blocks[index].as_text().map(|block| block.text.clone()).unwrap_or_default();
            let mut new = old.clone();
            let (from, to) = (char_to_byte(&old, edit.start as usize), char_to_byte(&old, edit.end as usize));
            new.replace_range(from..to, &edit.replacement);
            if new == old {
                count += 1;
                continue;
            }
            if let Some(block) = self.note.blocks[index].as_text_mut() {
                block.spans = patch_range(
                    &block.spans,
                    char_count(&new) as u32,
                    edit.start,
                    edit.end,
                    edit.start + char_count(&edit.replacement) as u32,
                    None,
                );
                block.text = new;
            }
            count += 1;
        }
        self.touch();
        count
    }

    /// 在光标处把文本块一分为二，返回新块的标识。
    ///
    /// 在空的列表项上按回车不分块，而是把它变回普通段落，返回 None。
    pub fn split_block(&mut self, block_id: BlockId, char_index: usize) -> Option<BlockId> {
        let index = self.block_index(block_id)?;
        let current = self.note.blocks[index].as_text()?;
        let outdent_only = current.text.is_empty() && current.kind.is_list();
        let new_id = random_block_id().ok()?;

        self.push_undo();
        self.reset_typing_state();
        let Block::Text(block) = &mut self.note.blocks[index] else {
            return None;
        };
        if outdent_only {
            block.kind = TextKind::Paragraph;
            self.touch();
            return None;
        }

        let byte = char_to_byte(&block.text, char_index);
        let tail_text = block.text.split_off(byte);
        let head_len = char_count(&block.text) as u32;
        // 样式区间跟着文字走：落在前半的留下，落到后半的平移到新块。
        let mut head_spans = Vec::new();
        let mut tail_spans = Vec::new();
        for span in &block.spans {
            if span.start < head_len {
                head_spans.push(StyleSpan {
                    start: span.start,
                    end: span.end.min(head_len),
                    style: span.style,
                });
            }
            if span.end > head_len {
                tail_spans.push(StyleSpan {
                    start: span.start.saturating_sub(head_len),
                    end: span.end - head_len,
                    style: span.style,
                });
            }
        }
        block.spans = normalize(head_spans, head_len);
        let new_block = TextBlock {
            id: new_id,
            kind: block.kind.continuation(),
            spans: normalize(tail_spans, char_count(&tail_text) as u32),
            text: tail_text,
        };
        self.note.blocks.insert(index + 1, Block::Text(new_block));
        self.touch();
        Some(new_id)
    }

    /// 把本块并到上一个文本块的末尾，返回合并后光标应停的字符位置。
    ///
    /// 在列表项的行首按退格不合并，而是把它变回普通段落，返回 None。
    pub fn merge_with_previous(&mut self, block_id: BlockId) -> Option<usize> {
        let index = self.block_index(block_id)?;
        let current = self.note.blocks[index].as_text()?;
        let outdent_only = current.kind.is_list();
        let offset = if outdent_only {
            0
        } else {
            let (previous, _) = self.text_pair(index)?;
            if matches!(self.note.title_source, TitleSource::Block(id) if id == previous.id || id == current.id) {
                return None;
            }
            char_count(&previous.text)
        };

        self.push_undo();
        self.reset_typing_state();
        if outdent_only {
            if let Some(block) = self.note.blocks.get_mut(index).and_then(Block::as_text_mut) {
                block.kind = TextKind::Paragraph;
            }
            self.touch();
            return None;
        }

        let removed = self.note.blocks.remove(index);
        let Block::Text(mut current) = removed else {
            self.note.blocks.insert(index, removed);
            return None;
        };
        let Some(Block::Text(previous_block)) = self.note.blocks.get_mut(index - 1) else {
            self.note.blocks.insert(index, Block::Text(current));
            return None;
        };
        for span in &mut current.spans {
            span.start += offset as u32;
            span.end += offset as u32;
        }
        let total = offset + char_count(&current.text);
        previous_block.spans.extend(current.spans);
        previous_block.text.push_str(&current.text);
        previous_block.spans = normalize(std::mem::take(&mut previous_block.spans), total as u32);
        self.touch();
        Some(offset)
    }

    /// 删除一个块。删完空了就补一个空段落。返回建议接下来聚焦的文本块。
    pub fn remove_block(&mut self, block_id: BlockId) -> Option<BlockId> {
        let index = self.block_index(block_id)?;
        self.push_undo();
        self.reset_typing_state();
        self.note.blocks.remove(index);
        if self.note.title_source == TitleSource::Block(block_id) {
            self.note.title_source = TitleSource::Body;
        }
        if self.note.blocks.is_empty() {
            if let Ok(empty) = TextBlock::new_empty() {
                self.note.blocks.push(Block::Text(empty));
            }
        }
        self.touch();
        self.nearest_text_block(index)
    }

    /// 从 `from` 开始就近找一个文本块：先看它自己及之后的，再往前找。
    fn nearest_text_block(&self, from: usize) -> Option<BlockId> {
        self.note.blocks[from..]
            .iter()
            .chain(self.note.blocks[..from].iter().rev())
            .find_map(Block::as_text)
            .map(|block| block.id)
    }

    /// 在 `anchor` 之后插入一个块；anchor 为 None 时插到末尾。返回新块的标识。
    pub fn insert_block_after(&mut self, anchor: Option<BlockId>, block: Block) -> BlockId {
        let index = anchor
            .and_then(|id| self.block_index(id))
            .map_or(self.note.blocks.len(), |index| index + 1);
        self.push_undo();
        self.reset_typing_state();
        let id = block.id();
        let index = index.min(self.note.blocks.len());
        self.note.blocks.insert(index, block);
        self.touch();
        id
    }

    pub fn append_block(&mut self, block: Block) -> BlockId {
        self.insert_block_after(None, block)
    }

    pub fn add_title(&mut self) -> Option<BlockId> {
        if !self.note.can_add_title() {
            return None;
        }
        let title = TextBlock::new_empty().ok()?;
        let id = title.id;
        self.push_undo();
        self.reset_typing_state();
        self.note.blocks.insert(0, Block::Text(title));
        self.note.title_source = TitleSource::Block(id);
        self.touch();
        Some(id)
    }

    /// 没有文本块聚焦时粘贴了一段文字：在末尾追加一个段落。
    pub fn append_paragraph(&mut self, text: &str) -> Option<BlockId> {
        if text.is_empty() {
            return None;
        }
        let mut block = TextBlock::new_empty().ok()?;
        block.text = text.to_string();
        Some(self.append_block(Block::Text(block)))
    }

    /// 整块上移或下移一格。
    pub fn move_block(&mut self, block_id: BlockId, offset: i32) -> bool {
        let Some(index) = self.block_index(block_id) else {
            return false;
        };
        let target = index as i64 + offset as i64;
        if target < 0 || target >= self.note.blocks.len() as i64 {
            return false;
        }
        let target = target as usize;
        self.push_undo();
        self.reset_typing_state();
        let block = self.note.blocks.remove(index);
        self.note.blocks.insert(target, block);
        self.touch();
        true
    }

    pub fn set_text_kind(&mut self, block_id: BlockId, kind: TextKind) -> bool {
        let Some(text) = self
            .note
            .blocks
            .iter()
            .find_map(|block| block.as_text().filter(|text| text.id == block_id))
        else {
            return false;
        };
        if text.kind == kind {
            return false;
        }
        self.push_undo();
        self.reset_typing_state();
        for block in &mut self.note.blocks {
            if let Some(text) = block.as_text_mut() {
                if text.id == block_id {
                    text.kind = kind;
                }
            }
        }
        self.touch();
        true
    }

    /// 切换行内样式。有选区时作用于选区；没有选区时设置接下来要输入的样式。
    pub fn toggle_style(&mut self, block_id: BlockId, range: Option<(u32, u32)>, flag: StyleFlag) {
        let Some(text) = self.text_block(block_id) else {
            return;
        };
        let (start, end) = match range {
            Some((a, b)) if a != b => (a.min(b), a.max(b)),
            _ => {
                let caret = range.map_or(0, |(a, b)| a.min(b));
                let mut style = self.caret_style(block_id, caret);
                style.set(flag, !style.get(flag));
                self.pending_style = Some((block_id, caret, style));
                self.last_typing = None;
                return;
            }
        };
        let turn_on = !style_at(&text.spans, start).get(flag);
        self.push_undo();
        self.reset_typing_state();
        if let Some(block) = self.note.blocks.iter_mut().filter_map(Block::as_text_mut).find(|block| block.id == block_id) {
            let text_len = char_count(&block.text) as u32;
            block.spans = apply_to_range(&block.spans, text_len, start, end, |style| {
                style.set(flag, turn_on)
            });
        }
        self.touch();
    }

    /// 在本块后面复制一份（新标识），返回新块标识。媒体块复制后和原块共用同一个文件。
    pub fn duplicate_block(&mut self, block_id: BlockId) -> Option<BlockId> {
        let index = self.block_index(block_id)?;
        let copy = self.note.blocks[index].duplicated()?;
        let fresh_id = copy.id();
        self.push_undo();
        self.reset_typing_state();
        self.note.blocks.insert(index + 1, copy);
        self.touch();
        Some(fresh_id)
    }

    /// 给文本块设颜色；传 None 清掉颜色。
    pub fn set_color(&mut self, block_id: BlockId, range: Option<(u32, u32)>, color: Option<u32>) {
        let Some(_) = self.text_block(block_id) else {
            return;
        };
        let (start, end) = match range {
            Some((a, b)) if a != b => (a.min(b), a.max(b)),
            _ => {
                let caret = range.map_or(0, |(a, b)| a.min(b));
                let mut style = self.caret_style(block_id, caret);
                style.color = color;
                self.pending_style = Some((block_id, caret, style));
                self.last_typing = None;
                return;
            }
        };
        self.push_undo();
        self.reset_typing_state();
        if let Some(block) = self.note.blocks.iter_mut().filter_map(Block::as_text_mut).find(|block| block.id == block_id) {
            let text_len = char_count(&block.text) as u32;
            block.spans = apply_to_range(&block.spans, text_len, start, end, |style| {
                style.color = color
            });
        }
        self.touch();
    }

    fn caret_style(&self, block_id: BlockId, caret: u32) -> InlineStyle {
        if let Some((id, position, style)) = self.pending_style {
            if id == block_id && position == caret {
                return style;
            }
        }
        self.text_block(block_id)
            .map(|block| style_at(&block.spans, caret.saturating_sub(1)))
            .unwrap_or_default()
    }

    pub fn set_image_width(&mut self, block_id: BlockId, width: Option<f32>) {
        let is_image = |block: &mut Block| matches!(block, Block::Image(image) if image.id == block_id);
        if !self.note.blocks.iter_mut().any(is_image) {
            return;
        }
        self.push_undo();
        self.reset_typing_state();
        for block in &mut self.note.blocks {
            if let Block::Image(image) = block {
                if image.id == block_id {
                    image.display_width = width;
                }
            }
        }
        self.touch();
    }

    /// 改一块音频的文件名，计作一步撤销。媒体内容、扩展名和格式保持原样。
    pub fn rename_audio(&mut self, block_id: BlockId, stem: &str) -> Result<bool, InvalidAudioName> {
        let Some(index) = self.block_index(block_id).filter(|_| self.note.deleted_at.is_none()) else {
            return Ok(false);
        };
        let Block::Audio(audio) = &self.note.blocks[index] else {
            return Ok(false);
        };
        let (old_stem, extension) = audio_name_parts(&audio.file_name);
        if old_stem == stem && !stem.trim().is_empty() {
            return Ok(false);
        }
        let stem = stem.trim();
        let stem = match stem.rsplit_once('.') {
            Some((name, typed)) if extension.get(1..).is_some_and(|ext| ext.eq_ignore_ascii_case(typed)) => name.trim_end(),
            _ => stem,
        };
        if !valid_file_name(stem) {
            return Err(InvalidAudioName);
        }
        let name = format!("{stem}{extension}");
        if !valid_file_name(&name) {
            return Err(InvalidAudioName);
        }
        if audio.file_name == name {
            return Ok(false);
        }
        self.push_undo();
        self.reset_typing_state();
        if let Block::Audio(audio) = &mut self.note.blocks[index] {
            audio.file_name = name;
        }
        self.touch();
        Ok(true)
    }
}

/// 一次替换是否落在文本块的范围内。
fn valid_edit(note: &Note, edit: &Edit) -> bool {
    note.blocks
        .iter()
        .filter_map(Block::as_text)
        .find(|block| block.id == edit.block)
        .is_some_and(|block| edit.start <= edit.end && (edit.end as usize) <= char_count(&block.text))
}

/// 把字节下标的选区换算成字符下标的选区，并夹到文本长度内。
pub fn char_range(text: &str, start_byte: usize, end_byte: usize) -> (u32, u32) {
    let start = byte_to_char(text, start_byte) as u32;
    let end = byte_to_char(text, end_byte) as u32;
    (start.min(end), start.max(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_text(editor: &Editor) -> BlockId {
        editor.note().blocks[0].as_text().unwrap().id
    }

    fn type_text(editor: &mut Editor, block: BlockId, appended: &str) {
        let current = editor.text_block(block).unwrap().text.clone();
        let next = format!("{current}{appended}");
        assert!(editor.text_changed(block, &next));
    }

    fn marks(editor: &Editor, block: BlockId) -> Vec<bool> {
        let text = editor.text_block(block).unwrap();
        text.text
            .chars()
            .enumerate()
            .map(|(index, _)| style_at(&text.spans, index as u32).bold)
            .collect()
    }

    #[test]
    fn audio_rename_preserves_blob_and_suffix_and_is_one_undo_step() {
        let mut note = Note::new().unwrap();
        let audio = crate::model::MediaBlock {
            id: random_block_id().unwrap(),
            blob_id: "stored-audio".into(),
            file_name: "Original.FLAC".into(),
            mime: "audio/flac".into(),
            size: 100,
            duration_secs: Some(2.5),
        };
        let index = note.blocks.len();
        note.blocks.push(Block::Audio(audio.clone()));
        let mut editor = Editor::new(note);

        assert_eq!(editor.rename_audio(audio.id, "Original"), Ok(false));
        for unsafe_name in ["", " ", "../a", r"a\b", "C:audio", "CON", "clip.", ".flac"] {
            assert_eq!(editor.rename_audio(audio.id, unsafe_name), Err(InvalidAudioName), "{unsafe_name:?}");
        }
        assert!(!editor.can_undo());
        assert!(!editor.dirty());

        assert_eq!(editor.rename_audio(audio.id, " 会议纪要.flac "), Ok(true));
        let mut renamed = audio.clone();
        renamed.file_name = "会议纪要.FLAC".into();
        assert_eq!(editor.note().blocks[index], Block::Audio(renamed.clone()));
        assert!(editor.dirty());
        assert!(editor.undo());
        assert_eq!(editor.note().blocks[index], Block::Audio(audio.clone()));
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.note().blocks[index], Block::Audio(renamed));

        editor.note_meta_mut().deleted_at = Some(0);
        assert_eq!(editor.rename_audio(audio.id, "Trash"), Ok(false));
    }

    #[test]
    fn replacing_many_spans_is_one_undo_step() {
        let mut editor = Editor::new(Note::new().unwrap());
        let first = first_text(&editor);
        let second = editor.append_paragraph("猫在沙发上，猫在睡觉").unwrap();
        assert!(editor.text_changed(first, "猫和狗"));
        let hits = crate::find::find_in_note(editor.note(), "猫", false);
        assert_eq!(hits.len(), 3);
        assert_eq!(editor.replace_spans(&crate::find::replace_all(&hits, "狗")), 3);
        assert_eq!(editor.text_block(first).unwrap().text, "狗和狗");
        assert_eq!(editor.text_block(second).unwrap().text, "狗在沙发上，狗在睡觉");
        // 整批替换只占一步撤销：撤一步，两块一起回到替换前。
        assert!(editor.undo());
        assert_eq!(editor.text_block(first).unwrap().text, "猫和狗");
        assert_eq!(editor.text_block(second).unwrap().text, "猫在沙发上，猫在睡觉");
        assert!(editor.redo());
        assert_eq!(editor.text_block(second).unwrap().text, "狗在沙发上，狗在睡觉");
        // 替换完马上接着打字：打字是另一步，不会并进替换里。
        type_text(&mut editor, first, "！");
        assert_eq!(editor.text_block(first).unwrap().text, "狗和狗！");
        assert!(editor.undo());
        assert_eq!(editor.text_block(first).unwrap().text, "狗和狗");
    }

    #[test]
    fn replacing_keeps_each_span_on_its_own_text() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        assert!(editor.text_changed(block, "猫 狗 猫"));
        // 把中间的「狗」加粗。
        editor.toggle_style(block, Some((2, 3)), StyleFlag::Bold);
        assert_eq!(marks(&editor, block), vec![false, false, true, false, false]);
        let hits = crate::find::find_in_note(editor.note(), "猫", false);
        assert_eq!(editor.replace_spans(&crate::find::replace_all(&hits, "狗")), 2);
        // 两处换上来的字是普通字，中间原来加粗的还是粗体。
        assert_eq!(editor.text_block(block).unwrap().text, "狗 狗 狗");
        assert_eq!(marks(&editor, block), vec![false, false, true, false, false]);
    }

    #[test]
    fn out_of_range_replacements_are_skipped() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        assert!(editor.text_changed(block, "abcd"));
        let edits = [
            Edit { block, start: 1, end: 2, replacement: "X".into() },
            Edit { block, start: 5, end: 6, replacement: "Y".into() },
            Edit { block, start: 3, end: 2, replacement: "Z".into() },
        ];
        assert_eq!(editor.replace_spans(&edits), 1);
        assert_eq!(editor.text_block(block).unwrap().text, "aXcd");
        // 一处都换不了时不留撤销步骤。
        let undo_before = editor.can_undo();
        assert_eq!(editor.replace_spans(&edits[1..]), 0);
        assert_eq!(editor.can_undo(), undo_before);
    }

    #[test]
    fn typing_coalesces_into_one_undo_step() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        for ch in ["记", "一", "个", "想", "法"] {
            assert!(editor.text_changed(block, &format!("{}{ch}", editor.text_block(block).unwrap().text)));
        }
        assert_eq!(editor.text_block(block).unwrap().text, "记一个想法");
        assert!(editor.dirty());
        assert!(editor.undo());
        assert_eq!(editor.text_block(block).unwrap().text, "");
        assert!(!editor.undo());
        assert!(editor.redo());
        assert_eq!(editor.text_block(block).unwrap().text, "记一个想法");
        assert!(!editor.redo());
    }

    #[test]
    fn big_paste_is_its_own_undo_step() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        editor.text_changed(block, "短");
        let long = "长".repeat(COALESCE_MAX_CHARS + 1);
        editor.text_changed(block, &format!("短{long}"));
        assert!(editor.undo());
        assert_eq!(editor.text_block(block).unwrap().text, "短");
        assert!(editor.undo());
        assert_eq!(editor.text_block(block).unwrap().text, "");
    }

    #[test]
    fn editing_another_block_starts_a_new_undo_step() {
        let mut editor = Editor::new(Note::new().unwrap());
        let first = first_text(&editor);
        editor.text_changed(first, "甲");
        let second = editor.split_block(first, 3).unwrap();
        type_text(&mut editor, second, "乙");
        assert!(editor.undo());
        assert_eq!(editor.text_block(second).unwrap().text, "");
        assert!(editor.undo());
        assert_eq!(editor.note().blocks.len(), 1);
        assert_eq!(editor.text_block(first).unwrap().text, "甲");
    }

    #[test]
    fn split_moves_text_and_styles_then_merge_restores() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        editor.text_changed(block, "abcdef");
        editor.toggle_style(block, Some((2, 4)), StyleFlag::Bold);
        assert_eq!(marks(&editor, block), [false, false, true, true, false, false]);

        let tail = editor.split_block(block, 3).unwrap();
        let head_text = editor.text_block(block).unwrap();
        let tail_text = editor.text_block(tail).unwrap();
        assert_eq!(head_text.text, "abc");
        assert_eq!(tail_text.text, "def");
        assert_eq!(marks(&editor, block), [false, false, true]);
        assert_eq!(marks(&editor, tail), [true, false, false]);
        assert_eq!(tail_text.kind, TextKind::Paragraph);

        // 光标停在第二块行首按退格：并回第一块，样式原样接上。
        assert_eq!(editor.merge_with_previous(tail), Some(3));
        assert_eq!(editor.note().blocks.len(), 1);
        assert_eq!(editor.text_block(block).unwrap().text, "abcdef");
        assert_eq!(marks(&editor, block), [false, false, true, true, false, false]);
    }

    #[test]
    fn list_items_continue_on_enter_and_outdent_on_backspace_or_enter() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        editor.set_text_kind(block, TextKind::Bullet);
        editor.text_changed(block, "第一项");
        let second = editor.split_block(block, 3).unwrap();
        assert_eq!(editor.text_block(second).unwrap().kind, TextKind::Bullet);
        // 空的列表项上再按回车：变回普通段落，不再往下分。
        assert_eq!(editor.split_block(second, 0), None);
        assert_eq!(editor.text_block(second).unwrap().kind, TextKind::Paragraph);
        // 非空列表项行首按退格：也先退回普通段落，不合并。
        editor.set_text_kind(second, TextKind::Numbered);
        editor.text_changed(second, "二");
        assert_eq!(editor.merge_with_previous(second), None);
        assert_eq!(editor.text_block(second).unwrap().kind, TextKind::Paragraph);
        assert_eq!(editor.note().blocks.len(), 2);
        // 普通段落行首按退格才真正合并。
        assert_eq!(editor.merge_with_previous(second), Some(3));
        assert_eq!(editor.note().blocks.len(), 1);
        assert_eq!(editor.text_block(block).unwrap().text, "第一项二");
    }

    #[test]
    fn first_block_cannot_merge_backwards() {
        let mut editor = Editor::new(Note::new().unwrap());
        assert_eq!(editor.merge_with_previous(first_text(&editor)), None);
        assert!(!editor.undo());
    }

    #[test]
    fn remove_insert_and_move_blocks() {
        let mut editor = Editor::new(Note::new().unwrap());
        let first = first_text(&editor);
        editor.text_changed(first, "一");
        let divider = editor.append_block(Block::Divider { id: 4242 });
        assert_eq!(editor.note().blocks.len(), 2);
        let second = editor.insert_block_after(Some(first), Block::Text(TextBlock::new_empty().unwrap()));
        let order: Vec<BlockId> = editor.note().blocks.iter().map(Block::id).collect();
        assert_eq!(order, [first, second, divider]);
        assert!(editor.move_block(divider, -1));
        assert!(!editor.move_block(divider, -5));
        let order: Vec<BlockId> = editor.note().blocks.iter().map(Block::id).collect();
        assert_eq!(order, [first, divider, second]);
        assert_eq!(editor.remove_block(first), Some(second));
        assert_eq!(editor.note().blocks.len(), 2);
        assert!(editor.undo());
        assert_eq!(editor.note().blocks.len(), 3);
        // 全删光后自动补一个空段落。
        editor.remove_block(first);
        editor.remove_block(divider);
        editor.remove_block(second);
        assert_eq!(editor.note().blocks.len(), 1);
        assert!(editor.note().blocks[0].as_text().is_some());
    }

    #[test]
    fn pending_style_applies_to_typed_text_until_caret_moves() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        editor.text_changed(block, "普通");
        // 光标在末尾、没有选区时按加粗：接下来的字是粗体。
        editor.toggle_style(block, Some((2, 2)), StyleFlag::Bold);
        type_text(&mut editor, block, "粗");
        assert_eq!(marks(&editor, block), [false, false, true]);
        type_text(&mut editor, block, "体");
        assert_eq!(marks(&editor, block), [false, false, true, true]);
        // 光标挪到行首再打字：位置对不上待用样式，按左邻规则走，行首没有左邻就是普通。
        editor.text_changed(block, &format!("头{}", editor.text_block(block).unwrap().text));
        assert_eq!(marks(&editor, block), [false, false, false, true, true]);
    }

    #[test]
    fn formatting_targets_later_blocks_and_keeps_undo() {
        let mut editor = Editor::new(Note::new().unwrap());
        let first = first_text(&editor);
        let second = editor.append_paragraph("中文").unwrap();
        editor.toggle_style(second, Some((0, 2)), StyleFlag::Bold);
        editor.set_color(second, Some((0, 1)), Some(0x123456));
        assert_eq!(marks(&editor, second), [true, true]);
        assert_eq!(style_at(&editor.text_block(second).unwrap().spans, 0).color, Some(0x123456));
        assert!(editor.text_block(first).unwrap().spans.is_empty());
        assert!(editor.undo());
        assert_eq!(style_at(&editor.text_block(second).unwrap().spans, 0).color, None);
        assert_eq!(marks(&editor, second), [true, true]);
    }

    #[test]
    fn pending_styles_compose_toggle_off_and_form_an_undo_boundary() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        type_text(&mut editor, block, "a");
        editor.toggle_style(block, Some((1, 1)), StyleFlag::Bold);
        editor.toggle_style(block, Some((1, 1)), StyleFlag::Italic);
        editor.set_color(block, Some((1, 1)), Some(0x123456));
        type_text(&mut editor, block, "b");
        let style = style_at(&editor.text_block(block).unwrap().spans, 1);
        assert!(style.bold && style.italic);
        assert_eq!(style.color, Some(0x123456));
        assert!(editor.undo());
        assert_eq!(editor.text_block(block).unwrap().text, "a");
        editor.toggle_style(block, Some((1, 1)), StyleFlag::Bold);
        editor.toggle_style(block, Some((1, 1)), StyleFlag::Bold);
        type_text(&mut editor, block, "b");
        assert_eq!(marks(&editor, block), [false, false]);
        editor.toggle_style(block, Some((2, 2)), StyleFlag::Bold);
        type_text(&mut editor, block, "c");
        editor.toggle_style(block, Some((3, 3)), StyleFlag::Bold);
        type_text(&mut editor, block, "d");
        assert_eq!(marks(&editor, block), [false, false, true, false]);
    }

    #[test]
    fn moving_insertion_cancels_pending_style_instead_of_reactivating_it() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        type_text(&mut editor, block, "ab");
        editor.toggle_style(block, Some((2, 2)), StyleFlag::Bold);
        editor.text_changed(block, "xab");
        type_text(&mut editor, block, "y");
        assert_eq!(marks(&editor, block), [false, false, false, false]);
    }

    #[test]
    fn content_undo_redo_preserves_metadata_changes() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        type_text(&mut editor, block, "a");
        editor.note_meta_mut().pinned = true;
        editor.note_meta_mut().tags = vec!["标签".into()];
        assert!(editor.undo());
        assert!(editor.note().pinned);
        assert_eq!(editor.note().tags, ["标签"]);
        editor.note_meta_mut().pinned = false;
        assert!(editor.redo());
        assert!(!editor.note().pinned);
        assert_eq!(editor.note().tags, ["标签"]);
        assert_eq!(editor.text_block(block).unwrap().text, "a");
    }

    #[test]
    fn first_list_item_can_outdent_without_a_previous_text_block() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        editor.set_text_kind(block, TextKind::Bullet);
        assert_eq!(editor.merge_with_previous(block), None);
        assert_eq!(editor.text_block(block).unwrap().kind, TextKind::Paragraph);
        assert!(editor.undo());
        assert_eq!(editor.text_block(block).unwrap().kind, TextKind::Bullet);
    }

    #[test]
    fn explicit_replacement_keeps_styles_at_the_requested_repeated_character() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        type_text(&mut editor, block, "aa");
        editor.toggle_style(block, Some((0, 1)), StyleFlag::Bold);
        editor.toggle_style(block, Some((1, 2)), StyleFlag::Italic);
        assert_eq!(editor.replace_spans(&[Edit { block, start: 0, end: 1, replacement: "aa".into() }]), 1);
        assert_eq!(editor.text_block(block).unwrap().text, "aaa");
        assert_eq!(marks(&editor, block), [true, true, false]);
        assert!(style_at(&editor.text_block(block).unwrap().spans, 2).italic);
        assert!(editor.undo());
        assert_eq!(editor.text_block(block).unwrap().text, "aa");
        let spans = editor.text_block(block).unwrap().spans.clone();
        assert_eq!(editor.replace_spans(&[Edit { block, start: 0, end: 2, replacement: "aa".into() }]), 1);
        assert_eq!(editor.text_block(block).unwrap().spans, spans);
    }

    #[test]
    fn selection_style_toggle_is_one_undo_step() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        editor.text_changed(block, "hello");
        editor.toggle_style(block, Some((4, 1)), StyleFlag::Italic);
        let italics: Vec<bool> = editor.text_block(block).unwrap().spans.iter().map(|s| s.style.italic).collect();
        assert_eq!(italics, [true]);
        assert_eq!(editor.text_block(block).unwrap().spans[0].start, 1);
        assert_eq!(editor.text_block(block).unwrap().spans[0].end, 4);
        assert!(editor.undo());
        assert!(editor.text_block(block).unwrap().spans.is_empty());
    }

    #[test]
    fn duplicate_block_gets_a_fresh_id_and_can_be_undone() {
        let mut editor = Editor::new(Note::new().unwrap());
        let first = first_text(&editor);
        editor.text_changed(first, "要复制的想法");
        let copy = editor.duplicate_block(first).unwrap();
        assert_ne!(copy, first);
        assert_eq!(editor.note().blocks.len(), 2);
        assert_eq!(editor.text_block(copy).unwrap().text, "要复制的想法");
        // 复制出来的块有独立的标识，改它不影响原块。
        editor.text_changed(copy, "改过的副本");
        assert_eq!(editor.text_block(first).unwrap().text, "要复制的想法");
        // 整块删除后焦点落到相邻的文本块上；撤销能把它找回来。
        let next = editor.remove_block(copy).unwrap();
        assert_eq!(next, first);
        assert!(editor.undo());
        assert_eq!(editor.note().blocks.len(), 2);
        // 不存在的块不能复制。
        assert!(editor.duplicate_block(999_999).is_none());
    }

    #[test]
    fn caret_line_helpers() {
        let text = "ab\ncde\nf";
        assert_eq!(caret_line(text, 0), CaretLine { line: 0, column: 0, total: 3 });
        assert_eq!(caret_line(text, 3), CaretLine { line: 1, column: 0, total: 3 });
        assert_eq!(caret_line(text, 7), CaretLine { line: 2, column: 0, total: 3 });
        assert_eq!(caret_line(text, 99), CaretLine { line: 2, column: 1, total: 3 });
        assert_eq!(line_start_char_index(text, 0), 0);
        assert_eq!(line_start_char_index(text, 1), 3);
        assert_eq!(line_start_char_index(text, 1) + 2, 5);
    }

    #[test]
    fn undo_stack_is_bounded() {
        let mut editor = Editor::new(Note::new().unwrap());
        let block = first_text(&editor);
        // 每次都是大段改动、不合并；栈被压到上限，旧的自然丢弃。
        for step in 0..(MAX_UNDO + 10) {
            let filler = if step % 2 == 0 { 'a' } else { 'b' };
            editor.text_changed(block, &filler.to_string().repeat(COALESCE_MAX_CHARS + 1 + step));
        }
        let mut steps = 0;
        while editor.undo() {
            steps += 1;
        }
        assert_eq!(steps, MAX_UNDO);
    }
}
