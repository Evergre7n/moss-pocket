//! Layout and interaction tests for the desktop front end, driven through egui's rendered shapes.

use std::path::Path;
use std::time::Duration;

use crate::headless::Harness;
use crate::matrix::{choose, open_block_menu, select_settings, Dialog};
use crate::settings::{Backdrop, Language, Settings, ThemeMode};
use crate::settings_ui::SettingsPage;
use eframe::egui::{self, Id, Key, Modifiers, PointerButton, Rect, Theme};
use mp_core::media::MediaKind;
use mp_core::model::{Block, BlockId, MediaBlock, Note, TextBlock};
use mp_core::util::now_millis;
use mp_core::vault::{UnlockSecret, Vault};

const PASSWORD: &str = "frontend-qa";
const TOPIC: &str = "Field Notes";
const TEXT: &str = "Moss under the window. 苔藓藏在窗下。";
const SEARCH: &str = "search-box";
const THEMES: [(ThemeMode, Theme); 4] = [
    (ThemeMode::Light, Theme::Dark),
    (ThemeMode::Dark, Theme::Light),
    (ThemeMode::System, Theme::Light),
    (ThemeMode::System, Theme::Dark),
];
const BACKDROPS: [Backdrop; 5] = [
    Backdrop::Opaque,
    Backdrop::Transparent,
    Backdrop::Acrylic,
    Backdrop::Mica,
    Backdrop::SystemAcrylic,
];
const DISPLAYS: [(f32, f32, f32); 3] = [
    (720.0, 480.0, 1.0),
    (720.0, 480.0, 2.0),
    (1120.0, 740.0, 1.5),
];

#[derive(Clone, Copy, Debug)]
struct DisplayCase {
    language: Language,
    mode: ThemeMode,
    system: Theme,
    backdrop: Backdrop,
    width: f32,
    height: f32,
    dpi: f32,
}

impl DisplayCase {
    fn apply(self, h: &mut Harness) {
        let settings = h.app.settings_mut();
        settings.language = self.language;
        settings.theme_mode = self.mode;
        settings.backdrop = self.backdrop;
        h.system_theme = self.system;
        h.screen.pixels_per_point = self.dpi;
        h.app.settings_changed(&h.ctx);
        h.resize(egui::vec2(self.width, self.height));
        let expected = match self.mode {
            ThemeMode::Light => Theme::Light,
            ThemeMode::Dark => Theme::Dark,
            ThemeMode::System => self.system,
        };
        assert_eq!(h.ctx.theme(), expected, "{self:?}");
        assert_eq!(h.ctx.pixels_per_point(), self.dpi, "{self:?}");
    }

    fn evidence(self, h: &mut Harness, name: &str) {
        let compact_zh = self.language == Language::Zh && self.mode == ThemeMode::Light
            && self.backdrop == Backdrop::Mica && self.dpi == 1.0;
        let standard_en = self.language == Language::En && self.mode == ThemeMode::Dark
            && self.backdrop == Backdrop::Acrylic && self.dpi == 1.5;
        if compact_zh || standard_en {
            let fixture = if compact_zh { "zh-compact" } else { "en-standard" };
            h.frames(10);
            h.snapshot(&format!("{fixture}-{name}"));
        }
    }
}

fn cases() -> Vec<DisplayCase> {
    let mut cases = Vec::new();
    for language in [Language::Zh, Language::En] {
        for (mode, system) in THEMES {
            for backdrop in BACKDROPS {
                for (width, height, dpi) in DISPLAYS {
                    cases.push(DisplayCase { language, mode, system, backdrop, width, height, dpi });
                }
            }
        }
    }
    cases
}

fn media_cases() -> impl Iterator<Item = DisplayCase> {
    [Language::Zh, Language::En].into_iter().flat_map(|language| {
        [DISPLAYS[0], DISPLAYS[2]].into_iter().map(move |(width, height, dpi)| DisplayCase {
            language,
            mode: ThemeMode::System,
            system: Theme::Light,
            backdrop: Backdrop::Opaque,
            width,
            height,
            dpi,
        })
    })
}

fn focused(h: &Harness) -> Option<Id> {
    h.ctx.memory(|memory| memory.focused())
}

fn response(h: &Harness, id: Id) -> Rect {
    let rect = h.ctx.read_response(id).unwrap_or_else(|| panic!("missing widget {id:?}")).rect;
    assert!(h.ctx.content_rect().contains_rect(rect), "widget {id:?} is outside the screen: {rect:?}");
    rect
}

fn visible(h: &Harness, labels: &[&str]) {
    for label in labels {
        let rect = h.caption(label);
        assert!(h.ctx.content_rect().contains_rect(rect), "{label:?} outside the viewport: {rect:?}");
        assert!(h.caption_is_unclipped(label), "{label:?} clipped: {rect:?}");
    }
}

fn absent(h: &Harness, label: &str) {
    assert!(h.labels(label).next().is_none(), "unexpected {label:?}");
}

fn disjoint(h: &Harness, labels: &[&str], case: DisplayCase) {
    for (index, label) in labels.iter().enumerate() {
        for other in &labels[index + 1..] {
            assert!(!h.caption(label).intersects(h.caption(other)), "{label} overlaps {other}, {case:?}");
        }
    }
}

fn en(h: &mut Harness) {
    h.app.settings_mut().language = Language::En;
    h.app.settings_changed(&h.ctx);
}

fn sample(title: &str, text: &str) -> Note {
    let mut note = Note::new().unwrap();
    note.blocks[0].as_text_mut().unwrap().text = title.into();
    let mut body = TextBlock::new_empty().unwrap();
    body.text = text.into();
    note.blocks.push(Block::Text(body));
    note
}

fn open_fixture(tag: &str, seed: &[Note]) -> Harness {
    let mut h = Harness::new(tag);
    en(&mut h);
    let (mut vault, _) = Vault::create(&Vault::data_dir(&h.dir), PASSWORD).unwrap();
    for note in seed {
        vault.save_note(note).unwrap();
    }
    drop(vault);
    h.app.refresh_vault();
    assert!(h.unlock(PASSWORD));
    h.frames(4);
    h
}

fn sidebar_row(h: &Harness, label: &str) -> Rect {
    let search = response(h, Id::unique(SEARCH));
    h.labels(label).find(|rect| rect.right() <= search.right())
        .unwrap_or_else(|| panic!("row {label:?} is absent from the sidebar"))
}

fn picker_text(h: &mut Harness, field: &str, value: &str) {
    h.click_field(Id::unique(field));
    h.press(Key::A, Modifiers::CTRL);
    h.step(vec![egui::Event::Text(value.to_owned())]);
}

fn picker_directory(h: &mut Harness, path: &Path) {
    picker_text(h, "picker-location", path.to_str().unwrap());
    h.click(h.app.tr().picker_go());
    h.frames(3);
}

#[test]
fn auth_layout_matrix_draws_setup_password_and_recovery_inputs() {
    let mut setup = Harness::new("frontend-auth");
    let mut locked = Harness::new("frontend-unlock");
    locked.create_vault(PASSWORD);
    for case in cases() {
        case.apply(&mut setup);
        let tr = setup.app.tr();
        visible(&setup, &["Pocket", tr.setup_title(), tr.create_vault()]);
        let first = response(&setup, Id::unique("lock-password"));
        let second = response(&setup, Id::unique("lock-confirm"));
        assert!(first.bottom() < second.top(), "{case:?}");
        case.evidence(&mut setup, "setup");

        case.apply(&mut locked);
        visible(&locked, &["Pocket", tr.unlock_title(), tr.unlock(), tr.use_recovery_key()]);
        let password = response(&locked, Id::unique("lock-secret"));
        assert!(password.bottom() < locked.caption(tr.unlock()).top(), "{case:?}");
        case.evidence(&mut locked, "password");
        locked.click(tr.use_recovery_key());
        locked.frames(3);
        visible(&locked, &[tr.unlock_recovery_sub(), tr.unlock(), tr.use_password()]);
        assert_eq!(focused(&locked), Some(Id::unique("lock-secret")), "{case:?}");
        case.evidence(&mut locked, "recovery-input");
        locked.click(tr.use_password());
        locked.frames(3);
        assert!(!locked.app.lock_ui().unwrap().use_recovery);
    }
}

#[test]
fn workspace_matrix_draws_editor_find_popups_and_all_settings_tabs() {
    let seed = sample(TOPIC, TEXT);
    let body = seed.blocks[1].id();
    let body_widget = Id::unique(("block", body));
    let mut h = open_fixture("frontend-workspace", &[seed]);
    for case in cases() {
        case.apply(&mut h);
        let tr = h.app.tr();
        let toolbar = [tr.saved(), tr.find(), tr.export(), tr.record_audio(), tr.insert()];
        visible(&h, &toolbar);
        visible(&h, &[tr.new_note(), tr.settings(), tr.tags_hint()]);
        disjoint(&h, &toolbar, case);
        let search = response(&h, Id::unique(SEARCH));
        let writing = response(&h, body_widget);
        assert!(search.right() < writing.left(), "{case:?}");
        for label in toolbar {
            assert!(h.caption(label).bottom() < writing.top(), "{label}, {case:?}");
        }
        case.evidence(&mut h, "editor");

        h.press(Key::H, Modifiers::CTRL);
        h.frames(3);
        if h.app.session().unwrap().find.query.is_empty() {
            h.text("Moss");
        }
        visible(&h, &[
            tr.find_prev(), tr.find_next(), tr.find_replace(), tr.find_replace_all(), tr.close(), "1 / 1",
        ]);
        for labels in [
            &[tr.find_prev(), tr.find_next(), tr.close(), "1 / 1"][..],
            &[tr.find_replace(), tr.find_replace_all(), tr.close()][..],
        ] {
            disjoint(&h, labels, case);
        }
        let query = response(&h, Id::unique("find-query"));
        let replacement = response(&h, Id::unique("find-replacement"));
        assert!(query.bottom() < replacement.top(), "{case:?}");
        for label in [tr.record_audio(), tr.insert()] {
            assert!(h.caption(label).bottom() < query.top(), "{label}, {case:?}");
        }
        case.evidence(&mut h, "find-replace");
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(3);
        assert!(!h.app.session().unwrap().find.open);

        h.click(tr.export());
        h.frames(3);
        visible(&h, &["Markdown", tr.export_html(), tr.export_docx(), "PDF"]);
        case.evidence(&mut h, "export");
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(2);
        absent(&h, tr.export_docx());

        open_block_menu(&mut h, body);
        visible(&h, &[
            tr.paragraph(), tr.heading1(), tr.heading2(), tr.heading3(), tr.bullet(), tr.numbered(),
            tr.quote(), tr.move_up(), tr.move_down(), tr.duplicate(), tr.insert_divider(),
            tr.insert_media(), tr.delete_block(),
        ]);
        case.evidence(&mut h, "block-menu");
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(2);
        absent(&h, tr.delete_block());

        let row = sidebar_row(&h, TOPIC);
        h.click_at(row.center(), PointerButton::Secondary);
        h.frames(3);
        visible(&h, &[tr.move_to_trash(), "Markdown", tr.export_html(), tr.export_docx(), "PDF"]);
        case.evidence(&mut h, "note-menu");
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(2);
        absent(&h, tr.move_to_trash());

        h.click(tr.settings());
        h.frames(4);
        assert!(h.app.settings_open);
        visible(&h, &[tr.appearance(), tr.typography(), tr.language(), tr.security(), tr.close()]);
        for (page, label, shot) in [
            (SettingsPage::Appearance, tr.theme(), "appearance"),
            (SettingsPage::Typography, tr.heading_font(), "typography"),
            (SettingsPage::Language, "English", "language"),
            (SettingsPage::Security, tr.auto_lock(), "security"),
        ] {
            select_settings(&mut h, page);
            visible(&h, &[label]);
            case.evidence(&mut h, shot);
        }
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(3);
        assert!(!h.app.settings_open, "{case:?}");
        assert_eq!(h.app.session().unwrap().editor.note().blocks[1].as_text().unwrap().text, TEXT);
    }
}

#[test]
fn media_insert_menu_targets_the_body_and_is_hidden_in_trash() {
    let mut archived = sample("Archived media note", "Read-only paragraph");
    archived.deleted_at = Some(now_millis());
    let mut h = open_fixture("frontend-media-insert", &[archived]);
    let image = h.dir.join("picture.png");
    image::RgbImage::from_pixel(20, 12, image::Rgb([80, 150, 110])).save(&image).unwrap();
    let audio = h.dir.join("tone.wav");
    std::fs::write(&audio, crate::scenarios::wav_bytes(8_000)).unwrap();
    let video = h.dir.join("clip.mp4");
    std::fs::write(&video, b"video import fixture").unwrap();

    for case in media_cases() {
        case.apply(&mut h);
        let tr = h.app.tr();
        h.click(tr.new_note());
        h.frames(3);
        let body = h.app.session().unwrap().editor.note().blocks[0].id();
        let body_field = Id::unique(("block", body));
        assert_eq!(focused(&h), Some(body_field), "{case:?}");
        h.ime_commit("正文");
        h.text(" first");
        h.press(Key::End, Modifiers::CTRL);
        h.press(Key::Enter, Modifiers::NONE);
        h.text("Following paragraph");

        let toolbar = [tr.add_title(), tr.find(), tr.export(), tr.record_audio(), tr.insert()];
        visible(&h, &toolbar);
        disjoint(&h, &toolbar, case);
        let items = [tr.insert_image(), tr.insert_audio(), tr.insert_video()];
        h.click(tr.insert());
        h.frames(3);
        visible(&h, &items);
        disjoint(&h, &items, case);
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(3);
        for label in items {
            absent(&h, label);
        }

        for (index, (label, kind, path)) in [
            (tr.insert_image(), MediaKind::Image, &image),
            (tr.insert_audio(), MediaKind::Audio, &audio),
            (tr.insert_video(), MediaKind::Video, &video),
        ].into_iter().enumerate() {
            h.click_field(body_field);
            assert_eq!(focused(&h), Some(body_field), "{case:?}");
            h.click(tr.insert());
            h.frames(3);
            choose(&mut h, label, Dialog::Import, None);
            assert_eq!(h.app.session().unwrap().editor.note().blocks.len(), index + 2, "{case:?}");
            absent(&h, label);

            h.click_field(body_field);
            h.click(tr.insert());
            h.frames(3);
            choose(&mut h, label, Dialog::Import, Some(vec![path.clone()]));
            assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == index + 3),
                "{kind:?} was not imported, {case:?}");
            let note = h.app.session().unwrap().editor.note();
            let inserted = match (&note.blocks[1], kind) {
                (Block::Image(_), MediaKind::Image) => true,
                (Block::Audio(media), MediaKind::Audio) | (Block::Video(media), MediaKind::Video) => {
                    media.file_name == path.file_name().unwrap().to_str().unwrap()
                }
                _ => false,
            };
            assert!(inserted, "{kind:?} was not inserted after the focused paragraph, {case:?}");
            assert_eq!(note.blocks[0].as_text().unwrap().text, "正文 first");
            assert_eq!(note.blocks.last().unwrap().as_text().unwrap().text, "Following paragraph");
            assert!(note.can_add_title());
        }

        h.click(tr.trash());
        h.frames(4);
        let row = sidebar_row(&h, "Archived media note");
        h.click_at(row.center(), PointerButton::Primary);
        h.frames(3);
        visible(&h, &[tr.restore(), "Read-only paragraph"]);
        for label in toolbar.into_iter().chain(items) {
            absent(&h, label);
        }
        h.click(tr.notes());
        h.frames(4);
    }
}

#[test]
fn image_audio_and_video_open_themed_surfaces_and_return_to_the_same_note() {
    let mut h = open_fixture("frontend-media-viewer", &[sample(TOPIC, TEXT)]);
    let image = h.dir.join("picture.png");
    let audio = h.dir.join("quiet.wav");
    let video = h.dir.join("broken clip.mp4");
    let pixels = image::RgbImage::from_pixel(52, 36, image::Rgb([64, 178, 112]));
    pixels.save(&image).unwrap();
    let mut wav = crate::scenarios::wav_bytes(8_000);
    wav[44..].fill(0);
    std::fs::write(&audio, wav).unwrap();
    std::fs::write(&video, b"invalid video fixture").unwrap();
    let files = vec![image, audio, video];
    h.drop_paths(files.clone());
    assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 5));
    for file in files {
        std::fs::remove_file(file).unwrap();
    }
    let current = h.app.session().unwrap().current.clone();
    let text = h.app.session().unwrap().editor.note().plain_text();

    let viewer_cases = cases().into_iter().filter(|case| match case.language {
        Language::En => case.width == DISPLAYS[2].0,
        Language::Zh => case.width == DISPLAYS[0].0 && case.dpi == DISPLAYS[0].2,
        Language::System => false,
    });
    for case in viewer_cases {
        case.apply(&mut h);
        let tr = h.app.tr();
        for (entry, title, shot) in [
            (tr.view_image(), tr.insert_image(), "image-viewer"),
            (tr.open_audio_player(), tr.insert_audio(), "audio-player"),
            (tr.play_video(), tr.insert_video(), "video-error"),
        ] {
            h.click(entry);
            h.frames(3);
            let session = h.app.session().unwrap();
            assert!(session.editor_ui.overlay_open(), "{entry:?}, {case:?}");
            visible(&h, &[tr.close(), title]);
            if entry == tr.view_image() {
                visible(&h, &["52 × 36"]);
                assert_eq!(h.labels("52 × 36").count(), 2, "missing the viewer's image dimensions, {case:?}");
                absent(&h, tr.loading_image());
                absent(&h, tr.image_view_failed());
            } else if entry == tr.open_audio_player() {
                visible(&h, &[tr.play(), tr.stop(), "quiet.wav"]);
                assert_eq!(h.labels("quiet.wav").count(), 2, "missing the player file label, {case:?}");
                assert_eq!(h.labels("0:00 / 0:01").count(), 2, "missing the player duration, {case:?}");
            } else if entry == tr.play_video() {
                visible(&h, &[tr.video_playback_failed(), "broken clip.mp4"]);
            }
            case.evidence(&mut h, shot);

            h.press(Key::N, Modifiers::CTRL);
            h.press(Key::F, Modifiers::CTRL);
            h.ime_commit("background typing");
            let session = h.app.session().unwrap();
            assert_eq!(session.current, current, "{case:?}");
            assert!(!session.find.open, "{case:?}");
            assert_eq!(session.editor.note().plain_text(), text, "{case:?}");

            if entry == tr.view_image() {
                h.press(Key::Escape, Modifiers::NONE);
            } else {
                h.click(tr.close());
            }
            h.frames(3);
            assert!(!h.app.session().unwrap().editor_ui.overlay_open(), "{entry:?}, {case:?}");
            absent(&h, tr.close());
            visible(&h, &[tr.find()]);
        }
    }

    h.click(h.app.tr().view_image());
    h.frames(3);
    assert!(h.app.session().unwrap().editor_ui.overlay_open());
    h.press(Key::L, Modifiers::CTRL);
    assert!(!h.app.is_unlocked());
    assert!(h.unlock(PASSWORD));
    let session = h.app.session().unwrap();
    assert!(!session.editor_ui.overlay_open());
    assert_eq!(session.current, current);
    assert_eq!(session.editor.note().plain_text(), text);

    let tr = h.app.tr();
    let row = sidebar_row(&h, TOPIC);
    h.click_at(row.center(), PointerButton::Secondary);
    h.frames(3);
    h.click(tr.move_to_trash());
    h.frames(3);
    h.click(tr.trash());
    h.frames(3);
    let archived = sidebar_row(&h, TOPIC);
    h.click_at(archived.center(), PointerButton::Primary);
    h.frames(3);
    assert!(h.app.session().unwrap().current_in_trash);
    absent(&h, tr.insert());
    absent(&h, tr.record_audio());
    for entry in [tr.view_image(), tr.open_audio_player(), tr.play_video()] {
        h.click(entry);
        h.frames(3);
        assert!(h.app.session().unwrap().editor_ui.overlay_open(), "{entry:?}");
        h.click(tr.close());
        h.frames(3);
        assert!(!h.app.session().unwrap().editor_ui.overlay_open(), "{entry:?}");
    }
    let session = h.app.session().unwrap();
    assert_eq!(session.current, current);
    assert!(session.current_in_trash);
    assert_eq!(session.editor.note().plain_text(), text);
}

#[test]
fn themed_picker_filters_files_and_imports_multiple_selections_in_order() {
    let mut h = open_fixture("frontend-media-picker", &[]);
    h.app.settings_mut().language = Language::Zh;
    h.app.settings_mut().theme_mode = ThemeMode::Light;
    h.app.settings_mut().backdrop = Backdrop::Mica;
    h.app.settings_changed(&h.ctx);
    h.resize(egui::vec2(720.0, 480.0));
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    h.text("Active draft");
    let current = h.app.session().unwrap().current.clone();
    let files = h.dir.join("chosen files");
    std::fs::create_dir(&files).unwrap();
    let wav = crate::scenarios::wav_bytes(8_000);
    for name in ["first.wav", "second.wav"] {
        std::fs::write(files.join(name), &wav).unwrap();
    }
    std::fs::write(files.join("filtered.mp4"), b"video").unwrap();

    let open = |h: &mut Harness| {
        h.click(tr.insert());
        h.frames(2);
        h.click(tr.insert_audio());
        h.frames(3);
        visible(h, &[tr.picker_location(), tr.picker_open(), tr.picker_cancel()]);
        picker_directory(h, &files);
        assert!(h.wait_for_caption(5.0, "second.wav"));
        visible(h, &["first.wav", "second.wav"]);
        absent(h, "filtered.mp4");
    };

    open(&mut h);
    h.press(Key::N, Modifiers::CTRL);
    h.press(Key::F, Modifiers::CTRL);
    assert_eq!(h.app.session().unwrap().current, current);
    assert!(!h.app.session().unwrap().find.open);
    h.click("first.wav");
    h.click(tr.picker_cancel());
    h.frames(3);
    absent(&h, tr.picker_location());
    let session = h.app.session().unwrap();
    assert_eq!(session.editor.note().plain_text(), "Active draft");
    assert_eq!(session.editor.note().blocks.len(), 1);

    open(&mut h);
    h.click("second.wav");
    h.click("first.wav");
    h.click(tr.picker_open());
    assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 3));
    absent(&h, tr.picker_location());
    let session = h.app.session().unwrap();
    assert_eq!(session.editor.note().plain_text(), "Active draft");
    for (block, expected_name) in session.editor.note().blocks[1..].iter().zip(["second.wav", "first.wav"]) {
        let Block::Audio(media) = block else { panic!("expected an audio block") };
        assert_eq!(media.file_name, expected_name);
        assert_eq!(session.vault.read_blob(&media.blob_id).unwrap(), wav);
    }

    open(&mut h);
    h.click("second.wav");
    h.press(Key::L, Modifiers::CTRL);
    assert!(!h.app.is_unlocked());
    assert!(h.unlock(PASSWORD));
    absent(&h, tr.picker_location());
    assert_eq!(h.app.session().unwrap().editor.note().blocks.len(), 3);
}

#[test]
fn themed_media_save_picker_confirms_replacement_then_saves_original_bytes() {
    let mut h = open_fixture("frontend-media-save", &[sample(TOPIC, TEXT)]);
    h.app.settings_mut().theme_mode = ThemeMode::Dark;
    h.app.settings_mut().backdrop = Backdrop::Acrylic;
    h.app.settings_changed(&h.ctx);
    h.frames(3);
    let tr = h.app.tr();
    let input = h.dir.join("drawing.png");
    image::RgbImage::from_pixel(24, 16, image::Rgb([80, 150, 110])).save(&input).unwrap();
    let expected = std::fs::read(&input).unwrap();
    h.drop_paths(vec![input.clone()]);
    assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 3));
    std::fs::remove_file(input).unwrap();
    let id = h.app.session().unwrap().editor.note().blocks.iter()
        .find(|block| matches!(block, Block::Image(_))).unwrap().id();
    let output = h.dir.join("old drawing.png");
    std::fs::write(&output, b"original output").unwrap();

    open_block_menu(&mut h, id);
    h.click(tr.save_as());
    h.frames(3);
    visible(&h, &[tr.picker_location(), tr.picker_save(), tr.picker_cancel()]);
    picker_directory(&mut h, output.parent().unwrap());
    assert!(h.wait_for_caption(5.0, "old drawing.png"));
    let path = output.to_str().unwrap();
    picker_text(&mut h, "picker-filename", path);
    h.click(tr.picker_save());
    h.frames(3);
    visible(&h, &[tr.picker_replace(), tr.picker_keep_editing()]);
    assert_eq!(std::fs::read(&output).unwrap(), b"original output");
    h.click(tr.picker_keep_editing());
    h.frames(3);
    absent(&h, tr.picker_replace());
    h.click(tr.picker_save());
    h.frames(3);
    h.click(tr.picker_replace());
    h.frames(3);
    absent(&h, tr.picker_location());
    assert_eq!(std::fs::read(output).unwrap(), expected);
}

#[test]
fn recording_toolbar_matrix_shows_time_and_can_cancel_or_finish() {
    let mut h = open_fixture("frontend-recording-toolbar", &[]);
    let wav = crate::scenarios::wav_bytes(16_000);
    let elapsed = Duration::from_secs(65);
    for case in media_cases() {
        case.apply(&mut h);
        let tr = h.app.tr();
        h.click(tr.new_note());
        h.frames(3);
        let body = h.app.session().unwrap().editor.note().blocks[0].id();
        let body_field = Id::unique(("block", body));
        assert_eq!(focused(&h), Some(body_field), "{case:?}");
        h.text("Before recording");
        h.press(Key::End, Modifiers::CTRL);
        h.press(Key::Enter, Modifiers::NONE);
        h.text("Following paragraph");

        for action in [tr.cancel_recording(), tr.finish_recording()] {
            h.click_field(body_field);
            assert_eq!(focused(&h), Some(body_field), "{case:?}");
            let _ = h.app.session_mut().unwrap().start_synthetic_recording(wav.clone(), elapsed);
            h.frames(3);
            let controls = [tr.recording(), tr.finish_recording(), tr.cancel_recording(), "1:05"];
            visible(&h, &controls);
            disjoint(&h, &controls, case);
            absent(&h, tr.record_audio());
            let writing = response(&h, body_field);
            for label in controls {
                assert!(h.caption(label).bottom() < writing.top(), "{label}, {case:?}");
            }
            h.click(action);
            h.frames(3);
            if action == tr.cancel_recording() {
                let session = h.app.session().unwrap();
                assert_eq!(session.editor.note().blocks.len(), 2, "{case:?}");
                assert!(!session.media.has_background_work(), "{case:?}");
            } else {
                assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 3),
                    "the completed recording was not imported, {case:?}");
                let session = h.app.session().unwrap();
                let Block::Audio(audio) = &session.editor.note().blocks[1] else {
                    panic!("the recording was not inserted after the focused paragraph, {case:?}");
                };
                assert_eq!(session.vault.read_blob(&audio.blob_id).unwrap(), wav);
            }
            visible(&h, &[tr.record_audio(), tr.insert(), tr.add_title(), tr.find(), tr.export()]);
            for label in controls {
                absent(&h, label);
            }
            let note = h.app.session().unwrap().editor.note();
            assert_eq!(note.blocks[0].as_text().unwrap().text, "Before recording");
            assert_eq!(note.blocks.last().unwrap().as_text().unwrap().text, "Following paragraph");
            assert!(note.can_add_title());
        }
    }
}

fn audio_block(note: &Note, id: BlockId) -> &MediaBlock {
    let Some(Block::Audio(audio)) = note.blocks.iter().find(|block| block.id() == id) else {
        panic!("missing audio block {id}");
    };
    audio
}

fn begin_audio_rename(h: &mut Harness, id: BlockId, name: &str) -> Id {
    visible(h, &[name]);
    h.click(name);
    h.frames(3);
    let field = Id::unique(("audio-name-edit", id));
    response(h, field);
    assert_eq!(focused(h), Some(field), "clicking {name:?} should focus its rename field");
    field
}

#[test]
fn audio_rename_recording_matrix_supports_enter_escape_blur_validation_and_undo() {
    let mut h = open_fixture("frontend-audio-rename", &[]);
    let wav = crate::scenarios::wav_bytes(16_000);
    let rename_cases = media_cases().map(|mut case| {
        if case.language == Language::Zh && case.dpi == 1.0 {
            case.mode = ThemeMode::Light;
            case.backdrop = Backdrop::Mica;
        } else if case.language == Language::En && case.dpi == 1.5 {
            case.mode = ThemeMode::Dark;
            case.backdrop = Backdrop::Acrylic;
        }
        case
    });
    for case in rename_cases {
        case.apply(&mut h);
        let tr = h.app.tr();
        h.click(tr.new_note());
        h.frames(3);
        let body = h.app.session().unwrap().editor.note().blocks[0].id();
        let body_field = Id::unique(("block", body));
        assert_eq!(focused(&h), Some(body_field), "{case:?}");
        let (body_text, stem, final_stem) = if case.language == Language::Zh {
            ("在窗边录下一段雨声", "窗边雨声", "雨声片段")
        } else {
            ("Listen to the rain by the window", "Window rain", "Rain excerpt")
        };
        h.text(body_text);
        visible(&h, &[tr.record_audio()]);
        h.app.session_mut().unwrap().start_synthetic_recording(wav.clone(), Duration::from_secs(2));
        h.frames(3);
        h.click(tr.finish_recording());
        assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 2));
        let Block::Audio(original) = &h.app.session().unwrap().editor.note().blocks[1] else {
            panic!("recording should follow the body paragraph, {case:?}");
        };
        let original = original.clone();
        assert_eq!(original.file_name, tr.recording_file_name(), "{case:?}");
        visible(&h, &[tr.play(), tr.open_audio_player(), &original.file_name]);
        disjoint(&h, &[tr.play(), tr.open_audio_player(), &original.file_name], case);
        case.evidence(&mut h, "recording-original");

        let id = original.id;
        let field = begin_audio_rename(&mut h, id, &original.file_name);
        visible(&h, &[".wav"]);
        let stem_field = response(&h, field);
        let suffix = h.caption(".wav");
        assert!(stem_field.right() <= suffix.left() && stem_field.y_range().contains(suffix.center().y),
            "the extension should stay on the same line as the stem, {case:?}: {stem_field:?} {suffix:?}");
        if case.language == Language::Zh {
            h.ime_preedit("chuang");
            let note = h.app.session().unwrap().editor.note();
            assert_eq!(audio_block(note, id).file_name, original.file_name);
            h.ime_commit(stem);
        } else {
            h.text(stem);
        }
        case.evidence(&mut h, "recording-editing");
        h.press(Key::Enter, Modifiers::NONE);
        h.frames(3);
        let entered = format!("{stem}.wav");
        visible(&h, &[&entered]);
        assert!(h.ctx.read_response(field).is_none(), "Enter should end editing, {case:?}");

        begin_audio_rename(&mut h, id, &entered);
        h.text("Cancel me");
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(3);
        visible(&h, &[&entered]);
        assert!(h.ctx.read_response(field).is_none(), "Escape should end editing, {case:?}");

        for invalid in ["   ", "../outside"] {
            begin_audio_rename(&mut h, id, &entered);
            h.text(invalid);
            h.press(Key::Enter, Modifiers::NONE);
            h.frames(3);
            assert_eq!(focused(&h), Some(field), "{invalid:?}, {case:?}");
            visible(&h, &[tr.picker_bad_file_name()]);
            let note = h.app.session().unwrap().editor.note();
            assert_eq!(audio_block(note, id).file_name, entered, "{case:?}");
            h.press(Key::Escape, Modifiers::NONE);
            h.frames(3);
        }

        begin_audio_rename(&mut h, id, &entered);
        h.text(r"C:\outside");
        h.click_field(body_field);
        h.frames(3);
        visible(&h, &[&entered, tr.picker_bad_file_name()]);
        assert!(h.ctx.read_response(field).is_none(), "invalid blur should close the field, {case:?}");

        begin_audio_rename(&mut h, id, &entered);
        h.text(final_stem);
        h.click_field(body_field);
        h.frames(3);
        let renamed = format!("{final_stem}.wav");
        visible(&h, &[&renamed, tr.play(), tr.open_audio_player(), tr.audio_renamed()]);
        absent(&h, tr.picker_bad_file_name());
        assert!(h.ctx.read_response(field).is_none(), "blur should end editing, {case:?}");
        h.press(Key::Z, Modifiers::CTRL);
        h.frames(3);
        visible(&h, &[&entered]);
        h.press(Key::Y, Modifiers::CTRL);
        h.frames(3);
        visible(&h, &[&renamed]);
        case.evidence(&mut h, "recording-renamed");

        let session = h.app.session().unwrap();
        let note = session.editor.note();
        assert_eq!(note.plain_text(), body_text, "{case:?}");
        assert!(note.can_add_title(), "{case:?}");
        assert_eq!(note.blocks.len(), 2, "{case:?}");
        assert_eq!(*audio_block(note, id), MediaBlock { file_name: renamed, ..original });
        assert_eq!(session.vault.read_blob(&audio_block(note, id).blob_id).unwrap(), wav);
    }
}

#[test]
fn audio_rename_existing_note_persists_exports_and_keeps_trash_read_only() {
    let seed = sample(TOPIC, TEXT);
    let note_id = seed.id.clone();
    let mut h = open_fixture("frontend-audio-rename-existing", &[seed]);
    let tr = h.app.tr();
    let wav = crate::scenarios::wav_bytes(8_000);
    let input = h.dir.join("older recording.WAV");
    std::fs::write(&input, &wav).unwrap();
    h.drop_paths(vec![input.clone()]);
    assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 3));
    std::fs::remove_file(input).unwrap();
    h.press(Key::L, Modifiers::CTRL);
    assert!(!h.app.is_unlocked());
    assert!(h.unlock(PASSWORD));
    h.frames(3);
    let session = h.app.session().unwrap();
    assert_eq!(session.current.as_deref(), Some(note_id.as_str()));
    let original = session.editor.note().blocks.iter().find_map(|block| match block {
        Block::Audio(audio) => Some(audio.clone()),
        _ => None,
    }).expect("the saved note should have its original audio block");
    let id = original.id;
    let blob_path = Vault::data_dir(&h.dir).join("blobs").join(format!("{}.blob", original.blob_id));
    let ciphertext = std::fs::read(&blob_path).unwrap();
    assert_eq!(original.file_name, "older recording.WAV");
    let field = begin_audio_rename(&mut h, id, &original.file_name);
    visible(&h, &[".WAV"]);
    h.text("Field memo.wav");
    h.press(Key::Enter, Modifiers::NONE);
    h.frames(3);
    let renamed = "Field memo.WAV";
    visible(&h, &[renamed]);

    h.press(Key::L, Modifiers::CTRL);
    assert!(!h.app.is_unlocked());
    assert!(h.unlock(PASSWORD));
    h.frames(3);
    let session = h.app.session().unwrap();
    assert_eq!(session.current.as_deref(), Some(note_id.as_str()));
    assert_eq!(*audio_block(session.editor.note(), id), MediaBlock { file_name: renamed.into(), ..original });
    assert_eq!(std::fs::read(blob_path).unwrap(), ciphertext);
    visible(&h, &[renamed]);
    absent(&h, "older recording.WAV");

    let output = h.dir.join("renamed output");
    std::fs::create_dir(&output).unwrap();
    open_block_menu(&mut h, id);
    h.click(tr.save_as());
    h.frames(3);
    let filename_field = response(&h, Id::unique("picker-filename"));
    assert!(h.labels(renamed).any(|rect| filename_field.contains_rect(rect)), "Save as should suggest the renamed file");
    picker_directory(&mut h, &output);
    assert!(h.wait_for_caption(5.0, tr.picker_empty()));
    h.click(tr.picker_save());
    h.frames(3);
    assert_eq!(std::fs::read(output.join(renamed)).unwrap(), wav);

    h.click(tr.export());
    h.frames(3);
    choose(&mut h, "Markdown", Dialog::Export("md"), Some(vec![output.clone()]));
    visible(&h, &[tr.export_done()]);
    let markdown = std::fs::read_to_string(output.join(format!("{TOPIC}.md"))).unwrap();
    assert!(markdown.contains(renamed) && markdown.contains("Field%20memo.WAV"), "{markdown}");
    assert_eq!(std::fs::read(output.join(format!("{TOPIC}_files")).join(renamed)).unwrap(), wav);

    let row = sidebar_row(&h, TOPIC);
    h.click_at(row.center(), PointerButton::Secondary);
    h.frames(3);
    h.click(tr.move_to_trash());
    h.frames(3);
    h.click(tr.trash());
    h.frames(3);
    let row = sidebar_row(&h, TOPIC);
    h.click_at(row.center(), PointerButton::Primary);
    h.frames(3);
    assert!(h.app.session().unwrap().current_in_trash);
    visible(&h, &[renamed, tr.play(), tr.open_audio_player()]);
    h.click(renamed);
    h.frames(3);
    assert!(h.ctx.read_response(field).is_none(), "trash should not open an audio rename field");
    assert_eq!(audio_block(h.app.session().unwrap().editor.note(), id).file_name, renamed);
    h.snapshot("en-trash-read-only");
}

#[test]
#[ignore = "requires an available Windows audio output endpoint; plays silence"]
fn audio_rename_while_playing_preserves_inline_and_expanded_transport() {
    let mut h = open_fixture("frontend-audio-rename-playing", &[sample(TOPIC, TEXT)]);
    let tr = h.app.tr();
    let mut wav = crate::scenarios::wav_bytes(96_000);
    wav[44..].fill(0);
    h.app.session_mut().unwrap().start_synthetic_recording(wav, Duration::from_secs(12));
    h.frames(3);
    h.click(tr.finish_recording());
    assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 3));
    let session = h.app.session().unwrap();
    let id = session.editor.note().blocks.iter().find(|block| matches!(block, Block::Audio(_))).unwrap().id();
    let original = audio_block(session.editor.note(), id).clone();
    h.click(tr.play());
    let playing = |app: &crate::app::MossPocket| {
        app.session().unwrap().media.audio_state().is_some_and(|state| {
            state.playing && state.blob_id.as_deref() == Some(original.blob_id.as_str()) && state.position > 0.1
        })
    };
    assert!(h.wait_until(4.0, playing), "silent audio should start and progress");
    let before = h.app.session().unwrap().media.audio_state().unwrap().position;
    begin_audio_rename(&mut h, id, &original.file_name);
    h.text("Live edit");
    h.press(Key::Enter, Modifiers::NONE);
    h.frames(3);
    visible(&h, &["Live edit.wav", tr.pause(), tr.stop()]);
    let state = h.app.session().unwrap().media.audio_state().unwrap();
    assert!(state.playing && state.position >= before);
    assert_eq!(state.blob_id.as_deref(), Some(original.blob_id.as_str()));
    h.click(tr.open_audio_player());
    h.frames(3);
    assert!(h.app.session().unwrap().editor_ui.overlay_open());
    assert_eq!(h.labels("Live edit.wav").count(), 2, "both transports should display the saved name");
    assert!(playing(&h.app));
    h.click(tr.stop());
    assert!(h.wait_until(3.0, |app| {
        app.session().unwrap().media.audio_state().is_some_and(|state| state.blob_id.is_none())
    }));
}

#[test]
fn library_rail_scrolls_resizes_filters_opens_and_restores() {
    let mut first = sample(TOPIC, TEXT);
    first.pinned = true;
    first.tags.push("matrix-tag".into());
    let mut deleted = sample("Archived sketch", "Preserve this archived paragraph.");
    deleted.deleted_at = Some(now_millis());
    let mut seed = vec![first, deleted.clone()];
    for index in 0..24 {
        let mut note = sample(&format!("Entry {index:02}"), "Ordinary capture");
        note.updated_at = now_millis().saturating_sub(100_000 + index * 60_000);
        seed.push(note);
    }
    let mut h = open_fixture("frontend-library", &seed);
    let tr = h.app.tr();
    visible(&h, &[tr.pinned_notes(), tr.recent_notes()]);
    h.snapshot("01-library");
    let search = Id::unique(SEARCH);
    let initial = response(&h, search).right();
    let edge = egui::pos2(initial + 16.0, 320.0);
    h.drag(edge, edge + egui::vec2(74.0, 0.0));
    h.frames(4);
    let resized = response(&h, search).right();
    assert!(resized > initial + 40.0, "sidebar edge did not resize: {initial} -> {resized}");
    h.scroll(egui::pos2(120.0, 410.0), -2600.0);
    let row = sidebar_row(&h, "Entry 23");
    h.click_at(row.center(), PointerButton::Primary);
    h.frames(4);
    assert_eq!(h.app.session().unwrap().editor.note().title(), "Entry 23");
    for (word, expected) in [("matrix-tag", true), ("window", true), ("unmatched xyz", false)] {
        h.click_field(search);
        h.press(Key::A, Modifiers::CTRL);
        h.text(word);
        h.frames(5);
        if expected {
            let row = sidebar_row(&h, TOPIC);
            h.click_at(row.center(), PointerButton::Primary);
            h.frames(4);
            assert_eq!(h.app.session().unwrap().editor.note().title(), TOPIC);
        } else {
            visible(&h, &[tr.find_none()]);
            h.snapshot("02-no-results");
        }
    }
    h.click(tr.trash());
    h.frames(4);
    let row = sidebar_row(&h, "Archived sketch");
    h.click_at(row.center(), PointerButton::Primary);
    h.frames(3);
    visible(&h, &[tr.restore()]);
    for label in [tr.find(), tr.export(), tr.add_title(), tr.pin(), tr.new_note()] {
        absent(&h, label);
    }
    let body = deleted.blocks[1].id();
    assert!(h.ctx.read_response(Id::unique(("block-menu", body))).is_none());
    let box_rect = response(&h, Id::unique(("block", body)));
    h.click_at(box_rect.center(), PointerButton::Primary);
    h.text(" editing is forbidden");
    h.press(Key::F, Modifiers::CTRL);
    assert!(!h.app.session().unwrap().find.open);
    assert_eq!(h.app.session().unwrap().editor.note().plain_text(), deleted.plain_text());
    h.snapshot("03-trash-readonly");
    h.click(tr.restore());
    h.frames(3);
    visible(&h, &[tr.empty_trash()]);
    h.click(tr.notes());
    h.frames(4);
    assert_eq!(h.app.session().unwrap().view.search, "unmatched xyz");
    let mut vault = Vault::unlock(&Vault::data_dir(&h.dir), UnlockSecret::Password(PASSWORD)).unwrap();
    assert!(vault.load_note(&deleted.id).unwrap().deleted_at.is_none());
}

#[test]
fn note_context_export_targets_the_clicked_row_and_keeps_active_draft() {
    let mut first = sample(TOPIC, TEXT);
    first.pinned = true;
    let other = sample("Reference excerpt", "This text belongs to the row menu.");
    let mut h = open_fixture("frontend-context-export", &[first, other]);
    let tr = h.app.tr();
    let active = h.app.session().unwrap().current.clone();
    h.press(Key::End, Modifiers::CTRL);
    h.text(" unsaved editor text");
    let row = sidebar_row(&h, "Reference excerpt");
    h.click_at(row.center(), PointerButton::Secondary);
    h.frames(3);
    let path = h.dir.join("reference.html");
    h.click(tr.export_html());
    h.frames(3);
    visible(&h, &[tr.picker_location(), tr.picker_save(), tr.picker_cancel()]);
    picker_text(&mut h, "picker-filename", path.to_str().unwrap());
    h.click(tr.picker_save());
    h.frames(3);
    absent(&h, tr.picker_location());
    let html = std::fs::read_to_string(path).unwrap();
    assert!(html.contains("This text belongs to the row menu."));
    assert!(!html.contains("unsaved editor text"));
    assert_eq!(h.app.session().unwrap().current, active);
    assert!(h.app.session().unwrap().editor.note().plain_text().contains("unsaved editor text"));
    assert_eq!(Settings::load(&h.dir).language, Language::En);
}

#[test]
fn auth_keyboard_validation_and_broken_vault_are_visible_at_minimum_size() {
    let mut h = Harness::new("frontend-auth-errors");
    en(&mut h);
    h.screen.pixels_per_point = 1.0;
    h.resize(egui::vec2(720.0, 480.0));
    let tr = h.app.tr();
    let password = Id::unique("lock-password");
    let confirm = Id::unique("lock-confirm");
    assert_eq!(focused(&h), Some(password));
    h.text("short");
    h.press(Key::Enter, Modifiers::NONE);
    h.frames(3);
    assert_eq!(focused(&h), Some(confirm));
    h.text("short");
    h.press(Key::Enter, Modifiers::NONE);
    h.frames(3);
    visible(&h, &[tr.password_too_short()]);
    assert_eq!(focused(&h), Some(password));
    h.snapshot("01-short-password");
    h.press(Key::A, Modifiers::CTRL);
    h.text(PASSWORD);
    h.press(Key::Enter, Modifiers::NONE);
    h.frames(3);
    assert_eq!(focused(&h), Some(confirm));
    h.press(Key::Enter, Modifiers::NONE);
    h.frames(3);
    visible(&h, &[tr.password_mismatch()]);
    assert_eq!(focused(&h), Some(confirm));
    h.press(Key::A, Modifiers::CTRL);
    h.text(PASSWORD);
    h.press(Key::Enter, Modifiers::NONE);
    assert!(h.wait_until(30.0, crate::app::MossPocket::vault_ready_to_enter));
    visible(&h, &[tr.recovery_title(), tr.copy(), tr.saved_continue()]);
    h.snapshot("02-recovery-created");
    h.press(Key::Space, Modifiers::NONE);
    h.frames(4);
    assert!(h.app.is_unlocked());
    visible(&h, &[tr.no_note_open(), tr.new_note()]);
    h.snapshot("03-empty-library");
    h.press(Key::L, Modifiers::CTRL);
    h.frames(4);
    h.text("wrong-pass");
    h.press(Key::Enter, Modifiers::NONE);
    assert!(h.wait_until(30.0, |app| app.lock_ui().is_some_and(|lock| lock.error.is_some())));
    visible(&h, &[tr.wrong_password()]);
    h.snapshot("04-wrong-password");
    let db = Vault::data_dir(&h.dir);
    std::fs::write(db.join("vault.json"), b"{bad json").unwrap();
    h.app.refresh_vault();
    h.frames(4);
    visible(&h, &[tr.unlock_title()]);
    assert!(h.ctx.read_response(Id::unique("lock-secret")).is_none());
    absent(&h, tr.unlock());
    absent(&h, tr.use_recovery_key());
    h.snapshot("05-broken-vault");
}

#[test]
fn modal_and_search_focus_do_not_absorb_the_first_new_note_input() {
    let mut h = open_fixture("frontend-input-focus", &[sample(TOPIC, TEXT)]);
    let tr = h.app.tr();
    let search = Id::unique(SEARCH);
    let previous = h.app.session().unwrap().current.clone();
    h.click_field(search);
    h.text("Field");
    h.press(Key::H, Modifiers::CTRL);
    h.frames(3);
    assert!(h.app.session().unwrap().find.open);
    h.press(Key::N, Modifiers::CTRL);
    h.frames(3);
    assert_ne!(h.app.session().unwrap().current, previous);
    assert!(!h.app.session().unwrap().find.open);
    let body = h.app.session().unwrap().editor.note().blocks[0].id();
    let body_id = Id::unique(("block", body));
    assert_eq!(focused(&h), Some(body_id));
    h.ime_preedit("linggan");
    assert!(h.app.session().unwrap().editor.note().plain_text().is_empty());
    h.ime_commit("灵感");
    h.text(" first");
    let initial = h.app.session().unwrap().editor.note().plain_text();
    assert_eq!(initial, "灵感 first");
    assert!(h.app.session().unwrap().editor.note().can_add_title());
    h.snapshot("01-typing-first");

    h.click(tr.settings());
    h.frames(4);
    let current = h.app.session().unwrap().current.clone();
    h.press(Key::N, Modifiers::CTRL);
    h.press(Key::H, Modifiers::CTRL);
    h.ime_commit("Background input");
    h.text("Background typing");
    assert_eq!(h.app.session().unwrap().current, current);
    assert_eq!(h.app.session().unwrap().editor.note().plain_text(), initial);
    h.press(Key::Escape, Modifiers::NONE);
    h.frames(3);
    assert!(!h.app.settings_open);
    h.click_field(body_id);
    h.press(Key::End, Modifiers::CTRL);
    h.text(" again");
    assert_eq!(h.app.session().unwrap().editor.note().plain_text(), "灵感 first again");
    h.press(Key::L, Modifiers::CTRL);
    h.frames(4);
    assert!(!h.app.is_unlocked());
    assert!(h.unlock(PASSWORD));
    assert_eq!(h.app.session().unwrap().editor.note().plain_text(), "灵感 first again");
    assert!(h.app.session().unwrap().editor.note().can_add_title());
}

#[test]
fn font_catalog_items_are_reachable_and_title_and_body_selections_stay_independent() {
    let mut h = open_fixture("frontend-font-catalog", &[]);
    let tr = h.app.tr();
    h.click(tr.settings());
    h.frames(4);
    select_settings(&mut h, SettingsPage::Typography);
    let faces = crate::fonts::available_faces();
    assert!(!faces.is_empty(), "Windows font catalog is empty");
    let pick = |h: &mut Harness, role: &str, old: &str, new: &str| {
        let row = h.caption(role);
        let selected = h.labels(old).find(|rect| (rect.center().y - row.center().y).abs() < 8.0)
            .unwrap_or_else(|| panic!("font combo for {role:?} has no {old:?}"));
        h.click_at(selected.center(), PointerButton::Primary);
        h.frames(4);
        if !h.caption_is_unclipped(new) {
            let popup = selected.left_bottom() + egui::vec2(40.0, 68.0);
            h.scroll(popup, 2800.0);
            for _ in 0..10 {
                if h.caption_is_unclipped(new) {
                    break;
                }
                h.scroll(popup, -72.0);
            }
        }
        h.click(new);
        h.frames(4);
    };
    let mut old = tr.default_font();
    for face in faces {
        pick(&mut h, tr.heading_font(), old, face.english_name);
        let settings = Settings::load(&h.dir);
        assert_eq!(settings.heading_font, face.key);
        assert_eq!(settings.body_font, crate::fonts::DEFAULT);
        visible(&h, &[tr.font_preview_title(), tr.font_preview_body()]);
        old = face.english_name;
    }
    let body_face = faces[0];
    pick(&mut h, tr.body_font(), tr.default_font(), body_face.english_name);
    assert_eq!(Settings::load(&h.dir).body_font, body_face.key);
    h.snapshot("01-distinct-fonts");
    pick(&mut h, tr.heading_font(), old, tr.default_font());
    assert_eq!(Settings::load(&h.dir).heading_font, crate::fonts::DEFAULT);
    assert_eq!(Settings::load(&h.dir).body_font, body_face.key);
    h.app.settings_mut().body_font = "missing-font-qa".into();
    h.app.settings_changed(&h.ctx);
    h.frames(4);
    visible(&h, &[tr.font_unavailable(), tr.font_preview_body()]);
    h.snapshot("02-missing-font-fallback");
}

#[test]
fn composition_is_visible_but_never_persists_until_committed() {
    let mut h = open_fixture("frontend-ime", &[]);
    h.press(Key::N, Modifiers::CTRL);
    h.frames(3);
    h.text("keep the committed text");
    // Save with the editor still open until the final commit has been entered.
    assert!(h.app.session_mut().unwrap().save_current());
    let note_id = h.app.session().unwrap().current.clone().unwrap();
    let mut vault = Vault::unlock(&Vault::data_dir(&h.dir), UnlockSecret::Password(PASSWORD)).unwrap();
    h.press(Key::A, Modifiers::CTRL);
    h.ime_preedit("preedit only");
    h.frames(3);
    visible(&h, &["preedit only"]);
    assert!(h.app.session_mut().unwrap().save_current());
    assert_eq!(vault.load_note(&note_id).unwrap().plain_text(), "keep the committed text");
    h.snapshot("01-preedit-only");
    h.ime_preedit("");
    h.frames(3);
    visible(&h, &["keep the committed text"]);
    assert_eq!(h.app.session().unwrap().editor.note().plain_text(), "keep the committed text");

    h.press(Key::A, Modifiers::CTRL);
    h.ime_preedit("xiang");
    h.step(vec![
        egui::Event::Ime(egui::ImeEvent::Commit("想".into())),
        egui::Event::Ime(egui::ImeEvent::Preedit { text: "fa".into(), active_range_chars: None }),
    ]);
    assert!(h.app.session_mut().unwrap().save_current());
    assert_eq!(vault.load_note(&note_id).unwrap().plain_text(), "想");
    h.ime_commit("法");
    h.request_close();
    assert_eq!(vault.load_note(&note_id).unwrap().plain_text(), "想法");
    h.snapshot("02-ime-committed");
}

#[test]
fn compact_settings_scroll_to_all_lower_actions() {
    for (language, suffix) in [(Language::En, "en"), (Language::Zh, "zh")] {
        let mut h = open_fixture("frontend-settings-scroll", &[]);
        h.screen.pixels_per_point = 1.0;
        h.resize(egui::vec2(720.0, 480.0));
        h.app.settings_mut().language = language;
        h.app.settings_changed(&h.ctx);
        h.frames(4);
        let tr = h.app.tr();
        h.click(tr.settings());
        h.frames(4);
        select_settings(&mut h, SettingsPage::Typography);
        let preview = h.caption(tr.font_preview_title()).center();
        h.scroll(preview, -850.0);
        visible(&h, &[tr.font_size(), tr.line_spacing(), tr.paragraph_spacing(), tr.close()]);
        h.snapshot(&format!("{suffix}-typography-bottom"));
        select_settings(&mut h, SettingsPage::Security);
        let section = h.caption(tr.auto_lock()).center();
        h.scroll(section, -850.0);
        visible(&h, &[tr.apply(), tr.regenerate_recovery(), tr.lock(), tr.close()]);
        h.snapshot(&format!("{suffix}-security-bottom"));
        h.press(Key::Escape, Modifiers::NONE);
        h.frames(4);
        assert!(!h.app.settings_open);
    }
}
