//! 界面场景测试。全程无窗口：不创建窗口、不碰键鼠，断言的是真实界面代码跑出来的状态。

#![cfg(test)]

use crate::app::MossPocket;
use crate::headless::Harness;
use crate::i18n::{Lang, Tr};
use crate::settings::{Backdrop, Language, ThemeMode};
use crate::settings_ui::{SettingsForm, SettingsPage};
use eframe::egui;
use mp_core::model::Block;
use mp_core::vault::{UnlockSecret, Vault};
use std::time::Duration;

fn ctrl(h: &mut Harness, key: egui::Key) {
    h.press(key, egui::Modifiers::CTRL);
}

fn enter(h: &mut Harness) {
    h.press(egui::Key::Enter, egui::Modifiers::NONE);
}

/// 当前笔记的纯文本。
fn text_of(h: &Harness) -> String {
    h.app.session().expect("应处于已解锁状态").editor.note().plain_text()
}

fn block_count(h: &Harness) -> usize {
    h.app.session().unwrap().editor.note().blocks.len()
}

/// 当前笔记的块数；还没解锁时返回 None。
fn blocks_of(app: &MossPocket) -> Option<usize> {
    app.session().map(|session| session.editor.note().blocks.len())
}

fn bold_spans(h: &Harness) -> usize {
    h.app
        .session()
        .unwrap()
        .editor
        .note()
        .blocks
        .iter()
        .filter_map(Block::as_text)
        .map(|text| text.spans.iter().filter(|span| span.style.bold).count())
        .sum()
}

#[test]
fn writing_layout_keeps_primary_fields_in_the_window() {
    use mp_core::model::{Note, TextBlock, TextKind};

    let mut h = Harness::new("moss-brand");
    h.app.settings_mut().language = Language::Zh;
    h.app.settings_mut().theme_mode = ThemeMode::Light;
    h.app.settings_changed(&h.ctx);
    let (mut vault, _) = Vault::create(&Vault::data_dir(&h.dir), "archive-test").unwrap();
    let archive = [
        ("雨后的标本", "夜行", true),
        ("留白练习", "阅读", false),
        ("冬天的光", "观察", false),
        ("纸上的城市", "手记", false),
    ];
    for (index, (title, tag, pinned)) in archive.into_iter().enumerate() {
        let mut note = Note::new().unwrap();
        note.blocks[0].as_text_mut().unwrap().text = title.into();
        note.tags = vec![tag.into()];
        note.pinned = pinned;
        note.updated_at = note.updated_at.saturating_sub(index as u64 * 1_200_000);
        if pinned {
            for (kind, text) in [
                (TextKind::Paragraph, "雨停后沿墙走了一段。砖缝里的苔藓存住了一点水，俯身看时，像一片极小的树林。"),
                (TextKind::Heading2, "观察"),
                (TextKind::Bullet, "蒴柄向外弯，顶端浅褐，叶尖比根部更亮。"),
                (TextKind::Bullet, "先拍一张远景，再记录近处的纹理。"),
                (TextKind::Quote, "下次带一支铅笔。"),
            ] {
                let mut block = TextBlock::new_empty().unwrap();
                block.kind = kind;
                block.text = text.into();
                note.blocks.push(Block::Text(block));
            }
        }
        vault.save_note(&note).unwrap();
    }
    drop(vault);
    h.app.refresh_vault();
    h.snapshot("01-unlock-light");

    h.resize(egui::vec2(720.0, 480.0));
    h.snapshot("02-unlock-compact");
    let password = h.ctx.read_response(egui::Id::unique("lock-secret")).unwrap().rect;
    assert!(password.left() > 0.0 && password.right() <= 720.0 && password.bottom() < 480.0);
    assert!(h.unlock("archive-test"));
    assert_eq!(h.app.session().unwrap().editor.note().title(), "雨后的标本");
    h.release_focus();
    h.snapshot("03-editor-compact");
    let note = h.app.session().unwrap().editor.note();
    let title = h.ctx.read_response(egui::Id::unique(("block", note.blocks[0].id()))).unwrap().rect;
    let text = h.ctx.read_response(egui::Id::unique(("block", note.blocks[1].id()))).unwrap().rect;
    let search = h.ctx.read_response(egui::Id::unique("search-box")).unwrap().rect;
    assert!((title.left() - text.left()).abs() < 1.0, "标题和正文应使用同一条左对齐线");
    assert!(search.right() < title.left() && text.right() <= 720.0, "笔记列表与正文应完整分列");

    h.resize(egui::vec2(1120.0, 740.0));
    h.snapshot("04-editor-light");
    h.app.settings_mut().theme_mode = ThemeMode::Dark;
    h.app.settings_changed(&h.ctx);
    h.snapshot("05-editor-dark");
}

#[test]
fn idle_timeout_renders_the_lock_screen_without_additional_input() {
    let mut h = unlocked_with_new_note("idle-lock");
    h.text("Hide this when the timer expires");
    h.app.settings_open = true;
    h.app.settings_mut().auto_lock_minutes = 1;
    h.frames(2);
    let password = egui::Id::unique("lock-secret");
    assert!(h.ctx.read_response(password).is_none());

    h.app.set_idle_time(Duration::from_secs(61));
    h.step(Vec::new());
    assert!(!h.app.is_unlocked());
    assert!(!h.app.settings_open);
    assert!(h.ctx.read_response(password).is_some(), "the timeout frame must paint the lock screen");
    h.snapshot("01-auto-locked");
}

#[test]
fn settings_keeps_shortcuts_out_of_the_notebook_until_closed() {
    let mut h = unlocked_with_new_note("settings-shortcuts");
    h.text("Keep this note");
    let original = h.app.session().unwrap().current.clone();
    h.app.settings_open = true;
    h.frames(2);
    ctrl(&mut h, egui::Key::N);
    ctrl(&mut h, egui::Key::H);
    assert_eq!(h.app.session().unwrap().current, original);
    assert_eq!(text_of(&h), "Keep this note");
    assert!(!h.app.session().unwrap().find.open);
    h.snapshot("01-settings-workspace");

    h.press(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!h.app.settings_open);
    ctrl(&mut h, egui::Key::N);
    assert_ne!(h.app.session().unwrap().current, original);
    h.snapshot("02-shortcuts-restored");
}

#[test]
fn trash_has_its_own_search_and_selection() {
    let mut h = unlocked_with_new_note("trash-navigation");
    h.text("Main note");
    let session = h.app.session_mut().unwrap();
    let original = session.current.clone();
    session.view.search = "Main".into();
    let mut deleted = mp_core::model::Note::new().unwrap();
    deleted.blocks[0].as_text_mut().unwrap().text = "Restore this note".into();
    deleted.deleted_at = Some(mp_core::util::now_millis());
    session.vault.save_note(&deleted).unwrap();

    session.toggle_trash_view();
    assert!(session.view.show_trash);
    assert!(session.view.search.is_empty());
    assert!(session.current.is_none());
    h.snapshot("01-trash-no-selection");

    let session = h.app.session_mut().unwrap();
    session.view.search = "Restore".into();
    session.open(&deleted.id);
    assert!(session.current_in_trash);
    h.snapshot("02-trash-read-only");
    let session = h.app.session_mut().unwrap();
    session.restore(&deleted.id);
    assert!(session.current.is_none());
    assert!(session.visible_summaries().is_empty());
    session.toggle_trash_view();
    assert_eq!(session.view.search, "Main");
    assert_eq!(session.current, original);
    session.toggle_trash_view();
    assert_eq!(session.view.search, "Restore");
    assert!(session.current.is_none());
    h.snapshot("03-trash-restored");
}

#[test]
fn first_run_creates_vault_shows_recovery_key_and_enters() {
    let mut h = Harness::new("first-run");
    assert!(!h.app.is_unlocked());
    h.snapshot("01-setup");

    h.text(" test1234 ");
    enter(&mut h);
    h.text(" test1234 ");
    h.snapshot("02-filled");
    enter(&mut h);

    assert!(h.wait_until(40.0, |app| app.vault_ready_to_enter()), "建库没在时限内完成");
    let recovery = h.app.lock_ui().and_then(|lock| lock.recovery.clone()).expect("应显示恢复密钥");
    assert_eq!(recovery.len(), 31, "恢复密钥的排版长度不对：{recovery}");
    h.snapshot("03-recovery");

    // 焦点在「继续」按钮上，回车或空格都该能走下一步。
    enter(&mut h);
    if !h.app.is_unlocked() {
        h.press(egui::Key::Space, egui::Modifiers::NONE);
    }
    assert!(h.app.is_unlocked(), "没能进入主界面");
    h.snapshot("04-main");

    // 磁盘上确实有了库，而且密码和恢复密钥都能解开、错的密码解不开。
    let root = Vault::data_dir(&h.dir);
    assert!(root.join("vault.json").exists());
    assert!(root.join("index.enc").exists());
    assert!(Vault::unlock(&root, UnlockSecret::Password(" test1234 ")).is_ok());
    assert!(Vault::unlock(&root, UnlockSecret::RecoveryKey(&recovery.to_lowercase())).is_ok());
    assert!(Vault::unlock(&root, UnlockSecret::Password("wrong-pass")).is_err());
    ctrl(&mut h, egui::Key::L);
    assert!(!h.app.is_unlocked());
    assert!(h.unlock(" test1234 "), "创建后重新锁定应打开解锁页，保留密码的空格");
}

#[test]
fn short_or_mismatched_password_is_refused_before_anything_is_written() {
    let mut h = Harness::new("setup-validation");
    h.text("123");
    enter(&mut h);
    h.text("123");
    enter(&mut h);
    h.frames(5);
    let error = h.app.lock_ui().unwrap().error.clone().unwrap_or_default();
    assert!(error.contains("6 位") || error.contains("6 characters"), "{error}");

    // 两个框现在内容不同，再提交应当提示不一致。
    ctrl(&mut h, egui::Key::A);
    h.text("456789");
    enter(&mut h);
    h.frames(3);
    let lock = h.app.lock_ui().unwrap();
    assert_ne!(lock.secret, lock.confirm);
    assert!(lock.error.is_some());
    assert!(!h.app.vault_ready_to_enter());
    assert!(!Vault::data_dir(&h.dir).exists(), "校验没过就不该建库");
}

#[test]
fn wrong_password_is_reported_then_the_right_one_unlocks() {
    let mut h = Harness::new("unlock");
    h.create_vault("right-pass");
    h.snapshot("01-lock");

    h.text("wrong-pass");
    enter(&mut h);
    assert!(
        h.wait_until(40.0, |app| app.lock_ui().is_some_and(|lock| lock.error.is_some())),
        "错密码没有报错"
    );
    let error = h.app.lock_ui().unwrap().error.clone().unwrap_or_default();
    assert!(error.contains("密码不正确") || error.contains("Wrong password"), "{error}");
    assert!(!h.app.is_unlocked());
    h.snapshot("02-wrong");

    // 全选旧内容重输。
    ctrl(&mut h, egui::Key::A);
    h.text("right-pass");
    enter(&mut h);
    assert!(h.wait_until(40.0, |app| app.is_unlocked()), "正确密码没能解锁");
    h.snapshot("03-main");
}

#[test]
fn new_note_focuses_and_preserves_body_without_a_title() {
    let mut h = Harness::new("body-first");
    h.create_vault("capture-pass");
    assert!(h.unlock("capture-pass"));
    ctrl(&mut h, egui::Key::N);
    h.frames(3);
    let session = h.app.session().unwrap();
    let body = session.editor.note().blocks[0].id();
    let note_id = session.editor.note().id.clone();
    assert!(session.editor.note().title_block().is_none());
    assert_eq!(session.editor_ui.focused_block(), Some(body));
    h.snapshot("01-body-ready");

    h.text("把散步时想到的那句话写下来。");
    enter(&mut h);
    h.text("写完再整理成文章。");
    ctrl(&mut h, egui::Key::L);
    assert!(h.unlock("capture-pass"));
    h.frames(2);

    let session = h.app.session_mut().unwrap();
    let note = session.editor.note();
    assert_eq!(note.id, note_id);
    assert_eq!(note.plain_text(), "把散步时想到的那句话写下来。\n写完再整理成文章。");
    assert!(note.title_block().is_none());
    assert_eq!(session.editor_ui.focused_block(), note.blocks.last().map(Block::id));
    assert_eq!(session.vault.summaries()[0].title, "把散步时想到的那句话写下来。");
    let exported = crate::export::collect(note, &session.vault, session.tr).unwrap();
    assert!(exported.iter().all(|block| matches!(block, crate::export::ExportBlock::Text { title: false, .. })));
    h.snapshot("02-body-reopened");

    let session = h.app.session_mut().unwrap();
    let title = session.editor.add_title().unwrap();
    assert!(session.editor.text_changed(title, "散步手记"));
    assert_eq!(session.editor.note().title(), "散步手记");
    assert!(session.editor.merge_with_previous(body).is_none());
    assert_eq!(session.editor.remove_block(title), Some(body));
    assert!(session.editor.note().title_block().is_none());
    assert_eq!(session.editor.note().title(), "把散步时想到的那句话写下来。");
    assert!(session.editor.undo());
    assert_eq!(session.editor.note().title_block().map(|block| block.id), Some(title));
    assert!(session.editor.redo());
    assert!(session.editor.note().title_block().is_none());
    assert!(session.save_current());
}

#[test]
fn typing_ime_splitting_merging_styling_undo_and_autosave() {
    let mut h = Harness::new("edit");
    h.create_vault("edit-pass");
    assert!(h.unlock("edit-pass"));

    ctrl(&mut h, egui::Key::N);
    h.frames(3);
    assert!(h.app.session().unwrap().current.is_some(), "Ctrl+N 没有新建笔记");
    assert_eq!(block_count(&h), 1);
    h.snapshot("01-new-note");

    h.text("First idea ");
    // 模拟中文输入法：先组字、再提交。
    h.ime_preedit("ni");
    h.ime_preedit("nihao");
    h.snapshot("02-ime-composing");
    h.ime_commit("你好");
    assert_eq!(text_of(&h), "First idea 你好");

    // 回车拆成两块，在第二块里打字。
    enter(&mut h);
    h.text("body lin");
    // 普通退格照常删掉光标前的一个字符，不该触发合并。
    h.press(egui::Key::Backspace, egui::Modifiers::NONE);
    assert_eq!(text_of(&h), "First idea 你好\nbody li");
    // 这一段输入要连着打完：中途不截图，才能和前面合并成同一步撤销。
    h.text("ne");
    assert_eq!(block_count(&h), 2);
    assert_eq!(text_of(&h), "First idea 你好\nbody line");
    h.snapshot("03-two-blocks");
    h.resize(egui::vec2(720.0, 480.0));
    h.snapshot("03b-compact-editor");
    let compact_blocks: Vec<_> = h.app.session().unwrap().editor.note().blocks.iter().map(Block::id).collect();
    let responses: Vec<_> = compact_blocks.iter().map(|id| {
        h.ctx.read_response(egui::Id::unique(("block", *id))).unwrap().rect
    }).collect();
    assert!(responses[0].bottom() < responses[1].top(), "正文块应垂直排列");
    assert!(responses.iter().all(|rect| rect.right() <= 720.0), "正文块必须在窗口内");
    h.resize(egui::vec2(1120.0, 740.0));

    // 光标移到第二块行首再按退格：并回第一块。
    h.press(egui::Key::Home, egui::Modifiers::NONE);
    h.press(egui::Key::Backspace, egui::Modifiers::NONE);
    assert_eq!(block_count(&h), 1, "行首退格没有并入上一块");
    assert_eq!(text_of(&h), "First idea 你好body line");
    h.snapshot("04-merged");

    // 全选本块再加粗。
    ctrl(&mut h, egui::Key::A);
    ctrl(&mut h, egui::Key::B);
    assert!(bold_spans(&h) > 0, "加粗没生效");
    h.snapshot("05-bold");

    // 撤销三步：加粗、合并、第二块的输入。
    ctrl(&mut h, egui::Key::Z);
    assert_eq!(bold_spans(&h), 0, "撤销加粗没生效");
    ctrl(&mut h, egui::Key::Z);
    assert_eq!(block_count(&h), 2, "撤销合并没生效");
    ctrl(&mut h, egui::Key::Z);
    assert_eq!(text_of(&h), "First idea 你好\n", "连着打的字应合并成一步撤销");
    ctrl(&mut h, egui::Key::Y);
    assert_eq!(text_of(&h), "First idea 你好\nbody line", "重做没生效");

    // 停手之后自动落盘；文件里不能有明文。
    std::thread::sleep(Duration::from_millis(1100));
    h.frames(3);
    let root = Vault::data_dir(&h.dir);
    // 只数正式文件；`.prev` 是上一版的备份，属正常存在。
    let notes: Vec<_> = std::fs::read_dir(root.join("notes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "note"))
        .collect();
    assert_eq!(notes.len(), 1, "应有且只有一条笔记");
    let bytes = std::fs::read(&notes[0]).unwrap();
    assert!(!bytes.windows(9).any(|window| window == b"body line"), "笔记文件里有明文");

    // 锁定之后内存里的会话应当释放。
    ctrl(&mut h, egui::Key::L);
    assert!(!h.app.is_unlocked());
    h.snapshot("06-locked");
}

#[test]
fn translucent_look_is_drawn_over_a_fake_desktop() {
    let mut h = Harness::with_backdrop("glass", Backdrop::Acrylic);
    h.app.settings_mut().theme_mode = ThemeMode::Light;
    h.app.reapply_settings(&h.ctx);
    h.create_vault("glass-pass");
    let lock = image::open(h.snapshot("01-lock")).unwrap().into_rgba8();
    let background = lock.get_pixel(30, 100);
    assert!(background.0[0] > 150 && background.0[0] < 246, "the light tint and test backdrop should both be visible");
    assert_eq!(background, lock.get_pixel(580, 735), "the lock form should share the window background");
    assert!(h.unlock("glass-pass"));
    ctrl(&mut h, egui::Key::N);
    h.text("半透明外观测试 Transparent look");
    enter(&mut h);
    h.text("第二段正文，用来检查中文字体和行距。");
    let editor = image::open(h.snapshot("02-main")).unwrap().into_rgba8();
    assert_eq!(editor.get_pixel(510, 285), editor.get_pixel(1657, 285), "the editor and its outer margin should share a background");
    assert_eq!(editor.get_pixel(307, 20), editor.get_pixel(307, 66), "the rail should extend behind the window controls without a top seam");
}

/// 造一段很短的单声道波形：16 位、每秒 8000 个采样。
pub(crate) fn wav_bytes(samples: usize) -> Vec<u8> {
    let data_bytes = samples * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&8000u32.to_le_bytes());
    out.extend_from_slice(&16000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for index in 0..samples {
        let sample = ((index as f32 / 8.0).sin() * 8000.0) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// 新建一条笔记并解锁，返回准备好接收媒体的测试台。
fn unlocked_with_new_note(tag: &str) -> Harness {
    let mut h = Harness::new(tag);
    h.create_vault("media-pass");
    assert!(h.unlock("media-pass"));
    ctrl(&mut h, egui::Key::N);
    h.frames(3);
    h
}

#[test]
fn failed_save_keeps_editor_open_until_lock_can_save() {
    let mut h = unlocked_with_new_note("lock-write-failure");
    h.text("keep this edit");
    let notes = Vault::data_dir(&h.dir).join("notes");
    std::fs::remove_dir(&notes).unwrap();
    std::fs::write(&notes, []).unwrap();
    ctrl(&mut h, egui::Key::L);
    assert!(h.app.is_unlocked());
    assert_eq!(text_of(&h), "keep this edit");

    std::fs::remove_file(&notes).unwrap();
    std::fs::create_dir(&notes).unwrap();
    ctrl(&mut h, egui::Key::L);
    assert!(!h.app.is_unlocked());
}

#[test]
fn pasted_image_becomes_an_encrypted_image_block() {
    let mut h = unlocked_with_new_note("image");
    h.text("标题");
    // 粘贴一张 2×2 的纯红图。
    let red = egui::Color32::from_rgb(255, 0, 0);
    let image = std::sync::Arc::new(egui::ColorImage::new([2, 2], vec![red; 4]));
    h.step(vec![egui::Event::PasteImage(image)]);
    assert!(h.wait_until(30.0, |app| blocks_of(app) == Some(2)), "粘贴的图片没有变成一块");

    let session = h.app.session().unwrap();
    let Block::Image(block) = &session.editor.note().blocks[1] else {
        panic!("第二块应当是图片");
    };
    assert_eq!((block.width, block.height), (2, 2));
    let blob_id = block.blob_id.clone();
    let raw = std::fs::read(Vault::data_dir(&h.dir).join("blobs").join(format!("{blob_id}.blob"))).unwrap();
    // 库里的文件是密文：找不到 PNG 的文件头。
    assert!(!raw.windows(4).any(|window| window == b"\x89PNG"), "媒体文件没有加密");

    // 解密回来确实是那张图。
    let vault = Vault::unlock(&Vault::data_dir(&h.dir), UnlockSecret::Password("media-pass")).unwrap();
    let bytes = vault.read_blob(&blob_id).unwrap();
    assert!(bytes.windows(4).any(|window| window == b"\x89PNG"));
    h.snapshot("02-image");
}

#[test]
fn dropped_files_become_blocks_and_unknown_files_are_refused() {
    let mut h = unlocked_with_new_note("drop");
    let root = h.data_root();
    let tone = wav_bytes(800); // 0.1 秒
    std::fs::write(root.join("tone.wav"), &tone).unwrap();
    std::fs::write(root.join("notes.txt"), b"hello").unwrap();
    h.drop_paths(vec![root.join("tone.wav"), root.join("notes.txt")]);

    assert!(h.wait_until(30.0, |app| blocks_of(app) == Some(2)), "拖入的音频没有变成一块");
    let session = h.app.session().unwrap();
    let Block::Audio(audio) = &session.editor.note().blocks[1] else {
        panic!("第二块应当是音频");
    };
    assert_eq!(audio.file_name, "tone.wav");
    assert_eq!(audio.size as usize, tone.len());
    let duration = audio.duration_secs.expect("这段音频应当读得出时长");
    assert!((duration - 0.1).abs() < 0.01, "时长不对：{duration}");

    // 不认识的文件不会变成块；过一会儿再看一眼，确认没有迟到的结果。
    std::thread::sleep(Duration::from_millis(500));
    h.frames(5);
    assert_eq!(block_count(&h), 2);
    h.snapshot("02-audio-block");
}

#[test]
fn audio_blob_decodes_through_the_real_decoder_without_making_sound() {
    // 不打开任何输出设备，所以不会出声。验证的是「边读边解密的读取器」能被真正的解码器当普通文件用。
    let dir = std::env::temp_dir().join(format!("moss-pocket-audio-{}", std::process::id()));
    let (vault, _) = Vault::create(&dir, "audio-pass").unwrap();
    let bytes = wav_bytes(8000); // 正好 1 秒
    let (blob_id, _) = vault.import_blob(&bytes[..]).unwrap();

    let duration = crate::audio::probe_duration(&vault.blob_handle(), &blob_id, "audio/wav", Tr(Lang::Zh)).unwrap();
    assert!((duration - 1.0).abs() < 0.01, "时长不对：{duration}");

    use rodio::Source as _;
    let decoder = rodio::Decoder::builder()
        .with_data(vault.open_blob(&blob_id).unwrap())
        .with_mime_type("audio/wav")
        .build()
        .unwrap();
    assert_eq!(decoder.channels().get(), 1);
    assert_eq!(decoder.sample_rate().get(), 8000);
    // 能一路读到结尾，而且采样数一个不少：说明跨块的定位和读取都没问题。
    assert_eq!(decoder.count(), 8000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn video_stream_server_serves_ranges_over_decrypted_bytes() {
    use std::io::{Read, Write};
    let dir = std::env::temp_dir().join(format!("moss-pocket-stream-{}", std::process::id()));
    let (vault, _) = Vault::create(&dir, "video-pass").unwrap();
    let data: Vec<u8> = (0..3000u32).map(|index| (index % 251) as u8).collect();
    let (blob_id, _) = vault.import_blob(&data[..]).unwrap();
    let server = crate::stream::StreamServer::start(vault.blob_handle()).unwrap();
    let url = server.url(&blob_id, "clip.mp4");
    assert!(url.starts_with("http://127.0.0.1:"), "只应监听本机回环地址：{url}");
    let (authority, path) = {
        let rest = url.strip_prefix("http://").unwrap();
        let (authority, path) = rest.split_once('/').unwrap();
        (authority.to_string(), format!("/{path}"))
    };

    let get = |target: &str, range: Option<&str>| -> (String, Vec<u8>) {
        let mut stream = std::net::TcpStream::connect(&authority).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let range_header = range.map(|value| format!("Range: {value}\r\n")).unwrap_or_default();
        let request = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n{range_header}\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap_or_else(|error| {
            panic!("read for range {range:?} failed after {} bytes: {error}", raw.len());
        });
        let split = raw.windows(4).position(|window| window == b"\r\n\r\n").expect("应答里应有空行");
        (String::from_utf8_lossy(&raw[..split]).to_string(), raw[split + 4..].to_vec())
    };

    // 整个文件。
    let (head, body) = get(&path, None);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("Content-Type: video/mp4"));
    assert!(head.contains("Accept-Ranges: bytes"));
    assert_eq!(body, data);
    // 播放器最常用的「从某处到结尾」。
    let (head, body) = get(&path, Some("bytes=1000-"));
    assert!(head.starts_with("HTTP/1.1 206"), "{head}");
    assert!(head.contains("Content-Range: bytes 1000-2999/3000"), "{head}");
    assert_eq!(body, data[1000..]);
    // 指定区间。
    let (head, body) = get(&path, Some("bytes=5-9"));
    assert!(head.contains("Content-Range: bytes 5-9/3000"), "{head}");
    assert_eq!(body, data[5..=9]);
    // 结尾越界时夹到最后一个字节。
    let (head, body) = get(&path, Some("bytes=2990-99999"));
    assert!(head.contains("Content-Range: bytes 2990-2999/3000"), "{head}");
    assert_eq!(body, data[2990..]);
    // 播放器可以取消旧的请求，立即从新的位置读取。
    let mut canceled = std::net::TcpStream::connect(&authority).unwrap();
    write!(canceled, "GET {path} HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-\r\n\r\n").unwrap();
    drop(canceled);
    let (head, body) = get(&path, Some("bytes=1200-1204"));
    assert!(head.contains("Content-Range: bytes 1200-1204/3000"), "{head}");
    assert_eq!(body, data[1200..=1204]);
    // 令牌不对：直接拒绝，不泄露任何内容。
    let (head, body) = get(&format!("/wrong-token/{blob_id}/clip.mp4"), None);
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(body.is_empty());

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn media_blocks_can_be_copied_cut_and_pasted() {
    let mut h = unlocked_with_new_note("blocks");
    h.text("标题");
    let image = std::sync::Arc::new(egui::ColorImage::new([3, 2], vec![egui::Color32::from_rgb(1, 2, 3); 6]));
    h.step(vec![egui::Event::PasteImage(image)]);
    assert!(h.wait_until(30.0, |app| blocks_of(app) == Some(2)));
    let image_id = match &h.app.session().unwrap().editor.note().blocks[1] {
        Block::Image(image) => image.id,
        _ => panic!("第二块应当是图片"),
    };

    // 点了图片、没有正文聚焦时，Ctrl+C 复制整块。粘贴走真实的按键：刚复制过一块、
    // 系统剪贴板没变过，粘的就是这一块，不会去读真实剪贴板。
    h.app.session_mut().unwrap().selected_block = Some(image_id);
    h.release_focus();
    ctrl(&mut h, egui::Key::C);
    assert!(h.app.session().unwrap().clip.is_some(), "Ctrl+C 没有复制这一块");
    ctrl(&mut h, egui::Key::V);
    h.frames(2);
    assert_eq!(block_count(&h), 3);
    let blobs: Vec<String> = h.app.session().unwrap().editor.note().blocks.iter().filter_map(Block::blob_id).map(str::to_string).collect();
    assert_eq!(blobs.len(), 2);
    assert_eq!(blobs[0], blobs[1], "复制出来的图片应和原图共用同一个文件");

    // 剪切：这一块消失；再粘贴，回到末尾。
    ctrl(&mut h, egui::Key::X);
    assert_eq!(block_count(&h), 2, "Ctrl+X 没有剪切这一块");
    ctrl(&mut h, egui::Key::V);
    h.frames(2);
    assert_eq!(block_count(&h), 3);
    // 撤销把最后一次粘贴撤掉。
    ctrl(&mut h, egui::Key::Z);
    assert_eq!(block_count(&h), 2);

    // 图片宽度设置会随笔记存盘。留在笔记里的是复制出来的那一块，和原图共用同一个文件。
    let note_id = h.app.session().unwrap().current.clone().unwrap();
    let remaining = h.app.session().unwrap().editor.note().blocks[1].id();
    assert_ne!(remaining, image_id, "原图已被剪切，留下的应是复制出来的那一块");
    h.app.session_mut().unwrap().editor.set_image_width(remaining, Some(123.0));
    std::thread::sleep(Duration::from_millis(1100));
    h.frames(5);
    let mut vault = Vault::unlock(&Vault::data_dir(&h.dir), UnlockSecret::Password("media-pass")).unwrap();
    let widths: Vec<Option<f32>> = vault
        .load_note(&note_id)
        .unwrap()
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Image(image) => Some(image.display_width),
            _ => None,
        })
        .collect();
    assert_eq!(widths, vec![Some(123.0)]);
    h.snapshot("02-copied");
}

#[test]
fn settings_window_applies_changes_and_saves_them() {
    let mut h = Harness::new("settings");
    h.create_vault("old-pass-1");
    assert!(h.unlock("old-pass-1"));
    ctrl(&mut h, egui::Key::N);
    h.text("设置测试 Settings test");
    h.app.settings_open = true;
    h.snapshot("01-open");
    assert_eq!(h.app.lang(), Lang::Zh);
    let title = egui::Id::unique(("block", h.app.session().unwrap().editor.note().blocks[0].id()));
    let original_height = h.ctx.read_response(title).unwrap().rect.height();
    let original_ui_font = egui::TextStyle::Body.resolve(&h.ctx.style_of(egui::Theme::Light));

    // 界面上的按钮和滑块最终调用的就是这条路径：改设置、即时生效、存盘。
    let ctx = h.ctx.clone();
    {
        let app = &mut h.app;
        app.settings_mut().theme_mode = ThemeMode::Light;
        app.settings_mut().base_font_size = 20.0;
        app.settings_mut().language = Language::En;
        app.settings_changed(&ctx);
    }
    h.frames(4);
    assert_eq!(h.app.lang(), Lang::En, "切换语言应即时生效");
    let updated = h.ctx.read_response(title).unwrap().rect;
    assert!(updated.height() > original_height);
    assert_eq!(egui::TextStyle::Body.resolve(&h.ctx.style_of(egui::Theme::Light)), original_ui_font);
    h.snapshot("02-english-light-large");
    h.resize(egui::vec2(720.0, 480.0));
    h.snapshot("03-compact-settings");

    // 设置写到了盘上，下次启动会读回来。
    let saved = std::fs::read_to_string(h.dir.join("MOSS-Pocket.settings.json")).unwrap();
    assert!(saved.contains("\"language\": \"en\""), "{saved}");
    assert!(saved.contains("\"base_font_size\": 20.0"), "{saved}");
}

#[test]
fn typography_uses_independent_families_and_loads_older_settings() {
    use crate::settings::Settings;

    let old: Settings = serde_json::from_str(r#"{"base_font_size":19.0}"#).unwrap();
    assert_eq!(old.heading_font, crate::fonts::DEFAULT);
    assert_eq!(old.body_font, crate::fonts::DEFAULT);
    assert_eq!(old.base_font_size, 19.0);

    let mut h = unlocked_with_new_note("typography");
    h.ctx.fonts_mut(|fonts| {
        let families = &fonts.definitions().families;
        for heading in [false, true] {
            let font = crate::fonts::note_font(24.0, heading, false);
            assert_eq!(families[&font.family], families[&egui::FontFamily::Proportional]);
        }
    });
    let width = |h: &Harness, heading| {
        h.ctx.fonts_mut(|fonts| fonts.layout_no_wrap(
            "WWWWiiii 0123".into(),
            crate::fonts::note_font(24.0, heading, false),
            egui::Color32::WHITE,
        ).size().x)
    };
    let initial_heading = width(&h, true);
    let initial_body = width(&h, false);
    let has_face = |key| crate::fonts::available_faces().iter().any(|face| face.key == key);
    let heading = if has_face("cambria") { "cambria" } else { crate::fonts::DEFAULT };
    let body = if has_face("consolas") { "consolas" } else { crate::fonts::DEFAULT };
    h.app.settings_mut().heading_font = heading.into();
    h.app.settings_mut().body_font = body.into();
    h.app.settings_changed(&h.ctx);
    h.frames(3);
    if heading != crate::fonts::DEFAULT {
        assert_ne!(width(&h, true), initial_heading);
    }
    if body != crate::fonts::DEFAULT {
        assert_ne!(width(&h, false), initial_body);
    }
    let saved = Settings::load(&h.dir);
    assert_eq!(saved.heading_font, heading);
    assert_eq!(saved.body_font, body);

    h.app.settings_mut().heading_font = "unavailable_font".into();
    h.app.settings_changed(&h.ctx);
    h.frames(3);
    assert_eq!(width(&h, true), initial_heading);
    assert_eq!(Settings::load(&h.dir).heading_font, "unavailable_font");
    h.app.settings_mut().heading_font = if has_face("kai_ti") { "kai_ti".into() } else { heading.into() };
    h.app.settings_changed(&h.ctx);
    h.app.settings_open = true;
    h.app.settings_form.section = SettingsPage::Typography;
    h.snapshot("settings-preview");
}

#[test]
fn password_and_recovery_key_can_be_changed_from_settings() {
    let mut h = Harness::new("settings-security");
    h.create_vault("old-pass-1");
    assert!(h.unlock("old-pass-1"));
    let mut form = SettingsForm::default();
    form.old_password = "wrong-old-1".into();
    form.new_password = "new-pass-2".into();
    form.confirm_password = "new-pass-2".into();
    assert!(crate::settings_actions::change_password(&mut h.app, &mut form).is_err(), "旧密码不对时不该改");

    form.old_password = "old-pass-1".into();
    form.confirm_password = "different-9".into();
    assert!(crate::settings_actions::change_password(&mut h.app, &mut form).is_err(), "两次新密码不一致时不该改");

    form.confirm_password = "new-pass-2".into();
    crate::settings_actions::change_password(&mut h.app, &mut form).unwrap();
    assert!(form.new_password.is_empty(), "改完之后表单应清空");

    crate::settings_actions::regenerate_key(&mut h.app, &mut form).unwrap();
    let key = form.fresh_key.clone().unwrap();
    assert_eq!(key.len(), 31);
    form.section = SettingsPage::Security;
    h.app.settings_form = form;
    h.app.settings_open = true;
    h.snapshot("01-new-key-shown");
    h.app.settings_open = false;

    // 锁定后走锁屏界面：旧密码被拒，新密码能进。
    ctrl(&mut h, egui::Key::L);
    h.frames(6);
    h.text("old-pass-1");
    enter(&mut h);
    assert!(
        h.wait_until(30.0, |app| app.lock_ui().is_some_and(|lock| lock.error.is_some())),
        "旧密码不该再能解锁"
    );
    h.snapshot("02-old-password-rejected");
    ctrl(&mut h, egui::Key::A);
    h.text("new-pass-2");
    enter(&mut h);
    assert!(h.wait_until(30.0, |app| app.is_unlocked()), "新密码应能解锁");

    let root = Vault::data_dir(&h.dir);
    assert!(Vault::unlock(&root, UnlockSecret::Password("old-pass-1")).is_err());
    assert!(Vault::unlock(&root, UnlockSecret::Password("new-pass-2")).is_ok());
    assert!(Vault::unlock(&root, UnlockSecret::RecoveryKey(&key)).is_ok());
}

#[test]
fn search_box_finds_notes_by_body_text() {
    let mut h = unlocked_with_new_note("search");
    h.text("水果清单");
    enter(&mut h);
    h.text("苹果 香蕉 橘子");
    ctrl(&mut h, egui::Key::N);
    h.text("车辆清单");
    enter(&mut h);
    h.text("汽车 引擎 轮胎");
    std::thread::sleep(Duration::from_millis(1100));
    h.frames(5);

    // 在搜索框里输入一个只出现在第一条正文里的词。
    h.ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("search-box")));
    h.frames(4);
    h.text("香蕉");
    h.frames(3);
    let hits = h.app.session_mut().unwrap().visible_summaries();
    assert_eq!(hits.len(), 1, "应只命中正文里有「香蕉」的那一条");
    assert!(hits[0].title.contains("水果"));
    h.snapshot("02-search");

    // 标题照旧能搜到。
    ctrl(&mut h, egui::Key::A);
    h.text("车辆");
    h.frames(3);
    let hits = h.app.session_mut().unwrap().visible_summaries();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("车辆"));
}

/// 查找栏当前停在第几处、一共几处。
fn find_state(h: &mut Harness) -> (usize, usize) {
    let session = h.app.session_mut().unwrap();
    let revision = session.editor.revision();
    let marks = session.find.marks(session.editor.note(), revision);
    (session.find.current, marks.iter().map(|(_, list)| list.len()).sum())
}

#[test]
fn find_bar_finds_steps_and_replaces_in_the_note() {
    let mut h = unlocked_with_new_note("find");
    h.text("猫的日常");
    enter(&mut h);
    h.text("猫在沙发上，狗在睡觉");
    enter(&mut h);
    h.text("cat Cat 猫");
    h.frames(2);

    // 光标在末尾时按 Ctrl+F：查找栏打开，焦点进了查找框。
    ctrl(&mut h, egui::Key::F);
    h.frames(4);
    assert!(h.app.session().unwrap().find.open, "Ctrl+F 没有打开查找栏");
    assert!(h.app.session().unwrap().other_text_focus, "焦点应在查找框里");
    h.text("猫");
    h.frames(2);
    // 从光标处往后已经没有，绕回第一处（标题）。
    assert_eq!(find_state(&mut h), (0, 3));
    h.snapshot("01-find-hits");

    // 回车下一处，Shift+回车上一处，F3 也是下一处。
    enter(&mut h);
    assert_eq!(find_state(&mut h).0, 1);
    h.press(egui::Key::Enter, egui::Modifiers::SHIFT);
    assert_eq!(find_state(&mut h).0, 0);
    h.press(egui::Key::F3, egui::Modifiers::NONE);
    assert_eq!(find_state(&mut h).0, 1);

    // 在查找框里换查找内容：英文默认不区分大小写，区分之后只剩小写的那一个。
    ctrl(&mut h, egui::Key::A);
    h.text("cat");
    h.frames(2);
    assert_eq!(find_state(&mut h), (0, 2));
    h.app.session_mut().unwrap().find.match_case = true;
    h.app.session_mut().unwrap().find.conditions_changed();
    h.frames(2);
    assert_eq!(find_state(&mut h), (0, 1));
    h.app.session_mut().unwrap().find.match_case = false;
    h.app.session_mut().unwrap().find.conditions_changed();
    h.frames(2);

    // Ctrl+H 展开替换那一行。把焦点挪进替换框再输入。
    ctrl(&mut h, egui::Key::H);
    h.frames(3);
    assert!(h.app.session().unwrap().find.replace, "Ctrl+H 没有展开替换栏");
    h.ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("find-replacement")));
    h.frames(4);
    h.text("狗");
    h.frames(2);
    h.snapshot("02-find-replace");

    // 在替换框里按回车：只换当前这一处，然后停到下一处。
    enter(&mut h);
    h.frames(2);
    assert_eq!(text_of(&h), "猫的日常\n猫在沙发上，狗在睡觉\n狗 Cat 猫");
    assert_eq!(find_state(&mut h), (0, 1));

    // 「全部替换」按钮走的就是这条路：剩下的全部换掉。
    assert_eq!(crate::find_bar::replace_everywhere(h.app.session_mut().unwrap()), 1);
    h.frames(2);
    assert_eq!(text_of(&h), "猫的日常\n猫在沙发上，狗在睡觉\n狗 狗 猫");
    assert_eq!(find_state(&mut h).1, 0);

    // Esc 关栏，焦点回到正文。两次替换各占一步撤销。
    h.press(egui::Key::Escape, egui::Modifiers::NONE);
    h.frames(4);
    assert!(!h.app.session().unwrap().find.open, "Esc 没有关掉查找栏");
    assert!(h.app.session().unwrap().editor_ui.focused_block().is_some(), "关栏后焦点应回到正文");
    ctrl(&mut h, egui::Key::Z);
    assert_eq!(text_of(&h), "猫的日常\n猫在沙发上，狗在睡觉\n狗 Cat 猫");
    ctrl(&mut h, egui::Key::Z);
    assert_eq!(text_of(&h), "猫的日常\n猫在沙发上，狗在睡觉\ncat Cat 猫");

    // 正文里选中了文字再按 Ctrl+F：选中的内容直接成为查找内容，第一处就是它。
    ctrl(&mut h, egui::Key::A);
    h.frames(2);
    ctrl(&mut h, egui::Key::F);
    h.frames(4);
    assert_eq!(h.app.session().unwrap().find.query, "cat Cat 猫");
    assert_eq!(find_state(&mut h), (0, 1));
    h.snapshot("03-find-selection");
}

#[test]
fn keys_pressed_in_the_side_boxes_do_not_touch_the_note() {
    let mut h = unlocked_with_new_note("side-focus");
    h.text("标题");
    let image = std::sync::Arc::new(egui::ColorImage::new([3, 2], vec![egui::Color32::from_rgb(1, 2, 3); 6]));
    h.step(vec![egui::Event::PasteImage(image)]);
    assert!(h.wait_until(30.0, |app| blocks_of(app) == Some(2)));
    let image_id = match &h.app.session().unwrap().editor.note().blocks[1] {
        Block::Image(image) => image.id,
        _ => panic!("第二块应当是图片"),
    };

    // 选中着图片块（剪切会作用于它），同时焦点在左侧的搜索框里。测试不碰真实剪贴板。
    h.app.session_mut().unwrap().selected_block = Some(image_id);
    h.ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("search-box")));
    h.frames(4);
    crate::clipboard::fake(Some(crate::clipboard::ClipboardContent::Text("粘贴的字".into())));

    // 粘贴：文字只进搜索框，正文不多出段落。
    ctrl(&mut h, egui::Key::V);
    h.frames(3);
    assert_eq!(h.app.session().unwrap().view.search, "粘贴的字");
    assert_eq!(block_count(&h), 2, "在搜索框里粘贴不应往正文里加段落");

    // 全选再剪切：剪的是搜索框里的字，选中的图片块必须还在。
    ctrl(&mut h, egui::Key::A);
    ctrl(&mut h, egui::Key::X);
    h.frames(3);
    assert_eq!(h.app.session().unwrap().view.search, "", "搜索框里的字没有被剪掉");
    assert_eq!(block_count(&h), 2, "在搜索框里剪切不应剪掉选中的图片块");
    assert!(h.app.session().unwrap().clip.is_none(), "内部剪贴板不应被搜索框里的剪切动到");
    crate::clipboard::fake(None);
}

#[test]
fn pasted_text_goes_where_the_focus_is() {
    let mut h = unlocked_with_new_note("paste-text");
    crate::clipboard::fake(Some(crate::clipboard::ClipboardContent::Text("剪贴板里的字".into())));

    // 焦点在正文里：插到光标处，不多出块。
    ctrl(&mut h, egui::Key::V);
    h.frames(2);
    assert_eq!(text_of(&h), "剪贴板里的字");
    assert_eq!(block_count(&h), 1);

    // 焦点在搜索框里：只进搜索框，正文不动。
    h.ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("search-box")));
    h.frames(4);
    ctrl(&mut h, egui::Key::V);
    h.frames(3);
    assert_eq!(h.app.session().unwrap().view.search, "剪贴板里的字");
    assert_eq!(text_of(&h), "剪贴板里的字");

    // 哪儿都没有焦点：追加一个段落。
    h.release_focus();
    ctrl(&mut h, egui::Key::V);
    h.frames(2);
    assert_eq!(text_of(&h), "剪贴板里的字\n剪贴板里的字");
    crate::clipboard::fake(None);
}

#[test]
fn pasting_files_from_the_file_manager_imports_them() {
    let mut h = unlocked_with_new_note("paste-files");
    let root = h.data_root();
    let tone = wav_bytes(800);
    std::fs::write(root.join("tone.wav"), &tone).unwrap();
    std::fs::write(root.join("notes.txt"), b"hello").unwrap();
    // 剪贴板里是从资源管理器复制的两个文件：认得的导入，不认识的拒绝。
    crate::clipboard::fake(Some(crate::clipboard::ClipboardContent::Files(vec![
        root.join("tone.wav"),
        root.join("notes.txt"),
    ])));
    ctrl(&mut h, egui::Key::V);
    assert!(h.wait_until(30.0, |app| blocks_of(app) == Some(2)), "粘贴的音频没有变成一块");
    let Block::Audio(audio) = &h.app.session().unwrap().editor.note().blocks[1] else {
        panic!("第二块应当是音频");
    };
    assert_eq!((audio.file_name.as_str(), audio.size as usize), ("tone.wav", tone.len()));
    std::thread::sleep(Duration::from_millis(500));
    h.frames(3);
    assert_eq!(block_count(&h), 2, "不认识的文件不应变成块");
    crate::clipboard::fake(None);
}

#[test]
fn password_box_accepts_paste_from_the_clipboard() {
    let mut h = Harness::new("paste-lock");
    h.create_vault("lock-pass");
    h.frames(3);
    crate::clipboard::fake(Some(crate::clipboard::ClipboardContent::Text("lock-pass".into())));
    h.text("wrong");
    ctrl(&mut h, egui::Key::A);
    ctrl(&mut h, egui::Key::V);
    enter(&mut h);
    assert!(h.wait_until(40.0, |app| app.is_unlocked()), "粘贴进密码框的密码没能解锁");
    crate::clipboard::fake(None);
}

#[test]
fn gif_webp_and_bmp_images_import_too() {
    let mut h = unlocked_with_new_note("formats");
    h.text("各种图片");
    let root = h.data_root();
    let pixels = vec![egui::Color32::from_rgb(10, 200, 30); 12];
    let rgba = image::RgbaImage::from_raw(4, 3, pixels.iter().flat_map(|pixel| pixel.to_srgba_unmultiplied()).collect()).unwrap();
    for (name, format) in [
        ("a.gif", image::ImageFormat::Gif),
        ("b.webp", image::ImageFormat::WebP),
        ("c.bmp", image::ImageFormat::Bmp),
    ] {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba.clone()).write_to(&mut bytes, format).unwrap();
        std::fs::write(root.join(name), bytes.into_inner()).unwrap();
    }
    h.drop_paths(vec![root.join("a.gif"), root.join("b.webp"), root.join("c.bmp")]);
    assert!(h.wait_until(30.0, |app| blocks_of(app) == Some(4)), "GIF/WebP/BMP 没有全部变成图片块");
    for block in h.app.session().unwrap().editor.note().blocks.iter().skip(1) {
        let Block::Image(image) = block else {
            panic!("应当是图片块");
        };
        assert_eq!((image.width, image.height), (4, 3));
    }
    h.snapshot("01-formats");
}
