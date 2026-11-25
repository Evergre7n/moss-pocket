//! 字体选项和加载。只映射实际选用的系统字体，文档和界面各自有字族。

use crate::settings::Settings;
use eframe::egui::{self, FontDefinitions, FontFamily, FontId};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub const DEFAULT: &str = "default";

type FontFile = (&'static str, u32);

pub struct Face {
    pub key: &'static str,
    pub chinese_name: &'static str,
    pub english_name: &'static str,
    regular: FontFile,
    bold: Option<FontFile>,
}

impl Face {
    const fn new(
        key: &'static str,
        chinese_name: &'static str,
        english_name: &'static str,
        regular: FontFile,
        bold: Option<FontFile>,
    ) -> Self {
        Self { key, chinese_name, english_name, regular, bold }
    }
}

const FACES: &[Face] = &[
    Face::new("fang_song", "仿宋", "FangSong", ("simfang.ttf", 0), None),
    Face::new("kai_ti", "楷体", "KaiTi", ("simkai.ttf", 0), None),
    Face::new("deng_xian", "等线", "DengXian", ("Deng.ttf", 0), Some(("Dengb.ttf", 0))),
    Face::new("ya_hei", "微软雅黑", "Microsoft YaHei", ("msyh.ttc", 1), Some(("msyhbd.ttc", 1))),
    Face::new("hei_ti", "黑体", "SimHei", ("simhei.ttf", 0), None),
    Face::new("georgia", "Georgia", "Georgia", ("georgia.ttf", 0), Some(("georgiab.ttf", 0))),
    Face::new("cambria", "Cambria", "Cambria", ("cambria.ttc", 0), Some(("cambriab.ttf", 0))),
    Face::new("segoe_ui", "Segoe UI", "Segoe UI", ("segoeui.ttf", 0), Some(("segoeuib.ttf", 0))),
    Face::new("calibri", "Calibri", "Calibri", ("calibri.ttf", 0), Some(("calibrib.ttf", 0))),
    Face::new("arial", "Arial", "Arial", ("arial.ttf", 0), Some(("arialbd.ttf", 0))),
    Face::new("bahnschrift", "Bahnschrift", "Bahnschrift", ("bahnschrift.ttf", 0), None),
    Face::new("cascadia_mono", "Cascadia Mono", "Cascadia Mono", ("CascadiaMono.ttf", 0), None),
    Face::new("consolas", "Consolas", "Consolas", ("consola.ttf", 0), Some(("consolab.ttf", 0))),
];

fn system_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("Fonts")))
        .as_deref()
}

pub fn available_faces() -> &'static [&'static Face] {
    static AVAILABLE: OnceLock<Vec<&'static Face>> = OnceLock::new();
    AVAILABLE.get_or_init(|| {
        system_dir().map_or_else(Vec::new, |dir| {
            FACES.iter().filter(|face| dir.join(face.regular.0).is_file()).collect()
        })
    })
}

fn load(fonts: &mut FontDefinitions, dir: Option<&Path>, (file, index): FontFile) -> Option<String> {
    let key = format!("mp-{file}-{index}");
    if fonts.font_data.contains_key(&key) {
        return Some(key);
    }
    let path = dir?.join(file);
    let data = std::fs::File::open(&path).ok().and_then(|file| {
        // 系统字体目录下的文件受保护，Windows 在映射期间不会允许覆盖它。
        let mapped = unsafe { memmap2::MmapOptions::new().map(&file) }.ok()?;
        Some(egui::FontData::from_blob(Arc::new(mapped), index))
    }).or_else(|| {
        let mut data = egui::FontData::from_owned(std::fs::read(path).ok()?);
        data.index = index;
        Some(data)
    })?;
    fonts.font_data.insert(key.clone(), Arc::new(data));
    Some(key)
}

fn stack(
    fonts: &mut FontDefinitions,
    dir: Option<&Path>,
    files: &[FontFile],
    fallback: &[String],
) -> Vec<String> {
    let mut names = Vec::new();
    for name in files.iter().filter_map(|file| load(fonts, dir, *file)).chain(fallback.iter().cloned()) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn named(name: &'static str) -> FontFamily {
    FontFamily::Name(name.into())
}

fn apply_note_face(
    fonts: &mut FontDefinitions,
    settings_face: &str,
    roles: (&'static str, &'static str),
    fallback: (&[String], &[String]),
) {
    let face = available_faces().iter().find(|face| face.key == settings_face);
    let regular = face.and_then(|face| load(fonts, system_dir(), face.regular));
    let bold = face.and_then(|face| face.bold).and_then(|file| load(fonts, system_dir(), file));
    for (role, picked, defaults) in [(roles.0, regular, fallback.0), (roles.1, bold, fallback.1)] {
        let mut names = defaults.to_vec();
        if let Some(name) = picked {
            names.retain(|font| font != &name);
            names.insert(0, name);
        }
        fonts.families.insert(named(role), names);
    }
}

pub fn install(ctx: &egui::Context, settings: &Settings) {
    type Selection = (String, String);
    let id = egui::Id::unique("moss-pocket-note-fonts");
    let selection = (settings.heading_font.clone(), settings.body_font.clone());
    if ctx.data(|data| data.get_temp::<Selection>(id)).as_ref() == Some(&selection) {
        return;
    }

    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let dir = system_dir();
    let body = stack(&mut fonts, dir, &[("segoeui.ttf", 0), ("msyh.ttc", 1)], &fallback);
    let bold = stack(&mut fonts, dir, &[("segoeuib.ttf", 0), ("msyhbd.ttc", 1)], &body);
    let display = stack(&mut fonts, dir, &[("georgia.ttf", 0), ("simfang.ttf", 0)], &body);
    let mono = stack(&mut fonts, dir, &[("consola.ttf", 0), ("msyh.ttc", 1)], &fallback);

    apply_note_face(&mut fonts, &selection.0, ("mp-note-heading", "mp-note-heading-bold"), (&body, &bold));
    apply_note_face(&mut fonts, &selection.1, ("mp-note-body", "mp-note-body-bold"), (&body, &bold));
    fonts.families.insert(FontFamily::Proportional, body);
    fonts.families.insert(FontFamily::Monospace, mono);
    fonts.families.insert(named("mp-bold"), bold);
    fonts.families.insert(named("mp-display"), display);
    ctx.set_fonts(fonts);
    ctx.data_mut(|data| data.insert_temp(id, selection));
}

pub fn note_font(size: f32, heading: bool, bold: bool) -> FontId {
    let name = match (heading, bold) {
        (true, false) => "mp-note-heading",
        (true, true) => "mp-note-heading-bold",
        (false, false) => "mp-note-body",
        (false, true) => "mp-note-body-bold",
    };
    FontId::new(size, named(name))
}
