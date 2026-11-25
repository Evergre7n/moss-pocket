//! 无窗口测试台。不创建任何窗口、不碰键鼠：直接调用真实的界面代码、喂模拟事件，
//! 再把 egui 的绘制数据用软件光栅化存成图片，供人工核对画面。

#![cfg(test)]

use crate::app::MossPocket;
use crate::settings::Backdrop;
use eframe::egui;
use mp_core::vault::Vault;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 离屏画布：积累贴图，把三角形画到内存里的像素上。
pub struct Offscreen {
    pub points: egui::Vec2,
    pub pixels_per_point: f32,
    background: [f32; 4],
    textures: HashMap<egui::TextureId, std::sync::Arc<egui::ColorImage>>,
    pixels: Vec<[f32; 4]>,
}

impl Offscreen {
    pub fn new(points: egui::Vec2, pixels_per_point: f32, background: [f32; 4]) -> Self {
        let (width, height) = (points.x * pixels_per_point, points.y * pixels_per_point);
        Self {
            points,
            pixels_per_point,
            background,
            textures: HashMap::new(),
            pixels: vec![background; (width * height) as usize],
        }
    }

    fn size_px(&self) -> [usize; 2] {
        [
            (self.points.x * self.pixels_per_point) as usize,
            (self.points.y * self.pixels_per_point) as usize,
        ]
    }

    fn apply(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta) {
        let egui::ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        let Some([x, y]) = delta.pos else {
            self.textures.insert(id, Arc::clone(image));
            return;
        };
        let texture = self
            .textures
            .entry(id)
            .or_insert_with(|| {
                Arc::new(egui::ColorImage::filled([x + width, y + height], egui::Color32::TRANSPARENT))
            });
        // 这一帧里可能还有别的三角形的引用，先独占再改。
        let texture = Arc::make_mut(texture);
        for row in 0..height {
            for col in 0..width {
                let (tx, ty) = (x + col, y + row);
                if tx < texture.size[0] && ty < texture.size[1] {
                    texture.pixels[ty * texture.size[0] + tx] = image.pixels[row * width + col];
                }
            }
        }
    }

    /// 收下这一帧新增或变化的贴图。字体贴图是逐帧长出来的，每帧都要收。
    /// egui 要求渲染方用完后显式清空，否则丢弃时会报「有未处理的贴图」。
    pub fn absorb(&mut self, delta: &mut egui::TexturesDelta) {
        for (id, updates) in &delta.set {
            for update in updates {
                self.apply(*id, update);
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
        delta.clear();
    }

    /// 把这一帧的绘制指令光栅化到画布上。耗 CPU，只在要截图时调用。
    pub fn paint(&mut self, ctx: &egui::Context, shapes: Vec<egui::epaint::ClippedShape>, scale: f32) {
        // 和真实窗口一样：每一帧都从清屏色开始画，上一帧的画面不会留下来。
        self.pixels.fill(self.background);
        // egui 的顶点位置仍以逻辑点计，光栅化时按画布的 DPI 转成像素。
        let ratio = self.pixels_per_point;
        for primitive in ctx.tessellate(shapes, scale) {
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let texture = self.textures.get(&mesh.texture_id).cloned();
            for triangle in mesh.indices.chunks_exact(3) {
                let vertices = [
                    &mesh.vertices[triangle[0] as usize],
                    &mesh.vertices[triangle[1] as usize],
                    &mesh.vertices[triangle[2] as usize],
                ];
                self.triangle(vertices, texture.as_deref(), primitive.clip_rect, ratio);
            }
        }
    }

    fn triangle(
        &mut self,
        vertices: [&egui::epaint::Vertex; 3],
        texture: Option<&egui::ColorImage>,
        clip: egui::Rect,
        ratio: f32,
    ) {
        let position = |vertex: &egui::epaint::Vertex| (vertex.pos.x * ratio, vertex.pos.y * ratio);
        let (a, b, c) = (position(vertices[0]), position(vertices[1]), position(vertices[2]));
        let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        if area.abs() < f32::EPSILON {
            return;
        }
        let inverse = 1.0 / area;
        let [width, height] = self.size_px();
        let min_x = (a.0.min(b.0).min(c.0).floor() as usize).clamp(0, width);
        let max_x = (a.0.max(b.0).max(c.0).ceil() as usize).clamp(0, width);
        let min_y = (a.1.min(b.1).min(c.1).floor() as usize).clamp(0, height);
        let max_y = (a.1.max(b.1).max(c.1).ceil() as usize).clamp(0, height);
        // 裁剪区和三角形包围盒取交集；交集为空就什么都不画。
        let clip_min_x = ((clip.min.x * ratio).ceil() as usize).clamp(min_x, width);
        let clip_max_x = ((clip.max.x * ratio).floor() as usize).clamp(min_x, max_x);
        let clip_min_y = ((clip.min.y * ratio).ceil() as usize).clamp(min_y, height);
        let clip_max_y = ((clip.max.y * ratio).floor() as usize).clamp(min_y, max_y);
        if clip_min_x >= clip_max_x || clip_min_y >= clip_max_y {
            return;
        }

        let (texture_width, texture_height, texels) = match texture {
            Some(texture) => (texture.size[0] as f32, texture.size[1] as f32, Some(&texture.pixels)),
            None => (0.0, 0.0, None),
        };
        for y in clip_min_y..clip_max_y {
            let py = y as f32 + 0.5;
            for x in clip_min_x..clip_max_x {
                let px = x as f32 + 0.5;
                let w0 = ((b.0 - px) * (c.1 - py) - (c.0 - px) * (b.1 - py)) * inverse;
                let w1 = ((c.0 - px) * (a.1 - py) - (a.0 - px) * (c.1 - py)) * inverse;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let weights = [w0, w1, w2];
                let mut color = [0.0f32; 4];
                let (mut u, mut v) = (0.0f32, 0.0f32);
                for (index, vertex) in vertices.iter().enumerate() {
                    for channel in 0..4 {
                        color[channel] += weights[index] * vertex.color[channel] as f32 / 255.0;
                    }
                    u += weights[index] * vertex.uv.x;
                    v += weights[index] * vertex.uv.y;
                }
                if let Some(texels) = texels {
                    let tx = (u * texture_width).floor().clamp(0.0, texture_width - 1.0) as usize;
                    let ty = (v * texture_height).floor().clamp(0.0, texture_height - 1.0) as usize;
                    let texel = texels[ty * texture_width as usize + tx];
                    for channel in 0..4 {
                        color[channel] *= texel[channel] as f32 / 255.0;
                    }
                }
                // 预乘 alpha 的「覆盖」混合。
                let keep = 1.0 - color[3];
                let pixel = &mut self.pixels[y * width + x];
                for channel in 0..4 {
                    pixel[channel] = color[channel] + pixel[channel] * keep;
                }
            }
        }
    }

    pub fn save_png(&self, path: &Path) {
        let [width, height] = self.size_px();
        let mut rgb = Vec::with_capacity(width * height * 3);
        for pixel in &self.pixels {
            for channel in 0..3 {
                rgb.push((pixel[channel].clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        image::save_buffer(path, &rgb, width as u32, height as u32, image::ColorType::Rgb8)
            .expect("写截图");
    }
}

/// 驱动一个真实的 `MossPocket` 走界面流程。
pub struct Harness {
    pub app: MossPocket,
    pub ctx: egui::Context,
    pub screen: Offscreen,
    pub dir: PathBuf,
    pub tag: String,
    shots: PathBuf,
    time: f64,
    /// 只有截图前的那几帧才真正光栅化。
    paint: bool,
    /// 下一帧要模拟成「刚拖进窗口」的文件。
    dropped: Option<Vec<PathBuf>>,
    pub system_theme: egui::Theme,
    pub maximized: bool,
    close_requested: bool,
    pub commands: Vec<egui::ViewportCommand>,
    pub platform_commands: Vec<egui::OutputCommand>,
    labels: Vec<(String, egui::Rect, bool)>,
}

/// 一个被拖进窗口的文件。egui 只认这个特征，界面代码通过它拿路径。
#[derive(Debug)]
struct DroppedPath(PathBuf);

impl egui::DroppedFile for DroppedPath {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

impl Harness {
    /// `tag` 用来给临时目录和截图命名。默认用不透明底，画面看得清。
    pub fn new(tag: &str) -> Self {
        Self::with_backdrop(tag, Backdrop::Opaque)
    }

    pub fn with_backdrop(tag: &str, backdrop: Backdrop) -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("moss-pocket-ui-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let shots = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/headless-shots");
        std::fs::create_dir_all(&shots).expect("建截图目录");
        let ctx = egui::Context::default();
        let mut app = MossPocket::headless(ctx.clone(), dir.clone());
        app.settings_mut().backdrop = backdrop;
        app.reapply_settings(&ctx);
        // 半透明时给一个带渐变的假桌面，方便看出透明效果。
        let background = match backdrop {
            Backdrop::Opaque => [0.0, 0.0, 0.0, 1.0],
            _ => [0.45, 0.30, 0.18, 1.0],
        };
        Self {
            app,
            ctx,
            screen: Offscreen::new(egui::vec2(1120.0, 740.0), 1.5, background),
            dir,
            tag: tag.to_string(),
            shots,
            time: 0.0,
            paint: false,
            dropped: None,
            system_theme: egui::Theme::Dark,
            maximized: false,
            close_requested: false,
            commands: Vec::new(),
            platform_commands: Vec::new(),
            labels: Vec::new(),
        }
    }

    /// 临时数据目录本身，测试往里放素材文件用。
    pub fn data_root(&self) -> PathBuf {
        self.dir.clone()
    }

    /// 走一帧。
    pub fn step(&mut self, events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(self.screen.points.x, self.screen.points.y),
        ));
        raw.time = Some(self.time);
        raw.predicted_dt = 1.0 / 60.0;
        raw.focused = true;
        raw.system_theme = Some(self.system_theme);
        if let Some(viewport) = raw.viewports.get_mut(&raw.viewport_id) {
            viewport.native_pixels_per_point = Some(self.screen.pixels_per_point);
            viewport.maximized = Some(self.maximized);
            if std::mem::take(&mut self.close_requested) {
                viewport.events.push(egui::ViewportEvent::Close);
            }
        }
        raw.events = events;
        if let Some(paths) = self.dropped.take() {
            raw.dropped_files = paths.into_iter().map(|path| Arc::new(DroppedPath(path)) as egui::DroppedFileHandle).collect();
        }
        let app = &mut self.app;
        // 和窗口程序一样：拿到窗口的根 Ui，把界面画上去。
        let mut output = self.ctx.run_ui(raw, |ui| app.draw(ui));
        self.labels.clear();
        for shape in &output.shapes {
            collect_labels(&shape.shape, shape.clip_rect, &mut self.labels);
        }
        self.commands = output.viewport_output.remove(&egui::ViewportId::ROOT)
            .map_or_else(Vec::new, |viewport| viewport.commands);
        self.platform_commands = output.platform_output.commands;
        self.screen.absorb(&mut output.textures_delta);
        if self.paint {
            self.screen.background = if self.app.settings().backdrop == Backdrop::Opaque {
                let style = self.ctx.style_of(self.ctx.theme());
                crate::theme::clear_color(&style.visuals, self.app.settings())
            } else {
                [0.45, 0.30, 0.18, 1.0]
            };
            self.screen.paint(&self.ctx, output.shapes, output.pixels_per_point);
        }
    }

    pub fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.step(Vec::new());
        }
    }

    pub fn labels(&self, caption: &str) -> impl Iterator<Item = egui::Rect> + '_ {
        let caption = caption.to_owned();
        self.labels.iter().filter_map(move |(text, rect, _)| (text == &caption).then_some(*rect))
    }

    pub fn caption(&self, caption: &str) -> egui::Rect {
        self.labels(caption).last().unwrap_or_else(|| panic!("no visible caption {caption:?}; labels: {:?}", self.labels))
    }

    pub fn has_label_prefix(&self, prefix: &str) -> bool {
        self.labels.iter().any(|(text, _, _)| text.starts_with(prefix))
    }

    pub fn label_prefix(&self, prefix: &str) -> egui::Rect {
        self.labels.iter().find_map(|(text, rect, _)| text.starts_with(prefix).then_some(*rect))
            .unwrap_or_else(|| panic!("no visible caption starting with {prefix:?}; labels: {:?}", self.labels))
    }

    pub fn caption_is_unclipped(&self, caption: &str) -> bool {
        self.labels.iter().rev().find(|(text, _, _)| text == caption)
            .is_some_and(|(_, _, unclipped)| *unclipped)
    }

    pub fn move_pointer(&mut self, position: egui::Pos2) {
        self.step(vec![egui::Event::PointerMoved(position)]);
    }

    pub fn pointer_button(&mut self, pos: egui::Pos2, button: egui::PointerButton, pressed: bool) {
        self.step(vec![egui::Event::PointerButton { pos, button, pressed, modifiers: egui::Modifiers::NONE }]);
    }

    pub fn click_at(&mut self, position: egui::Pos2, button: egui::PointerButton) {
        self.move_pointer(position);
        self.pointer_button(position, button, true);
        self.pointer_button(position, button, false);
    }

    pub fn click(&mut self, caption: &str) {
        self.click_at(self.caption(caption).center(), egui::PointerButton::Primary);
    }

    pub fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        self.move_pointer(from);
        self.pointer_button(from, egui::PointerButton::Primary, true);
        self.move_pointer(to);
        self.pointer_button(to, egui::PointerButton::Primary, false);
    }

    pub fn click_field(&mut self, id: egui::Id) {
        let rect = self.ctx.read_response(id).unwrap_or_else(|| panic!("missing field {id:?}")).rect;
        self.click_at(rect.center(), egui::PointerButton::Primary);
    }

    pub fn scroll(&mut self, position: egui::Pos2, points: f32) {
        self.move_pointer(position);
        self.step(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, points),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers::NONE,
        }]);
        self.frames(12);
    }

    pub fn request_close(&mut self) {
        self.close_requested = true;
        self.step(Vec::new());
    }

    pub fn resize(&mut self, points: egui::Vec2) {
        self.screen.points = points;
        let [width, height] = self.screen.size_px();
        self.screen.pixels.resize(width * height, self.screen.background);
        self.frames(3);
    }

    /// 逐个字符输入已经确定的文字。
    pub fn text(&mut self, text: &str) {
        for ch in text.chars() {
            self.step(vec![egui::Event::Text(ch.to_string())]);
        }
    }

    fn key_event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: platform_modifiers(modifiers),
        }
    }

    /// 按一个键（按下、松开各走一帧）。前后像真实窗口层一样先报告修饰键状态的变化。
    pub fn press(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        if modifiers != egui::Modifiers::NONE {
            self.step(vec![egui::Event::ModifiersChanged(platform_modifiers(modifiers))]);
        }
        self.step(vec![Self::key_event(key, true, modifiers)]);
        self.step(vec![Self::key_event(key, false, modifiers)]);
        if modifiers != egui::Modifiers::NONE {
            self.step(vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        }
    }

    /// 模拟输入法的「正在组字」状态。
    pub fn ime_preedit(&mut self, text: &str) {
        self.step(vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: text.to_string(),
            active_range_chars: None,
        })]);
    }

    /// 模拟输入法把一串文字提交进文本框。
    pub fn ime_commit(&mut self, text: &str) {
        self.step(vec![egui::Event::Ime(egui::ImeEvent::Commit(text.to_string()))]);
    }

    /// 模拟把文件拖进窗口。下一帧生效。
    pub fn drop_paths(&mut self, paths: Vec<PathBuf>) {
        self.dropped = Some(paths);
    }

    /// 模拟用户点了正文以外的地方：文本框失去键盘焦点。
    pub fn release_focus(&mut self) {
        self.ctx.memory_mut(|memory| memory.stop_text_input());
        self.frames(2);
    }

    /// 反复走帧直到条件成立或超时。解锁在后台线程里算密钥，要等它。
    pub fn wait_until(&mut self, seconds: f64, mut condition: impl FnMut(&MossPocket) -> bool) -> bool {
        self.wait_for(seconds, |harness| condition(&harness.app))
    }

    pub fn wait_for_caption(&mut self, seconds: f64, caption: &str) -> bool {
        self.wait_for(seconds, |harness| harness.labels(caption).next().is_some())
    }

    fn wait_for(&mut self, seconds: f64, mut condition: impl FnMut(&Self) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        loop {
            self.frames(2);
            if condition(self) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    /// 存一张当前画面的截图，返回路径。
    pub fn snapshot(&mut self, name: &str) -> PathBuf {
        self.frames(2);
        self.paint = true;
        self.frames(1);
        self.paint = false;
        let path = self.shots.join(format!("{}-{name}.png", self.tag));
        self.screen.save_png(&path);
        path
    }

    /// 不经界面，直接在临时目录里建好库，再让应用重新看一眼。
    pub fn create_vault(&mut self, password: &str) -> String {
        let (vault, recovery) = Vault::create(&Vault::data_dir(&self.dir), password).expect("建库");
        drop(vault);
        self.app.refresh_vault();
        recovery
    }

    /// 走锁屏界面用密码解锁，返回是否成功进入。
    pub fn unlock(&mut self, password: &str) -> bool {
        // 先走几帧，等输入框拿到焦点再打字；否则头几个字符会丢。
        self.frames(5);
        self.text(password);
        self.press(egui::Key::Enter, egui::Modifiers::NONE);
        self.wait_until(30.0, MossPocket::is_unlocked)
    }
}

fn collect_labels(shape: &egui::Shape, clip: egui::Rect, labels: &mut Vec<(String, egui::Rect, bool)>) {
    match shape {
        egui::Shape::Text(text) => {
            let bounds = text.galley.rect.translate(text.pos.to_vec2());
            let rect = bounds.intersect(clip);
            if rect.is_positive() && !text.galley.job.text.is_empty() {
                let unclipped = !text.galley.elided && clip.expand(0.5).contains_rect(bounds);
                labels.push((text.galley.job.text.clone(), rect, unclipped));
            }
        }
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_labels(shape, clip, labels);
            }
        }
        _ => {}
    }
}

/// 把「逻辑上按住的修饰键」翻译成 Windows 上窗口层实际会上报的标志。
///
/// egui 0.37 把 Ctrl 和「命令键」记成两个独立的标志，而窗口层在 Windows 上按 Ctrl
/// 会同时置上这两个。窗口程序里的快捷键都是按 `Modifiers::COMMAND` 写的，
/// 测试台如果不照实翻译，所有 Ctrl 组合键都会匹配不上。
fn platform_modifiers(modifiers: egui::Modifiers) -> egui::Modifiers {
    if modifiers.ctrl {
        modifiers | egui::Modifiers::COMMAND
    } else {
        modifiers
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
