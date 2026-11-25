//! 从实际绘制出的控件发起动作，使用独立笔记库和模拟的系统文件对话框返回值。

use std::cell::RefCell;
use std::path::PathBuf;

use eframe::egui::{self, Id, Key, Modifiers, PointerButton, Pos2, Theme, ViewportCommand};
use mp_core::model::{Block, TextKind};
use mp_core::vault::{UnlockSecret, Vault};

use crate::headless::Harness;
use crate::settings::{Backdrop, Language, Settings, ThemeMode};
use crate::settings_ui::SettingsPage;

const PASSWORD: &str = "matrix-vault-pass";
const BACKDROPS: [Backdrop; 5] = [
    Backdrop::Opaque, Backdrop::Transparent, Backdrop::Acrylic, Backdrop::Mica, Backdrop::SystemAcrylic,
];
const THEMES: [(ThemeMode, Theme, Theme); 4] = [
    (ThemeMode::System, Theme::Light, Theme::Light),
    (ThemeMode::System, Theme::Dark, Theme::Dark),
    (ThemeMode::Light, Theme::Dark, Theme::Light),
    (ThemeMode::Dark, Theme::Light, Theme::Dark),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dialog {
    Import,
    SaveMedia,
    Export(&'static str),
}

thread_local! {
    static PICK: RefCell<Option<(Dialog, Option<Vec<PathBuf>>)>> = const { RefCell::new(None) };
}

/// 外层 `None` 走真实选择器，`Some(None)` 模拟取消。
pub(crate) fn take_file_dialog(kind: Dialog) -> Option<Option<Vec<PathBuf>>> {
    let (expected, result) = PICK.with(|slot| slot.borrow_mut().take())?;
    assert_eq!(kind, expected);
    Some(result)
}

pub(crate) fn choose(h: &mut Harness, label: &str, dialog: Dialog, choice: Option<Vec<PathBuf>>) {
    let position = h.caption(label).center();
    PICK.with(|slot| assert!(slot.borrow_mut().replace((dialog, choice)).is_none()));
    h.click_at(position, PointerButton::Primary);
    assert!(PICK.with(|slot| slot.borrow_mut().take().is_none()), "the item did not open {dialog:?}");
    h.frames(2);
}

fn locked_fixture(tag: &str) -> Harness {
    let mut h = Harness::new(tag);
    h.app.settings_mut().language = Language::En;
    h.app.settings_changed(&h.ctx);
    h.create_vault(PASSWORD);
    h.frames(5);
    h
}

fn workspace(tag: &str) -> Harness {
    let mut h = locked_fixture(tag);
    assert!(h.unlock(PASSWORD));
    h.frames(4);
    h
}

fn active_note(h: &Harness) -> &mp_core::model::Note {
    h.app.session().unwrap().editor.note()
}

pub(crate) fn open_block_menu(h: &mut Harness, block: u64) {
    h.click_field(Id::unique(("block-menu", block)));
    h.frames(2);
}

pub(crate) fn select_settings(h: &mut Harness, expected: SettingsPage) {
    let tr = h.app.tr();
    let name = match expected {
        SettingsPage::Appearance => tr.appearance(),
        SettingsPage::Typography => tr.typography(),
        SettingsPage::Language => tr.language(),
        SettingsPage::Security => tr.security(),
    };
    h.click(name);
    h.frames(3);
    assert_eq!(h.app.settings_form.section, expected);
}

fn assert_visible(h: &Harness, label: &str, screen: egui::Vec2) {
    let rect = h.caption(label);
    assert!(rect.left() >= 0.0 && rect.top() >= 0.0 && rect.right() <= screen.x && rect.bottom() <= screen.y, "{label:?} {rect:?} at {screen:?}");
}

#[test]
fn appearance_cross_product_renders_locked_editor_and_settings() {
    let mut h = locked_fixture("matrix-appearance");
    for unlocked in [false, true] {
        if unlocked {
            assert!(h.unlock(PASSWORD));
            h.click(h.app.tr().new_note());
            h.frames(4);
            h.text("Body first 苔藓");
        }
        for language in [Language::En, Language::Zh] {
            for (mode, system, expected) in THEMES {
                for backdrop in BACKDROPS {
                    let settings = h.app.settings_mut();
                    settings.language = language;
                    settings.theme_mode = mode;
                    settings.backdrop = backdrop;
                    h.system_theme = system;
                    h.app.settings_changed(&h.ctx);
                    let tr = h.app.tr();
                    for size in [egui::vec2(720.0, 480.0), egui::vec2(1120.0, 740.0)] {
                        h.resize(size);
                        assert_eq!(h.ctx.theme(), expected, "{language:?} {mode:?} {backdrop:?}");
                        assert_visible(&h, "Pocket", size);
                        if !unlocked {
                            assert_visible(&h, tr.unlock_title(), size);
                            assert_visible(&h, tr.use_recovery_key(), size);
                            let password = h.ctx.read_response(Id::unique("lock-secret")).unwrap().rect;
                            assert!(h.ctx.content_rect().contains_rect(password));
                            continue;
                        }
                        assert_visible(&h, tr.new_note(), size);
                        assert_visible(&h, tr.add_title(), size);
                        let body = active_note(&h).blocks[0].id();
                        let body = h.ctx.read_response(Id::unique(("block", body))).unwrap().rect;
                        let search = h.ctx.read_response(Id::unique("search-box")).unwrap().rect;
                        assert!(h.ctx.content_rect().contains_rect(body) && search.right() < body.left());
                        h.click(tr.settings());
                        h.frames(4);
                        assert!(h.app.settings_open);
                        for tab in [tr.appearance(), tr.typography(), tr.language(), tr.security(), tr.close()] {
                            assert_visible(&h, tab, size);
                        }
                        h.click(tr.close());
                        h.frames(2);
                        assert!(!h.app.settings_open);
                    }
                }
            }
        }
    }
    assert_eq!(active_note(&h).plain_text(), "Body first 苔藓");
}

#[test]
fn materials_render_a_continuous_titlebar_at_three_dpi_scales() {
    let mut h = workspace("matrix-materials");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    h.text("An idea worth keeping");
    h.request_close();
    for (theme, tone) in [(ThemeMode::Light, "light"), (ThemeMode::Dark, "dark")] {
        for (backdrop, material) in BACKDROPS.into_iter().zip(["opaque", "transparent", "acrylic", "mica", "system-acrylic"]) {
            h.app.settings_mut().theme_mode = theme;
            h.app.settings_mut().backdrop = backdrop;
            h.app.settings_changed(&h.ctx);
            for scale in [1.0, 1.5, 2.0] {
                h.screen.pixels_per_point = scale;
                h.resize(egui::vec2(720.0, 480.0));
                assert_eq!(h.ctx.pixels_per_point(), scale);
                assert_visible(&h, "Pocket", egui::vec2(720.0, 480.0));
                let search = h.ctx.read_response(Id::unique("search-box")).unwrap().rect;
                assert!(search.right() < h.caption("An idea worth keeping").left());
                if scale != 1.5 {
                    continue;
                }
                let path = h.snapshot(&format!("{tone}-{material}"));
                let pixels = image::open(&path).unwrap().into_rgba8();
                let x = 800;
                let edge = (crate::window_ui::HEIGHT * scale) as u32;
                assert_eq!(pixels.get_pixel(x, edge - 2), pixels.get_pixel(x, edge + 2), "titlebar seam for {tone}-{material}");
            }
        }
    }
}

#[test]
fn click_new_type_without_title_edit_metadata_and_reopen() {
    let mut h = workspace("matrix-body");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    let body = active_note(&h).blocks[0].id();
    assert_eq!(h.ctx.memory(|memory| memory.focused()), Some(Id::unique(("block", body))));
    h.ime_preedit("linggan");
    h.ime_commit("灵感");
    h.text(" first");
    h.click(tr.add_title());
    h.frames(3);
    assert_eq!(active_note(&h).blocks[1].id(), body);
    h.text("标题");
    h.press(Key::Enter, Modifiers::NONE);
    h.frames(3);
    assert_eq!(active_note(&h).title(), "标题");
    h.click(tr.pin());
    assert!(active_note(&h).pinned);
    h.frames(2);
    h.click(tr.unpin());
    assert!(!active_note(&h).pinned);
    h.click(tr.tags_hint());
    h.text("ideas, moss");
    h.press(Key::Enter, Modifiers::NONE);
    h.click(tr.lock());
    assert!(!h.app.is_unlocked());
    assert!(h.unlock(PASSWORD));
    let note = active_note(&h);
    assert_eq!(note.title(), "标题");
    assert_eq!(note.blocks[1].as_text().unwrap().text, "灵感 first");
    assert_eq!(note.tags, ["ideas", "moss"]);
}

#[test]
fn right_click_note_pin_trash_restore_and_destroy() {
    let mut h = workspace("matrix-trash-menu");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    h.click(tr.add_title());
    h.frames(3);
    h.text("Matrix archive sample");
    h.press(Key::Enter, Modifiers::NONE);
    h.text("These details survive trash.");
    h.request_close();
    h.frames(3);
    let id = active_note(&h).id.clone();
    let row_menu = |h: &mut Harness| {
        let rail_right = h.ctx.read_response(Id::unique("search-box")).unwrap().rect.right();
        let row = h.labels("Matrix archive sample").find(|rect| rect.right() <= rail_right).expect("note must be in the sidebar");
        h.click_at(row.center(), PointerButton::Secondary);
        h.frames(2);
    };
    row_menu(&mut h);
    h.click(tr.pin());
    assert!(active_note(&h).pinned);
    row_menu(&mut h);
    h.click(tr.unpin());
    assert!(!active_note(&h).pinned);
    row_menu(&mut h);
    h.click(tr.move_to_trash());
    h.frames(3);
    h.click(tr.trash());
    h.frames(3);
    row_menu(&mut h);
    h.click(tr.restore());
    h.frames(3);
    h.click(tr.notes());
    h.frames(3);
    row_menu(&mut h);
    h.click(tr.move_to_trash());
    h.frames(3);
    h.click(tr.trash());
    h.frames(3);
    row_menu(&mut h);
    h.click(tr.delete_forever());
    h.frames(3);
    assert_visible(&h, tr.empty_trash(), h.ctx.content_rect().size());
    let mut vault = Vault::unlock(&Vault::data_dir(&h.dir), UnlockSecret::Password(PASSWORD)).unwrap();
    assert!(vault.load_note(&id).is_err());
}

#[test]
fn click_all_find_controls_with_mixed_case_and_replacements() {
    let mut h = workspace("matrix-find-menu");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    h.text("Birch birch BIRCH birch");
    h.click(tr.find());
    h.frames(3);
    h.text("birch");
    h.frames(2);
    h.caption("1 / 4");
    h.click("Aa");
    h.frames(2);
    h.caption("1 / 2");
    h.click(tr.find_next());
    h.frames(2);
    h.caption("2 / 2");
    h.click(tr.find_prev());
    h.frames(2);
    h.caption("1 / 2");
    h.click(tr.find_show_replace());
    h.frames(2);
    h.click_field(Id::unique("find-replacement"));
    h.text("moss");
    h.click(tr.find_replace());
    h.frames(2);
    assert_eq!(active_note(&h).plain_text(), "Birch moss BIRCH birch");
    h.click(tr.find_replace_all());
    h.frames(2);
    assert_eq!(active_note(&h).plain_text(), "Birch moss BIRCH moss");
    h.caption(tr.find_none());
    h.click(tr.close());
    assert!(!h.app.session().unwrap().find.open);
}

#[test]
fn shortcuts_distinguish_cut_strikethrough_and_reverse_find() {
    let mut h = workspace("matrix-shortcuts");
    h.press(Key::N, Modifiers::CTRL);
    h.frames(3);
    h.text("moss moss moss");
    h.press(Key::A, Modifiers::CTRL);
    for key in [Key::B, Key::I, Key::U] {
        h.press(key, Modifiers::CTRL);
    }
    h.press(Key::X, Modifiers::CTRL | Modifiers::SHIFT);
    let body = active_note(&h).blocks[0].as_text().unwrap();
    assert_eq!(body.text, "moss moss moss", "Ctrl+Shift+X must preserve selected text");
    let span = body.spans[0];
    assert!(span.style.bold && span.style.italic && span.style.underline && span.style.strikethrough);
    h.press(Key::Z, Modifiers::CTRL);
    assert!(!active_note(&h).blocks[0].as_text().unwrap().spans[0].style.strikethrough);
    h.press(Key::Z, Modifiers::CTRL | Modifiers::SHIFT);
    assert!(active_note(&h).blocks[0].as_text().unwrap().spans[0].style.strikethrough);
    h.press(Key::Escape, Modifiers::NONE);
    h.press(Key::F, Modifiers::CTRL);
    h.frames(3);
    let tr = h.app.tr();
    h.click_field(Id::unique("find-query"));
    h.press(Key::A, Modifiers::CTRL);
    h.text("moss");
    h.caption("1 / 3");
    h.press(Key::F3, Modifiers::SHIFT);
    h.caption("3 / 3");
    h.press(Key::F3, Modifiers::NONE);
    h.caption("1 / 3");
    h.press(Key::Escape, Modifiers::NONE);
    assert!(!h.app.session().unwrap().find.open);
    h.press(Key::H, Modifiers::CTRL);
    assert!(h.app.session().unwrap().find.replace);
    h.click(tr.close());
    assert!(!h.app.session().unwrap().find.open);

    let first = active_note(&h).blocks[0].id();
    let first_field = Id::unique(("block", first));
    h.frames(4);
    h.click_field(first_field);
    h.press(Key::A, Modifiers::CTRL);
    assert_eq!(h.ctx.memory(|memory| memory.focused()), Some(first_field));
    let selection = egui::text_edit::TextEditState::load(&h.ctx, first_field).unwrap().cursor.char_range().unwrap();
    assert!(!selection.is_empty());
    h.press(Key::X, Modifiers::CTRL);
    assert_eq!(active_note(&h).plain_text(), "");
    h.press(Key::Z, Modifiers::CTRL);
    assert_eq!(active_note(&h).plain_text(), "moss moss moss");
    h.press(Key::End, Modifiers::CTRL);
    h.press(Key::Enter, Modifiers::NONE);
    h.text("Move me");
    let second = active_note(&h).blocks[1].id();
    h.press(Key::ArrowUp, Modifiers::ALT);
    assert_eq!(active_note(&h).blocks[0].id(), second);
    h.press(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(active_note(&h).blocks[1].id(), second);
}

#[test]
fn click_block_type_duplicate_move_divider_and_delete() {
    let mut h = workspace("matrix-block-menu");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    h.text("Seed");
    let block = active_note(&h).blocks[0].id();
    for (kind, label) in [
        (TextKind::Heading1, tr.heading1()), (TextKind::Heading2, tr.heading2()),
        (TextKind::Heading3, tr.heading3()), (TextKind::Bullet, tr.bullet()),
        (TextKind::Numbered, tr.numbered()), (TextKind::Quote, tr.quote()),
        (TextKind::Paragraph, tr.paragraph()),
    ] {
        open_block_menu(&mut h, block);
        h.click(label);
        h.frames(2);
        assert_eq!(active_note(&h).blocks[0].as_text().unwrap().kind, kind);
    }
    open_block_menu(&mut h, block);
    h.click(tr.duplicate());
    h.frames(3);
    let copy = active_note(&h).blocks[1].id();
    assert_ne!(copy, block);
    open_block_menu(&mut h, copy);
    h.click(tr.move_up());
    h.frames(3);
    assert_eq!(active_note(&h).blocks[0].id(), copy);
    open_block_menu(&mut h, copy);
    h.click(tr.move_down());
    h.frames(3);
    assert_eq!(active_note(&h).blocks[1].id(), copy);
    open_block_menu(&mut h, copy);
    h.click(tr.insert_divider());
    h.frames(3);
    assert!(matches!(active_note(&h).blocks[2], Block::Divider { .. }));
    open_block_menu(&mut h, copy);
    h.click(tr.delete_block());
    h.frames(3);
    assert_eq!(active_note(&h).plain_text(), "Seed");
    h.press(Key::Z, Modifiers::CTRL);
    assert_eq!(active_note(&h).plain_text(), "Seed\nSeed");
    h.press(Key::Y, Modifiers::CTRL);
    assert_eq!(active_note(&h).plain_text(), "Seed");
}

#[test]
fn click_export_formats_and_cancel_without_touching_real_dialogs() {
    let mut h = workspace("matrix-export-menu");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    h.text("Portable export 漢字");
    let cancel_dir = h.dir.join("cancel");
    std::fs::create_dir(&cancel_dir).unwrap();
    h.click(tr.export());
    h.frames(3);
    choose(&mut h, "Markdown", Dialog::Export("md"), None);
    assert_eq!(std::fs::read_dir(&cancel_dir).unwrap().count(), 0);
    assert!(h.app.session().unwrap().notice.is_none());
    for (label, extension, prefix) in [
        ("Markdown", "md", b"Portable export".as_slice()),
        (tr.export_html(), "html", b"<!DOCTYPE html>".as_slice()),
        (tr.export_docx(), "docx", b"PK".as_slice()),
    ] {
        h.click(tr.export());
        h.frames(3);
        h.caption("PDF");
        let output = h.dir.join(format!("Portable export 漢字.{extension}"));
        let result = if extension == "md" { h.dir.clone() } else { output.clone() };
        choose(&mut h, label, Dialog::Export(extension), Some(vec![result]));
        h.caption(tr.export_done());
        let bytes = std::fs::read(&output).unwrap();
        assert!(bytes.starts_with(prefix), "{output:?}");
    }
    h.click(tr.export());
    h.frames(3);
    choose(&mut h, "PDF", Dialog::Export("pdf"), None);
    h.click(tr.export());
    h.frames(3);
    choose(&mut h, tr.export_html(), Dialog::Export("html"), Some(vec![cancel_dir]));
    assert!(h.has_label_prefix(tr.export_failed()));
}

#[test]
fn window_buttons_and_save_on_close_use_real_viewport_events() {
    let mut h = workspace("matrix-window");
    let tr = h.app.tr();
    let right = h.ctx.content_rect().right();
    for (offset, maximized, expected) in [
        (93.0, false, ViewportCommand::Minimized(true)),
        (59.0, false, ViewportCommand::Maximized(true)),
        (59.0, true, ViewportCommand::Maximized(false)),
        (25.0, false, ViewportCommand::Close),
    ] {
        h.maximized = maximized;
        h.click_at(Pos2::new(right - offset, 17.0), PointerButton::Primary);
        assert!(h.commands.contains(&expected), "{:?}", h.commands);
    }
    let bar = Pos2::new(right / 2.0, 12.0);
    h.frames(30);
    h.click_at(bar, PointerButton::Primary);
    h.click_at(bar, PointerButton::Primary);
    assert!(h.commands.contains(&ViewportCommand::Maximized(true)), "titlebar double-click commands: {:?}", h.commands);
    h.move_pointer(bar);
    h.pointer_button(bar, PointerButton::Primary, true);
    let shifted = bar + egui::vec2(24.0, 6.0);
    h.move_pointer(shifted);
    assert!(h.commands.contains(&ViewportCommand::StartDrag));
    h.pointer_button(shifted, PointerButton::Primary, false);
    h.click(tr.new_note());
    h.frames(3);
    h.text("Save on close");
    let id = active_note(&h).id.clone();
    h.request_close();
    assert!(!h.commands.contains(&ViewportCommand::CancelClose));
    let mut vault = Vault::unlock(&Vault::data_dir(&h.dir), UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(vault.load_note(&id).unwrap().plain_text(), "Save on close");
}

#[test]
fn close_refuses_failed_save_and_keeps_the_body_until_retry() {
    let mut h = workspace("matrix-close-failure");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    h.text("Recover this unsaved idea");
    let id = active_note(&h).id.clone();
    let notes = Vault::data_dir(&h.dir).join("notes");
    std::fs::remove_dir(&notes).unwrap();
    std::fs::write(&notes, b"notes has become a file").unwrap();
    h.request_close();
    assert!(h.commands.contains(&ViewportCommand::CancelClose));
    h.frames(3);
    assert!(h.has_label_prefix(tr.save_before_close()));
    assert_eq!(active_note(&h).plain_text(), "Recover this unsaved idea");
    std::fs::remove_file(&notes).unwrap();
    std::fs::create_dir(&notes).unwrap();
    h.request_close();
    assert!(!h.commands.contains(&ViewportCommand::CancelClose));
    let mut vault = Vault::unlock(&Vault::data_dir(&h.dir), UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(vault.load_note(&id).unwrap().plain_text(), "Recover this unsaved idea");
}

#[test]
fn portable_settings_load_defaults_sanitize_and_round_trip() {
    let h = Harness::new("matrix-settings-file");
    let path = Settings::path(&h.dir);
    assert_eq!(Settings::load(&h.dir), Settings::default());
    std::fs::write(&path, b"{corrupt").unwrap();
    assert_eq!(Settings::load(&h.dir), Settings::default());
    std::fs::write(&path, r#"{"base_font_size":900,"line_spacing":-1,"list_width":5000,"auto_lock_minutes":99999,"theme_mode":"dark"}"#).unwrap();
    let settings = Settings::load(&h.dir);
    assert_eq!(settings.base_font_size, 28.0);
    assert_eq!(settings.line_spacing, 1.0);
    assert_eq!(settings.list_width, 560.0);
    assert_eq!(settings.auto_lock_minutes, 1440);
    assert_eq!(settings.theme_mode, ThemeMode::Dark);
    settings.save(&h.dir).unwrap();
    assert_eq!(Settings::load(&h.dir), settings);
}

#[test]
fn click_setup_copy_recovery_and_open_with_recovery_key() {
    let mut h = Harness::new("matrix-recovery");
    h.app.settings_mut().language = Language::En;
    h.app.settings_changed(&h.ctx);
    h.frames(4);
    let tr = h.app.tr();
    h.click_field(Id::unique("lock-password"));
    h.text(PASSWORD);
    h.click_field(Id::unique("lock-confirm"));
    h.text(PASSWORD);
    h.click(tr.create_vault());
    assert!(h.wait_until(30.0, crate::app::MossPocket::vault_ready_to_enter));
    let recovery = h.app.lock_ui().unwrap().recovery.clone().unwrap();
    h.frames(3);
    h.click(tr.copy());
    assert!(h.platform_commands.contains(&egui::OutputCommand::CopyText(recovery.clone())));
    h.frames(2);
    h.caption(tr.copied());
    h.click(tr.saved_continue());
    h.frames(3);
    assert!(h.app.is_unlocked());
    h.click(tr.lock());
    h.frames(4);
    h.click(tr.use_recovery_key());
    h.frames(3);
    h.caption(tr.unlock_recovery_sub());
    h.click(tr.use_password());
    h.frames(3);
    h.caption(tr.unlock_sub());
    h.click(tr.use_recovery_key());
    h.frames(3);
    h.text("not-a-recovery-key");
    h.click(tr.unlock());
    assert!(h.wait_until(30.0, |app| app.lock_ui().is_some_and(|lock| lock.error.is_some())));
    assert!(!h.app.is_unlocked());
    h.click_field(Id::unique("lock-secret"));
    h.press(Key::A, Modifiers::CTRL);
    h.text(&recovery);
    h.click(tr.unlock());
    assert!(h.wait_until(30.0, crate::app::MossPocket::is_unlocked));
}

#[test]
fn click_appearance_typography_language_and_security_settings() {
    let mut h = workspace("matrix-settings-controls");
    let tr = h.app.tr();
    h.click(tr.settings());
    h.frames(4);
    for (mode, label) in [
        (ThemeMode::Light, tr.theme_light()),
        (ThemeMode::Dark, tr.theme_dark()),
        (ThemeMode::System, tr.theme_system()),
    ] {
        h.click(label);
        h.frames(2);
        assert_eq!(Settings::load(&h.dir).theme_mode, mode);
    }
    let mut current = tr.backdrop_opaque();
    for (mode, label) in [
        (Backdrop::Transparent, tr.backdrop_transparent()),
        (Backdrop::Acrylic, tr.backdrop_acrylic()),
        (Backdrop::Mica, tr.backdrop_mica()),
        (Backdrop::SystemAcrylic, tr.backdrop_system_acrylic()),
        (Backdrop::Opaque, tr.backdrop_opaque()),
    ] {
        h.click(current);
        h.frames(3);
        h.click(label);
        h.frames(3);
        assert_eq!(Settings::load(&h.dir).backdrop, mode);
        current = label;
    }
    select_settings(&mut h, SettingsPage::Typography);
    h.caption(tr.font_preview_body());
    let face = crate::fonts::available_faces().first().copied().expect("Windows font catalog");
    let pick_font = |h: &mut Harness, role: &str| {
        let label = h.caption(role);
        let combo = h.labels(tr.default_font()).find(|rect| (rect.center().y - label.center().y).abs() < 8.0).unwrap();
        h.click_at(combo.center(), PointerButton::Primary);
        h.frames(3);
        h.click(face.english_name);
        h.frames(3);
    };
    pick_font(&mut h, tr.heading_font());
    assert_eq!(Settings::load(&h.dir).heading_font, face.key);
    assert_eq!(Settings::load(&h.dir).body_font, crate::fonts::DEFAULT);
    pick_font(&mut h, tr.body_font());
    assert_eq!(Settings::load(&h.dir).body_font, face.key);

    select_settings(&mut h, SettingsPage::Language);
    h.click("简体中文");
    h.frames(3);
    assert_eq!(Settings::load(&h.dir).language, Language::Zh);
    h.caption(h.app.tr().security());
    h.click(h.app.tr().lang_system());
    h.frames(3);
    assert_eq!(Settings::load(&h.dir).language, Language::System);
    h.click("English");
    h.frames(3);
    assert_eq!(Settings::load(&h.dir).language, Language::En);

    select_settings(&mut h, SettingsPage::Security);
    for (id, secret) in [
        ("settings-old-password", PASSWORD),
        ("settings-new-password", "new-matrix-pass"),
        ("settings-confirm-password", "oops-matrix-pass"),
    ] {
        if id == "settings-confirm-password" {
            let middle = h.caption(tr.new_password()).center();
            h.scroll(middle, -430.0);
        }
        h.click_field(Id::unique(id));
        h.text(secret);
    }
    h.click(tr.apply());
    h.frames(3);
    h.caption(tr.err_password_mismatch());
    h.click_field(Id::unique("settings-confirm-password"));
    h.press(Key::A, Modifiers::CTRL);
    h.text("new-matrix-pass");
    h.click(tr.apply());
    h.frames(3);
    h.caption(tr.password_changed());
    h.click(tr.regenerate_recovery());
    h.frames(3);
    let recovery = h.app.settings_form.fresh_key.clone().unwrap();
    let middle = h.caption(tr.apply()).center();
    h.scroll(middle, -430.0);
    h.click(tr.copy());
    assert!(h.platform_commands.contains(&egui::OutputCommand::CopyText(recovery)));
    h.click(tr.lock());
    assert!(!h.app.is_unlocked());
    assert!(h.unlock("new-matrix-pass"));
}

#[test]
fn insert_image_from_menu_resize_reset_copy_cut_and_save_original() {
    let mut h = workspace("matrix-media-menu");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(3);
    let body = active_note(&h).blocks[0].id();
    h.text("A drawing");
    let input = h.dir.join("图样.png");
    image::RgbImage::from_pixel(250, 110, image::Rgb([80, 150, 110])).save(&input).unwrap();
    open_block_menu(&mut h, body);
    choose(&mut h, tr.insert_media(), Dialog::Import, None);
    assert_eq!(active_note(&h).blocks.len(), 1);
    open_block_menu(&mut h, body);
    choose(&mut h, tr.insert_media(), Dialog::Import, Some(vec![input]));
    assert!(h.wait_until(30.0, |app| app.session().unwrap().editor.note().blocks.len() == 2));
    let Block::Image(image) = &active_note(&h).blocks[1] else { panic!("expected an image") };
    let id = image.id;
    let blob = image.blob_id.clone();
    let ctx = h.ctx.clone();
    assert!(h.wait_until(30.0, |_| ctx.read_response(Id::unique(("image-resize", id))).is_some()));
    h.frames(4);
    let grip = h.ctx.read_response(Id::unique(("image-resize", id))).unwrap().rect.center();
    let target = grip - egui::vec2(95.0, 0.0);
    h.drag(grip, target);
    h.frames(3);
    let Block::Image(image) = &active_note(&h).blocks[1] else { panic!("expected an image") };
    assert!((145.0..=165.0).contains(&image.display_width.unwrap()), "{:?}", image.display_width);
    let picture = h.ctx.read_response(Id::unique(("image-resize", id))).unwrap().rect.center() - egui::vec2(35.0, 20.0);
    h.click_at(picture, PointerButton::Primary);
    h.click_at(picture, PointerButton::Primary);
    let Block::Image(image) = &active_note(&h).blocks[1] else { panic!("expected an image") };
    assert!(image.display_width.is_none());
    open_block_menu(&mut h, id);
    choose(&mut h, tr.save_as(), Dialog::SaveMedia, None);
    let output = h.dir.join("saved.png");
    open_block_menu(&mut h, id);
    choose(&mut h, tr.save_as(), Dialog::SaveMedia, Some(vec![output.clone()]));
    assert_eq!(std::fs::read(output).unwrap(), h.app.session().unwrap().vault.read_blob(&blob).unwrap());
    open_block_menu(&mut h, id);
    h.click(tr.copy_block());
    assert!(h.app.session().unwrap().clip.is_some());
    open_block_menu(&mut h, id);
    h.click(tr.cut_block());
    assert_eq!(active_note(&h).blocks.len(), 1);
}

#[test]
fn settings_sliders_and_color_pickers_change_saved_values() {
    let mut h = workspace("matrix-style-controls");
    let tr = h.app.tr();
    h.click(tr.settings());
    h.frames(4);

    let background = tr.backdrop_transparent();
    h.click(tr.backdrop_opaque());
    h.frames(3);
    h.click(background);
    h.frames(3);
    let initial = Settings::load(&h.dir);
    let slider = |h: &mut Harness, label: &str| {
        let target = h.caption(label).left_center() + egui::vec2(198.0, 0.0);
        h.click_at(target, PointerButton::Primary);
        h.frames(3);
    };
    slider(&mut h, tr.opacity());
    slider(&mut h, tr.corner_radius());
    assert_ne!(Settings::load(&h.dir).tint_alpha, initial.tint_alpha);
    assert_ne!(Settings::load(&h.dir).corner_radius, initial.corner_radius);
    let color_fields: [(&str, fn(Settings) -> u32); 2] = [(tr.tint_color(), |s| s.tint), (tr.accent_color(), |s| s.accent)];
    for (label, field) in color_fields {
        let initial = field(Settings::load(&h.dir));
        let well = egui::pos2(h.caption(tr.theme()).left() + 187.0, h.caption(label).center().y);
        h.click_at(well, PointerButton::Primary);
        h.frames(4);
        let red = h.label_prefix("R ").center();
        h.drag(red, red + egui::vec2(30.0, 0.0));
        h.frames(3);
        assert_ne!(field(Settings::load(&h.dir)), initial);
        h.click(tr.theme());
        h.frames(3);
        assert!(h.app.settings_open);
    }
    select_settings(&mut h, SettingsPage::Typography);
    slider(&mut h, tr.font_size());
    slider(&mut h, tr.line_spacing());
    let preview = h.caption(tr.font_preview_body()).center();
    h.scroll(preview, -220.0);
    slider(&mut h, tr.paragraph_spacing());
    let saved = Settings::load(&h.dir);
    assert_ne!(saved.base_font_size, initial.base_font_size);
    assert_ne!(saved.line_spacing, initial.line_spacing);
    assert_ne!(saved.paragraph_spacing, initial.paragraph_spacing);
    select_settings(&mut h, SettingsPage::Security);
    let idle = h.caption(tr.auto_lock()).left_center() + egui::vec2(117.0, 34.0);
    h.click_at(idle, PointerButton::Primary);
    assert_ne!(Settings::load(&h.dir).auto_lock_minutes, initial.auto_lock_minutes);
}

#[test]
fn same_vault_lock_prevents_a_second_writer() {
    let h = Harness::new("matrix-instance-lock");
    let path = h.dir.join(".lock");
    let other = h.dir.join("other.lock");
    let first = crate::win::lock_instance(&path).expect("first writer");
    assert!(crate::win::lock_instance(&path).is_none());
    assert!(crate::win::lock_instance(&other).is_some());
    drop(first);
    assert!(crate::win::lock_instance(&path).is_some());
}

#[test]
#[ignore = "requires an available Windows audio output endpoint; plays silence"]
fn audio_controls_play_pause_resume_stop_and_replay_silence() {
    let mut h = workspace("matrix-native-audio");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(4);
    let wav = h.dir.join("silence.wav");
    let mut bytes = crate::scenarios::wav_bytes(16_000);
    bytes[44..].fill(0);
    std::fs::write(&wav, bytes).unwrap();
    h.drop_paths(vec![wav]);
    assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 2));
    let playing = |app: &crate::app::MossPocket| app.session().unwrap().media.audio_state().is_some_and(|state| state.playing);
    h.frames(4);
    h.click(tr.play());
    let ctx = h.ctx.clone();
    assert!(h.wait_until(3.0, playing), "audio did not start; diagnostic viewport: {:?}", ctx.content_rect());
    h.frames(2);
    h.click(tr.pause());
    assert!(h.wait_until(3.0, |app| app.session().unwrap().media.audio_state().is_some_and(|state| state.paused)));
    h.frames(2);
    let stop = h.caption(tr.stop());
    let open = h.caption(tr.open_audio_player());
    let to = egui::pos2(egui::lerp(stop.right()..=open.left(), 0.48), stop.center().y);
    h.click_at(to, PointerButton::Primary);
    let seeked_while_paused = |app: &crate::app::MossPocket| {
        app.session().unwrap().media.audio_state().is_some_and(|state| state.paused && state.position > 0.9)
    };
    assert!(h.wait_until(2.0, seeked_while_paused), "clicking the inline timeline should seek while paused");
    let paused_at = h.app.session().unwrap().media.audio_state().unwrap().position;
    let other = h.dir.join("second-silence.wav");
    let mut bytes = crate::scenarios::wav_bytes(32_000);
    bytes[44..].fill(0);
    std::fs::write(&other, bytes).unwrap();
    h.drop_paths(vec![other]);
    assert!(h.wait_until(10.0, |app| app.session().unwrap().editor.note().blocks.len() == 3));
    h.frames(3);
    let other_time = h.caption("0:00 / 0:04");
    let other_play = h.labels(tr.play()).find(|rect| (rect.center().y - other_time.center().y).abs() < 8.0).unwrap();
    let other_open = h.labels(tr.open_audio_player()).find(|rect| (rect.center().y - other_play.center().y).abs() < 8.0).unwrap();
    let inactive_track = egui::pos2(egui::lerp(other_play.right()..=other_open.left(), 0.55), other_play.center().y);
    h.click_at(inactive_track, PointerButton::Primary);
    std::thread::sleep(std::time::Duration::from_millis(180));
    h.frames(3);
    let still_paused = h.app.session().unwrap().media.audio_state().unwrap();
    assert!(still_paused.paused && (still_paused.position - paused_at).abs() < 0.05,
        "using the inactive timeline at {inactive_track:?} changed paused audio at {paused_at}: {still_paused:?}");
    h.frames(2);
    let active_row_play = |h: &Harness| {
        let row_y = h.caption(tr.stop()).center().y;
        h.labels(tr.play()).find(|rect| (rect.center().y - row_y).abs() < 8.0).unwrap().center()
    };
    h.click_at(active_row_play(&h), PointerButton::Primary);
    assert!(h.wait_until(3.0, playing));
    assert!(h.wait_until(5.0, |app| !playing(app)));
    h.frames(2);
    h.click_at(active_row_play(&h), PointerButton::Primary);
    assert!(h.wait_until(3.0, playing), "an audio block should replay after reaching the end");
    h.frames(2);
    h.click(tr.stop());
    assert!(h.wait_until(3.0, |app| app.session().unwrap().media.audio_state().is_some_and(|state| state.blob_id.is_none())));

    let row = h.caption("0:00 / 0:02");
    let open = h.labels(tr.open_audio_player())
        .find(|rect| (rect.center().y - row.center().y).abs() < 8.0).unwrap();
    h.click_at(open.center(), PointerButton::Primary);
    h.frames(3);
    assert!(h.app.session().unwrap().editor_ui.overlay_open());
    h.click(tr.play());
    assert!(h.wait_until(3.0, playing));
    h.click(tr.pause());
    assert!(h.wait_until(3.0, |app| app.session().unwrap().media.audio_state().is_some_and(|state| state.paused)));
    let stop = h.caption(tr.stop());
    let elapsed = h.caption("0:00 / 0:02");
    let to = egui::pos2(egui::lerp(stop.right()..=elapsed.left(), 0.67), stop.center().y);
    h.click_at(to, PointerButton::Primary);
    assert!(h.wait_until(2.0, seeked_while_paused), "clicking the expanded timeline should seek while paused");
    h.click(tr.play());
    assert!(h.wait_until(3.0, playing));
    assert!(h.wait_until(5.0, |app| !playing(app)));
    h.click(tr.replay());
    assert!(h.wait_until(3.0, playing));
    h.click(tr.stop());
    assert!(h.wait_until(3.0, |app| app.session().unwrap().media.audio_state().is_some_and(|state| state.blob_id.is_none())));
    h.click(tr.close());
    h.frames(3);
    assert!(!h.app.session().unwrap().editor_ui.overlay_open());
}

#[test]
#[ignore = "requires headless Microsoft Edge"]
fn click_pdf_export_generates_a_pdf_from_current_unsaved_note() {
    let mut h = workspace("matrix-pdf-menu");
    let tr = h.app.tr();
    h.click(tr.new_note());
    h.frames(4);
    h.text("Printed from the active note");
    h.click(tr.export());
    h.frames(3);
    let path = h.dir.join("note.pdf");
    choose(&mut h, "PDF", Dialog::Export("pdf"), Some(vec![path.clone()]));
    h.caption(tr.export_done());
    assert!(std::fs::read(path).unwrap().starts_with(b"%PDF-"));
}

