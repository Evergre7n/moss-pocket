//! 把一条笔记导出成 Markdown、HTML、Word 文档、PDF。
//!
//! 四种格式共用同一个中间模型：先把笔记里的块和媒体整理好，再各自排版。
//! 媒体文件在这里才解密，解密后的内容只写到用户选定的导出位置。

use crate::i18n::{self, Tr};
use crate::media::decode_preview;
use mp_core::model::{Block, InlineStyle, Note, StyleSpan, TextKind, TitleSource};
use mp_core::vault::Vault;
use std::path::{Path, PathBuf};

/// 导出用的中间模型：一条笔记被整理成一串块。
#[derive(Debug, Clone, PartialEq)]
pub enum ExportBlock {
    Text {
        kind: TextKind,
        /// 是不是单独排版的标题块。
        title: bool,
        /// 按样式切好的一段段文字。
        runs: Vec<(InlineStyle, String)>,
    },
    Image {
        data: Vec<u8>,
        mime: &'static str,
        /// 扩展名，不带点。
        extension: &'static str,
        width: usize,
        height: usize,
    },
    Audio { file_name: String, data: Vec<u8> },
    Video { file_name: String, data: Vec<u8> },
    Divider,
}

/// 导出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
    Docx,
    Pdf,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Docx => "docx",
            Format::Pdf => "pdf",
        }
    }
}

/// 把文本块按样式区间切成 (样式, 文字) 的片段，相邻同样式的已合并。
pub fn split_runs(text: &str, spans: &[StyleSpan]) -> Vec<(InlineStyle, String)> {
    let style_at = |position: usize| {
        spans
            .iter()
            .find(|span| span.start as usize <= position && position < span.end as usize)
            .map(|span| span.style)
            .unwrap_or_default()
    };
    let mut runs: Vec<(InlineStyle, String)> = Vec::new();
    for (index, ch) in text.chars().enumerate() {
        let style = style_at(index);
        match runs.last_mut() {
            Some((last_style, buffer)) if *last_style == style => buffer.push(ch),
            _ => runs.push((style, ch.to_string())),
        }
    }
    runs
}

/// 看文件头判断图片格式：返回 (MIME 类型, 扩展名)。
pub fn sniff_image(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(("image/png", "png"))
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpeg"))
    } else if data.starts_with(b"GIF8") {
        Some(("image/gif", "gif"))
    } else if data.starts_with(b"RIFF") && data.len() > 12 && &data[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else if data.starts_with(b"BM") {
        Some(("image/bmp", "bmp"))
    } else {
        None
    }
}

/// 整理一条笔记，媒体在这里解密。任何一步读不出来就整体报错，不导出半份。
/// 失败原因用界面语言 `tr` 写。
pub fn collect(note: &Note, vault: &Vault, tr: Tr) -> Result<Vec<ExportBlock>, String> {
    let title = if note.title_source == TitleSource::Legacy {
        note.blocks.iter().filter_map(Block::as_text).find(|text| !text.text.trim().is_empty())
    } else {
        note.title_block().filter(|text| !text.text.trim().is_empty())
    };
    let read = |blob_id: &str| vault.read_blob(blob_id).map_err(|error| tr.core_error(&error));
    note.blocks
        .iter()
        .map(|block| {
            Ok(match block {
                Block::Text(text) => ExportBlock::Text {
                    kind: text.kind,
                    title: title.is_some_and(|title| title.id == text.id),
                    runs: split_runs(&text.text, &text.spans),
                },
                Block::Image(image) => {
                    let data = read(&image.blob_id)?;
                    let (mime, extension) = sniff_image(&data).ok_or_else(|| "有一张图片的格式认不出来".to_string())?;
                    ExportBlock::Image {
                        data,
                        mime,
                        extension,
                        width: image.width as usize,
                        height: image.height as usize,
                    }
                }
                Block::Audio(media) => ExportBlock::Audio { file_name: media.file_name.clone(), data: read(&media.blob_id)? },
                Block::Video(media) => ExportBlock::Video { file_name: media.file_name.clone(), data: read(&media.blob_id)? },
                Block::Divider { .. } => ExportBlock::Divider,
            })
        })
        .collect()
}

/// 把标题变成能当文件名的样子：去掉路径里不允许的字符，限制长度。
pub fn safe_file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|ch| {
            if ch.is_control() || r#"\/:*?"<>|"#.contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    let shortened: String = cleaned.chars().take(60).collect::<String>().trim_end_matches([' ', '.']).to_string();
    if shortened.is_empty() {
        "未命名".to_string()
    } else if crate::file_picker::is_windows_device_name(&shortened) {
        format!("_{shortened}")
    } else {
        shortened
    }
}

/// 媒体文件夹里已经有的文件名不能重复；重名时前面加序号。
fn unique_media_name(existing: &[(String, &[u8])], wanted: &str) -> String {
    if !existing.iter().any(|(name, _)| name.eq_ignore_ascii_case(wanted)) {
        return wanted.to_string();
    }
    let (stem, extension) = wanted.rsplit_once('.').map(|(stem, ext)| (stem.to_string(), format!(".{ext}"))).unwrap_or_else(|| (wanted.to_string(), String::new()));
    for index in 2.. {
        let candidate = format!("{stem}-{index}{extension}");
        if !existing.iter().any(|(name, _)| name.eq_ignore_ascii_case(&candidate)) {
            return candidate;
        }
    }
    unreachable!("序号总有一个没用过")
}

/// 一段文字的行数。
fn line_prefix(text: &str, prefix: &str) -> String {
    if text.is_empty() {
        prefix.trim_end().to_string()
    } else {
        text.lines().map(|line| format!("{prefix}{line}")).collect::<Vec<_>>().join("\n")
    }
}

/// 导出成 Markdown：一个 `.md` 文件加一个同名的媒体文件夹。
fn export_markdown_to(blocks: &[ExportBlock], target: &Path) -> Result<PathBuf, String> {
    let folder = target.parent().unwrap_or_else(|| Path::new("."));
    let stem = target.file_stem().map(|value| value.to_string_lossy().into_owned()).unwrap_or_else(|| "note".into());
    let media_dir = folder.join(format!("{stem}_files"));
    let mut media: Vec<(String, &[u8])> = Vec::new();
    let mut markdown = String::new();
    let mut number = 0usize;
    let mut previous_was_numbered = false;

    for block in blocks {
        match block {
            ExportBlock::Text { kind, title, runs } => {
                let mut text = String::new();
                for (style, piece) in runs {
                    text.push_str(&inline_markdown(*style, piece));
                }
                if *title {
                    markdown.push_str("# ");
                    markdown.push_str(&text);
                    markdown.push_str("\n\n");
                    continue;
                }
                let line = match kind {
                    TextKind::Heading1 => format!("## {text}"),
                    TextKind::Heading2 => format!("### {text}"),
                    TextKind::Heading3 => format!("#### {text}"),
                    TextKind::Quote => line_prefix(&text, "> "),
                    TextKind::Bullet => line_prefix(&text, "- "),
                    TextKind::Numbered => {
                        number = if previous_was_numbered { number + 1 } else { 1 };
                        format!("{number}. {text}")
                    }
                    TextKind::Paragraph => text,
                };
                previous_was_numbered = *kind == TextKind::Numbered;
                markdown.push_str(&line);
                markdown.push_str("\n\n");
            }
            ExportBlock::Image { data, extension, .. } => {
                previous_was_numbered = false;
                let name = unique_media_name(&media, &format!("image.{extension}"));
                markdown.push_str(&format!("![图片]({})\n\n", relative_url(&format!("{stem}_files/{name}"))));
                media.push((name, data.as_slice()));
            }
            ExportBlock::Audio { file_name, data } | ExportBlock::Video { file_name, data } => {
                previous_was_numbered = false;
                let name = unique_media_name(&media, &safe_file_name(file_name));
                markdown.push_str(&format!("{file_name}\n\n[{stem}_files/{name}]({})\n\n", relative_url(&format!("{stem}_files/{name}"))));
                media.push((name, data.as_slice()));
            }
            ExportBlock::Divider => {
                previous_was_numbered = false;
                markdown.push_str("---\n\n");
            }
        }
    }

    // 先确认目录能建，再写媒体，最后写正文：中途出错时正文不会先落一份半的。
    if !media.is_empty() {
        std::fs::create_dir_all(&media_dir).map_err(|error| error.to_string())?;
    }
    for (name, bytes) in &media {
        std::fs::write(media_dir.join(name), bytes).map_err(|error| error.to_string())?;
    }
    std::fs::write(target, markdown).map_err(|error| error.to_string())?;
    Ok(target.to_path_buf())
}

fn inline_markdown(style: InlineStyle, text: &str) -> String {
    let mut out = text.to_string();
    if style.underline {
        out = format!("<u>{out}</u>");
    }
    if style.strikethrough {
        out = format!("~~{out}~~");
    }
    if style.italic {
        out = format!("*{out}*");
    }
    if style.bold {
        out = format!("**{out}**");
    }
    if let Some(color) = style.color {
        out = format!("<span style=\"color:#{color:06x}\">{out}</span>");
    }
    out
}

/// 音视频不超过这个大小的直接嵌进网页；更大的放在同名文件夹里用相对路径引用。
const INLINE_MEDIA_LIMIT: usize = 8 * 1000 * 1000;

fn xml_escape(text: &str) -> String {
    text.chars().filter(|&ch| matches!(ch, '\t' | '\n' | '\r') || ch >= ' ')
        .collect::<String>().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn relative_url(path: &str) -> String {
    path.chars().map(|ch| {
        if ch.is_control() || matches!(ch, ' ' | '"' | '\'' | '#' | '%' | '?' | '(' | ')' | '<' | '>') {
            let mut buffer = [0u8; 4];
            ch.encode_utf8(&mut buffer).as_bytes().iter().map(|byte| format!("%{byte:02X}")).collect::<String>()
        } else {
            ch.to_string()
        }
    }).collect()
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let packed = ((chunk[0] as u32) << 16) | ((*chunk.get(1).unwrap_or(&0) as u32) << 8) | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(packed >> 18) as usize & 63] as char);
        out.push(ALPHABET[(packed >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(packed >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[packed as usize & 63] as char } else { '=' });
    }
    out
}

fn inline_html(style: InlineStyle, text: &str) -> String {
    let mut out = xml_escape(text);
    if style.underline {
        out = format!("<u>{out}</u>");
    }
    if style.strikethrough {
        out = format!("<s>{out}</s>");
    }
    if style.italic {
        out = format!("<em>{out}</em>");
    }
    if style.bold {
        out = format!("<strong>{out}</strong>");
    }
    if let Some(color) = style.color {
        out = format!("<span style=\"color:#{color:06x}\">{out}</span>");
    }
    out
}

fn html_runs(runs: &[(InlineStyle, String)]) -> String {
    let mut out = String::new();
    for (style, text) in runs {
        // 段内换行显示成硬换行。
        out.push_str(&inline_html(*style, text).replace('\n', "<br>"));
    }
    out
}

/// 图片嵌进网页时用缩小后的预览图：单个文件不至于太大，打印出来也够清楚。
fn image_data_uri(data: &[u8], mime: &str) -> Result<String, String> {
    let embedded = if mime == "image/png" && data.len() <= INLINE_MEDIA_LIMIT {
        (mime.to_string(), data.to_vec())
    } else {
        let (_, preview) = decode_preview(data)?;
        ("image/png".to_string(), crate::media::encode_png(&preview)?)
    };
    Ok(format!("data:{};base64,{}", embedded.0, base64(&embedded.1)))
}

const HTML_STYLE: &str = "body{margin:0;background:#f4f4f5;color:#18181b;font-family:'Segoe UI','Microsoft YaHei',system-ui,sans-serif;line-height:1.7}\
article{max-width:760px;margin:0 auto;padding:40px 24px;background:#fff}h1{font-size:2em;margin:0 0 .6em}h2{font-size:1.5em}h3{font-size:1.25em}\
blockquote{margin:1em 0;padding:.2em 1em;border-left:4px solid #d4d4d8;color:#52525b}figure{margin:1em 0}img{max-width:100%;height:auto}\
hr{border:0;border-top:1px solid #d4d4d8;margin:1.5em 0}audio,video{max-width:100%}\
@media print{body{background:#fff}article{max-width:none;padding:0}figure,audio,video{break-inside:avoid}}";

/// 导出成网页文件。图片嵌在文件里；音视频不超过 8 MB 的也嵌进去，更大的放在同名文件夹里。
pub fn export_html(blocks: &[ExportBlock], target: &Path, title: &str) -> Result<(), String> {
    let stem = target.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_else(|| "note".to_string());
    let mut media: Vec<(String, &[u8], &'static str)> = Vec::new();
    let mut body = String::new();
    let mut number = 0usize;
    let mut previous_was_numbered = false;

    for block in blocks {
        match block {
            ExportBlock::Text { kind, title, runs } => {
                let inner = html_runs(runs);
                if *title {
                    body.push_str(&format!("<h1>{inner}</h1>"));
                    continue;
                }
                let html = match kind {
                    TextKind::Heading1 => format!("<h2>{inner}</h2>"),
                    TextKind::Heading2 => format!("<h3>{inner}</h3>"),
                    TextKind::Heading3 => format!("<h4>{inner}</h4>"),
                    TextKind::Quote => format!("<blockquote>{inner}</blockquote>"),
                    TextKind::Bullet => format!("<ul><li>{inner}</li></ul>"),
                    TextKind::Numbered => {
                        number = if previous_was_numbered { number + 1 } else { 1 };
                        format!("<ol start=\"{number}\"><li>{inner}</li></ol>")
                    }
                    TextKind::Paragraph => format!("<p>{inner}</p>"),
                };
                previous_was_numbered = *kind == TextKind::Numbered;
                body.push_str(&html);
            }
            ExportBlock::Image { data, mime, width, height, .. } => {
                previous_was_numbered = false;
                let source = image_data_uri(data, mime)?;
                body.push_str(&format!(
                    "<figure><img src=\"{source}\" width=\"{width}\" height=\"{height}\" alt=\"image\"></figure>"
                ));
            }
            ExportBlock::Audio { file_name, data } | ExportBlock::Video { file_name, data } => {
                previous_was_numbered = false;
                let tag = if matches!(block, ExportBlock::Audio { .. }) { "audio" } else { "video" };
                let mime = mp_core::media::classify(file_name).map(|kind| kind.mime(file_name)).unwrap_or("application/octet-stream");
                let label = xml_escape(file_name);
                let source = if data.len() <= INLINE_MEDIA_LIMIT {
                    format!("data:{mime};base64,{}", base64(data))
                } else {
                    let name = unique_media_name(&media.iter().map(|(name, bytes, _)| (name.clone(), *bytes)).collect::<Vec<_>>(), &safe_file_name(file_name));
                    media.push((name.clone(), data.as_slice(), mime));
                    relative_url(&format!("{stem}_files/{name}"))
                };
                body.push_str(&format!("<p><{tag} controls preload=\"none\" src=\"{source}\"></{tag}><br>{label}</p>"));
            }
            ExportBlock::Divider => {
                previous_was_numbered = false;
                body.push_str("<hr>");
            }
        }
    }

    let folder = target.parent().unwrap_or_else(|| Path::new("."));
    if !media.is_empty() {
        std::fs::create_dir_all(folder.join(format!("{stem}_files"))).map_err(|error| error.to_string())?;
    }
    let document = format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>{HTML_STYLE}</style>\n</head>\n<body>\n<article>\n{body}\n</article>\n</body>\n</html>\n",
        xml_escape(title)
    );
    for (name, bytes, _) in &media {
        std::fs::write(folder.join(format!("{stem}_files")).join(name), bytes).map_err(|error| error.to_string())?;
    }
    std::fs::write(target, document).map_err(|error| error.to_string())
}

/// 一次导出。`target` 是要写出的文件路径；Markdown 写的是它所在的文件夹。
/// `tr` 只在 PDF 导出失败时用来写原因，其余格式的失败原因是系统给的。
pub fn export(format: Format, blocks: &[ExportBlock], target: &Path, title: &str, tr: Tr) -> Result<(), String> {
    match format {
        Format::Markdown => export_markdown_to(blocks, target).map(|_| ()),
        Format::Html => export_html(blocks, target, title),
        Format::Docx => export_docx(blocks, target, title),
        Format::Pdf => export_pdf(blocks, target, title, tr),
    }
}

/// 极简 ZIP 写入器：不压缩，只按标准格式打包。Word 文档本质上就是一个 ZIP。
#[derive(Default)]
struct ZipWriter {
    data: Vec<u8>,
    /// (文件名, 校验值, 大小, 在包里的起始偏移)
    entries: Vec<(String, u32, u32, u32)>,
}

impl ZipWriter {
    fn add(&mut self, name: &str, bytes: &[u8]) {
        let crc = crc32(bytes);
        let offset = self.data.len() as u32;
        let size = bytes.len() as u32;
        let mut header = Vec::new();
        header.extend_from_slice(b"PK\x03\x04");
        header.extend_from_slice(&20u16.to_le_bytes());
        // 置 UTF-8 位：文件名按 UTF-8 存。
        header.extend_from_slice(&0x0800u16.to_le_bytes());
        // 压缩方式 0（不压缩），时间 0，日期记成 1980-01-01。
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        header.extend_from_slice(&0x0021u16.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes());
        self.data.extend(header);
        self.data.extend_from_slice(name.as_bytes());
        self.data.extend_from_slice(bytes);
        self.entries.push((name.to_string(), crc, size, offset));
    }

    fn finish(mut self) -> Vec<u8> {
        let directory_start = self.data.len() as u32;
        for (name, crc, size, offset) in &self.entries {
            let mut entry = Vec::new();
            entry.extend_from_slice(b"PK\x01\x02");
            for value in [20u16, 20, 0x0800, 0, 0, 0x0021] {
                entry.extend_from_slice(&value.to_le_bytes());
            }
            entry.extend_from_slice(&crc.to_le_bytes());
            entry.extend_from_slice(&size.to_le_bytes());
            entry.extend_from_slice(&size.to_le_bytes());
            entry.extend_from_slice(&(name.len() as u16).to_le_bytes());
            for value in [0u16, 0, 0, 0] {
                entry.extend_from_slice(&value.to_le_bytes());
            }
            entry.extend_from_slice(&0u32.to_le_bytes());
            entry.extend_from_slice(&offset.to_le_bytes());
            self.data.extend(entry);
            self.data.extend_from_slice(name.as_bytes());
        }
        let directory_size = self.data.len() as u32 - directory_start;
        let count = self.entries.len() as u16;
        self.data.extend_from_slice(b"PK\x05\x06");
        for value in [0u16, 0, count, count] {
            self.data.extend_from_slice(&value.to_le_bytes());
        }
        self.data.extend_from_slice(&directory_size.to_le_bytes());
        self.data.extend_from_slice(&directory_start.to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// 一页正文宽度内的最大图片宽度（按每英寸 96 像素算）。A4 减去页边距约 6.5 英寸。
const DOCX_MAX_IMAGE_PX: f64 = 624.0;

const DOCX_STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Segoe UI\" w:hAnsi=\"Segoe UI\" w:eastAsia=\"Microsoft YaHei\"/>\
<w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/></w:rPr></w:rPrDefault>\
<w:pPrDefault><w:pPr><w:spacing w:after=\"140\" w:line=\"330\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Title\"><w:name w:val=\"Title\"/><w:basedOn w:val=\"Normal\"/>\
<w:rPr><w:b/><w:sz w:val=\"48\"/><w:szCs w:val=\"48\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"34\"/><w:szCs w:val=\"34\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"1\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"28\"/><w:szCs w:val=\"28\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"2\"/></w:pPr><w:rPr><w:b/><w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:ind w:left=\"567\"/><w:spacing w:before=\"140\" w:after=\"140\"/></w:pPr>\
<w:rPr><w:i/><w:color w:val=\"52525B\"/></w:rPr></w:style>\
</w:styles>";

fn docx_runs(runs: &[(InlineStyle, String)]) -> String {
    let mut out = String::new();
    for (style, text) in runs {
        let mut properties = String::new();
        if style.bold {
            properties.push_str("<w:b/>");
        }
        if style.italic {
            properties.push_str("<w:i/>");
        }
        if style.underline {
            properties.push_str("<w:u w:val=\"single\"/>");
        }
        if style.strikethrough {
            properties.push_str("<w:strike/>");
        }
        if let Some(color) = style.color {
            properties.push_str(&format!("<w:color w:val=\"{color:06X}\"/>"));
        }
        let body = xml_escape(text).replace('\n', "</w:t><w:br/><w:t xml:space=\"preserve\">");
        out.push_str(&format!("<w:r><w:rPr>{properties}</w:rPr><w:t xml:space=\"preserve\">{body}</w:t></w:r>"));
    }
    out
}

fn docx_paragraph(properties: &str, runs: &[(InlineStyle, String)]) -> String {
    format!("<w:p><w:pPr>{properties}</w:pPr>{}</w:p>", docx_runs(runs))
}

/// 在文字前面加一个记号（项目符号、编号）。记号不带正文的样式。
fn with_marker(marker: &str, runs: &[(InlineStyle, String)]) -> Vec<(InlineStyle, String)> {
    let mut out = vec![(InlineStyle::default(), marker.to_string())];
    out.extend(runs.iter().cloned());
    out
}

fn docx_caption(text: &str) -> String {
    format!(
        "<w:p><w:pPr><w:ind w:left=\"420\"/></w:pPr><w:r><w:rPr><w:i/><w:color w:val=\"71717A\"/></w:rPr>\
<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        xml_escape(text)
    )
}

/// 导出成 Word 和 WPS 都能直接打开的文档。图片内嵌；音视频留一行说明。
pub fn export_docx(blocks: &[ExportBlock], target: &Path, _title: &str) -> Result<(), String> {
    let mut zip = ZipWriter::default();
    let mut body = String::new();
    let mut relationships = String::from(
        "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    );
    let mut next_relationship = 2usize;
    let mut image_index = 0usize;
    let mut has_jpeg = false;
    let mut number = 0usize;
    let mut previous_was_numbered = false;

    for block in blocks {
        match block {
            ExportBlock::Text { kind, title, runs } => {
                let list_number = if previous_was_numbered { number + 1 } else { 1 };
                previous_was_numbered = *kind == TextKind::Numbered;
                number = list_number;
                let paragraph = if *title {
                    docx_paragraph("<w:pStyle w:val=\"Title\"/>", runs)
                } else {
                    match kind {
                        TextKind::Heading1 => docx_paragraph("<w:pStyle w:val=\"Heading1\"/>", runs),
                        TextKind::Heading2 => docx_paragraph("<w:pStyle w:val=\"Heading2\"/>", runs),
                        TextKind::Heading3 => docx_paragraph("<w:pStyle w:val=\"Heading3\"/>", runs),
                        TextKind::Quote => docx_paragraph("<w:pStyle w:val=\"Quote\"/>", runs),
                        TextKind::Bullet => docx_paragraph("<w:ind w:left=\"420\"/>", &with_marker("• ", runs)),
                        TextKind::Numbered => docx_paragraph("<w:ind w:left=\"420\"/>", &with_marker(&format!("{list_number}. "), runs)),
                        TextKind::Paragraph => docx_paragraph("", runs),
                    }
                };
                body.push_str(&paragraph);
            }
            ExportBlock::Image { data, extension, width, height, .. } => {
                previous_was_numbered = false;
                // Word 只认 PNG 和 JPEG，其他格式转成 PNG。
                let (bytes, file_extension) = match *extension {
                    "png" => (data.clone(), "png"),
                    "jpeg" => {
                        has_jpeg = true;
                        (data.clone(), "jpeg")
                    }
                    _ => {
                        let (_, preview) = decode_preview(data)?;
                        (crate::media::encode_png(&preview)?, "png")
                    }
                };
                image_index += 1;
                let relationship = format!("rId{next_relationship}");
                next_relationship += 1;
                let name = format!("image{image_index}.{file_extension}");
                relationships.push_str(&format!(
                    "<Relationship Id=\"{relationship}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/{name}\"/>"
                ));
                zip.add(&format!("word/media/{name}"), &bytes);
                // 像素换算成 Word 用的长度单位：每像素 9525。
                let scale = (DOCX_MAX_IMAGE_PX / *width as f64).min(1.0);
                let (extent_x, extent_y) =
                    ((*width as f64 * scale * 9525.0) as u64, (*height as f64 * scale * 9525.0) as u64);
                body.push_str(&format!(
                    "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
<wp:extent cx=\"{extent_x}\" cy=\"{extent_y}\"/><wp:docPr id=\"{image_index}\" name=\"{name}\"/>\
<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic>\
<pic:nvPicPr><pic:cNvPr id=\"{image_index}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"{relationship}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{extent_x}\" cy=\"{extent_y}\"/></a:xfrm>\
<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
                ));
            }
            ExportBlock::Audio { file_name, .. } => {
                previous_was_numbered = false;
                body.push_str(&docx_caption(&format!("音频：{file_name}")));
            }
            ExportBlock::Video { file_name, .. } => {
                previous_was_numbered = false;
                body.push_str(&docx_caption(&format!("视频：{file_name}")));
            }
            ExportBlock::Divider => {
                previous_was_numbered = false;
                body.push_str(
                    "<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"A1A1AA\"/></w:pBdr></w:pPr></w:p>",
                );
            }
        }
    }

    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
<w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
<w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/>\
</w:sectPr></w:body></w:document>"
    );
    let jpeg_default = if has_jpeg { "<Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>" } else { "" };
    let content_types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/>{jpeg_default}\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
</Types>"
    );
    zip.add("[Content_Types].xml", content_types.as_bytes());
    zip.add(
        "_rels/.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
    );
    zip.add("word/document.xml", document.as_bytes());
    zip.add(
        "word/_rels/document.xml.rels",
        format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>").as_bytes(),
    );
    zip.add("word/styles.xml", DOCX_STYLES.as_bytes());
    std::fs::write(target, zip.finish()).map_err(|error| error.to_string())
}

/// 系统自带的 Edge 在哪儿。Windows 10 和 11 都预装了它。
fn find_edge() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
        PathBuf::from(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"Microsoft\Edge\Application\msedge.exe"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// 把本地文件路径写成浏览器能打开的网址，非 ASCII 的字符按字节转义。
fn file_url(path: &Path) -> Result<String, String> {
    let full = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let text = full.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).replace('\\', "/");
    let encoded: String = text
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"/:-._~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    Ok(format!("file:///{encoded}"))
}

fn pdf_is_complete(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt as _;

    // Edge 写出文件后，仍可能持有写入句柄。
    let Ok(mut file) = std::fs::OpenOptions::new().read(true).share_mode(0).open(path) else {
        return false;
    };
    let mut header = [0; 5];
    let mut tail = [0; 8];
    file.read_exact(&mut header).is_ok()
        && &header == b"%PDF-"
        && file.seek(SeekFrom::End(-(tail.len() as i64))).is_ok()
        && file.read_exact(&mut tail).is_ok()
        && tail.trim_ascii_end().ends_with(b"%%EOF")
}

/// 导出成 PDF：先生成网页，再用系统自带的 Edge 以无界面模式把它「打印」成 PDF。
///
/// 这样中文字体由系统负责，文件小，版式和网页导出一致。全程不出窗口、不抢前台；
/// Edge 用一个一次性的临时配置目录，用完即删，不碰任何已有的浏览器配置。
pub fn export_pdf(blocks: &[ExportBlock], target: &Path, title: &str, tr: Tr) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    use std::time::{Duration, Instant};
    // CREATE_NO_WINDOW：不弹命令窗。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let edge = find_edge().ok_or_else(|| tr.err_edge_missing().to_string())?;
    let work = std::env::temp_dir().join(format!("moss-pocket-pdf-{}", mp_core::util::random_id().map_err(|error| error.to_string())?));
    let document = work.join("document");
    std::fs::create_dir_all(&document).map_err(|error| error.to_string())?;

    let html_path = document.join("note.html");
    let result = export_html(blocks, &html_path, title).and_then(|_| file_url(&html_path)).and_then(|url| {
        let printed = document.join("printed.pdf");
        let output = printed.to_string_lossy().to_string();
        let profile = work.join("profile");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut browser = std::process::Command::new(&edge)
            .args(["--headless", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--no-pdf-header-footer"])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("--print-to-pdf={output}"))
            .arg(&url)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| i18n::edge_failed_to_start(tr, &error.to_string()))?;
        loop {
            let status = browser.try_wait().map_err(|error| error.to_string())?;
            if let Some(status) = status {
                if !status.success() {
                    return Err(i18n::edge_pdf_failed(tr, status.code().unwrap_or(-1)));
                }
                // Edge 可以把打印交给另一个进程，先退出启动进程。
                if pdf_is_complete(&printed) {
                    return std::fs::copy(&printed, target).map(|_| ()).map_err(|error| error.to_string());
                }
            }
            if Instant::now() >= deadline {
                if status.is_none() {
                    let _ = browser.kill();
                }
                return Err(match tr.0 {
                    crate::i18n::Lang::Zh => "等待 Edge 生成 PDF 超时",
                    _ => "Timed out waiting for Edge to write the PDF",
                }.into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    // 先清理导出的明文，避免临时配置里的锁妨碍清理。
    let _ = std::fs::remove_dir_all(&document);
    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match std::fs::remove_dir_all(&work) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) if Instant::now() >= cleanup_deadline => break,
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> InlineStyle {
        InlineStyle {
            bold: true,
            ..InlineStyle::default()
        }
    }

    /// 造一张 4×4 的纯蓝 PNG。
    fn blue_png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn sample_blocks() -> Vec<ExportBlock> {
        vec![
            ExportBlock::Text {
                kind: TextKind::Paragraph,
                title: true,
                runs: vec![(InlineStyle::default(), "导出 & 测试".to_string())],
            },
            ExportBlock::Text {
                kind: TextKind::Paragraph,
                title: false,
                runs: vec![(InlineStyle::default(), "普通".to_string()), (bold(), "加粗".to_string())],
            },
            ExportBlock::Text {
                kind: TextKind::Bullet,
                title: false,
                runs: vec![(InlineStyle::default(), "条目".to_string())],
            },
            ExportBlock::Image { data: blue_png(), mime: "image/png", extension: "png", width: 4, height: 4 },
            ExportBlock::Audio { file_name: "tone.wav".to_string(), data: b"RIFFfake".to_vec() },
            ExportBlock::Divider,
        ]
    }

    /// 建一个本次测试独占的临时目录，测试结束时删除。
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("moss-pocket-export-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 从 ZIP 里按名字取出一个文件的内容（只认本模块写出来的这种不压缩的包）。
    fn unzip_entry(zip: &[u8], name: &str) -> Option<Vec<u8>> {
        let mut cursor = 0usize;
        while zip[cursor..].starts_with(b"PK\x03\x04") {
            let size = u32::from_le_bytes(zip[cursor + 18..cursor + 22].try_into().unwrap()) as usize;
            let name_len = u16::from_le_bytes(zip[cursor + 26..cursor + 28].try_into().unwrap()) as usize;
            let extra_len = u16::from_le_bytes(zip[cursor + 28..cursor + 30].try_into().unwrap()) as usize;
            let start = cursor + 30 + name_len + extra_len;
            let found = &zip[cursor + 30..cursor + 30 + name_len];
            if found == name.as_bytes() {
                return Some(zip[start..start + size].to_vec());
            }
            cursor = start + size;
        }
        None
    }

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn chosen_markdown_path_and_reserved_file_names_are_preserved() {
        let scratch = Scratch::new("md-path");
        let target = scratch.0.join("chosen name.md");
        export(Format::Markdown, &sample_blocks(), &target, "different title", Tr(crate::i18n::Lang::En)).unwrap();
        assert!(target.is_file());
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.contains("chosen%20name_files/image.png"));
        assert_eq!(safe_file_name("CON.txt"), "_CON.txt");
        let existing = [("TONE.WAV".into(), b"".as_slice())];
        assert_eq!(unique_media_name(&existing, "tone.wav"), "tone-2.wav");
    }

    #[test]
    fn word_line_breaks_are_run_children_and_url_delimiters_are_encoded() {
        let text = docx_runs(&[(InlineStyle::default(), "first\nsecond".into())]);
        assert!(text.contains("first</w:t><w:br/><w:t xml:space=\"preserve\">second"));
        assert_eq!(relative_url("a #1/(clip).mp4"), "a%20%231/%28clip%29.mp4");
    }

    #[test]
    fn markdown_export_writes_note_and_media_folder() {
        let scratch = Scratch::new("md");
        let title = "我的:想法";
        let target = scratch.0.join(format!("{}.md", safe_file_name(title)));
        export(Format::Markdown, &sample_blocks(), &target, title, Tr(crate::i18n::Lang::Zh)).unwrap();
        assert_eq!(target.file_name().unwrap(), "我的_想法.md");
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.starts_with("# 导出 & 测试\n"), "{text}");
        assert!(text.contains("普通**加粗**"), "{text}");
        assert!(text.contains("- 条目"), "{text}");
        assert!(text.contains("![图片](我的_想法_files/image.png)"), "{text}");
        assert!(text.contains("\n---\n"), "{text}");
        let media = scratch.0.join("我的_想法_files");
        assert_eq!(std::fs::read(media.join("image.png")).unwrap(), blue_png());
        assert_eq!(std::fs::read(media.join("tone.wav")).unwrap(), b"RIFFfake");
    }

    #[test]
    fn html_export_is_a_single_file_with_embedded_image() {
        let scratch = Scratch::new("html");
        let target = scratch.0.join("note.html");
        export_html(&sample_blocks(), &target, "导出 & 测试").unwrap();
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.starts_with("<!DOCTYPE html>"));
        assert!(text.contains("<title>导出 &amp; 测试</title>"), "标题里的特殊字符要转义");
        assert!(text.contains("<h1>导出 &amp; 测试</h1>"));
        assert!(text.contains("普通<strong>加粗</strong>"));
        assert!(text.contains("src=\"data:image/png;base64,"), "图片应嵌在文件里");
        assert!(text.contains("<audio controls"), "小音频应嵌在文件里");
        assert!(!scratch.0.join("note_files").exists(), "媒体都嵌进去了，不该有文件夹");
    }

    #[test]
    fn html_export_puts_large_media_beside_the_file() {
        let scratch = Scratch::new("html-big");
        let blocks = vec![ExportBlock::Video { file_name: "大视频.mp4".to_string(), data: vec![0u8; INLINE_MEDIA_LIMIT + 1] }];
        let target = scratch.0.join("note.html");
        export_html(&blocks, &target, "t").unwrap();
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.contains("src=\"note_files/大视频.mp4\""));
        assert!(scratch.0.join("note_files").join("大视频.mp4").is_file());
    }

    #[test]
    fn docx_export_is_a_valid_package_that_word_can_open() {
        let scratch = Scratch::new("docx");
        let target = scratch.0.join("note.docx");
        export_docx(&sample_blocks(), &target, "t").unwrap();
        let zip = std::fs::read(&target).unwrap();
        assert!(zip.starts_with(b"PK\x03\x04"));
        assert!(zip.ends_with(&[0, 0, 0, 0]), "包尾是结束记录");
        for required in ["[Content_Types].xml", "_rels/.rels", "word/document.xml", "word/styles.xml", "word/_rels/document.xml.rels", "word/media/image1.png"] {
            assert!(unzip_entry(&zip, required).is_some(), "包里缺 {required}");
        }
        assert_eq!(unzip_entry(&zip, "word/media/image1.png").unwrap(), blue_png());
        let document = String::from_utf8(unzip_entry(&zip, "word/document.xml").unwrap()).unwrap();
        assert!(document.contains("导出 &amp; 测试"), "{document}");
        assert!(document.contains("<w:pStyle w:val=\"Title\"/>"));
        assert!(document.contains("<w:b/>"));
        assert!(document.contains("• "));
        let relationships = unzip_entry(&zip, "word/_rels/document.xml.rels").unwrap();
        assert!(relationships.windows(12).any(|window| window == b"media/image1"));
    }

    /// 真的调起无界面的 Edge 打印一次。平时不跑，免得每轮测试都起浏览器。
    #[test]
    #[ignore = "需要系统里装有 Edge，手动跑：cargo test pdf_export -- --ignored"]
    fn pdf_export_prints_through_headless_edge() {
        let scratch = Scratch::new("pdf");
        let target = scratch.0.join("note.pdf");
        export_pdf(&sample_blocks(), &target, "导出测试", Tr(crate::i18n::Lang::Zh)).unwrap();
        let bytes = std::fs::read(&target).unwrap();
        assert!(bytes.starts_with(b"%PDF-"), "不是 PDF 文件");
        assert!(bytes.trim_ascii_end().ends_with(b"%%EOF"), "PDF 文件未写完整");
        assert!(bytes.len() > 1000);
    }
}
