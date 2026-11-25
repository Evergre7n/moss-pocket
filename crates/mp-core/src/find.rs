//! 在一条笔记里查找，以及生成替换用的清单。只做纯计算；怎么显示、怎么交互由界面层决定。

use crate::model::{Block, BlockId, Note};
use std::collections::HashMap;

/// 一处命中：某个文本块里从 `start` 到 `end` 的字符（不含 `end`）。位置按字符数计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub block: BlockId,
    pub start: u32,
    pub end: u32,
}

/// 一次替换：把某个文本块里从 `start` 到 `end` 的字符换成 `replacement`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub block: BlockId,
    pub start: u32,
    pub end: u32,
    pub replacement: String,
}

/// 大小写折叠：只做一对一的转换。一个字符会变成多个字符的（比如 ß）保持原样，保证下标一一对应。
fn fold(ch: char) -> char {
    let mut lower = ch.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => one,
        _ => ch,
    }
}

/// 查找内容里有没有大小写之分。纯中文、数字、符号没有，可以直接按原文找。
fn has_case(query: &str) -> bool {
    query.chars().any(|ch| ch.is_lowercase() || ch.is_uppercase())
}

/// 列出 `text` 里所有命中的字符范围，互不重叠。`match_case` 为假时忽略大小写。
pub fn find_in(text: &str, query: &str, match_case: bool) -> Vec<(u32, u32)> {
    if query.is_empty() || text.is_empty() {
        return Vec::new();
    }
    if match_case || !has_case(query) {
        // 区分大小写，或者查找内容根本没有大小写：按原文找，再把字节位置换成字符位置。
        let length = query.chars().count();
        let starts: Vec<usize> = text.match_indices(query).map(|(byte, _)| byte).collect();
        let mut hits = Vec::with_capacity(starts.len());
        let (mut index, mut next) = (0u32, 0usize);
        for (byte, _) in text.char_indices() {
            while starts.get(next).is_some_and(|start| *start < byte) {
                next += 1;
            }
            if starts.get(next) == Some(&byte) {
                hits.push((index, index + length as u32));
            }
            index += 1;
        }
        return hits;
    }
    let haystack: Vec<char> = text.chars().collect();
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut hits = Vec::new();
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        if haystack[start..start + needle.len()]
            .iter()
            .zip(&needle)
            .all(|(a, b)| fold(*a) == *b)
        {
            hits.push((start as u32, (start + needle.len()) as u32));
            start += needle.len();
        } else {
            start += 1;
        }
    }
    hits
}

/// 整条笔记里的命中处，按正文顺序排好。
pub fn find_in_note(note: &Note, query: &str, match_case: bool) -> Vec<Hit> {
    let mut hits = Vec::new();
    for block in &note.blocks {
        if let Block::Text(text) = block {
            hits.extend(
                find_in(&text.text, query, match_case)
                    .into_iter()
                    .map(|(start, end)| Hit { block: text.id, start, end }),
            );
        }
    }
    hits
}

/// 把所有命中处换成 `replacement` 的替换清单。
pub fn replace_all(hits: &[Hit], replacement: &str) -> Vec<Edit> {
    hits.iter()
        .map(|hit| Edit {
            block: hit.block,
            start: hit.start,
            end: hit.end,
            replacement: replacement.to_string(),
        })
        .collect()
}

/// 块在笔记里的序号，给「谁在前谁在后」用。
fn block_order(note: &Note) -> HashMap<BlockId, usize> {
    note.blocks.iter().enumerate().map(|(index, block)| (block.id(), index)).collect()
}

/// 从 `block` 的第 `position` 个字符往后，第一处命中的序号。命中处只要还没结束（`end > position`）就算数，
/// 所以光标停在命中处里面时选中的就是它。后面没有了就绕回第一处。没有命中时返回 None。
pub fn hit_from(note: &Note, hits: &[Hit], block: Option<BlockId>, position: u32) -> Option<usize> {
    hits.first()?;
    let Some(block) = block else {
        return Some(0);
    };
    let order = block_order(note);
    let from = order.get(&block).copied().unwrap_or(usize::MAX);
    Some(
        hits.iter()
            .position(|hit| {
                let index = order.get(&hit.block).copied().unwrap_or(usize::MAX);
                index > from || (index == from && hit.end > position)
            })
            .unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_characters_not_bytes() {
        assert_eq!(find_in("中文abc中文abc", "abc", false), vec![(2, 5), (7, 10)]);
        assert_eq!(find_in("记一个想法", "想法", false), vec![(3, 5)]);
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find_in("aaaa", "aa", true), vec![(0, 2), (2, 4)]);
        assert_eq!(find_in("aAaA", "aa", false), vec![(0, 2), (2, 4)]);
        assert_eq!(find_in("猫AaA猫Aa", "aa", false), vec![(1, 3), (5, 7)]);
    }

    #[test]
    fn case_is_ignored_unless_asked() {
        assert_eq!(find_in("Hello hello HELLO", "hello", false).len(), 3);
        assert_eq!(find_in("Hello hello HELLO", "hello", true), vec![(6, 11)]);
        // 没有大小写之分的查找内容，两种设置结果一样。
        assert_eq!(find_in("中文中文", "中文", false), find_in("中文中文", "中文", true));
    }

    #[test]
    fn nothing_matches_gives_nothing() {
        assert!(find_in("abc", "", false).is_empty());
        assert!(find_in("", "a", false).is_empty());
        assert!(find_in("短", "比它长得多", false).is_empty());
        assert!(find_in("abc", "z", false).is_empty());
    }

    #[test]
    fn note_hits_follow_text_order_across_blocks() {
        let mut note = Note::new().unwrap();
        note.blocks[0].as_text_mut().unwrap().text = "甲乙".into();
        let first = note.blocks[0].id();
        // 非文字块跳过，不打乱顺序。
        note.blocks.push(Block::Divider { id: first.wrapping_add(1) });
        let mut second = crate::model::TextBlock::new_empty().unwrap();
        second.text = "乙甲".into();
        let second_id = second.id;
        note.blocks.push(Block::Text(second));
        assert_eq!(
            find_in_note(&note, "甲", false),
            vec![Hit { block: first, start: 0, end: 1 }, Hit { block: second_id, start: 1, end: 2 }]
        );
    }

    #[test]
    fn replace_list_covers_every_hit() {
        let edits = replace_all(&[Hit { block: 3, start: 1, end: 4 }, Hit { block: 9, start: 0, end: 2 }], "x");
        assert_eq!(edits.len(), 2);
        assert_eq!((edits[0].block, edits[0].start, edits[0].end), (3, 1, 4));
        assert_eq!(edits[1].replacement, "x");
    }

    #[test]
    fn hit_from_starts_at_the_caret_and_wraps_around() {
        let mut note = Note::new().unwrap();
        note.blocks[0].as_text_mut().unwrap().text = "乙甲甲".into();
        let first = note.blocks[0].id();
        let mut second = crate::model::TextBlock::new_empty().unwrap();
        second.text = "甲甲".into();
        note.blocks.push(Block::Text(second));
        let hits = find_in_note(&note, "甲甲", false);
        assert_eq!(hits.len(), 2);
        // 光标在第一块开头：选第一处。
        assert_eq!(hit_from(&note, &hits, Some(first), 0), Some(0));
        // 光标停在命中处里面：还是这一处。
        assert_eq!(hit_from(&note, &hits, Some(first), 2), Some(0));
        // 刚过第一处：到第二块去找。
        assert_eq!(hit_from(&note, &hits, Some(first), 3), Some(1));
        // 最后一块的最后：绕回第一处。
        assert_eq!(hit_from(&note, &hits, Some(hits[1].block), 2), Some(0));
        // 不知道光标在哪：第一处。
        assert_eq!(hit_from(&note, &hits, None, 0), Some(0));
        assert_eq!(hit_from(&note, &[], Some(first), 0), None);
    }
}
