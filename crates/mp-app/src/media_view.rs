//! In-note media viewer. A single modal owns video playback and shares the note's audio engine.

use crate::i18n::Tr;
use crate::media::{format_size, MediaHub};
use crate::theme::{self, Palette};
use crate::video::{VideoPlayer, VideoSnapshot, VideoState};
use eframe::egui::{self, Color32, Id, Rect, Vec2};
use mp_core::media::MediaKind;
use mp_core::model::{ImageBlock, MediaBlock};
use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle};
use std::time::{Duration, Instant};

const VIDEO_STARTUP_TIMEOUT: Duration = Duration::from_secs(25);

pub(crate) fn set_parent(ctx: &egui::Context, frame: &eframe::Frame) {
    if let Ok(handle) = frame.window_handle() {
        if let RawWindowHandle::Win32(parent) = handle.as_raw() {
            ctx.data_mut(|data| data.insert_temp(Id::unique("media-viewer-window"), parent));
        }
    }
}

pub(crate) enum MediaViewer {
    Image(ImageBlock),
    Audio { block: MediaBlock, error: Option<String> },
    Video {
        block: MediaBlock,
        player: Option<VideoPlayer>,
        error: Option<String>,
        load_deadline: Instant,
    },
}

impl MediaViewer {
    pub(crate) fn video(ctx: &egui::Context, media: &mut MediaHub, block: &MediaBlock, tr: Tr) -> Self {
        let player = (|| {
            let parent = ctx.data(|data| data.get_temp::<Win32WindowHandle>(Id::unique("media-viewer-window")))
                .ok_or_else(|| tr.video_playback_failed().to_owned())?;
            let url = media.video_url(&block.blob_id, &block.file_name)
                .ok_or_else(|| tr.video_playback_failed().to_owned())?;
            let player = VideoPlayer::open(parent, &url, ctx)?;
            media.stop_audio();
            Ok(player)
        })();
        let (player, error) = match player {
            Ok(player) => (Some(player), None),
            Err(error) => {
                media.stop_video_stream();
                (None, Some(error))
            }
        };
        Self::Video { block: block.clone(), player, error, load_deadline: Instant::now() + VIDEO_STARTUP_TIMEOUT }
    }

    pub(crate) fn capture_audio_error(&mut self, message: &str) -> bool {
        if let Self::Audio { error, .. } = self {
            *error = Some(message.to_owned());
            return true;
        }
        false
    }

    pub(crate) fn hide_native(&self, ctx: &egui::Context) {
        if let Self::Video { player: Some(player), .. } = self {
            let _ = player.set_bounds(None, ctx.pixels_per_point());
        }
    }

    fn poll_video(&mut self, ctx: &egui::Context, media: &mut MediaHub, tr: Tr) -> Option<VideoSnapshot> {
        let Self::Video { player, error, load_deadline, .. } = self else { return None };
        let mut snapshot = player.as_ref()?.snapshot();
        let wait = loading_remaining(snapshot.state, *load_deadline, Instant::now());
        let failure = snapshot.error.take().or_else(|| {
            wait.filter(Duration::is_zero).map(|_| tr.video_startup_timed_out().to_owned())
        });
        if let Some(message) = failure {
            *error = Some(message);
            *player = None;
            media.stop_video_stream();
            return None;
        }
        // Target the same deadline even if unrelated input or native events repaint early.
        if let Some(delay) = wait {
            ctx.request_repaint_after(delay);
        }
        Some(snapshot)
    }

    pub(crate) fn show(&mut self, parent: &mut egui::Ui, media: &mut MediaHub, tr: Tr) -> bool {
        let ctx = parent.ctx();
        let video_snapshot = self.poll_video(ctx, media, tr);
        let palette = Palette::for_ui(parent);
        let window = ctx.content_rect();
        let width = (window.width() - 66.0).clamp(1.0, if matches!(self, Self::Audio { .. }) { 520.0 } else { 900.0 });
        let stage_height = (window.height() - 212.0).clamp(64.0, 610.0);
        let frame = egui::Frame::popup(parent.style())
            .inner_margin(egui::Margin::same(14))
            .corner_radius(10)
            .fill(palette.surface)
            .stroke(egui::Stroke::new(1.0, palette.line));
        let id = Id::unique("media-viewer");
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).fade_in(false))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(width);
                let title = match self {
                    Self::Image(_) => tr.insert_image(),
                    Self::Audio { .. } => tr.insert_audio(),
                    Self::Video { .. } => tr.insert_video(),
                };
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 30.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui.add(theme::quiet_button(tr.close())).on_hover_text("Esc").clicked() {
                            ui.close();
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(title).font(theme::bold(16.0)).color(palette.ink));
                        });
                    },
                );
                ui.add_space(theme::SMALL_GAP);
                match self {
                    Self::Image(block) => {
                        let stage = stage(ui, palette, stage_height);
                        if let Some(texture) = media.preview(&block.blob_id) {
                            let image = fit(stage, texture.size_vec2());
                            ui.painter().image(texture.id(), image, Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
                        } else if media.preview_failed(&block.blob_id) {
                            stage_status(ui, stage, tr.image_view_failed(), palette.danger);
                        } else {
                            stage_status(ui, stage, tr.loading_image(), palette.muted);
                        }
                        ui.label(egui::RichText::new(format!("{} × {}", block.width, block.height)).small().color(palette.muted));
                        None
                    }
                    Self::Audio { block, error } => {
                        metadata(ui, palette, block);
                        ui.add_space(theme::SMALL_GAP);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().slider_width = (width - 240.0).clamp(65.0, 260.0);
                            if audio_controls(ui, media, block, tr, true) {
                                *error = None;
                            }
                        });
                        if let Some(error) = error {
                            ui.add(egui::Label::new(egui::RichText::new(error.as_str()).color(palette.danger)).truncate())
                                .on_hover_text(error.as_str());
                        }
                        None
                    }
                    Self::Video { block, player, error, .. } => {
                        let playback = player.as_ref().zip(video_snapshot.as_ref());
                        show_video(ui, palette, stage_height, block, playback, error, tr)
                    }
                }
            });
        if modal.should_close() {
            if let Self::Video { player, .. } = self {
                *player = None;
                media.stop_video_stream();
            }
            ctx.request_repaint();
            return false;
        }
        if let Self::Video { player, error, .. } = self {
            if let Some(Err(message)) = player.as_ref().map(|player| player.set_bounds(modal.inner, ctx.pixels_per_point())) {
                *error = Some(message);
                *player = None;
                media.stop_video_stream();
                ctx.request_repaint();
            }
        }
        true
    }
}

fn show_video(
    ui: &mut egui::Ui,
    palette: Palette,
    stage_height: f32,
    block: &MediaBlock,
    playback: Option<(&VideoPlayer, &VideoSnapshot)>,
    error: &mut Option<String>,
    tr: Tr,
) -> Option<Rect> {
    let stage = stage(ui, palette, stage_height);
    let fatal = playback.is_none();
    let loading = playback.is_some_and(|(_, state)| state.state == VideoState::Loading);
    if fatal {
        stage_status(ui, stage, tr.video_playback_failed(), palette.danger);
    } else if loading {
        stage_status(ui, stage, tr.loading_video(), palette.muted);
    }
    if let Some(message) = error.as_deref().filter(|message| *message != tr.video_playback_failed()) {
        let hint = if message.starts_with("Media Foundation") {
            tr.video_codec_hint()
        } else {
            message
        };
        ui.add(egui::Label::new(egui::RichText::new(hint).color(palette.muted)).wrap())
            .on_hover_text(message);
    }
    metadata(ui, palette, block);
    ui.add_space(theme::SMALL_GAP);
    let ready = !fatal && !loading;
    if let Some((player, snapshot)) = playback {
        let width = ui.available_width();
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (width - 255.0).clamp(65.0, 450.0);
            let label = match snapshot.state {
                VideoState::Playing => tr.pause(),
                VideoState::Ended => tr.replay(),
                _ => tr.play(),
            };
            if ui.add_enabled(ready, transport_button(label, palette)).clicked() {
                *error = player.toggle().err();
            }
            let stoppable = ready && (snapshot.state == VideoState::Playing || snapshot.position > 0.0);
            if ui.add_enabled(stoppable, egui::Button::new(tr.stop())).clicked() {
                *error = player.stop().err();
            }
            if let Some(seconds) = video_timeline(ui, block, snapshot, ready, tr) {
                *error = player.seek(seconds).err();
            }
        });
    }
    if !ready || !ui.is_visible() || ui.is_sizing_pass() {
        return None;
    }
    let aspect = playback.and_then(|(_, state)| state.aspect_ratio).unwrap_or(16.0 / 9.0);
    Some(fit(stage, egui::vec2(aspect, 1.0)))
}

fn loading_remaining(state: VideoState, deadline: Instant, now: Instant) -> Option<Duration> {
    (state == VideoState::Loading).then(|| deadline.saturating_duration_since(now))
}

fn video_timeline(ui: &mut egui::Ui, block: &MediaBlock, snapshot: &VideoSnapshot, ready: bool, tr: Tr) -> Option<f64> {
    if let Some(duration) = video_duration(block, snapshot.duration) {
        let position = if snapshot.state == VideoState::Ended { duration } else { snapshot.position.min(duration) };
        timeline(ui, Id::unique(("video-progress", block.id)), position, Some(duration), ready, tr)
    } else {
        ui.label(egui::RichText::new(clock(snapshot.position as f32)).weak().small());
        None
    }
}

fn video_duration(block: &MediaBlock, reported: Option<f64>) -> Option<f64> {
    // Media Foundation estimates duration from bitrate for MPEG streams without a usable index.
    let estimated = |mime| matches!(mime, "video/mp2t" | "video/mpeg");
    if estimated(block.mime.as_str()) || estimated(MediaKind::Video.mime(&block.file_name)) {
        return None;
    }
    reported
}

fn stage(ui: &mut egui::Ui, palette: Palette, height: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    ui.painter().rect(rect, 6, palette.raised, egui::Stroke::new(1.0, palette.line), egui::StrokeKind::Inside);
    rect.shrink(2.0)
}

fn stage_status(ui: &mut egui::Ui, rect: Rect, text: &str, color: Color32) {
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(14.0), color);
}

fn fit(rect: Rect, size: Vec2) -> Rect {
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    Rect::from_center_size(rect.center(), size * scale)
}

fn metadata(ui: &mut egui::Ui, palette: Palette, block: &MediaBlock) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format_size(block.size)).small().color(palette.muted));
        ui.add(egui::Label::new(&block.file_name).truncate()).on_hover_text(&block.file_name);
    });
}

fn transport_button(label: &str, palette: Palette) -> egui::Button<'static> {
    theme::primary_button(label, palette).min_size(egui::vec2(56.0, 28.0))
}

/// Shared audio commands keep inline and expanded playback on the same transport.
pub(crate) fn audio_controls(ui: &mut egui::Ui, media: &mut MediaHub, block: &MediaBlock, tr: Tr, expanded: bool) -> bool {
    let state = media.audio_state().filter(|state| state.blob_id.as_deref() == Some(block.blob_id.as_str()));
    let playing = state.as_ref().is_some_and(|state| state.playing);
    let paused = state.as_ref().is_some_and(|state| state.paused);
    let position = state.as_ref().map_or(0.0, |state| f64::from(state.position));
    let duration = state.as_ref().and_then(|state| state.duration.map(f64::from)).or(block.duration_secs);
    let label = if playing { tr.pause() } else if expanded && state.is_some() && !paused { tr.replay() } else { tr.play() };
    let button = if expanded { transport_button(label, Palette::for_ui(ui)) } else { egui::Button::new(label) };
    let mut command = false;
    if ui.add(button).clicked() {
        if playing {
            media.pause_audio();
        } else if paused {
            media.resume_audio();
        } else {
            media.play_audio(ui.ctx(), &block.blob_id, &block.mime);
        }
        command = true;
    }
    if (expanded || state.is_some()) && ui.add_enabled(state.is_some(), egui::Button::new(tr.stop())).clicked() {
        media.stop_audio();
        command = true;
    }
    let id = Id::unique(("audio-progress", block.id, expanded));
    if let Some(seconds) = timeline(ui, id, position, duration, playing || paused, tr) {
        media.seek_audio(seconds as f32);
        command = true;
    }
    command
}

fn timeline(ui: &mut egui::Ui, id: Id, position: f64, duration: Option<f64>, active: bool, tr: Tr) -> Option<f64> {
    let duration = duration.filter(|end| end.is_finite() && *end > 0.0);
    let end = duration.unwrap_or(1.0);
    let mut sought = ui.ctx().data(|data| data.get_temp::<f64>(id)).unwrap_or(position).clamp(0.0, end);
    let seekable = active && duration.is_some();
    let slider = ui.add_enabled(seekable, egui::Slider::new(&mut sought, 0.0..=end).show_value(false))
        .on_hover_text(tr.media_seek());
    if slider.dragged() {
        ui.ctx().data_mut(|data| data.insert_temp(id, sought));
    } else {
        ui.ctx().data_mut(|data| data.remove::<f64>(id));
    }
    let shown = if slider.dragged() || slider.drag_stopped() { sought } else { position };
    let label = match duration {
        Some(end) if end < 1.0 => format!("{:.0} / {:.0} ms", shown.clamp(0.0, end) * 1000.0, end * 1000.0),
        Some(end) => format!("{} / {}", clock(shown as f32), clock(end as f32)),
        None => format!("{} / -:--", clock(shown as f32)),
    };
    ui.label(egui::RichText::new(label).weak().small());
    (seekable && (slider.drag_stopped() || (slider.changed() && !slider.dragged()))).then_some(sought)
}

pub(crate) fn clock(seconds: f32) -> String {
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, secs) = (total / 3600, total % 3600 / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpeg_estimates_do_not_set_the_total_or_seek_range() {
        let mut block = MediaBlock {
            id: 1,
            blob_id: "blob".into(),
            file_name: "clip.TS".into(),
            mime: "video/mp4".into(),
            size: 150_000,
            duration_secs: None,
        };
        for name in ["clip.TS", "clip.mpg", "clip.MPEG"] {
            block.file_name = name.into();
            assert_eq!(video_duration(&block, Some(132.0)), None);
        }
        block.file_name = "clip.mp4".into();
        assert_eq!(video_duration(&block, Some(2.4)), Some(2.4));
        block.mime = "video/mp2t".into();
        assert_eq!(video_duration(&block, Some(132.0)), None);
    }

    #[test]
    fn stalled_startup_expires_without_affecting_ready_video() {
        let now = Instant::now();
        let deadline = now + VIDEO_STARTUP_TIMEOUT;
        let before = deadline - Duration::from_secs(1);
        assert_eq!(loading_remaining(VideoState::Loading, deadline, before), Some(Duration::from_secs(1)));
        assert_eq!(loading_remaining(VideoState::Loading, deadline, deadline), Some(Duration::ZERO));
        let after = deadline + Duration::from_secs(1);
        assert_eq!(loading_remaining(VideoState::Loading, deadline, after), Some(Duration::ZERO));
        assert_eq!(loading_remaining(VideoState::Playing, deadline, after), None);
        assert_eq!(loading_remaining(VideoState::Paused, deadline, after), None);
    }
}
