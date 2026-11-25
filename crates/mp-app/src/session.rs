//! 已解锁后的会话：当前笔记、编辑器状态、列表视图，以及它们上面的操作。

use crate::editor_ui::EditorUi;
use crate::i18n::Tr;
use crate::media::{ImportJob, ImportSource, MediaEvent, MediaHub};
use crate::recording::Recorder;
use eframe::egui;
use mp_core::editor::Editor;
use mp_core::model::{char_count, normalize_tags, Block, BlockId, Note, TextBlock, TextKind, TitleSource};
use mp_core::util::now_millis;
use mp_core::vault::Vault;
use std::time::{Duration, Instant};

struct ActiveRecording {
    recorder: Recorder,
    note_id: String,
    anchor: Option<BlockId>,
    anchor_revision: u64,
}

/// 左侧列表的视图状态。
#[derive(Default)]
pub struct ListView {
    pub search: String,
    pub show_trash: bool,
    other_search: String,
    last_note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll_imports(session: &mut Session, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.media.pending_import_count() > 0 {
            assert!(Instant::now() < deadline, "media import did not finish");
            session.poll_media(ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn navigation_preserves_dirty_edits_on_failure_and_routes_imports_to_original_note() {
        let path = std::env::temp_dir().join(format!("moss-session-{}", mp_core::util::random_id().unwrap()));
        {
            let (vault, _) = Vault::create(&path, "test1234").unwrap();
            let mut session = Session::new(vault, Tr(crate::i18n::Lang::En));
            session.new_note();
            let first = session.current.clone().unwrap();
            session.editor.append_paragraph("original");
            assert!(session.save_current());
            session.new_note();
            let second = session.current.clone().unwrap();
            session.apply_import(&first, Block::Divider { id: 12345 }, None);
            assert_eq!(session.current.as_deref(), Some(second.as_str()));
            assert!(session.vault.load_note(&first).unwrap().blocks.iter().any(|block| block.id() == 12345));

            session.editor.append_paragraph("unsaved");
            session.editor.note_meta_mut().id = "invalid".into();
            session.new_note();
            assert_eq!(session.current.as_deref(), Some(second.as_str()));
            assert!(session.editor.note().plain_text().contains("unsaved"));
            session.open(&first);
            assert_eq!(session.current.as_deref(), Some(second.as_str()));
            assert!(session.editor.dirty());
            assert!(session.notice.is_some());
        }
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn recording_retries_failed_encryption_and_stays_with_its_original_note() {
        let path = std::env::temp_dir().join(format!("moss-recording-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = Vault::create(&path, "test1234").unwrap();
        let mut session = Session::new(vault, Tr(crate::i18n::Lang::En));
        let ctx = egui::Context::default();
        let wav = crate::scenarios::wav_bytes(800);
        let blobs = path.join("blobs");
        std::fs::remove_dir(&blobs).unwrap();
        std::fs::write(&blobs, b"temporarily unavailable").unwrap();

        session.new_note();
        let first = session.current.clone().unwrap();
        let anchor = session.editor.note().blocks[0].id();
        session.selected_block = Some(anchor);
        session.start_synthetic_recording(wav.clone(), Duration::from_secs(1));
        session.editor.append_paragraph("after the recording anchor");
        session.new_note();
        let second = session.current.clone().unwrap();
        assert_ne!(first, second);
        assert!(session.recording_elapsed().is_none());
        poll_imports(&mut session, &ctx);
        assert!(session.recording_retry_pending());
        assert!(session.notice.as_deref().unwrap().contains("Recording failed"));

        std::fs::remove_file(&blobs).unwrap();
        session.start_recording();
        session.start_recording();
        assert!(session.recording_elapsed().is_none());
        assert_eq!(session.media.pending_import_count(), 1);
        poll_imports(&mut session, &ctx);
        assert!(!session.recording_has_work());
        assert_eq!(session.current.as_deref(), Some(second.as_str()));
        assert!(session.editor.note().is_empty());
        let note = session.vault.load_note(&first).unwrap();
        assert_eq!(note.blocks[0].id(), anchor);
        let Block::Audio(recording) = &note.blocks[1] else { panic!("audio should follow the original anchor") };
        assert_eq!(recording.mime, "audio/wav");
        assert_eq!(recording.duration_secs, Some(0.1));
        assert_eq!(session.media.read_blob(&recording.blob_id).unwrap(), wav);
        let ciphertext = std::fs::read(blobs.join(format!("{}.blob", recording.blob_id))).unwrap();
        assert!(!ciphertext.starts_with(b"RIFF"));
        assert_ne!(ciphertext, wav);

        drop(session);
        let mut reopened = Vault::unlock(&path, mp_core::vault::UnlockSecret::Password("test1234")).unwrap();
        assert_eq!(reopened.load_note(&first).unwrap().blocks[1].blob_id(), Some(recording.blob_id.as_str()));
        assert_eq!(reopened.blob_handle().read(&recording.blob_id).unwrap(), wav);
        drop(reopened);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn trashing_keeps_the_recording_and_permanent_deletion_drops_late_results() {
        let path = std::env::temp_dir().join(format!("moss-recording-trash-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = Vault::create(&path, "test1234").unwrap();
        let mut session = Session::new(vault, Tr(crate::i18n::Lang::En));
        let ctx = egui::Context::default();
        let wav = crate::scenarios::wav_bytes(800);

        session.new_note();
        let retained = session.current.clone().unwrap();
        session.start_synthetic_recording(wav.clone(), Duration::ZERO);
        session.move_to_trash(&retained);
        assert!(session.current.is_none());
        assert!(session.recording_elapsed().is_none());
        poll_imports(&mut session, &ctx);
        let note = session.vault.load_note(&retained).unwrap();
        assert!(note.deleted_at.is_some());
        assert!(note.blocks.iter().any(|block| matches!(block, Block::Audio(_))));

        session.new_note();
        let destroyed = session.current.clone().unwrap();
        session.start_synthetic_recording(wav, Duration::ZERO);
        session.move_to_trash(&destroyed);
        session.destroy(&destroyed);
        poll_imports(&mut session, &ctx);
        assert!(matches!(session.vault.load_note(&destroyed), Err(mp_core::error::CoreError::NoteNotFound)));
        assert!(!session.recording_has_work());

        drop(session);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn recording_and_file_batch_keep_their_order_after_switching_empty_notes() {
        let path = std::env::temp_dir().join(format!("moss-media-order-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = Vault::create(&path, "test1234").unwrap();
        let mut session = Session::new(vault, Tr(crate::i18n::Lang::En));
        let ctx = egui::Context::default();
        let recording = crate::scenarios::wav_bytes(800);
        let wav = crate::scenarios::wav_bytes(1600);
        let png = crate::media::encode_png(&egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4])).unwrap();
        let first_file = path.join("first.png");
        let missing_file = path.join("missing.png");
        let last_file = path.join("last.wav");
        std::fs::write(&first_file, &png).unwrap();
        std::fs::write(&last_file, &wav).unwrap();

        session.new_note();
        let original = session.current.clone().unwrap();
        let anchor = session.editor.note().blocks[0].id();
        let revision = session.editor_ui.media_anchor_revision();
        session.editor_ui.follow_media_insertion(revision, anchor);
        session.start_synthetic_recording(recording.clone(), Duration::ZERO);
        session.finish_recording();
        session.import_batch_after(
            vec![ImportSource::File(first_file), ImportSource::File(missing_file), ImportSource::File(last_file)],
            Some(anchor),
            revision,
        );
        session.new_note();
        assert_ne!(session.current.as_deref(), Some(original.as_str()));

        poll_imports(&mut session, &ctx);
        assert!(session.editor.note().is_empty());
        assert!(session.notice.as_deref().unwrap().contains("missing.png"));
        let saved = session.vault.load_note(&original).unwrap();
        assert!(matches!(saved.blocks.as_slice(), [Block::Text(_), Block::Audio(_), Block::Image(_), Block::Audio(_)]));
        let contents: Vec<_> = saved.blob_ids().into_iter().map(|id| session.media.read_blob(id).unwrap()).collect();
        assert_eq!(contents, [recording, png, wav]);

        drop(session);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn late_import_does_not_replace_a_new_editor_anchor() {
        let path = std::env::temp_dir().join(format!("moss-media-anchor-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = Vault::create(&path, "test1234").unwrap();
        let mut session = Session::new(vault, Tr(crate::i18n::Lang::En));
        let ctx = egui::Context::default();
        session.new_note();
        let first = session.editor.note().blocks[0].id();
        let second = session.editor.append_paragraph("second position").unwrap();
        let source = ImportSource::Memory { name: "first.wav".into(), bytes: crate::scenarios::wav_bytes(800) };
        let revision = session.editor_ui.media_anchor_revision();
        session.import_batch_after(vec![source.clone()], Some(first), revision);

        session.editor_ui.reset();
        let revision = session.editor_ui.media_anchor_revision();
        session.editor_ui.follow_media_insertion(revision, second);
        session.selected_block = Some(second);
        poll_imports(&mut session, &ctx);
        assert_eq!(session.editor_ui.media_anchor(), Some(second));
        assert_eq!(session.selected_block, Some(second));
        assert_eq!(session.editor.note().blocks[2].id(), second);

        session.import(source);
        poll_imports(&mut session, &ctx);
        let inserted = session.editor.note().blocks[3].id();
        assert_eq!(session.editor_ui.media_anchor(), Some(inserted));
        assert_eq!(session.selected_block, Some(inserted));

        drop(session);
        std::fs::remove_dir_all(path).unwrap();
    }
}

pub struct Session {
    pub vault: Vault,
    pub editor: Editor,
    /// 界面语言。失败原因按它来写。
    pub tr: Tr,
    /// 当前打开的笔记标识；None 表示还没选。
    pub current: Option<String>,
    pub current_in_trash: bool,
    /// 当前笔记是否已经落过盘。没落过盘的空笔记不保存，切走就丢弃。
    pub persisted: bool,
    pub tags_input: String,
    pub view: ListView,
    pub editor_ui: EditorUi,
    pub(crate) export_picker: Option<crate::export_ui::PendingExport>,
    pub last_edit: Option<Instant>,
    /// 焦点是否在正文以外的输入框里（搜索框、标签框、查找栏、设置窗口）。
    pub other_text_focus: bool,
    /// 这一帧画到后面才知道的焦点情况（标签框、查找栏、设置窗口）。下一帧开头并进 `other_text_focus`。
    pub late_text_focus: bool,
    /// 最近一次操作的失败原因，给界面显示。
    pub notice: Option<String>,
    /// 媒体导入、图片缓存、音频播放、视频流式供数。
    pub media: MediaHub,
    /// 软件内部的整块剪贴板：复制或剪切下来的那一块。
    pub clip: Option<Block>,
    /// 复制那一块时系统剪贴板的序号。粘贴时序号没变，说明用户没在别处复制过别的，粘的就是这一块。
    pub clip_sequence: u32,
    /// 最后点选的那个非文字块，复制、剪切、粘贴整块时作用于它。
    pub selected_block: Option<BlockId>,
    /// 笔记内查找与替换的栏。
    pub find: crate::find_bar::FindBar,
    /// 每条笔记的正文缓存，全文搜索用。第一次搜到某条时解密并记住；笔记存盘后作废重取。
    full_text: std::collections::HashMap<String, String>,
    pending_imports: Vec<(String, Block, Option<BlockId>)>,
    /// 每个编辑位置上一次插入的块。切走后，同位置的排队项目也能接在它后面。
    imported_at_anchor: std::collections::HashMap<u64, BlockId>,
    recording: Option<ActiveRecording>,
    /// 还没加密入库的录音。写盘失败时把内存里的 WAV 留到下次保存。
    recording_retries: Vec<ImportJob>,
}

/// 随机数也拿不到时的兜底空笔记，保证界面还能画出来。
fn fallback_note() -> Note {
    Note {
        id: "00000000000000000000000000000000".into(),
        created_at: 0,
        updated_at: 0,
        pinned: false,
        tags: Vec::new(),
        deleted_at: None,
        title_source: Default::default(),
        blocks: vec![Block::Text(TextBlock {
            id: 0,
            kind: TextKind::Paragraph,
            text: String::new(),
            spans: Vec::new(),
        })],
    }
}

impl Session {
    pub fn new(mut vault: Vault, tr: Tr) -> Self {
        let _ = vault.purge_trash(now_millis());
        let media = MediaHub::new(vault.blob_handle(), tr);
        let latest = vault
            .summaries()
            .into_iter()
            .find(|summary| summary.deleted_at.is_none())
            .map(|summary| summary.id);
        let mut session = Self {
            vault,
            tr,
            editor: Editor::new(Note::new().unwrap_or_else(|_| fallback_note())),
            current: None,
            current_in_trash: false,
            persisted: false,
            tags_input: String::new(),
            view: ListView::default(),
            editor_ui: EditorUi::default(),
            export_picker: None,
            last_edit: None,
            other_text_focus: false,
            late_text_focus: false,
            notice: None,
            media,
            clip: None,
            clip_sequence: 0,
            selected_block: None,
            find: crate::find_bar::FindBar::default(),
            full_text: std::collections::HashMap::new(),
            pending_imports: Vec::new(),
            imported_at_anchor: std::collections::HashMap::new(),
            recording: None,
            recording_retries: Vec::new(),
        };
        if let Some(id) = latest {
            session.open(&id);
        }
        session
    }

    /// 打开一条笔记。当前笔记有改动就先存盘。
    pub fn open(&mut self, id: &str) {
        self.notice = None;
        self.finish_audio_rename();
        if self.current.as_deref() != Some(id) {
            self.finish_recording();
        }
        if !self.save_current() {
            return;
        }
        match self.vault.load_note(id) {
            Ok(note) => {
                self.current_in_trash = note.deleted_at.is_some();
                self.tags_input = note.tags.join(", ");
                self.editor.load(note);
                self.current = Some(id.to_string());
                self.persisted = true;
                self.editor_ui.reset();
                self.selected_block = None;
                self.find.note_changed();
                let note = self.editor.note();
                let mut text = note.blocks.iter().filter_map(Block::as_text);
                let resume = if note.title_source == TitleSource::Legacy {
                    text.next()
                } else {
                    text.rfind(|block| note.title_source != TitleSource::Block(block.id)).or_else(|| note.title_block())
                };
                if let Some(block) = resume {
                    self.editor_ui.request_focus(block.id, char_count(&block.text));
                }
            }
            Err(error) => {
                self.notice = Some(self.tr.core_error(&error));
            }
        }
    }

    /// 新建一条空笔记并打开它。
    pub fn new_note(&mut self) {
        self.notice = None;
        self.finish_audio_rename();
        self.finish_recording();
        if !self.save_current() {
            return;
        }
        let Ok(note) = Note::new_capture() else {
            return;
        };
        let id = note.id.clone();
        let first_block = note.blocks[0].id();
        if self.view.show_trash {
            std::mem::swap(&mut self.view.search, &mut self.view.other_search);
            self.view.show_trash = false;
        }
        self.view.search.clear();
        self.current_in_trash = false;
        self.tags_input.clear();
        self.editor.load(note);
        self.current = Some(id);
        self.persisted = false;
        self.editor_ui.reset();
        self.selected_block = None;
        self.find.close();
        self.find.note_changed();
        self.editor_ui.request_focus(first_block, 0);
    }

    pub fn toggle_trash_view(&mut self) {
        self.finish_audio_rename();
        self.finish_recording();
        if !self.save_current() {
            return;
        }
        let show_trash = !self.view.show_trash;
        if show_trash {
            self.view.last_note = self.current.clone().filter(|_| self.persisted);
        }
        self.view.show_trash = show_trash;
        std::mem::swap(&mut self.view.search, &mut self.view.other_search);
        self.dismiss_current();
        if !show_trash {
            if let Some(note) = self.view.last_note.take() {
                self.open(&note);
            }
        }
    }

    fn dismiss_current(&mut self) {
        self.current = None;
        self.current_in_trash = false;
        self.persisted = false;
        self.tags_input.clear();
        self.editor.load(fallback_note());
        self.editor_ui.reset();
        self.last_edit = None;
        self.selected_block = None;
        self.find.close();
        self.find.note_changed();
    }

    /// 把当前笔记写盘。没有改动、或是不值得保存的空新笔记，什么都不做。
    pub fn save_current(&mut self) -> bool {
        let recordings_queued = self.queue_recordings();
        let pending = std::mem::take(&mut self.pending_imports);
        for (note_id, block, anchor) in pending {
            self.apply_import(&note_id, block, anchor);
        }
        let imports_saved = self.pending_imports.is_empty() && recordings_queued;
        if self.current.is_none() {
            return imports_saved;
        }
        if !self.editor.dirty() && self.persisted {
            return imports_saved;
        }
        if !self.persisted && self.editor.note().is_empty() {
            return imports_saved;
        }
        let note = self.editor.note().clone();
        match self.vault.save_note(&note) {
            Ok(()) => {
                self.editor.mark_saved(now_millis());
                self.persisted = true;
                if let Some(id) = &self.current {
                    self.full_text.remove(id);
                }
                imports_saved
            }
            Err(error) => {
                self.notice = Some(self.tr.core_error(&error));
                false
            }
        }
    }

    /// 置顶或取消置顶。作用于当前笔记时直接改内存里的笔记。
    pub fn toggle_pin(&mut self, id: &str) {
        if self.current.as_deref() == Some(id) {
            let pinned = !self.editor.note().pinned;
            self.editor.note_meta_mut().pinned = pinned;
            self.save_current();
        } else if let Ok(mut note) = self.vault.load_note(id) {
            note.pinned = !note.pinned;
            if let Err(error) = self.vault.save_note(&note) {
                self.notice = Some(self.tr.core_error(&error));
            }
        }
    }

    pub fn move_to_trash(&mut self, id: &str) {
        self.set_trash(id, Some(now_millis()));
    }

    pub fn restore(&mut self, id: &str) {
        self.set_trash(id, None);
    }

    fn set_trash(&mut self, id: &str, deleted_at: Option<u64>) {
        self.notice = None;
        if self.current.as_deref() == Some(id) {
            self.finish_audio_rename();
            self.finish_recording();
            if !self.queue_recordings() {
                return;
            }
            let previous_deleted_at = self.editor.note().deleted_at;
            self.editor.note_meta_mut().deleted_at = deleted_at;
            let note = self.editor.note().clone();
            match self.vault.save_note(&note) {
                Ok(()) => {
                    self.editor.mark_saved(now_millis());
                    self.persisted = true;
                    self.current_in_trash = deleted_at.is_some();
                    self.full_text.remove(id);
                    if self.current_in_trash != self.view.show_trash {
                        self.dismiss_current();
                    }
                }
                Err(error) => {
                    self.editor.note_meta_mut().deleted_at = previous_deleted_at;
                    self.notice = Some(self.tr.core_error(&error));
                }
            }
        } else if let Ok(mut note) = self.vault.load_note(id) {
            note.deleted_at = deleted_at;
            if let Err(error) = self.vault.save_note(&note) {
                self.notice = Some(self.tr.core_error(&error));
            }
        }
    }

    /// 彻底删除一条笔记。
    pub fn destroy(&mut self, id: &str) {
        self.notice = None;
        if self.recording.as_ref().is_some_and(|capture| capture.note_id == id) {
            self.cancel_recording();
        }
        match self.vault.destroy_note(id) {
            Ok(()) => {
                if self.current.as_deref() == Some(id) {
                    self.dismiss_current();
                }
                self.full_text.remove(id);
                self.pending_imports.retain(|(note_id, _, _)| note_id != id);
                self.recording_retries.retain(|recording| recording.note_id != id);
            }
            Err(error) => self.notice = Some(self.tr.core_error(&error)),
        }
    }

    /// 列表里当前显示的条目：按搜索词过滤，并按是否显示回收站分开。
    pub fn visible_summaries(&mut self) -> Vec<mp_core::model::NoteSummary> {
        let needle = self.view.search.trim().to_lowercase();
        let show_trash = self.view.show_trash;
        let mut list: Vec<mp_core::model::NoteSummary> = self
            .vault
            .summaries()
            .into_iter()
            .filter(|summary| summary.deleted_at.is_some() == show_trash)
            .collect();
        if !needle.is_empty() {
            list.retain(|summary| self.matches(summary, &needle));
        }
        list
    }

    /// 搜索词是否命中这条笔记：先看标题和标签，都没命中再查正文。
    fn matches(&mut self, summary: &mp_core::model::NoteSummary, needle: &str) -> bool {
        if summary.title.to_lowercase().contains(needle)
            || summary.tags.iter().any(|tag| tag.to_lowercase().contains(needle))
        {
            return true;
        }
        // 正文按需解密一次，之后从缓存里查。读不出来的笔记当作没有正文。
        self.full_text
            .entry(summary.id.clone())
            .or_insert_with_key(|id| self.vault.load_note(id).map(|note| note.plain_text()).unwrap_or_default())
            .to_lowercase()
            .contains(needle)
    }

    /// 标签输入框的内容变了，同步到笔记上。
    pub fn set_tags(&mut self, text: &str) {
        let tags = normalize_tags(
            &text
                .split([',', '，'])
                .map(str::to_string)
                .collect::<Vec<_>>(),
        );
        if tags != self.editor.note().tags {
            self.editor.note_meta_mut().tags = tags;
            self.last_edit = Some(Instant::now());
        }
    }

    /// 插入一个块到锚点之后，走撤销栈。返回新块的标识。
    pub fn insert_block(&mut self, block: Block, anchor: Option<BlockId>) -> Option<BlockId> {
        if self.current_in_trash {
            return None;
        }
        let id = self.editor.insert_block_after(anchor, block);
        self.note_text_changed();
        Some(id)
    }

    /// 新内容插在哪儿：有焦点的文本块之后，没有就追加到末尾。
    fn insert_anchor(&self) -> Option<BlockId> {
        self.editor_ui.focused_block()
    }

    /// 导入一个图片、音频或视频文件。真正的工作在后台线程，完成后这一块才会出现。
    pub fn import(&mut self, source: ImportSource) {
        let anchor = self.editor_ui.media_anchor().or_else(|| self.insert_anchor());
        self.import_batch_after(vec![source], anchor, self.editor_ui.media_anchor_revision());
    }

    pub fn import_batch_after(&mut self, sources: Vec<ImportSource>, anchor: Option<BlockId>, anchor_revision: u64) {
        if self.current_in_trash || sources.is_empty() {
            return;
        }
        let Some(note_id) = self.current.clone() else { return };
        // Even a blank note must survive switching away while its import runs.
        if !self.save_current() || !self.persist_import_target() {
            return;
        }
        let jobs = sources
            .into_iter()
            .map(|source| ImportJob { source, anchor, anchor_revision, note_id: note_id.clone(), recording: false })
            .collect();
        self.media.import_batch(jobs);
    }

    fn persist_import_target(&mut self) -> bool {
        if !self.persisted {
            if let Err(error) = self.vault.save_note(self.editor.note()) {
                self.notice = Some(self.tr.core_error(&error));
                return false;
            }
            self.editor.mark_saved(now_millis());
            self.persisted = true;
        }
        true
    }

    pub fn start_recording(&mut self) {
        if self.recording.is_some() || self.current_in_trash || self.current.is_none() {
            return;
        }
        // 先保存上一段，避免磁盘写入失败时在内存中累积多段录音。
        if self.recording_retry_pending() {
            if self.queue_recordings() {
                self.notice = Some(self.tr.import_in_progress().into());
            }
            return;
        }
        if self.media.recording_import_pending() {
            self.notice = Some(self.tr.import_in_progress().into());
            return;
        }
        match Recorder::start(self.tr) {
            Ok(recorder) => self.attach_recording(recorder),
            Err(error) => self.report_recording_error(&error),
        }
    }

    fn attach_recording(&mut self, recorder: Recorder) {
        if let Some(note_id) = self.current.clone() {
            let anchor = self.editor_ui.media_anchor().or(self.selected_block);
            let anchor_revision = self.editor_ui.media_anchor_revision();
            self.recording = Some(ActiveRecording { recorder, note_id, anchor, anchor_revision });
        }
    }

    #[cfg(test)]
    pub(crate) fn start_synthetic_recording(&mut self, wav: Vec<u8>, elapsed: Duration) {
        assert!(self.recording.is_none() && self.current.is_some() && !self.current_in_trash);
        self.attach_recording(Recorder::synthetic(wav, elapsed));
    }

    pub fn finish_recording(&mut self) {
        let Some(ActiveRecording { recorder, note_id, anchor, anchor_revision }) = self.recording.take() else { return };
        match recorder.stop_with_error() {
            Ok((bytes, error)) => {
                if let Some(error) = error {
                    self.report_recording_error(&error);
                }
                let source = ImportSource::Memory { name: self.tr.recording_file_name().into(), bytes };
                self.recording_retries.push(ImportJob { source, anchor, anchor_revision, note_id, recording: true });
                self.queue_recordings();
            }
            Err(error) => self.report_recording_error(&error),
        }
    }

    pub fn cancel_recording(&mut self) {
        self.recording = None;
    }

    pub fn recording_elapsed(&self) -> Option<Duration> {
        self.recording.as_ref().map(|capture| capture.recorder.elapsed())
    }

    pub fn recording_has_work(&self) -> bool {
        self.recording.is_some() || self.media.recording_import_pending() || self.recording_retry_pending()
    }

    pub fn recording_retry_pending(&self) -> bool {
        !self.recording_retries.is_empty()
    }

    pub fn queue_recordings(&mut self) -> bool {
        // 即使正文还空着，异步导入也要有一条存盘的笔记可写回。
        let current_needs_save = !self.persisted
            && self.recording_retries.iter().any(|job| self.current.as_deref() == Some(job.note_id.as_str()));
        if current_needs_save && !self.persist_import_target() {
            return false;
        }
        self.media.import_batch(std::mem::take(&mut self.recording_retries));
        true
    }

    fn report_recording_error(&mut self, error: &str) {
        self.notice = Some(format!("{} ({error})", self.tr.recording_failed()));
    }

    fn poll_recording(&mut self) {
        let Some(capture) = &self.recording else { return };
        if capture.recorder.error().is_some() {
            self.finish_recording();
        } else if capture.recorder.reached_limit() {
            self.notice = Some(self.tr.recording_limit_reached().into());
            self.finish_recording();
        }
    }

    pub fn poll_media(&mut self, ctx: &egui::Context) {
        self.poll_recording();
        for event in self.media.poll() {
            match event {
                MediaEvent::Imported { note_id, anchor, anchor_revision, block, preview, recording } => {
                    let blob_id = block.blob_id().map(str::to_string);
                    let is_current = self.current.as_deref() == Some(note_id.as_str());
                    let anchor = self.imported_at_anchor.get(&anchor_revision).copied()
                        .filter(|&id| !is_current || self.block_by_id(id).is_some()).or(anchor);
                    self.imported_at_anchor.insert(anchor_revision, block.id());
                    if let Some(id) = self.apply_import(&note_id, block, anchor) {
                        if self.editor_ui.media_anchor_revision() == anchor_revision {
                            self.editor_ui.follow_media_insertion(anchor_revision, id);
                            self.selected_block = Some(id);
                        }
                        if recording {
                            self.save_current();
                        }
                    }
                    if let (Some(blob_id), Some(image)) = (blob_id, preview) {
                        self.media.store_preview(ctx, &blob_id, &image);
                    }
                }
                MediaEvent::ImageReady { blob_id, image } => self.media.store_preview(ctx, &blob_id, &image),
                MediaEvent::ImageFailed { message, .. } => self.notice = Some(message),
                MediaEvent::ImportFailed { message, retry: None } => self.notice = Some(message),
                MediaEvent::ImportFailed { message, retry: Some(job) } => {
                    if self.vault.summaries().iter().any(|note| note.id == job.note_id) {
                        self.recording_retries.push(job);
                        self.report_recording_error(&message);
                    }
                }
            }
        }
        if let Some(error) = self.media.take_audio_error() {
            if !self.editor_ui.capture_audio_error(&error) {
                self.notice = Some(error);
            }
        }
        if self.media.pending_import_count() == 0 {
            let revision = self.editor_ui.media_anchor_revision();
            self.imported_at_anchor.retain(|&key, _| key == revision);
        }
    }

    /// Deliver background work to the note that requested it, even after navigation.
    fn apply_import(&mut self, note_id: &str, block: Block, anchor: Option<BlockId>) -> Option<BlockId> {
        if self.pending_imports.iter().any(|(id, _, _)| id == note_id) {
            self.pending_imports.push((note_id.to_string(), block, anchor));
            return None;
        }
        if self.current.as_deref() == Some(note_id) {
            // An import requested before trashing still belongs to this note.
            let id = self.editor.insert_block_after(anchor, block);
            self.note_text_changed();
            return Some(id);
        }
        let note = match self.vault.load_note(note_id) {
            Ok(note) => note,
            Err(error) => {
                let missing = matches!(&error, mp_core::error::CoreError::NoteNotFound)
                    || matches!(&error, mp_core::error::CoreError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound);
                if !missing || self.vault.summaries().iter().any(|note| note.id == note_id) {
                    self.notice = Some(self.tr.core_error(&error));
                    self.pending_imports.push((note_id.to_string(), block, anchor));
                }
                return None;
            }
        };
        let mut editor = Editor::new(note);
        // A previous attempt may have written the note before its index failed.
        if !editor.note().blocks.iter().any(|existing| existing.id() == block.id()) {
            editor.insert_block_after(anchor, block.clone());
        }
        if let Err(error) = self.vault.save_note(editor.note()) {
            self.notice = Some(self.tr.core_error(&error));
            self.pending_imports.push((note_id.to_string(), block, anchor));
        } else {
            self.full_text.remove(note_id);
        }
        None
    }

    pub(crate) fn block_by_id(&self, id: BlockId) -> Option<&Block> {
        self.editor.note().blocks.iter().find(|block| block.id() == id)
    }

    /// 把一个非文字块复制到软件内部的剪贴板。
    pub fn copy_block(&mut self, id: BlockId) -> bool {
        match self.block_by_id(id) {
            Some(block @ (Block::Image(_) | Block::Audio(_) | Block::Video(_) | Block::Divider { .. })) => {
                self.clip = Some(block.clone());
                self.clip_sequence = crate::win::clipboard_sequence();
                true
            }
            _ => false,
        }
    }

    /// 内部剪贴板里有一块，而且复制之后系统剪贴板没变过。
    pub fn clip_is_fresh(&self) -> bool {
        self.clip.is_some() && crate::win::clipboard_sequence() == self.clip_sequence
    }

    /// 没有任何输入框有焦点、选中的是一个图片或音视频块：这时复制、剪切作用于整块。
    pub fn block_clipboard_target(&self) -> bool {
        !self.current_in_trash
            && !self.other_text_focus
            && self.editor_ui.focused_block().is_none()
            && self
                .selected_block
                .is_some_and(|id| self.block_by_id(id).is_some_and(|block| block.as_text().is_none()))
    }

    /// 每帧开头：把上一帧后半段记下的焦点情况接上。搜索框的焦点在顶栏画的时候再并进来。
    pub fn begin_frame_focus(&mut self) {
        self.other_text_focus = std::mem::take(&mut self.late_text_focus);
    }

    /// 把一个非文字块剪切到软件内部的剪贴板。
    pub fn cut_block(&mut self, id: BlockId) {
        if self.current_in_trash || !self.copy_block(id) {
            return;
        }
        self.editor.remove_block(id);
        self.selected_block = None;
        self.note_text_changed();
    }

    /// 把软件内部剪贴板上的那一块粘贴进来。粘贴在选中的块之后，没有选中就追加到末尾。
    /// 返回新块的标识；内部剪贴板是空的、或者正在回收站里时返回 None。
    pub fn paste_block(&mut self) -> Option<BlockId> {
        if self.current_in_trash {
            return None;
        }
        let block = self.clip.as_ref()?.duplicated()?;
        let anchor = self.selected_block.or_else(|| self.insert_anchor());
        self.insert_block(block, anchor)
    }

    pub fn note_text_changed(&mut self) {
        self.last_edit = Some(Instant::now());
    }

    pub(crate) fn apply_audio_rename(&mut self, block: BlockId, stem: &str) -> bool {
        if self.current.is_none() || self.current_in_trash {
            return true;
        }
        match self.editor.rename_audio(block, stem) {
            Ok(changed) => {
                if changed {
                    self.note_text_changed();
                    self.notice = Some(self.tr.audio_renamed().into());
                }
                true
            }
            Err(_) => false,
        }
    }

    pub(crate) fn finish_audio_rename(&mut self) {
        if let Some((block, stem)) = self.editor_ui.take_audio_name() {
            if !self.apply_audio_rename(block, &stem) {
                self.notice = Some(self.tr.picker_bad_file_name().into());
            }
        }
    }
}
