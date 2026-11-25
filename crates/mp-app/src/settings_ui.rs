//! 设置窗口。外观、语言、安全三个分区；每一项改动都即时生效并立刻存盘。

use crate::app::MossPocket;
use crate::i18n::{Lang, Tr};
use crate::settings::{Backdrop, Language, Settings, ThemeMode};
use crate::theme;
use eframe::egui::{self};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum SettingsPage {
    #[default]
    Appearance,
    Typography,
    Language,
    Security,
}

/// 设置窗口里几个输入框的内容。密码只放在内存里，关窗即清。
#[derive(Default)]
pub struct SettingsForm {
    /// 当前显示的设置分页。
    pub(crate) section: SettingsPage,
    pub(crate) old_password: String,
    pub(crate) new_password: String,
    pub(crate) confirm_password: String,
    /// 刚重新生成的恢复密钥，给用户看一眼。
    pub(crate) fresh_key: Option<String>,
}

/// 一次设置窗口交互之后，主程序要跟着做的事。
#[derive(Default)]
struct Outcome {
    changed: bool,
    lock: bool,
    message: Option<Result<String, String>>,
    /// 这一帧窗口里有输入框带着焦点。
    text_focus: bool,
}

/// 画设置窗口。返回这一帧有没有输入框带着焦点（有的话，编辑器要让出粘贴等快捷键）。
pub fn show(ui: &mut egui::Ui, app: &mut MossPocket, form: &mut SettingsForm, tr: Tr) -> bool {
    let mut outcome = Outcome::default();
    let palette = theme::Palette::for_ui(ui);
    let screen = ui.ctx().content_rect();
    egui::Window::new(tr.settings())
        .id(egui::Id::unique("settings-window"))
        .title_bar(false)
        .collapsible(false)
        .resizable(true)
        .fade_in(false)
        .default_width(540.0)
        .min_width(300.0)
        .max_width((screen.width() - 32.0).max(300.0))
        .max_height((screen.height() - 56.0).max(220.0))
        .frame(egui::Frame::window(ui.style()).fill(palette.surface).stroke(egui::Stroke::new(1.0, palette.line)).corner_radius(8).inner_margin(20.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
            ui.style_mut().text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tr.settings()).font(theme::display(26.0)).color(palette.ink));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::secondary_button(tr.close(), palette)).clicked() {
                        app.settings_open = false;
                    }
                });
            });
            ui.add_space(theme::SMALL_GAP);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let tab_width = ui.available_width() / 4.0;
                for (page, title) in [
                    (SettingsPage::Appearance, tr.appearance()),
                    (SettingsPage::Typography, tr.typography()),
                    (SettingsPage::Language, tr.language()),
                    (SettingsPage::Security, tr.security()),
                ] {
                    let selected = form.section == page;
                    let button = egui::Button::new(egui::RichText::new(title).size(13.0).color(if selected { palette.ink } else { palette.muted }))
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE).corner_radius(0).selected(selected);
                    let response = ui.add_sized([tab_width, 32.0], button);
                    let highlighted = selected || response.has_focus();
                    let stroke = if highlighted { egui::Stroke::new(2.0, palette.accent) } else { egui::Stroke::new(1.0, palette.line) };
                    ui.painter().hline(response.rect.x_range(), response.rect.bottom(), stroke);
                    if response.clicked() {
                        form.section = page;
                    }
                }
            });
            ui.add_space(10.0);
            egui::ScrollArea::vertical()
                .id_salt(("settings-content", form.section))
                .max_height((screen.height() - 180.0).clamp(100.0, 550.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    match form.section {
                        SettingsPage::Typography => outcome.changed |= typography(ui, app, tr),
                        SettingsPage::Language => outcome.changed |= language_section(ui, app, tr),
                        SettingsPage::Security => {
                            let security = security(ui, app, form, tr);
                            outcome.changed |= security.changed;
                            outcome.lock = security.lock;
                            outcome.message = security.message;
                            outcome.text_focus = security.text_focus;
                        }
                        SettingsPage::Appearance => outcome.changed |= appearance(ui, app, tr),
                    }
                    ui.add_space(8.0);
                });
        });
    if outcome.changed {
        app.settings_changed(ui.ctx());
    }
    if let Some(result) = outcome.message {
        app.push_notice(match result {
            Ok(text) => text,
            // 失败原因已经按界面语言写好了，原样显示。
            Err(text) => text,
        });
    }
    if outcome.lock {
        app.lock_now(ui.ctx());
    }
    outcome.text_focus
}

fn backdrop_name(backdrop: Backdrop, tr: Tr) -> &'static str {
    match backdrop {
        Backdrop::Opaque => tr.backdrop_opaque(),
        Backdrop::Transparent => tr.backdrop_transparent(),
        Backdrop::Acrylic => tr.backdrop_acrylic(),
        Backdrop::Mica => tr.backdrop_mica(),
        Backdrop::SystemAcrylic => tr.backdrop_system_acrylic(),
    }
}

fn appearance(ui: &mut egui::Ui, app: &mut MossPocket, tr: Tr) -> bool {
    let mut changed = false;

    field_label(ui, tr.theme());
    ui.horizontal_wrapped(|ui| {
        for (mode, name) in [
            (ThemeMode::System, tr.theme_system()),
            (ThemeMode::Light, tr.theme_light()),
            (ThemeMode::Dark, tr.theme_dark()),
        ] {
            let selected = app.settings().theme_mode == mode;
            if ui.add(choice_button(ui, name, selected)).clicked() {
                app.settings_mut().theme_mode = mode;
                changed = true;
            }
        }
    });

    ui.add_space(6.0);
    field_label(ui, tr.backdrop());
    ui.horizontal(|ui| {
        let current = backdrop_name(app.settings().backdrop, tr).to_string();
        egui::ComboBox::from_id_salt("backdrop-combo")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for backdrop in [Backdrop::Opaque, Backdrop::Transparent, Backdrop::Acrylic, Backdrop::Mica, Backdrop::SystemAcrylic] {
                    let selected = app.settings().backdrop == backdrop;
                    if ui.selectable_label(selected, backdrop_name(backdrop, tr)).clicked() {
                        app.settings_mut().backdrop = backdrop;
                        changed = true;
                    }
                }
            });
    });

    let backdrop = app.settings().backdrop;
    let effective = crate::theme::tint(app.settings(), ui.visuals().dark_mode);
    let mut tint = [effective.r(), effective.g(), effective.b()];
    ui.add_space(4.0);
    ui.add_enabled_ui(backdrop != Backdrop::Opaque, |ui| {
        ui.horizontal(|ui| {
            setting_row_label(ui, tr.tint_color());
            if ui.color_edit_button_srgb(&mut tint).changed() {
                app.settings_mut().tint = u32::from_be_bytes([0, tint[0], tint[1], tint[2]]);
                changed = true;
            }
        });
        if backdrop == Backdrop::Acrylic {
            ui.label(egui::RichText::new(tr.backdrop_acrylic_note()).weak().small());
        }
        changed |= slider_row(ui, app, tr.opacity(), 0.0..=255.0, |settings| f32::from(settings.tint_alpha), |settings, value| {
            settings.tint_alpha = value as u8;
        });
    });
    changed |= color_row(ui, app, tr.accent_color(), |settings| &mut settings.accent);
    changed |= slider_row(ui, app, tr.corner_radius(), 0.0..=24.0, |settings| settings.corner_radius, |settings, value| {
        settings.corner_radius = value;
    });
    changed
}

fn typography(ui: &mut egui::Ui, app: &mut MossPocket, tr: Tr) -> bool {
    let settings = app.settings_mut();
    let mut changed = font_picker(ui, "heading-font", tr.heading_font(), &mut settings.heading_font, tr);
    changed |= font_picker(ui, "body-font", tr.body_font(), &mut settings.body_font, tr);
    ui.add_space(theme::SMALL_GAP);
    let settings = app.settings();
    let palette = theme::Palette::for_ui(ui);
    egui::Frame::new()
        .fill(palette.rail)
        .stroke(egui::Stroke::new(1.0, palette.line))
        .corner_radius(6)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::kicker(tr.font_preview(), palette));
            ui.label(egui::RichText::new(tr.font_preview_title()).font(crate::fonts::note_font((settings.base_font_size * 2.55).min(48.0), true, false)));
            ui.add(egui::Label::new(egui::RichText::new(tr.font_preview_body()).font(crate::fonts::note_font(settings.base_font_size, false, false))).wrap());
        });
    ui.add_space(theme::SMALL_GAP);
    changed |= slider_row(ui, app, tr.font_size(), 11.0..=28.0, |settings| settings.base_font_size, |settings, value| {
        settings.base_font_size = value;
    });
    changed |= slider_row(ui, app, tr.line_spacing(), 1.0..=3.0, |settings| settings.line_spacing, |settings, value| {
        settings.line_spacing = value;
    });
    changed |= slider_row(ui, app, tr.paragraph_spacing(), 0.0..=40.0, |settings| settings.paragraph_spacing, |settings, value| {
        settings.paragraph_spacing = value;
    });
    changed
}

fn font_picker(ui: &mut egui::Ui, id: &str, label: &str, selected: &mut String, tr: Tr) -> bool {
    let faces = crate::fonts::available_faces();
    let name = |face: &crate::fonts::Face| match tr.0 {
        Lang::Zh => face.chinese_name,
        Lang::En => face.english_name,
    };
    let current = faces.iter().find(|face| face.key == selected.as_str()).map(|face| name(face));
    let missing = selected.as_str() != crate::fonts::DEFAULT && current.is_none();
    let mut changed = false;
    ui.horizontal(|ui| {
        setting_row_label(ui, label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(current.unwrap_or_else(|| tr.default_font()))
            .width(ui.available_width().clamp(92.0, 200.0))
            .show_ui(ui, |ui| {
                if ui.selectable_label(selected.as_str() == crate::fonts::DEFAULT, tr.default_font()).clicked() {
                    *selected = crate::fonts::DEFAULT.into();
                    changed = true;
                }
                for face in faces {
                    if ui.selectable_label(face.key == selected.as_str(), name(face)).clicked() {
                        *selected = face.key.into();
                        changed = true;
                    }
                }
            });
    });
    if missing {
        ui.label(egui::RichText::new(tr.font_unavailable()).small().weak());
    }
    changed
}

fn color_row(ui: &mut egui::Ui, app: &mut MossPocket, title: &str, target: fn(&mut Settings) -> &mut u32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        setting_row_label(ui, title);
        let color = theme::color(*target(app.settings_mut()));
        let mut srgb = [color.r(), color.g(), color.b()];
        // 颜色按 sRGB 字节值存，所以用 sRGB 的取色器，所见即所得。
        if ui.color_edit_button_srgb(&mut srgb).changed() {
            *target(app.settings_mut()) = (u32::from(srgb[0]) << 16) | (u32::from(srgb[1]) << 8) | u32::from(srgb[2]);
            changed = true;
        }
    });
    changed
}

fn slider_row(
    ui: &mut egui::Ui,
    app: &mut MossPocket,
    title: &str,
    range: std::ops::RangeInclusive<f32>,
    read: fn(&Settings) -> f32,
    write: fn(&mut Settings, f32),
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        setting_row_label(ui, title);
        ui.spacing_mut().slider_width = (ui.available_width() - 72.0).clamp(60.0, 220.0);
        let mut value = read(app.settings());
        if ui.add(egui::Slider::new(&mut value, range)).changed() {
            write(app.settings_mut(), value);
            changed = true;
        }
    });
    changed
}

fn language_section(ui: &mut egui::Ui, app: &mut MossPocket, tr: Tr) -> bool {
    let mut changed = false;
    // 语言名用它自己的写法，切换之前也认得出。
    ui.horizontal_wrapped(|ui| {
        for (language, name) in [(Language::System, tr.lang_system()), (Language::Zh, "简体中文"), (Language::En, "English")] {
            let selected = app.settings().language == language;
            if ui.add(choice_button(ui, name, selected)).clicked() {
                app.settings_mut().language = language;
                changed = true;
            }
        }
    });
    changed
}

/// 一个密码输入框。返回它这一帧是否有焦点。
fn password_field(ui: &mut egui::Ui, id: &'static str, value: &mut String, title: &str) -> bool {
    ui.vertical(|ui| {
        field_label(ui, title);
        egui::TextEdit::singleline(value)
            .id(egui::Id::unique(id))
            .password(true)
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(10, 8))
            .show(ui)
            .response
            .has_focus()
    })
    .inner
}

fn security(ui: &mut egui::Ui, app: &mut MossPocket, form: &mut SettingsForm, tr: Tr) -> Outcome {
    let mut outcome = Outcome::default();
    let palette = theme::Palette::for_ui(ui);

    field_label(ui, tr.auto_lock());
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 100.0).clamp(80.0, 240.0);
        let mut minutes = app.settings().auto_lock_minutes.min(240) as f32;
        let slider = egui::Slider::new(&mut minutes, 0.0..=240.0)
            .integer()
            .custom_formatter(|value, _| {
                if value <= 0.0 {
                    tr.auto_lock_off().to_string()
                } else {
                    format!("{value} {}", tr.minutes())
                }
            });
        if ui.add(slider).changed() {
            app.settings_mut().auto_lock_minutes = minutes as u32;
            outcome.changed = true;
        }
    });

    // 改密码和恢复密钥要在解锁状态下做。
    if app.is_unlocked() {
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(egui::RichText::new(tr.change_password()).font(theme::display(22.0)).color(palette.ink));
        let mut text_focus = false;
        text_focus |= password_field(ui, "settings-old-password", &mut form.old_password, tr.old_password());
        text_focus |= password_field(ui, "settings-new-password", &mut form.new_password, tr.new_password());
        text_focus |= password_field(ui, "settings-confirm-password", &mut form.confirm_password, tr.confirm_password());
        outcome.text_focus = text_focus;
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::primary_button(tr.apply(), palette)).clicked() {
                outcome.message =
                    Some(crate::settings_actions::change_password(app, form).map(|_| tr.password_changed().to_string()));
            }
            if ui.button(tr.regenerate_recovery()).clicked() {
                outcome.message =
                    Some(crate::settings_actions::regenerate_key(app, form).map(|_| tr.recovery_regenerated().to_string()));
            }
        });
        if let Some(key) = form.fresh_key.clone() {
            egui::Frame::new().fill(palette.rail).stroke(egui::Stroke::new(1.0, palette.line)).corner_radius(0).inner_margin(12.0).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(key.as_str()).strong().monospace()).selectable(true).wrap());
                if ui.button(tr.copy()).clicked() {
                    ui.ctx().copy_text(key.clone());
                }
            });
        }
        ui.add_space(12.0);
        if ui.button(tr.lock()).clicked() {
            outcome.lock = true;
        }
    }
    outcome
}

fn choice_button(ui: &egui::Ui, label: &str, selected: bool) -> egui::Button<'static> {
    let palette = theme::Palette::for_ui(ui);
    egui::Button::new(egui::RichText::new(label.to_owned()).size(13.0).color(if selected { palette.accent } else { palette.ink }))
        .fill(if selected { palette.accent_soft } else { palette.surface })
        .stroke(egui::Stroke::new(1.0, if selected { palette.accent } else { palette.line }))
        .corner_radius(5)
        .min_size(egui::vec2(0.0, 30.0))
}

fn field_label(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).size(12.0).color(theme::Palette::for_ui(ui).muted));
}

fn setting_row_label(ui: &mut egui::Ui, label: &str) {
    let width = (ui.available_width() * 0.34).clamp(90.0, 156.0);
    ui.add_sized([width, 22.0], egui::Label::new(label).halign(egui::Align::Min).selectable(false));
}
