//! 锁屏与首次建库。

use crate::i18n::Tr;
use crate::theme::{self, Palette};
use eframe::egui::{self};
use mp_core::vault::MIN_PASSWORD_LEN;

/// 锁屏界面状态机里下一个要拿焦点的控件。
///
/// 不能只用一个「下一帧抢焦点」的开关：密码框、确认框、继续按钮共用它会互相抢。
/// 另外控件包在浮动区域里时，第一帧请求焦点会被 egui 丢掉，所以要一直请求到真的拿到为止。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Secret,
    Confirm,
    Continue,
}

/// 锁屏界面的状态。
#[derive(Default)]
pub struct LockUi {
    pub secret: String,
    pub confirm: String,
    pub use_recovery: bool,
    pub error: Option<String>,
    pub busy: bool,
    /// 建库成功后要交给用户的恢复密钥；有值时显示「保存恢复密钥」那一页。
    pub recovery: Option<String>,
    pub copied: bool,
    focus_next: Option<FocusTarget>,
    /// 这次锁定是不是闲置自动触发的，用来显示一句提示。
    pub auto_locked: bool,
}

impl LockUi {
    pub fn fresh(auto_locked: bool) -> Self {
        Self {
            focus_next: Some(FocusTarget::Secret),
            auto_locked,
            ..Default::default()
        }
    }

    /// 让密码输入框重新拿焦点（比如口令输错之后）。
    pub fn focus_secret(&mut self) {
        self.focus_next = Some(FocusTarget::Secret);
    }

    /// 让「继续」按钮拿焦点（刚进「保存恢复密钥」页时）。
    pub fn focus_continue(&mut self) {
        self.focus_next = Some(FocusTarget::Continue);
    }
}

/// 用户在锁屏上触发的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockAction {
    Unlock { recovery: bool },
    Create,
    Continue,
}

/// 一行密码输入。返回 (输入框的响应, 用户是否按了回车)。
fn secret_input(
    ui: &mut egui::Ui,
    id: &'static str,
    target: FocusTarget,
    value: &mut String,
    hint: &str,
    mask: bool,
    focus_next: &mut Option<FocusTarget>,
) -> (egui::Response, bool) {
    let response = egui::TextEdit::singleline(value)
        .id(egui::Id::unique(id))
        .password(mask)
        .hint_text(hint)
        .desired_width(f32::INFINITY)
        .margin(egui::Margin::symmetric(10, 8))
        .show(ui)
        .response
        .response;
    if *focus_next == Some(target) {
        if response.has_focus() {
            *focus_next = None;
        } else {
            response.request_focus();
        }
    }
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    (response, enter)
}

fn show_error(ui: &mut egui::Ui, message: Option<&str>) {
    if let Some(message) = message {
        ui.add_space(8.0);
        ui.add(egui::Label::new(egui::RichText::new(message).color(Palette::for_ui(ui).danger)).wrap());
    }
}

/// 画锁屏。返回用户这次触发的动作。
pub fn show(
    ui: &mut egui::Ui,
    lock: &mut LockUi,
    tr: &Tr,
    vault_exists: bool,
    // 库头读不出来时的原因。
    vault_error: Option<&mp_core::CoreError>,
) -> Option<LockAction> {
    let mut action = None;
    let palette = Palette::for_ui(ui);
    let width = (ui.available_width() - 48.0).clamp(210.0, 368.0);
    let height = (ui.available_height() - 20.0).clamp(274.0, 470.0);
    let compact = ui.available_height() < 530.0;
    egui::Area::new(egui::Id::unique("lock-card"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .fade_in(false)
        .show(ui, |ui| {
            let vertical_padding = 14;
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(22, vertical_padding))
                .show(ui, |ui| {
                    ui.set_width(width - 44.0);
                    let top = ui.cursor().top();
                    theme::brand_lockup(ui, palette);
                    ui.add_space(if compact { 16.0 } else { theme::SECTION_GAP });
                    let brand_height = ui.cursor().top() - top;
                    let form_height = height - brand_height - 2.0 * f32::from(vertical_padding);
                    let form_bounds = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width - 44.0, form_height));
                    // An Area starts with the previous frame's size. Give the scroll view the
                    // current viewport budget so new errors and larger layouts can grow.
                    ui.scope_builder(egui::UiBuilder::new().max_rect(form_bounds), |ui| {
                        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = if compact { 4.0 } else { 6.0 };
                            if let Some(key) = lock.recovery.clone() {
                                action = recovery_page(ui, lock, tr, &key);
                            } else if !vault_exists && vault_error.is_none() {
                                action = setup_page(ui, lock, tr, compact);
                            } else {
                                action = unlock_page(ui, lock, tr, vault_error);
                            }
                        });
                    });
                });
        });
    action
}

/// 建库之后的「保存恢复密钥」页。
fn recovery_page(ui: &mut egui::Ui, lock: &mut LockUi, tr: &Tr, key: &str) -> Option<LockAction> {
    let palette = Palette::for_ui(ui);
    ui.label(egui::RichText::new(tr.recovery_title()).font(egui::FontId::proportional(29.0)).color(palette.ink));
    ui.add_space(4.0);
    ui.add(egui::Label::new(tr.recovery_sub()).wrap());
    ui.add_space(10.0);
    egui::Frame::new().fill(palette.rail).stroke(egui::Stroke::new(1.0, palette.line)).corner_radius(0).inner_margin(16.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add(egui::Label::new(egui::RichText::new(key).strong().monospace().size(17.0).color(palette.ink)).selectable(true).wrap());
        });
    });
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(tr.copy())).clicked() {
            ui.ctx().copy_text(key.to_string());
            lock.copied = true;
        }
        if lock.copied {
            ui.label(egui::RichText::new(tr.copied()).weak());
        }
    });
    ui.add_space(4.0);
    // 刚进这一页时让「继续」按钮拿焦点，回车或空格就能走下一步。
    let button = ui.add_sized([ui.available_width(), 40.0], theme::primary_button(tr.saved_continue(), palette));
    if lock.focus_next == Some(FocusTarget::Continue) {
        if button.has_focus() {
            lock.focus_next = None;
        } else {
            button.request_focus();
        }
    }
    button.clicked().then_some(LockAction::Continue)
}

/// 第一次使用：设置密码。
fn setup_page(ui: &mut egui::Ui, lock: &mut LockUi, tr: &Tr, compact: bool) -> Option<LockAction> {
    let palette = Palette::for_ui(ui);
    let title_size = if compact { 28.0 } else { 32.0 };
    ui.label(egui::RichText::new(tr.setup_title()).font(egui::FontId::proportional(title_size)).color(palette.ink));
    ui.add_space(4.0);
    let description = egui::RichText::new(tr.setup_sub()).size(if compact { 13.0 } else { 15.0 });
    ui.add(egui::Label::new(description).wrap());
    ui.add_space(if compact { 10.0 } else { 16.0 });
    ui.label(tr.password());
    let (_, enter) = secret_input(
        ui,
        "lock-password",
        FocusTarget::Secret,
        &mut lock.secret,
        tr.password_hint(),
        true,
        &mut lock.focus_next,
    );
    // 第一个框里按回车：把焦点挪到确认框，方便纯键盘操作。
    if enter && lock.focus_next.is_none() {
        lock.focus_next = Some(FocusTarget::Confirm);
    }
    ui.add_space(4.0);
    ui.label(tr.confirm_password());
    let (_, enter_confirm) = secret_input(
        ui,
        "lock-confirm",
        FocusTarget::Confirm,
        &mut lock.confirm,
        tr.confirm_password(),
        true,
        &mut lock.focus_next,
    );
    ui.add_space(12.0);
    let clicked = ui
        .add_enabled_ui(!lock.busy, |ui| ui.add_sized([ui.available_width(), 40.0], theme::primary_button(if lock.busy { tr.creating() } else { tr.create_vault() }, palette)))
        .inner
        .clicked();

    let mut action = None;
    if (enter_confirm || clicked) && !lock.busy {
        if lock.secret.chars().count() < MIN_PASSWORD_LEN {
            lock.error = Some(tr.password_too_short().into());
            lock.focus_next = Some(FocusTarget::Secret);
        } else if lock.secret != lock.confirm {
            lock.error = Some(tr.password_mismatch().into());
            lock.focus_next = Some(FocusTarget::Confirm);
        } else {
            lock.error = None;
            action = Some(LockAction::Create);
        }
    }
    show_error(ui, lock.error.as_deref());
    action
}

/// 日常解锁。
fn unlock_page(
    ui: &mut egui::Ui,
    lock: &mut LockUi,
    tr: &Tr,
    vault_error: Option<&mp_core::CoreError>,
) -> Option<LockAction> {
    let palette = Palette::for_ui(ui);
    ui.label(egui::RichText::new(tr.unlock_title()).font(egui::FontId::proportional(32.0)).color(palette.ink));
    if let Some(error) = vault_error {
        // 库头读不出来时不给输入框，只告诉用户出了什么问题。
        ui.add_space(8.0);
        ui.add(egui::Label::new(tr.core_error(error)).wrap());
        return None;
    }
    ui.add_space(2.0);
    let subtitle = if lock.use_recovery { tr.unlock_recovery_sub() } else { tr.unlock_sub() };
    ui.add(egui::Label::new(egui::RichText::new(subtitle).weak()).wrap());
    if lock.auto_locked {
        ui.add(egui::Label::new(egui::RichText::new(tr.auto_locked()).weak().small()).wrap());
    }
    ui.add_space(16.0);
    ui.label(if lock.use_recovery { tr.recovery_key() } else { tr.password() });
    let hint = if lock.use_recovery { tr.recovery_key_hint() } else { tr.password_hint() };
    let (_, enter) = secret_input(
        ui,
        "lock-secret",
        FocusTarget::Secret,
        &mut lock.secret,
        hint,
        !lock.use_recovery,
        &mut lock.focus_next,
    );
    ui.add_space(12.0);
    let clicked = ui
        .add_enabled_ui(!lock.busy, |ui| ui.add_sized([ui.available_width(), 40.0], theme::primary_button(if lock.busy { tr.unlocking() } else { tr.unlock() }, palette)))
        .inner
        .clicked();
    let mut action = None;
    if (enter || clicked) && !lock.busy && !lock.secret.trim().is_empty() {
        lock.error = None;
        action = Some(LockAction::Unlock {
            recovery: lock.use_recovery,
        });
    }
    show_error(ui, lock.error.as_deref());
    ui.add_space(8.0);
    let toggle = if lock.use_recovery { tr.use_password() } else { tr.use_recovery_key() };
    let link = ui
        .add_enabled_ui(!lock.busy, |ui| {
            ui.add(egui::Button::new(egui::RichText::new(toggle).color(palette.accent)).frame(false))
        })
        .inner
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if link.clicked() {
        lock.use_recovery = !lock.use_recovery;
        lock.secret.clear();
        lock.error = None;
        lock.focus_secret();
    }
    action
}
