//! 导出的界面入口：一个「导出」菜单，四种格式。导出本身在 `export.rs`。

use std::path::Path;

use crate::export::{self, Format};
use crate::file_picker::{FilePicker, PickerOutcome};
use crate::i18n::Tr;
use crate::session::Session;
use eframe::egui;
use mp_core::model::Note;

const FORMATS: [Format; 4] = [Format::Markdown, Format::Html, Format::Docx, Format::Pdf];

pub(crate) struct PendingExport {
    note: Note,
    title: String,
    format: Format,
    picker: FilePicker,
}

fn format_name(format: Format, tr: Tr) -> &'static str {
    match format {
        Format::Markdown => "Markdown",
        Format::Html => tr.export_html(),
        Format::Docx => tr.export_docx(),
        Format::Pdf => "PDF",
    }
}

/// 把一条笔记导出成指定格式：先问存到哪儿，再导出。
pub fn export_note(session: &mut Session, note_id: &str, format: Format, tr: Tr) {
    session.notice = None;
    let note = if session.current.as_deref() == Some(note_id) {
        session.finish_audio_rename();
        session.editor.note().clone()
    } else {
        match session.vault.load_note(note_id) {
            Ok(note) => note,
            Err(error) => {
                session.notice = Some(session.tr.core_error(&error));
                return;
            }
        }
    };
    let raw_title = note.title();
    let title = if raw_title.is_empty() { tr.untitled() } else { raw_title }.to_string();
    let file_name = format!("{}.{}", export::safe_file_name(&title), format.extension());
    #[cfg(test)]
    if let Some(choice) = crate::matrix::take_file_dialog(crate::matrix::Dialog::Export(format.extension())) {
        if let Some(path) = choice.and_then(|paths| paths.into_iter().next()) {
            let target = match format {
                Format::Markdown => path.join(&file_name),
                _ => path,
            };
            finish_export(session, &note, &title, format, &target, tr);
        }
        return;
    }
    let name = format_name(format, tr);
    let picker_title = format!("{} · {name}", tr.export());
    let initial_dir = crate::editor_ui::start_directory();
    let picker = match format {
        Format::Markdown => FilePicker::folder(&picker_title, file_name, initial_dir),
        _ => FilePicker::save(&picker_title, name, Some(format.extension()), file_name, initial_dir),
    };
    session.export_picker = Some(PendingExport { note, title, format, picker });
}

pub(crate) fn show_picker(ui: &mut egui::Ui, session: &mut Session, tr: Tr) {
    let Some(mut pending) = session.export_picker.take() else {
        return;
    };
    match pending.picker.show(ui, tr) {
        PickerOutcome::Pending => session.export_picker = Some(pending),
        PickerOutcome::Cancel => {}
        PickerOutcome::Selected(paths) => {
            if let Some(target) = paths.into_iter().next() {
                finish_export(session, &pending.note, &pending.title, pending.format, &target, tr);
            }
        }
    }
}

fn finish_export(session: &mut Session, note: &Note, title: &str, format: Format, target: &Path, tr: Tr) {
    let done = export::collect(note, &session.vault, tr)
        .and_then(|blocks| export::export(format, &blocks, target, title, tr));
    session.notice = Some(match done {
        Ok(()) => tr.export_done().to_string(),
        Err(error) => format!("{}：{error}", tr.export_failed()),
    });
}

/// 菜单里的四个导出项。点中之后关菜单并开始导出。
pub fn menu_items(ui: &mut egui::Ui, session: &mut Session, note_id: &str, tr: Tr) {
    for format in FORMATS {
        if ui.button(format_name(format, tr)).clicked() {
            ui.close();
            export_note(session, note_id, format, tr);
            return;
        }
    }
}

/// 正文上方的「导出」按钮，导出当前笔记。
pub fn export_button(ui: &mut egui::Ui, session: &mut Session, tr: Tr) {
    let note_id = session.editor.note().id.clone();
    let palette = crate::theme::Palette::for_ui(ui);
    let button = ui.add(crate::theme::secondary_button(tr.export(), palette));
    egui::Popup::menu(&button).show(|ui| menu_items(ui, session, &note_id, tr));
}
