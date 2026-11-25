//! 剪贴板快捷键。窗口层自带的翻译关掉了（见 `main.rs` 里的 `clipboard_shortcuts: false`），
//! 所以 Ctrl+C、X、V 作为普通按键到这里，由这里统一决定：给有焦点的文本框、给选中的块，还是导入文件。
//!
//! 不交给窗口层的原因：它碰到 Ctrl+V 只认文字和图片，剪贴板里是文件列表时什么都不发，
//! 「从资源管理器复制文件再粘贴进来」做不了；而且它会把按键吃掉，这边没法知道用户按过粘贴。

use crate::app::MossPocket;
use crate::media::ImportSource;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;

/// 剪贴板上现在是什么。
#[derive(Clone)]
pub(crate) enum ClipboardContent {
    Files(Vec<PathBuf>),
    Text(String),
    Image(Arc<egui::ColorImage>),
    Empty,
}

#[cfg(test)]
thread_local! {
    static FAKE: std::cell::RefCell<Option<ClipboardContent>> = const { std::cell::RefCell::new(None) };
}

/// 测试用：把系统剪贴板换成给定的内容，测试就不碰真实剪贴板。传 `None` 恢复。
#[cfg(test)]
pub(crate) fn fake(content: Option<ClipboardContent>) {
    FAKE.with(|slot| *slot.borrow_mut() = content);
}

impl ClipboardContent {
    /// 看一眼系统剪贴板。顺序和别的软件一样：先看文件，再看文字，最后看图片。
    pub(crate) fn read() -> Self {
        #[cfg(test)]
        if let Some(content) = FAKE.with(|slot| slot.borrow().clone()) {
            return content;
        }
        let files = crate::win::clipboard_files();
        if !files.is_empty() {
            return Self::Files(files);
        }
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return Self::Empty;
        };
        match clipboard.get_text() {
            Ok(text) if !text.is_empty() => Self::Text(text.replace("\r\n", "\n")),
            _ => match clipboard.get_image() {
                Ok(image) => Self::Image(Arc::new(egui::ColorImage::from_rgba_unmultiplied(
                    [image.width as usize, image.height as usize],
                    &image.bytes,
                ))),
                Err(_) => Self::Empty,
            },
        }
    }
}

/// 处理这一帧的剪贴板快捷键。每一帧最先调用，要在任何控件看到按键之前。
pub(crate) fn handle_keys(ui: &mut egui::Ui, app: &mut MossPocket) {
    // 也认老式的组合键：Shift+Insert 粘贴，Ctrl+Insert 复制。
    let paste = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::V))
        || ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Insert));
    // 选中的是一个图片或音视频块，而且没有任何输入框有焦点：复制、剪切作用于整块。
    let workspace = !app.settings_open && app.session().is_some_and(|session| {
        !session.editor_ui.overlay_open() && session.export_picker.is_none()
    });
    let block_selected = workspace && app.session_mut().is_some_and(|session| session.block_clipboard_target());
    let copy = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::C))
        || ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Insert));
    // Ctrl+Shift+X belongs to the editor's strikethrough shortcut.
    let cut = ui.input_mut(|input| {
        let before = input.events.len();
        input.events.retain(|event| !matches!(
            event,
            egui::Event::Key { key: egui::Key::X, pressed: true, modifiers, .. }
                if modifiers.matches_exact(egui::Modifiers::COMMAND)
        ));
        input.events.len() != before
    });
    if block_selected {
        let Some(session) = app.session_mut() else {
            return;
        };
        let block = session.selected_block.unwrap_or_default();
        if copy {
            session.copy_block(block);
        }
        if cut {
            session.cut_block(block);
        }
    } else {
        // 其余情形照系统习惯来：交给有焦点的文本框（或可选中文字的标签）自己处理。
        if copy {
            ui.input_mut(|i| i.events.push(egui::Event::Copy));
        }
        if cut {
            ui.input_mut(|i| i.events.push(egui::Event::Cut));
        }
    }
    if !paste {
        return;
    }
    // 刚复制过一块、之后系统剪贴板没变过：粘贴的就是那一块。
    let internal_clip = workspace && app.session_mut().is_some_and(|session| !session.other_text_focus && session.clip_is_fresh());
    if internal_clip {
        if let Some(session) = app.session_mut() {
            session.paste_block();
        }
        return;
    }
    match ClipboardContent::read() {
        ClipboardContent::Files(paths) if workspace => {
            for path in paths {
                if let Some(session) = app.session_mut() {
                    session.import(ImportSource::File(path));
                }
            }
        }
        ClipboardContent::Text(text) => ui.input_mut(|i| i.events.push(egui::Event::Paste(text))),
        ClipboardContent::Image(image) if workspace => ui.input_mut(|i| i.events.push(egui::Event::PasteImage(image))),
        _ => {}
    }
}
