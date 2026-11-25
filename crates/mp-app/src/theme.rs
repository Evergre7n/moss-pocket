//! 视觉：字体、配色、窗口底色与磨砂玻璃的配合。

use crate::settings::{Backdrop, Settings};
use eframe::egui::{self, Color32};

// 标题、正文和操作形成三级对比。留白靠统一的间距，色彩只指示状态。
#[derive(Clone, Copy)]
pub struct Palette {
    pub canvas: Color32,
    pub rail: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub line: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_ink: Color32,
    pub danger: Color32,
}

pub const DEFAULT_ACCENT: u32 = 0x3F5D4B;
pub const SMALL_GAP: f32 = 8.0;
pub const GROUP_GAP: f32 = 16.0;
pub const SECTION_GAP: f32 = 24.0;

impl Palette {
    fn new(dark: bool, raw_accent: Color32) -> Self {
        let (canvas, rail, surface, raised, line, ink, muted, danger) = if dark {
            (0x1B1E1B, 0x222622, 0x1B1E1B, 0x272C27, 0x373E37, 0xE9EDE6, 0xABB5AA, 0xF5A493)
        } else {
            (0xF8F9F5, 0xF0F2ED, 0xF8F9F5, 0xF1F3EE, 0xDCE1D9, 0x242C25, 0x58655C, 0xA1372A)
        };
        let surface = color(surface);
        let ink = color(ink);
        // 用户自选的颜色可能与底色很接近，逐步向正文颜色靠拢以保留可读性。
        let mut accent = raw_accent;
        while contrast(accent, surface) < 4.5 {
            accent = mix(accent, ink, 0.18);
        }
        Self {
            canvas: color(canvas), rail: color(rail), surface, raised: color(raised), line: color(line), ink,
            muted: color(muted), accent, accent_soft: mix(color(rail), accent, 0.10),
            accent_ink: on_color(accent), danger: color(danger),
        }
    }

    pub fn for_ui(ui: &egui::Ui) -> Self {
        Self::new(ui.visuals().dark_mode, ui.visuals().hyperlink_color)
    }
}

fn mix(a: Color32, b: Color32, fraction: f32) -> Color32 {
    let channel = |a: u8, b: u8| (f32::from(a) * (1.0 - fraction) + f32::from(b) * fraction).round() as u8;
    Color32::from_rgb(channel(a.r(), b.r()), channel(a.g(), b.g()), channel(a.b(), b.b()))
}

fn luminance(color: Color32) -> f32 {
    let linear = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("mp-bold".into()))
}

pub fn display(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("mp-display".into()))
}

pub fn kicker(text: &str, palette: Palette) -> egui::RichText {
    egui::RichText::new(text).font(bold(12.0)).color(palette.muted)
}

/// 使用中转站字标的透明图层。浅色版本只调整字形颜色，保留星云图层。
pub fn brand_lockup(ui: &mut egui::Ui, palette: Palette) {
    ui.horizontal(|ui| {
        let (rect, logo) = ui.allocate_exact_size(egui::vec2(103.0, 35.0), egui::Sense::hover());
        logo.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "MOSS"));
        let dark = ui.visuals().dark_mode;
        let scale = ui.ctx().pixels_per_point();
        let size = (rect.size() * scale).round();
        let dimensions = [size.x.max(1.0) as u32, size.y.max(1.0) as u32];
        if let Some(texture) = wordmark_texture(ui.ctx(), dark, dimensions) {
            let min = egui::pos2((rect.min.x * scale).round() / scale, (rect.min.y * scale).round() / scale);
            let aligned = egui::Rect::from_min_size(min, size / scale);
            let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
            ui.painter().image(texture.id(), aligned, uv, Color32::WHITE);
        } else {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "MOSS", bold(14.0), palette.ink);
        }
        ui.label(egui::RichText::new("Pocket").font(display(19.0)).color(palette.ink));
    });
}

fn wordmark_texture(ctx: &egui::Context, dark: bool, size: [u32; 2]) -> Option<egui::TextureHandle> {
    let cache = egui::Id::unique(("moss-relay-wordmark", dark));
    if let Some((stored_size, texture)) = ctx.data(|data| data.get_temp::<([u32; 2], egui::TextureHandle)>(cache)) {
        if stored_size == size {
            return Some(texture);
        }
    }
    let bytes = if dark {
        include_bytes!("../assets/relay-wordmark.png").as_slice()
    } else {
        include_bytes!("../assets/relay-wordmark-light.png").as_slice()
    };
    let pixels = image::load_from_memory(bytes).ok()?.into_rgba8();
    let linear: [f32; 256] = std::array::from_fn(|index| {
        let srgb = index as f32 / 255.0;
        if srgb <= 0.04045 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055).powf(2.4) }
    });
    let source = image::Rgba32FImage::from_fn(pixels.width(), pixels.height(), |x, y| {
        let [red, green, blue, alpha] = pixels.get_pixel(x, y).0;
        let coverage = f32::from(alpha) / 255.0;
        image::Rgba([
            linear[usize::from(red)] * coverage,
            linear[usize::from(green)] * coverage,
            linear[usize::from(blue)] * coverage,
            coverage,
        ])
    });
    // 缩小前在线性光中合成覆盖率，避免 GPU 双线性漏掉细字形。
    let filtered = image::imageops::resize(&source, size[0], size[1], image::imageops::FilterType::Lanczos3);
    let color = |component: f32, alpha: f32| -> u8 {
        let value = (component / alpha).clamp(0.0, 1.0);
        let srgb = if value <= 0.0031308 { 12.92 * value } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
        (srgb * 255.0).round() as u8
    };
    let composed = filtered.pixels().map(|pixel| {
        let [red, green, blue, alpha] = pixel.0;
        if alpha < 0.5 / 255.0 {
            return Color32::TRANSPARENT;
        }
        Color32::from_rgba_unmultiplied(color(red, alpha), color(green, alpha), color(blue, alpha), (alpha * 255.0).round() as u8)
    }).collect();
    let pixels = egui::ColorImage::new([size[0] as usize, size[1] as usize], composed);
    let texture = ctx.load_texture("MOSS wordmark", pixels, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(cache, (size, texture.clone())));
    Some(texture)
}

pub fn quiet_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(label).frame(true).frame_when_inactive(false)
}

pub fn secondary_button(label: &str, palette: Palette) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(label).size(12.0).color(palette.muted))
        .frame(true)
        .frame_when_inactive(false)
}

pub fn primary_button(label: &str, palette: Palette) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(label.to_owned()).strong().color(palette.accent_ink))
        .fill(palette.accent)
        .min_size(egui::vec2(0.0, 32.0))
}

/// 0xRRGGBB 转颜色。
pub fn color(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// 查找命中处的高亮色：普通命中，和当前停的那一处。深浅色主题各配一套。
pub fn highlight_colors(dark: bool) -> (Color32, Color32) {
    if dark {
        (color(0x403B2D), color(0x5A4A32))
    } else {
        (color(0xF2EAD2), color(0xEFDAA5))
    }
}

/// 放在 `background` 上看得清的文字颜色：底色亮就用近黑的字，暗就用近白的字。
pub fn on_color(background: Color32) -> Color32 {
    if luminance(background) > 0.179 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

/// 没改过的默认底色（深色）。用户没动它的时候，它跟着主题走。
pub const DEFAULT_TINT: u32 = 0x1B1E1B;
/// 浅色主题下配套的浅底色。
const LIGHT_TINT: u32 = 0xF8F9F5;

/// 实际生效的窗口底色。用户没改过底色时，浅色主题下自动换成浅底色，
/// 否则浅色主题的深色文字压在深色底上，看不清。
pub fn tint(settings: &Settings, dark: bool) -> Color32 {
    if matches!(settings.tint, DEFAULT_TINT | 0x211F1C | 0x1A221E | 0x1C_1C22) {
        if dark { color(DEFAULT_TINT) } else { color(LIGHT_TINT) }
    } else {
        color(settings.tint)
    }
}

fn text_styles() -> std::collections::BTreeMap<egui::TextStyle, egui::FontId> {
    use egui::{FontFamily, FontId, TextStyle};
    [
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Heading, display(23.25)),
        (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
    ]
    .into()
}

/// 把设置套到界面上：深浅色、强调色、圆角，以及窗口是否透明。
pub fn apply(ctx: &egui::Context, settings: &Settings) {
    ctx.set_theme(match settings.theme_mode {
        crate::settings::ThemeMode::System => egui::ThemePreference::System,
        crate::settings::ThemeMode::Light => egui::ThemePreference::Light,
        crate::settings::ThemeMode::Dark => egui::ThemePreference::Dark,
    });
    let accent = color(settings.accent);
    let see_through = settings.backdrop != Backdrop::Opaque;
    let radius = settings.corner_radius.round().clamp(0.0, 40.0) as u8;
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let mut visuals = match theme {
            egui::Theme::Dark => egui::Visuals::dark(),
            egui::Theme::Light => egui::Visuals::light(),
        };
        let palette = Palette::new(theme == egui::Theme::Dark, accent);
        apply_colors(&mut visuals, palette, radius);
        visuals.panel_fill = if see_through { Color32::TRANSPARENT } else { palette.canvas };
        ctx.set_visuals_of(theme, visuals);
        ctx.style_mut_of(theme, |style| {
            style.text_styles = text_styles();
            style.spacing.item_spacing = egui::vec2(SMALL_GAP, SMALL_GAP);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.visuals.window_corner_radius = radius.into();
        });
    }
}

fn apply_colors(visuals: &mut egui::Visuals, palette: Palette, radius: u8) {
    let hairline = egui::Stroke::new(1.0, palette.line);
    visuals.override_text_color = Some(palette.ink);
    visuals.weak_text_color = Some(palette.muted);
    visuals.selection.bg_fill = palette.accent_soft;
    visuals.selection.stroke = egui::Stroke::new(1.0, palette.accent);
    visuals.hyperlink_color = palette.accent;
    visuals.error_fg_color = palette.danger;
    visuals.window_fill = palette.surface;
    visuals.window_stroke = hairline;
    visuals.faint_bg_color = palette.rail;
    visuals.extreme_bg_color = palette.surface;
    for (widget, fill) in [
        (&mut visuals.widgets.noninteractive, palette.surface),
        (&mut visuals.widgets.inactive, palette.raised),
        (&mut visuals.widgets.hovered, palette.accent_soft),
        (&mut visuals.widgets.active, palette.accent_soft),
        (&mut visuals.widgets.open, palette.accent_soft),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = hairline;
        widget.fg_stroke = egui::Stroke::new(1.0, palette.ink);
        widget.corner_radius = (radius / 2).min(8).into();
        widget.expansion = 0.0;
    }
    visuals.widgets.active.bg_stroke = visuals.selection.stroke;
}

/// 窗口的清屏色。透明材质下清成全透明，让系统的模糊透出来。
pub fn clear_color(visuals: &egui::Visuals, settings: &Settings) -> [f32; 4] {
    if settings.backdrop == Backdrop::Opaque {
        visuals.panel_fill.to_normalized_gamma_f32()
    } else {
        [0.0, 0.0, 0.0, 0.0]
    }
}

/// 每帧最先画的一层：给透明窗口铺上用户选的底色。
pub fn paint_backdrop(ui: &egui::Ui, settings: &Settings, native_material: bool) {
    let palette = Palette::for_ui(ui);
    let tint = mix(palette.canvas, tint(settings, ui.visuals().dark_mode), 0.24);
    let value = u16::from(settings.tint_alpha);
    let alpha = match settings.backdrop {
        Backdrop::Opaque => 255,
        Backdrop::Transparent => (176 + (79 * value) / 255) as u8,
        _ if !native_material => 255,
        Backdrop::Mica => ((96 * value) / 255) as u8,
        Backdrop::Acrylic | Backdrop::SystemAcrylic => ((220 * value) / 255) as u8,
    };
    ui.painter().rect_filled(
        ui.ctx().content_rect(),
        0,
        Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), alpha),
    );
}
