//! 已解锁的工作区：左侧笔记列表，右侧写作区。

use crate::editor_ui;
use crate::i18n::{format_time, Tr};
use crate::session::Session;
use crate::settings::{Backdrop, Settings};
use crate::theme::{self, Palette};
use eframe::egui::{self, Color32, FontId, Stroke};
use mp_core::model::NoteSummary;
use mp_core::util::now_millis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopAction {
    Lock,
    Settings,
}

pub fn show(ui: &mut egui::Ui, session: &mut Session, settings: &Settings, tr: &Tr) -> Option<TopAction> {
    let new_shortcut = ui.is_enabled() && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::N));
    if new_shortcut {
        session.new_note();
    }
    let action = notes_panel(ui, session, settings, tr);
    egui::CentralPanel::default()
        .frame(egui::Frame::new())
        .show(ui, |ui| editor_ui::show(ui, session, settings, tr));
    action
}

fn notes_panel(ui: &mut egui::Ui, session: &mut Session, settings: &Settings, tr: &Tr) -> Option<TopAction> {
    let palette = Palette::for_ui(ui);
    let mut action = None;
    let available = ui.available_width();
    let fraction = if available < 820.0 { 0.32 } else { 0.42 };
    let max_width = (available * fraction).clamp(200.0, 440.0);
    let rail = if settings.backdrop == Backdrop::Opaque {
        palette.rail
    } else {
        Color32::from_rgba_unmultiplied(palette.rail.r(), palette.rail.g(), palette.rail.b(), 58)
    };
    let frame = egui::Frame::new().fill(rail);
    egui::Panel::left("notes-list")
        .resizable(true)
        .default_size(settings.list_width.min(max_width))
        .size_range(200.0..=max_width)
        .frame(frame)
        .show(ui, |ui| {
            egui::Panel::bottom("index-actions")
                .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 10)))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let destination = if session.view.show_trash { tr.notes() } else { tr.trash() };
                        if ui.add(theme::secondary_button(destination, palette)).clicked() {
                            session.toggle_trash_view();
                        }
                        if ui.add(theme::secondary_button(tr.settings(), palette)).clicked() {
                            action = Some(TopAction::Settings);
                        }
                        if ui.add(theme::secondary_button(tr.lock(), palette)).on_hover_text("Ctrl+L").clicked() {
                            action = Some(TopAction::Lock);
                        }
                    });
                });

            egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
                index_header(ui, session, palette, tr);
                index_records(ui, session, palette, tr);
            });
        });
    action
}

fn index_header(ui: &mut egui::Ui, session: &mut Session, palette: Palette, tr: &Tr) {
    theme::brand_lockup(ui, palette);
    ui.add_space(theme::SECTION_GAP);
    ui.horizontal(|ui| {
        let heading = if session.view.show_trash { tr.trash() } else { tr.notes() };
        ui.label(egui::RichText::new(heading).font(theme::bold(15.0)).color(palette.ink));
        if !session.view.show_trash {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button = theme::quiet_button(tr.new_note()).shortcut_text("+");
                if ui.add(button).on_hover_text("Ctrl+N").clicked() {
                    session.new_note();
                }
            });
        }
    });
    let search = egui::TextEdit::singleline(&mut session.view.search)
        .id(egui::Id::unique("search-box"))
        .hint_text(egui::RichText::new(tr.search_hint()).size(13.0).color(palette.muted))
        .font(egui::FontId::proportional(13.0))
        .desired_width(f32::INFINITY)
        .margin(egui::Margin::symmetric(9, 6))
        .show(ui)
        .response;
    session.other_text_focus |= search.has_focus();
    ui.add_space(theme::GROUP_GAP);
}

fn index_records(ui: &mut egui::Ui, session: &mut Session, palette: Palette, tr: &Tr) {
    let trash = session.view.show_trash;
    let filtering = !session.view.search.trim().is_empty();
    let rows = session.visible_summaries();
    let now = now_millis();
    egui::ScrollArea::vertical()
        .id_salt(("note-list-scroll", trash))
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if rows.is_empty() {
                let empty = if filtering { Some(tr.find_none()) } else { trash.then(|| tr.empty_trash()) };
                if let Some(empty) = empty {
                    ui.label(egui::RichText::new(empty).small().color(palette.muted));
                }
                return;
            }
            let mut group = None;
            for summary in rows {
                let pinned = summary.pinned && !trash;
                if group != Some(pinned) {
                    if group.is_some() {
                        ui.add_space(theme::GROUP_GAP);
                    }
                    group = Some(pinned);
                    if !trash {
                        let label = if pinned { tr.pinned_notes() } else { tr.recent_notes() };
                        ui.label(theme::kicker(label, palette));
                        ui.add_space(4.0);
                    }
                }
                row(ui, session, &summary, tr, palette, now);
            }
        });
}

fn row(
    ui: &mut egui::Ui,
    session: &mut Session,
    summary: &NoteSummary,
    tr: &Tr,
    palette: Palette,
    now: u64,
) {
    let selected = session.current.as_deref() == Some(summary.id.as_str());
    let title = if summary.title.is_empty() { tr.untitled() } else { &summary.title };
    let (rect, response) = ui
        .push_id(&summary.id, |ui| {
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 51.0), egui::Sense::click())
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, title));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if selected {
        palette.accent_soft
    } else if response.hovered() {
        palette.surface
    } else {
        Color32::TRANSPARENT
    };
    let border = if response.has_focus() { Stroke::new(1.0, palette.accent) } else { Stroke::NONE };
    let painter = ui.painter();
    painter.rect(rect, 5, fill, border, egui::StrokeKind::Inside);
    if selected {
        painter.circle_filled(rect.right_center() - egui::vec2(10.0, 0.0), 2.0, palette.accent);
    }

    let time = format_time(tr, summary.updated_at, now);
    let detail = if summary.tags.is_empty() { time } else { format!("{time}  ·  {}", summary.tags.join("  ·  ")) };
    let left = rect.left() + 10.0;
    let width = (rect.right() - left - 22.0).max(0.0);
    let line = |text: &str, font: FontId, color: Color32, top: f32| {
        let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, width);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        painter.galley(egui::pos2(left, rect.top() + top), painter.layout_job(job), color);
    };
    line(title, theme::bold(14.0), palette.ink, 7.0);
    line(&detail, FontId::proportional(11.5), palette.muted, 28.0);

    if response.clicked() {
        session.open(&summary.id);
    }
    egui::Popup::context_menu(&response).show(|ui| {
        if session.view.show_trash {
            if ui.button(tr.restore()).clicked() {
                session.restore(&summary.id);
                ui.close();
            }
            if ui.button(tr.delete_forever()).clicked() {
                session.destroy(&summary.id);
                ui.close();
            }
        } else {
            if ui.button(if summary.pinned { tr.unpin() } else { tr.pin() }).clicked() {
                session.toggle_pin(&summary.id);
                ui.close();
            }
            if ui.button(tr.move_to_trash()).clicked() {
                session.move_to_trash(&summary.id);
                ui.close();
            }
            ui.separator();
            crate::export_ui::menu_items(ui, session, &summary.id, *tr);
        }
    });
}
