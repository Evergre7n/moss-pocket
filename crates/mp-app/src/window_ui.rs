//! 应用自己的窗控，沿用系统的移动、最大化和关闭行为。

use crate::i18n::Tr;
use crate::theme::Palette;
use eframe::egui::{self, Rect, Sense, Stroke, StrokeKind, Vec2, ViewportCommand, WidgetInfo};

pub const HEIGHT: f32 = 32.0;
const BUTTON_WIDTH: f32 = 34.0;
pub const CONTROL_STRIP: f32 = 3.0 * BUTTON_WIDTH + 8.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

pub fn show(ui: &mut egui::Ui, tr: &Tr, native_drag: bool) {
    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
    let palette = Palette::for_ui(ui);
    let window = ui.ctx().content_rect();
    let bar = Rect::from_min_size(window.min, egui::vec2(window.width(), HEIGHT));
    let controls_left = bar.right() - CONTROL_STRIP;
    if !native_drag {
        let drag_rect = Rect::from_min_max(bar.min, egui::pos2(controls_left, bar.bottom()));
        let drag = ui.interact(drag_rect, ui.scope_id().with("drag-window"), Sense::click_and_drag());
        if drag.double_clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        } else if drag.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
    }

    let resize = if maximized {
        (Control::Restore, tr.restore_window(), ViewportCommand::Maximized(false))
    } else {
        (Control::Maximize, tr.maximize_window(), ViewportCommand::Maximized(true))
    };
    for (index, (control, label, command)) in [
        (Control::Minimize, tr.minimize_window(), ViewportCommand::Minimized(true)),
        resize,
        (Control::Close, tr.close_window(), ViewportCommand::Close),
    ].into_iter().enumerate() {
        let left = controls_left + index as f32 * BUTTON_WIDTH;
        let bounds = Rect::from_min_size(egui::pos2(left, bar.top() + 4.0), egui::vec2(BUTTON_WIDTH, 26.0));
        let button = ui.interact(bounds, ui.scope_id().with(("window-control", index)), Sense::click()).on_hover_text(label);
        button.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label));
        if button.clicked() {
            ui.ctx().send_viewport_cmd(command);
        }
        let highlighted = button.hovered() || button.has_focus();
        if highlighted {
            ui.painter().rect_filled(bounds.shrink2(egui::vec2(2.0, 1.0)), 4, palette.accent_soft);
        }
        let color = if highlighted && control == Control::Close { palette.danger } else { palette.muted };
        let stroke = Stroke::new(1.2, color);
        draw_icon(ui.painter(), bounds.center(), control, stroke);
    }
}

fn draw_icon(painter: &egui::Painter, center: egui::Pos2, control: Control, stroke: Stroke) {
    let glyph = Rect::from_center_size(center, Vec2::splat(9.0));
    match control {
        Control::Minimize => {
            painter.hline(glyph.x_range(), center.y, stroke);
        }
        Control::Maximize => {
            painter.rect_stroke(glyph, 0.5, stroke, StrokeKind::Inside);
        }
        Control::Restore => {
            let front = Rect::from_min_max(glyph.min + egui::vec2(0.0, 2.0), glyph.max - egui::vec2(2.0, 0.0));
            painter.rect_stroke(front, 0.5, stroke, StrokeKind::Inside);
            painter.add(egui::Shape::line(
                vec![glyph.left_top() + egui::vec2(2.0, 0.0), glyph.right_top(), glyph.right_bottom() - egui::vec2(0.0, 2.0)],
                stroke,
            ));
        }
        Control::Close => {
            painter.line_segment([glyph.left_top(), glyph.right_bottom()], stroke);
            painter.line_segment([glyph.right_top(), glyph.left_bottom()], stroke);
        }
    }
}
