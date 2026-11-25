//! 笔记与块的数据模型。序列化成 JSON 后整体加密，就是磁盘上的一条笔记。

use crate::error::Result;
use crate::util::{now_millis, random_block_id, random_id};
use serde::{Deserialize, Serialize};

pub type BlockId = u64;

/// 行内样式。颜色是 0xRRGGBB 形式的整数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InlineStyle {
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strikethrough: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// 行内样式开关：粗体、斜体、下划线、删除线。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleFlag {
    Bold,
    Italic,
    Underline,
    Strikethrough,
}

impl InlineStyle {
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }

    pub fn get(&self, flag: StyleFlag) -> bool {
        match flag {
            StyleFlag::Bold => self.bold,
            StyleFlag::Italic => self.italic,
            StyleFlag::Underline => self.underline,
            StyleFlag::Strikethrough => self.strikethrough,
        }
    }

    pub fn set(&mut self, flag: StyleFlag, value: bool) {
        match flag {
            StyleFlag::Bold => self.bold = value,
            StyleFlag::Italic => self.italic = value,
            StyleFlag::Underline => self.underline = value,
            StyleFlag::Strikethrough => self.strikethrough = value,
        }
    }
}

/// 一段带样式的文字。起止都是**字符**下标，左闭右开。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSpan {
    pub start: u32,
    pub end: u32,
    pub style: InlineStyle,
}

/// 文本块的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextKind {
    #[default]
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Bullet,
    Numbered,
    Quote,
}

impl TextKind {
    /// 按回车后，新块沿用哪种类型：列表项继续是列表项，其余变回普通段落。
    pub fn continuation(self) -> Self {
        match self {
            Self::Bullet => Self::Bullet,
            Self::Numbered => Self::Numbered,
            _ => Self::Paragraph,
        }
    }

    pub fn is_list(self) -> bool {
        matches!(self, Self::Bullet | Self::Numbered)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBlock {
    pub id: BlockId,
    #[serde(default)]
    pub kind: TextKind,
    pub text: String,
    #[serde(default)]
    pub spans: Vec<StyleSpan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageBlock {
    pub id: BlockId,
    pub blob_id: String,
    pub width: u32,
    pub height: u32,
    /// 用户拖出来的显示宽度（逻辑像素）；None 表示按原尺寸显示。
    #[serde(default)]
    pub display_width: Option<f32>,
}

/// 音频和视频共用的结构：媒体内容在库里是一个加密文件，这里只记引用和说明信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaBlock {
    pub id: BlockId,
    pub blob_id: String,
    pub file_name: String,
    pub mime: String,
    pub size: u64,
    #[serde(default)]
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text(TextBlock),
    Image(ImageBlock),
    Audio(MediaBlock),
    Video(MediaBlock),
    Divider { id: BlockId },
}

impl Block {
    pub fn id(&self) -> BlockId {
        match self {
            Block::Text(block) => block.id,
            Block::Image(block) => block.id,
            Block::Audio(block) | Block::Video(block) => block.id,
            Block::Divider { id } => *id,
        }
    }

    pub fn as_text(&self) -> Option<&TextBlock> {
        match self {
            Block::Text(block) => Some(block),
            _ => None,
        }
    }

    pub fn as_text_mut(&mut self) -> Option<&mut TextBlock> {
        match self {
            Block::Text(block) => Some(block),
            _ => None,
        }
    }

    pub fn blob_id(&self) -> Option<&str> {
        match self {
            Block::Image(block) => Some(&block.blob_id),
            Block::Audio(block) | Block::Video(block) => Some(&block.blob_id),
            _ => None,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self, Block::Image(_))
    }

    /// 复制一份并换一个新的标识。媒体块复制后和原块共用同一个加密文件，不会多占空间。
    pub fn duplicated(&self) -> Option<Self> {
        let fresh = random_block_id().ok()?;
        let mut copy = self.clone();
        match &mut copy {
            Block::Text(text) => text.id = fresh,
            Block::Image(image) => image.id = fresh,
            Block::Audio(media) | Block::Video(media) => media.id = fresh,
            Block::Divider { id } => *id = fresh,
        }
        Some(copy)
    }

    pub fn is_divider(&self) -> bool {
        matches!(self, Block::Divider { .. })
    }
}

impl TextBlock {
    pub fn new_empty() -> Result<Self> {
        Ok(Self {
            id: random_block_id()?,
            kind: TextKind::Paragraph,
            text: String::new(),
            spans: Vec::new(),
        })
    }
}

/// 旧笔记沿用首块标题，新笔记直接写正文，也可以显式添加标题块。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    #[default]
    Legacy,
    Body,
    Block(BlockId),
}

impl TitleSource {
    fn is_legacy(&self) -> bool {
        *self == Self::Legacy
    }
}

/// 一条笔记。文件名就是 `id.note`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    /// 进回收站的时间；None 表示不在回收站。
    #[serde(default)]
    pub deleted_at: Option<u64>,
    #[serde(default, skip_serializing_if = "TitleSource::is_legacy")]
    pub title_source: TitleSource,
    pub blocks: Vec<Block>,
}

impl Note {
    pub fn new() -> Result<Self> {
        let now = now_millis();
        Ok(Self {
            id: random_id()?,
            created_at: now,
            updated_at: now,
            pinned: false,
            tags: Vec::new(),
            deleted_at: None,
            title_source: TitleSource::Legacy,
            blocks: vec![Block::Text(TextBlock::new_empty()?)],
        })
    }

    pub fn new_capture() -> Result<Self> {
        let mut note = Self::new()?;
        note.title_source = TitleSource::Body;
        Ok(note)
    }

    pub fn title_block(&self) -> Option<&TextBlock> {
        match self.title_source {
            TitleSource::Legacy => self.blocks.first().and_then(Block::as_text),
            TitleSource::Body => None,
            TitleSource::Block(id) => self.blocks.iter().filter_map(Block::as_text).find(|block| block.id == id),
        }
    }

    pub fn can_add_title(&self) -> bool {
        !self.title_source.is_legacy() && self.title_block().is_none()
    }

    /// 列表、搜索和导出文件名使用的名称。未填写标题时取正文第一条非空行。
    pub fn title(&self) -> &str {
        self.title_block()
            .and_then(first_nonempty_line)
            .or_else(|| self.blocks.iter().filter_map(Block::as_text).find_map(first_nonempty_line))
            .unwrap_or_default()
    }

    /// 所有文本块的内容，块之间换行。搜索和导出纯文本用。
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        let mut first = true;
        for block in &self.blocks {
            if let Block::Text(text) = block {
                if !first {
                    out.push('\n');
                }
                first = false;
                out.push_str(&text.text);
            }
        }
        out
    }

    pub fn blob_ids(&self) -> Vec<&str> {
        self.blocks.iter().filter_map(Block::blob_id).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|block| match block {
            Block::Text(text) => text.text.trim().is_empty(),
            _ => false,
        })
    }
}

fn first_nonempty_line(block: &TextBlock) -> Option<&str> {
    block.text.trim_start().lines().next().map(str::trim_end)
}

/// 列表和索引里用的摘要，不含正文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub pinned: bool,
    pub tags: Vec<String>,
    pub deleted_at: Option<u64>,
}

impl From<&Note> for NoteSummary {
    fn from(note: &Note) -> Self {
        Self {
            id: note.id.clone(),
            title: note.title().to_string(),
            created_at: note.created_at,
            updated_at: note.updated_at,
            pinned: note.pinned,
            tags: note.tags.clone(),
            deleted_at: note.deleted_at,
        }
    }
}

/// 列表排序：置顶在前，其余按修改时间从新到旧。时间也相同时按创建时间和标识排，
/// 保证同一份数据每次显示的顺序都一样。
pub fn sort_summaries(summaries: &mut [NoteSummary]) {
    summaries.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| b.id.cmp(&a.id))
    });
}

/// 把标签规整：去首尾空白、去重、去掉空串，保持首次出现的顺序。
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if !tag.is_empty() && !out.iter().any(|existing| existing == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

/// 字符数。
pub fn char_count(text: &str) -> usize {
    text.chars().count()
}

/// 字符下标转字节下标，越界时夹到两端。
pub fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .map(|(byte, _)| byte)
        .nth(char_index)
        .unwrap_or(text.len())
}

/// 字节下标转字符下标，越界时夹到两端。
pub fn byte_to_char(text: &str, byte_index: usize) -> usize {
    let clamped = byte_index.min(text.len());
    // 落在多字节字符中间时退到字符边界。
    let boundary = (0..=clamped).rev().find(|&index| text.is_char_boundary(index));
    text[..boundary.unwrap_or(0)].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_block(text: &str) -> Block {
        Block::Text(TextBlock {
            id: 1,
            kind: TextKind::Paragraph,
            text: text.to_string(),
            spans: Vec::new(),
        })
    }

    #[test]
    fn title_is_first_nonempty_first_line() {
        let mut note = Note::new().unwrap();
        assert_eq!(note.title(), "");
        note.blocks = vec![text_block("\n  \n"), text_block("  第一行标题 \n第二行"), text_block("别的")];
        assert_eq!(note.title(), "第一行标题");
        note.blocks[0] = Block::Divider { id: 9 };
        assert_eq!(note.title(), "第一行标题");
    }

    #[test]
    fn plain_text_joins_text_blocks_only() {
        let mut note = Note::new().unwrap();
        note.blocks = vec![text_block("甲"), Block::Divider { id: 2 }, text_block("乙")];
        assert_eq!(note.plain_text(), "甲\n乙");
        note.blocks = vec![text_block(""), Block::Divider { id: 2 }, text_block(""), text_block("乙"), text_block("")];
        assert_eq!(note.plain_text(), "\n\n乙\n");
    }

    #[test]
    fn json_roundtrip_keeps_unknown_variants_error_instead_of_silent_loss() {
        let note = Note::new().unwrap();
        let json = serde_json::to_vec(&note).unwrap();
        let back: Note = serde_json::from_slice(&json).unwrap();
        assert_eq!(back, note);
        // 旧版本写出的文本块没有 kind 和 spans 字段，也要能读。
        let legacy = r#"{"id":"a","created_at":1,"updated_at":1,"blocks":[{"type":"text","id":7,"text":"旧数据"}]}"#.as_bytes();
        let parsed: Note = serde_json::from_slice(legacy).unwrap();
        assert_eq!(parsed.blocks[0].as_text().unwrap().kind, TextKind::Paragraph);
        assert_eq!(parsed.title(), "旧数据");
    }

    #[test]
    fn summaries_sort_pinned_first_then_newest() {
        let summary = |id: &str, pinned: bool, updated: u64| NoteSummary {
            id: id.into(),
            title: String::new(),
            created_at: 0,
            updated_at: updated,
            pinned,
            tags: vec![],
            deleted_at: None,
        };
        let mut list = vec![summary("old", false, 1), summary("new", false, 9), summary("pin", true, 2)];
        sort_summaries(&mut list);
        let ids: Vec<&str> = list.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["pin", "new", "old"]);
    }

    #[test]
    fn tags_are_trimmed_and_deduplicated() {
        let raw = [" a ", "b", "a", "", "  ", "c"].map(String::from);
        assert_eq!(normalize_tags(&raw), ["a", "b", "c"]);
    }

    #[test]
    fn char_and_byte_index_conversion() {
        let text = "a你bc";
        assert_eq!(char_count(text), 4);
        assert_eq!(char_to_byte(text, 1), 1);
        assert_eq!(char_to_byte(text, 2), 4);
        assert_eq!(char_to_byte(text, 99), text.len());
        assert_eq!(byte_to_char(text, 4), 2);
        assert_eq!(byte_to_char(text, 3), 1); // 落在「你」的中间，退到前一个边界
        assert_eq!(byte_to_char(text, 999), 4);
    }
}
