//! 行内样式区间的纯函数。不依赖界面库，所有下标都是字符下标。

use crate::model::{InlineStyle, StyleSpan};

/// 找出新旧文本的公共前缀和公共后缀（按字符计）。返回 (前缀长度, 后缀长度)。
pub fn diff_prefix_suffix(old: &str, new: &str) -> (usize, usize) {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let mut prefix = 0;
    while prefix < old_chars.len() && prefix < new_chars.len() && old_chars[prefix] == new_chars[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_chars.len() - prefix
        && suffix < new_chars.len() - prefix
        && old_chars[old_chars.len() - 1 - suffix] == new_chars[new_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }
    (prefix, suffix)
}

/// 某个字符位置上的样式。
pub fn style_at(spans: &[StyleSpan], position: u32) -> InlineStyle {
    spans
        .iter()
        .find(|span| span.start <= position && position < span.end)
        .map(|span| span.style)
        .unwrap_or_default()
}

/// 规范化：夹到文本长度内、丢掉空段和无样式段、按起点排序、消解重叠、合并相邻同样式段。
/// 重叠时靠前的区间优先，靠后的区间只保留没被覆盖的部分。
pub fn normalize(mut spans: Vec<StyleSpan>, text_len: u32) -> Vec<StyleSpan> {
    spans.retain(|span| !span.style.is_plain() && span.end > span.start && span.start < text_len);
    for span in &mut spans {
        span.start = span.start.min(text_len);
        span.end = span.end.min(text_len);
    }
    spans.sort_by_key(|span| (span.start, span.end));
    let mut out: Vec<StyleSpan> = Vec::new();
    for span in spans {
        let start = span.start.max(out.last().map_or(0, |last: &StyleSpan| last.end));
        if start >= span.end {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.style == span.style && last.end == start {
                last.end = span.end;
                continue;
            }
        }
        out.push(StyleSpan {
            start,
            end: span.end,
            style: span.style,
        });
    }
    out
}

/// 文本被改过之后修补样式区间。
///
/// 新文字的样式按这个顺序决定：
/// 1. 这次改动正好是在 `insert_at` 处插入文字：用 `insert_style`（光标处按了加粗再打字）。
/// 2. 这次改动替换了一段旧文字（选中后输入、或删掉一段）：继承被替换的第一個字符的样式。
/// 3. 光标处纯插入：跟随左边那个字符的样式；行首没有左邻时跟随右边的。
pub fn patch(
    spans: &[StyleSpan],
    old: &str,
    new: &str,
    insert_at: Option<u32>,
    insert_style: Option<InlineStyle>,
) -> Vec<StyleSpan> {
    let (prefix, suffix) = diff_prefix_suffix(old, new);
    let old_len = old.chars().count();
    let new_len = new.chars().count();
    let old_end = old_len - suffix;
    let new_end = new_len - suffix;
    patch_range(
        spans,
        new_len as u32,
        prefix as u32,
        old_end as u32,
        new_end as u32,
        insert_at.map(|at| (at, insert_style.unwrap_or_default())),
    )
}

/// 修补已知替换范围的样式，避免重复文字导致文本差分把改动定位到别处。
pub(crate) fn patch_range(
    spans: &[StyleSpan],
    new_len: u32,
    prefix: u32,
    old_end: u32,
    new_end: u32,
    pending: Option<(u32, InlineStyle)>,
) -> Vec<StyleSpan> {
    let delta = new_end as i64 - old_end as i64;

    let inherited = if let Some((_, style)) = pending.filter(|(at, _)| *at == prefix && old_end == prefix) {
        style
    } else if old_end > prefix {
        style_at(spans, prefix)
    } else if prefix > 0 {
        style_at(spans, prefix - 1)
    } else if new_len > 0 {
        style_at(spans, 0)
    } else {
        InlineStyle::default()
    };

    let mut out = Vec::new();
    for span in spans {
        // 改动点之前的部分原样保留。
        if span.start < prefix {
            out.push(StyleSpan {
                start: span.start,
                end: span.end.min(prefix),
                style: span.style,
            });
        }
        // 被替换区间之后的部分整体平移。
        if span.end > old_end {
            out.push(StyleSpan {
                start: (span.start.max(old_end) as i64 + delta) as u32,
                end: (span.end as i64 + delta) as u32,
                style: span.style,
            });
        }
    }
    if new_end > prefix && !inherited.is_plain() {
        out.push(StyleSpan {
            start: prefix,
            end: new_end,
            style: inherited,
        });
    }
    normalize(out, new_len)
}

/// 给 `[start, end)` 里的每个字符套用 `modify`。区间内原有的样式是修改的基础。
pub fn apply_to_range(
    spans: &[StyleSpan],
    text_len: u32,
    start: u32,
    end: u32,
    modify: impl Fn(&mut InlineStyle),
) -> Vec<StyleSpan> {
    let start = start.min(text_len);
    let end = end.min(text_len).max(start);
    let mut out = Vec::new();
    for span in spans {
        if span.start < start {
            out.push(StyleSpan {
                start: span.start,
                end: span.end.min(start),
                style: span.style,
            });
        }
        if span.end > end {
            out.push(StyleSpan {
                start: span.start.max(end),
                end: span.end,
                style: span.style,
            });
        }
    }
    let mut cursor = start;
    while cursor < end {
        // 下一段的终点：区间内下一个区间的起点，或覆盖当前位置的区间的终点。
        let mut next = end;
        for span in spans {
            if span.start > cursor && span.start < next {
                next = span.start;
            }
            if span.start <= cursor && cursor < span.end {
                next = next.min(span.end);
            }
        }
        let mut style = style_at(spans, cursor);
        modify(&mut style);
        if !style.is_plain() {
            out.push(StyleSpan {
                start: cursor,
                end: next,
                style,
            });
        }
        cursor = next;
    }
    normalize(out, text_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOLD: InlineStyle = InlineStyle {
        bold: true,
        italic: false,
        underline: false,
        strikethrough: false,
        color: None,
    };
    const ITALIC: InlineStyle = InlineStyle {
        bold: false,
        italic: true,
        underline: false,
        strikethrough: false,
        color: None,
    };

    fn span(start: u32, end: u32, style: InlineStyle) -> StyleSpan {
        StyleSpan { start, end, style }
    }

    /// 把区间表渲染成每个字符一组样式标记，方便断言。
    fn render(text: &str, spans: &[StyleSpan]) -> Vec<String> {
        text.chars()
            .enumerate()
            .map(|(index, _)| {
                let style = style_at(spans, index as u32);
                let mut marks = String::new();
                if style.bold {
                    marks.push('B');
                }
                if style.italic {
                    marks.push('I');
                }
                marks
            })
            .collect()
    }

    fn patch_marks(old: &str, new: &str, spans: &[StyleSpan]) -> Vec<String> {
        render(new, &patch(spans, old, new, None, None))
    }

    #[test]
    fn diff_finds_prefix_and_suffix() {
        assert_eq!(diff_prefix_suffix("abcd", "abcd"), (4, 0));
        assert_eq!(diff_prefix_suffix("abcd", "abXd"), (2, 1));
        assert_eq!(diff_prefix_suffix("ab", "aab"), (1, 1));
        assert_eq!(diff_prefix_suffix("abc", "abcde"), (3, 0));
        assert_eq!(diff_prefix_suffix("你好世界", "你好"), (2, 0));
        assert_eq!(diff_prefix_suffix("", "x"), (0, 0));
        assert_eq!(diff_prefix_suffix("aa", "aaaa"), (2, 0));
    }

    #[test]
    fn typing_after_styled_text_continues_its_style() {
        let spans = vec![span(0, 4, BOLD)]; // "abcd" 全粗
        assert_eq!(patch_marks("abcd", "abXcd", &spans), ["B", "B", "B", "B", "B"]);
        assert_eq!(patch_marks("abcd", "abcdX", &spans), ["B", "B", "B", "B", "B"]);
        let spans = vec![span(0, 2, BOLD)]; // "ab" 粗，"cd" 普通
        // 光标在 a 和 b 之间打字：跟随左边的 a，仍是粗体。
        assert_eq!(patch_marks("abcd", "aXbcd", &spans), ["B", "B", "B", "", ""]);
        // 光标在 b 和 c 之间打字：跟随左边的 b，新字也是粗体（和 Word 一致；想停就再按一次加粗）。
        assert_eq!(patch_marks("abcd", "abXcd", &spans), ["B", "B", "B", "", ""]);
        // 行首打字：左边没有字符，跟随右边的 a，结果是普通。
        let spans = vec![span(1, 2, BOLD)];
        assert_eq!(patch_marks("abcd", "Xabcd", &spans), ["", "", "B", "", ""]);
    }

    #[test]
    fn pending_style_overrides_inheritance() {
        let spans = vec![span(0, 2, BOLD)];
        // 光标在粗体末尾，但用户刚按了斜体再打字：新字应当是斜体而不是粗体。
        let result = patch(&spans, "abcd", "abXcd", Some(2), Some(ITALIC));
        assert_eq!(render("abXcd", &result), ["B", "B", "I", "", ""]);
        // 插入位置对不上时不生效。
        let result = patch(&spans, "abcd", "aXbcd", Some(2), Some(ITALIC));
        assert_eq!(render("aXbcd", &result), ["B", "B", "B", "", ""]);
    }

    #[test]
    fn deletion_and_replacement_adjust_spans() {
        let spans = vec![span(2, 4, BOLD)]; // "abcd" 里 cd 粗
        assert_eq!(patch_marks("abcd", "ab", &spans), ["", ""]); // 删掉末尾的粗体
        assert_eq!(patch_marks("abcd", "abcdX", &spans), ["", "", "B", "B", "B"]);
        let spans = vec![span(1, 4, BOLD)];
        assert_eq!(patch_marks("abcd", "ad", &spans), ["", "B"]); // 删掉中间，剩下的 d 仍粗
        assert_eq!(patch_marks("abcd", "aXYd", &spans), ["", "B", "B", "B"]); // 替换中间，新字继承被替换内容的样式
        let spans = vec![span(0, 2, BOLD), span(2, 4, ITALIC)];
        assert_eq!(patch_marks("aXcd", "acd", &spans), ["B", "I", "I"]); // 删掉中间的 X，后面的斜体整体左移
        assert_eq!(patch_marks("abcd", "ac", &spans), ["B", "B"]); // bcd 被替换成一个 c：新字继承被替换内容起点的粗体，斜体段整段没了
    }

    #[test]
    fn chinese_and_emoji_are_counted_per_character() {
        let spans = vec![span(0, 3, BOLD)]; // "你好😀" 粗
        assert_eq!(patch_marks("你好😀世界", "你好😀X世界", &spans), ["B", "B", "B", "B", "", ""]);
        assert_eq!(patch_marks("你好😀世界", "你好世界", &spans), ["B", "B", "", ""]);
    }

    #[test]
    fn normalize_cleans_up() {
        let messy = vec![
            span(5, 9, BOLD),   // 超出长度
            span(3, 3, BOLD),   // 空
            span(0, 2, BOLD),
            span(2, 4, BOLD),   // 与前一个相邻同样式，应合并
            span(6, 8, ITALIC), // 完全越界
            span(1, 2, InlineStyle::default()), // 无样式
        ];
        assert_eq!(normalize(messy, 5), vec![span(0, 4, BOLD)]);
        // 后一个区间只拿到前一个没覆盖的部分。
        assert_eq!(
            normalize(vec![span(0, 3, BOLD), span(2, 5, ITALIC)], 9),
            vec![span(0, 3, BOLD), span(3, 5, ITALIC)]
        );
    }

    #[test]
    fn apply_to_range_splits_and_joins() {
        let spans = vec![span(2, 4, BOLD)];
        // 把 [1,5) 全设成粗体：应与原有粗体连成一段。
        assert_eq!(apply_to_range(&spans, 6, 1, 5, |s| s.bold = true), vec![span(1, 5, BOLD)]);
        // 把 [3,5) 的粗体去掉：只剩 [2,3)。
        assert_eq!(
            apply_to_range(&spans, 6, 3, 5, |s| s.bold = false),
            vec![span(2, 3, BOLD)]
        );
        // 在粗体上叠加斜体：粗体段被拆成 [2,3) 粗 和 [3,4) 粗斜，后面 [4,5) 斜。
        let mixed = apply_to_range(&spans, 6, 3, 5, |s| s.italic = true);
        assert_eq!(render("abcdef", &mixed), ["", "", "B", "BI", "I", ""]);
        // 越界的区间被夹住，空区间什么都不做。
        assert_eq!(apply_to_range(&spans, 6, 10, 20, |s| s.bold = true), spans);
    }
}
