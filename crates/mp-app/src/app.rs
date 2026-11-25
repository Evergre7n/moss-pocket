//! 应用主状态机：锁定、首次建库、已解锁三种状态之间的切换，以及自动保存、自动锁定、关窗保存。

use crate::i18n::{Lang, Tr};
use crate::lock_screen::{self, LockAction, LockUi};
use crate::notes_ui::{self, TopAction};
use crate::session::Session;
use crate::settings::{Backdrop, Language, Settings};
use crate::settings_ui::{self, SettingsForm};
use crate::theme;
use crate::win::BackdropApplier;
use eframe::egui;
use mp_core::CoreError;
use mp_core::vault::{UnlockSecret, Vault, VaultHeader};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

pub const APP_TITLE: &str = "MOSS-Pocket";
/// 停手多久之后自动保存。
const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);
/// 底部提示停留多久。
const NOTICE_DURATION: Duration = Duration::from_secs(3);

/// 后台线程回传的结果。解锁要算一阵子密钥，放在后台做，界面不会卡住。
/// 错误带着原始类型回来，到显示的时候才按当前语言写。
enum Msg {
    Unlocked(Result<Vault, CoreError>),
    Created(Result<(Vault, String), CoreError>),
}

enum State {
    Locked(LockUi),
    Unlocked(Session),
}

#[derive(Clone, Copy)]
enum ExitAfterRecording {
    Lock { auto: bool },
    Close,
}

pub struct MossPocket {
    exe_dir: PathBuf,
    settings: Settings,
    tr: Tr,
    state: State,
    exit_after_recording: Option<ExitAfterRecording>,
    inbox: Receiver<Msg>,
    last_activity: Instant,
    backdrop: BackdropApplier,
    notice: Option<(String, Instant)>,
    /// 建好但还没进入的库，等用户点「我已保存」。
    pending_vault: Option<Vault>,
    vault_exists: bool,
    /// 库头读不出来时的原因。带着原始类型，显示时才按当前语言写。
    vault_error: Option<CoreError>,
    /// 无窗口测试里为 true：不碰任何真实窗口。
    headless: bool,
    pub(crate) settings_open: bool,
    /// 设置窗口里几个输入框的内容。
    pub(crate) settings_form: SettingsForm,
    /// 自测模式：把启动耗时和内存写到这个文件后退出。
    benchmark: Option<PathBuf>,
    frames: u64,
    /// 第一帧开始画的时刻，相对进程创建，毫秒。
    first_frame_ms: f64,
    /// 装字体花了多少毫秒、私有内存涨了多少 KB。自测用。
    fonts_ms: f64,
    fonts_kb: isize,
    /// 窗口和显卡环境建好、开始建应用的时刻，相对进程创建，毫秒。自测用。
    window_ready_ms: f64,
}

impl MossPocket {
    /// 启动应用。`benchmark` 有值时进入自测模式（见 `main.rs` 的说明）。
    pub fn start(cc: &eframe::CreationContext<'_>, exe_dir: PathBuf, benchmark: Option<PathBuf>) -> Self {
        let mut app = Self::with_context(cc.egui_ctx.clone(), exe_dir, benchmark.is_some());
        app.benchmark = benchmark;
        app.window_ready_ms = crate::win::millis_since_process_start();
        // 自测时用不透明底：磨砂材质要系统合成，数字量不准也没必要。
        if app.benchmark.is_some() {
            app.settings.backdrop = Backdrop::Opaque;
            theme::apply(&cc.egui_ctx, &app.settings);
        }
        app
    }

    fn with_context(ctx: egui::Context, exe_dir: PathBuf, headless: bool) -> Self {
        let settings = Settings::load(&exe_dir);
        let lang = resolve_language(settings.language);
        let (fonts_ms, fonts_kb) = {
            let (before, clock) = (crate::win::memory_kb().1, Instant::now());
            crate::fonts::install(&ctx, &settings);
            (clock.elapsed().as_secs_f64() * 1000.0, crate::win::memory_kb().1 as isize - before as isize)
        };
        theme::apply(&ctx, &settings);

        let (_sender, inbox) = channel();
        let mut app = Self {
            exe_dir,
            settings,
            tr: Tr(lang),
            state: State::Locked(LockUi::fresh(false)),
            exit_after_recording: None,
            inbox,
            last_activity: Instant::now(),
            backdrop: BackdropApplier::default(),
            notice: None,
            pending_vault: None,
            vault_exists: false,
            vault_error: None,
            headless,
            settings_open: false,
            settings_form: SettingsForm::default(),
            benchmark: None,
            frames: 0,
            first_frame_ms: 0.0,
            fonts_ms,
            fonts_kb,
            window_ready_ms: 0.0,
        };
        app.refresh_vault_presence();
        app
    }

    /// 重新看一眼数据目录里有没有库。
    fn refresh_vault_presence(&mut self) {
        let data_dir = Vault::data_dir(&self.exe_dir);
        let (exists, error) = match VaultHeader::load(&data_dir) {
            Ok(Some(_)) => (true, None),
            Ok(None) => (false, None),
            Err(error) => (false, Some(error)),
        };
        self.vault_exists = exists;
        self.vault_error = error;
    }

    pub(crate) fn push_notice(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), Instant::now()));
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// 当前界面语言。
    pub(crate) fn tr(&self) -> Tr {
        self.tr
    }

    /// 立刻锁定（设置窗口里的「锁定」按钮）。
    pub(crate) fn lock_now(&mut self, ctx: &egui::Context) {
        self.lock(false, ctx);
    }

    /// 设置有改动：夹回可用范围、即时套到界面上、存盘。
    pub(crate) fn settings_changed(&mut self, ctx: &egui::Context) {
        self.settings.sanitize();
        self.tr = Tr(resolve_language(self.settings.language));
        // 换了语言，会话里报失败用的语言也跟着换。
        let tr = self.tr;
        if let Some(session) = self.session_mut() {
            session.tr = tr;
        }
        crate::fonts::install(ctx, &self.settings);
        theme::apply(ctx, &self.settings);
        if let Err(error) = self.settings.save(&self.exe_dir) {
            self.push_notice(format!("{}：{error}", self.tr.save_failed()));
        }
    }

    /// 存盘并回到锁屏。`auto` 表示这次锁定是闲置自动触发的。
    fn lock(&mut self, auto: bool, ctx: &egui::Context) {
        if self.wait_for_recording_exit(ExitAfterRecording::Lock { auto }) {
            return;
        }
        if self.import_busy() {
            if !auto {
                self.push_notice(self.tr.import_in_progress());
            }
            return;
        }
        if !self.save_all() {
            self.last_activity = Instant::now();
            return;
        }
        if let Some(text) = self.session_mut().and_then(|session| session.notice.take()) {
            self.push_notice(text);
        }
        self.pending_vault = None;
        self.settings_open = false;
        self.settings_form = SettingsForm::default();
        self.state = State::Locked(LockUi::fresh(auto));
        self.refresh_vault_presence();
        ctx.request_repaint();
    }

    fn import_busy(&self) -> bool {
        matches!(&self.state, State::Unlocked(session) if session.media.pending_import_count() > 0)
    }

    fn wait_for_recording_exit(&mut self, request: ExitAfterRecording) -> bool {
        if self.exit_after_recording.is_some() {
            if matches!(request, ExitAfterRecording::Close) {
                self.exit_after_recording = Some(request);
            }
            return true;
        }
        let Some(session) = self.session_mut().filter(|session| session.recording_has_work()) else { return false };
        session.finish_recording();
        if !session.queue_recordings() {
            self.last_activity = Instant::now();
            return true;
        }
        if session.media.pending_import_count() > 0 {
            self.exit_after_recording = Some(request);
            self.push_notice(self.tr.import_in_progress());
            return true;
        }
        false
    }

    fn finish_recording_exit(&mut self, ctx: &egui::Context) -> bool {
        if self.import_busy() {
            return false;
        }
        let Some(request) = self.exit_after_recording.take() else { return false };
        if self.notice.as_ref().is_some_and(|(message, _)| message == self.tr.import_in_progress()) {
            self.notice = None;
        }
        if self.session_mut().is_some_and(|session| session.recording_retry_pending()) {
            self.last_activity = Instant::now();
            return false;
        }
        match request {
            ExitAfterRecording::Lock { auto } => {
                self.lock(auto, ctx);
                false
            }
            ExitAfterRecording::Close => {
                if !self.save_before_close() {
                    return false;
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                true
            }
        }
    }

    fn save_before_close(&mut self) -> bool {
        if self.save_all() {
            return true;
        }
        let reason = self.notice.take().map_or_else(String::new, |(text, _)| format!("\n{text}"));
        self.push_notice(format!("{}{reason}", self.tr.save_before_close()));
        false
    }

    fn save_all(&mut self) -> bool {
        if let State::Unlocked(session) = &mut self.state {
            session.finish_audio_rename();
            if !session.save_current() {
                let failure = session.notice.take();
                self.push_notice(failure.unwrap_or_else(|| self.tr.save_failed().to_string()));
                return false;
            }
        }
        true
    }

    fn poll_messages(&mut self) {
        while let Ok(message) = self.inbox.try_recv() {
            match (message, &mut self.state) {
                (Msg::Unlocked(Ok(_)), State::Unlocked(_)) => {}
                (Msg::Unlocked(Ok(vault)), _) => {
                    self.last_activity = Instant::now();
                    self.state = State::Unlocked(Session::new(vault, self.tr));
                }
                (Msg::Unlocked(Err(error)), State::Locked(lock)) => {
                    lock.busy = false;
                    // 口令输错了，把焦点交还给输入框。
                    lock.focus_secret();
                    // 口令不对是最常见的情况，换成看得懂的说法；其余错误按类型解释。
                    lock.error = Some(if matches!(error, CoreError::Decrypt(_)) {
                        if lock.use_recovery {
                            self.tr.wrong_recovery_key().to_string()
                        } else {
                            self.tr.wrong_password().to_string()
                        }
                    } else {
                        self.tr.core_error(&error)
                    });
                }
                (Msg::Created(Ok((vault, key))), State::Locked(lock)) => {
                    lock.busy = false;
                    lock.error = None;
                    lock.recovery = Some(key);
                    lock.secret.clear();
                    lock.confirm.clear();
                    // 进到「保存恢复密钥」页时，让「继续」按钮拿焦点，回车就能走下一步。
                    lock.focus_continue();
                    self.pending_vault = Some(vault);
                }
                (Msg::Created(Err(error)), State::Locked(lock)) => {
                    lock.busy = false;
                    lock.error = Some(self.tr.core_error(&error));
                }
                _ => {}
            }
        }
    }

    fn show_notice(&mut self, ui: &mut egui::Ui) {
        let Some((text, at)) = &self.notice else {
            return;
        };
        let remaining = NOTICE_DURATION.saturating_sub(at.elapsed());
        if remaining.is_zero() {
            self.notice = None;
            return;
        }
        ui.ctx().request_repaint_after(remaining);
        let width = (ui.ctx().content_rect().width() - 56.0).clamp(1.0, 460.0);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let message = ui.painter().layout(text.clone(), font, ui.visuals().text_color(), width);
        egui::Area::new(egui::Id::unique("notice"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -28.0))
            .order(egui::Order::Foreground)
            .show(ui, |ui| {
                egui::Frame::window(ui.style())
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| ui.label(message));
            });
    }

    /// 自测：第二帧进来时首帧已经上屏。记下数字、写文件、退出。
    fn finish_benchmark(&mut self, ui: &egui::Ui) {
        let Some(path) = &self.benchmark else {
            return;
        };
        if self.frames != 2 {
            return;
        }
        let (working_set, private) = crate::win::memory_kb();
        let _ = std::fs::write(
            path,
            format!(
                concat!(
                    "{{\"first_frame_ms\":{:.1},\"second_frame_ms\":{:.1},\"ws_kb\":{},\"private_kb\":{},",
                    "\"fonts_ms\":{:.1},\"fonts_kb\":{},\"window_ready_ms\":{:.1}}}\n"
                ),
                self.first_frame_ms,
                crate::win::millis_since_process_start(),
                working_set,
                private,
                self.fonts_ms,
                self.fonts_kb,
                self.window_ready_ms
            ),
        );
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// 画一帧。窗口程序和无窗口测试都从这里进，所以测到的就是真实界面。
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.frames += 1;
        if self.benchmark.is_some() {
            self.finish_benchmark(ui);
        }
        if self.frames == 1 && self.benchmark.is_some() {
            self.first_frame_ms = crate::win::millis_since_process_start();
        }
        self.poll_messages();
        if let Some(session) = self.session_mut() {
            session.poll_media(ui.ctx());
        }
        if self.settings_open && !ui.any_popup_open()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.settings_open = false;
            self.settings_form = SettingsForm::default();
        }
        // 剪贴板快捷键最先处理：要在任何控件看到按键之前。
        if self.exit_after_recording.is_none() {
            crate::clipboard::handle_keys(ui, self);
        }
        if ui.input(|i| !i.events.is_empty() || i.pointer.any_down()) {
            self.last_activity = Instant::now();
        }
        if self.is_unlocked() && self.settings.auto_lock_minutes > 0 {
            let timeout = Duration::from_secs(u64::from(self.settings.auto_lock_minutes) * 60);
            let remaining = timeout.saturating_sub(self.last_activity.elapsed());
            if remaining.is_zero() {
                self.lock(true, ui.ctx());
            } else {
                ui.ctx().request_repaint_after(remaining);
            }
        }
        // 关窗：先把没存的写盘，再放行。
        let mut closing = false;
        if ui.input(|i| i.viewport().close_requested()) {
            if self.wait_for_recording_exit(ExitAfterRecording::Close) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.import_busy() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.push_notice(self.tr.import_in_progress());
            } else {
                closing = self.save_before_close();
                if !closing {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            }
        }
        closing |= self.finish_recording_exit(ui.ctx());
        let (native_drag, native_material) = if self.headless {
            (false, true)
        } else {
            let dark = ui.ctx().theme() == egui::Theme::Dark;
            let native_material = self.backdrop.apply(APP_TITLE, self.settings.backdrop, dark);
            let scale = ui.ctx().pixels_per_point();
            let native_drag = self.backdrop.install_chrome(
                APP_TITLE,
                (crate::window_ui::HEIGHT * scale).round() as u16,
                (crate::window_ui::CONTROL_STRIP * scale).round() as u16,
            );
            (native_drag, native_material)
        };
        theme::paint_backdrop(ui, &self.settings, native_material);
        crate::window_ui::show(ui, &self.tr, native_drag);

        let mut lock_request: Option<bool> = None;
        if matches!(self.state, State::Unlocked(_))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::L))
        {
            lock_request = Some(false);
        }

        let mut continue_requested = false;
        let mut pending_notice: Option<String> = None;
        let mut settings_requested = false;
        match &mut self.state {
            State::Locked(lock) => {
                if let Some(action) = lock_screen::show(
                    ui,
                    lock,
                    &self.tr,
                    self.vault_exists,
                    self.vault_error.as_ref(),
                ) {
                    match action {
                        LockAction::Continue => continue_requested = true,
                        LockAction::Unlock { recovery } => {
                            lock.busy = true;
                            lock.error = None;
                            let secret = if recovery { lock.secret.trim().to_string() } else { lock.secret.clone() };
                            let root = Vault::data_dir(&self.exe_dir);
                            let ctx = ui.ctx().clone();
                            let (sender, receiver) = channel();
                            self.inbox = receiver;
                            std::thread::spawn(move || {
                                let secret = if recovery {
                                    UnlockSecret::RecoveryKey(&secret)
                                } else {
                                    UnlockSecret::Password(&secret)
                                };
                                let result = Vault::unlock(&root, secret);
                                let _ = sender.send(Msg::Unlocked(result));
                                ctx.request_repaint();
                            });
                        }
                        LockAction::Create => {
                            lock.busy = true;
                            lock.error = None;
                            let password = lock.secret.clone();
                            let root = Vault::data_dir(&self.exe_dir);
                            let ctx = ui.ctx().clone();
                            let (sender, receiver) = channel();
                            self.inbox = receiver;
                            std::thread::spawn(move || {
                                let result = Vault::create(&root, &password);
                                let _ = sender.send(Msg::Created(result));
                                ctx.request_repaint();
                            });
                        }
                    }
                }
            }
            State::Unlocked(session) => {
                session.begin_frame_focus();
                let overlay_open = session.editor_ui.overlay_open() || session.export_picker.is_some();
                let workspace_enabled = !closing && !self.settings_open && self.exit_after_recording.is_none();
                let action = ui.add_enabled_ui(workspace_enabled && !overlay_open, |ui| {
                    notes_ui::show(ui, session, &self.settings, &self.tr)
                }).inner;
                if workspace_enabled {
                    crate::editor_ui::show_overlay(ui, session, self.tr);
                    crate::export_ui::show_picker(ui, session, self.tr);
                } else {
                    crate::editor_ui::hide_native_overlay(ui, session);
                }
                if session.media.has_background_work() {
                    ui.ctx().request_repaint_after(Duration::from_millis(80));
                } else if session.recording_elapsed().is_some() {
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
                // 停手片刻后自动保存。
                if let Some(edited) = session.last_edit {
                    let remaining = AUTOSAVE_DELAY.saturating_sub(edited.elapsed());
                    if !remaining.is_zero() {
                        ui.ctx().request_repaint_after(remaining);
                    } else {
                        session.last_edit = None;
                        let _ = session.save_current();
                        ui.ctx().request_repaint();
                    }
                }
                if let Some(text) = session.notice.take() {
                    pending_notice = Some(text);
                }
                match action {
                    Some(TopAction::Lock) => lock_request = Some(false),
                    Some(TopAction::Settings) => settings_requested = true,
                    None => {}
                }
            }
        }
        if let Some(text) = pending_notice {
            self.push_notice(text);
        }
        if settings_requested {
            self.settings_open = true;
        }
        if self.settings_open {
            // 表单先拿出来再用：设置窗口里的代码要同时借用应用的其余部分。
            let mut form = std::mem::take(&mut self.settings_form);
            let tr = self.tr;
            let text_focus = settings_ui::show(ui, self, &mut form, tr);
            if self.settings_open && self.is_unlocked() {
                self.settings_form = form;
            }
            if text_focus {
                if let Some(session) = self.session_mut() {
                    session.late_text_focus = true;
                }
            }
        }
        if continue_requested {
            if let Some(vault) = self.pending_vault.take() {
                self.last_activity = Instant::now();
                self.vault_exists = true;
                self.state = State::Unlocked(Session::new(vault, self.tr));
            }
        }
        if let Some(auto) = lock_request {
            self.lock(auto, ui.ctx());
        }
        self.show_notice(ui);
    }
}

impl eframe::App for MossPocket {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        theme::clear_color(visuals, &self.settings)
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        crate::media_view::set_parent(ui.ctx(), frame);
        self.draw(ui);
    }
}

impl MossPocket {
    pub fn is_unlocked(&self) -> bool {
        matches!(self.state, State::Unlocked(_))
    }

    #[cfg(test)]
    pub fn vault_ready_to_enter(&self) -> bool {
        self.pending_vault.is_some()
    }

    #[cfg(test)]
    pub fn lock_ui(&self) -> Option<&LockUi> {
        match &self.state {
            State::Locked(lock) => Some(lock),
            State::Unlocked(_) => None,
        }
    }

    pub(crate) fn session(&self) -> Option<&Session> {
        match &self.state {
            State::Unlocked(session) => Some(session),
            State::Locked(_) => None,
        }
    }

    pub(crate) fn session_mut(&mut self) -> Option<&mut Session> {
        match &mut self.state {
            State::Unlocked(session) => Some(session),
            State::Locked(_) => None,
        }
    }

    pub(crate) fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    #[cfg(test)]
    pub fn lang(&self) -> Lang {
        self.tr.0
    }

    #[cfg(test)]
    pub fn refresh_vault(&mut self) {
        self.refresh_vault_presence();
    }
}

/// 给无窗口测试用的入口。
#[cfg(test)]
impl MossPocket {
    pub fn headless(ctx: egui::Context, exe_dir: PathBuf) -> Self {
        Self::with_context(ctx, exe_dir, true)
    }

    pub fn reapply_settings(&mut self, ctx: &egui::Context) {
        crate::fonts::install(ctx, &self.settings);
        theme::apply(ctx, &self.settings);
    }

    pub fn set_idle_time(&mut self, elapsed: Duration) {
        self.last_activity = Instant::now() - elapsed;
    }
}

/// 界面语言：跟随系统时，系统是中文就用中文，其余一律用英文。
pub(crate) fn resolve_language(language: Language) -> Lang {
    match language {
        Language::System => {
            if crate::win::system_is_chinese() {
                Lang::Zh
            } else {
                Lang::En
            }
        }
        Language::Zh => Lang::Zh,
        Language::En => Lang::En,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::Harness;

    fn record_into_new_note(app: &mut MossPocket, wav: &[u8]) -> String {
        let session = app.session_mut().unwrap();
        session.new_note();
        session.start_synthetic_recording(wav.to_vec(), Duration::ZERO);
        session.current.clone().unwrap()
    }

    fn assert_recording_saved(vault: &mut Vault, note_id: &str, wav: &[u8]) {
        let note = vault.load_note(note_id).unwrap();
        let recording = note.blocks.iter().find_map(mp_core::model::Block::blob_id).unwrap();
        assert_eq!(vault.blob_handle().read(recording).unwrap(), wav);
    }

    #[test]
    fn lock_and_window_close_wait_for_the_recording_to_be_saved() {
        let mut harness = Harness::new("recording-before-exit");
        let path = Vault::data_dir(&harness.dir);
        let password = "test1234";
        let (vault, _) = Vault::create(&path, password).unwrap();
        harness.app.state = State::Unlocked(Session::new(vault, harness.app.tr));
        let wav = crate::scenarios::wav_bytes(800);

        let first = record_into_new_note(&mut harness.app, &wav);
        harness.app.lock_now(&harness.ctx);
        assert!(harness.app.is_unlocked());
        assert!(harness.wait_until(5.0, |app| !app.is_unlocked()));
        assert!(harness.app.notice.is_none(), "the lock screen should dismiss the completed import notice");
        let mut unlocked = Vault::unlock(&path, UnlockSecret::Password(password)).unwrap();
        assert_recording_saved(&mut unlocked, &first, &wav);

        harness.app.state = State::Unlocked(Session::new(unlocked, harness.app.tr));
        let second = record_into_new_note(&mut harness.app, &wav);
        harness.request_close();
        assert!(harness.commands.iter().any(|command| matches!(command, egui::ViewportCommand::CancelClose)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !harness.commands.iter().any(|command| matches!(command, egui::ViewportCommand::Close)) {
            assert!(Instant::now() < deadline, "window close did not continue after saving");
            harness.frames(1);
            std::thread::sleep(Duration::from_millis(5));
        }
        let session = harness.app.session_mut().unwrap();
        assert!(!session.recording_has_work());
        assert_recording_saved(&mut session.vault, &second, &wav);
    }
}
