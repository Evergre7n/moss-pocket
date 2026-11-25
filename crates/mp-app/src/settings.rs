//! 用户设置。明文 JSON，放在 exe 旁边，不含任何机密；没解锁也能改外观。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    System,
    Zh,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// 窗口背景材质。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backdrop {
    /// 亚克力：实时模糊窗口后面的内容。
    Acrylic,
    /// 不透明。
    #[default]
    Opaque,
    /// 纯半透明，不模糊。
    Transparent,
    /// 云母：取桌面壁纸做底。
    Mica,
    /// Windows 11 自带的系统亚克力。
    SystemAcrylic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Settings {
    pub language: Language,
    pub theme_mode: ThemeMode,
    pub backdrop: Backdrop,
    /// 窗口底色，0xRRGGBB。
    pub tint: u32,
    /// 窗口底色的不透明度，0–255。
    pub tint_alpha: u8,
    /// 强调色（选中、链接、按钮），0xRRGGBB。
    pub accent: u32,
    /// 圆角大小。
    pub corner_radius: f32,
    /// 标题和正文分别指定字族。旧配置自动使用原来的搭配。
    pub heading_font: String,
    pub body_font: String,
    /// 正文字号。
    pub base_font_size: f32,
    /// 行距倍数。
    pub line_spacing: f32,
    /// 块与块之间的额外间距。
    pub paragraph_spacing: f32,
    /// 左侧列表宽度。
    pub list_width: f32,
    /// 闲置多少分钟后自动锁定；0 表示不自动锁。
    pub auto_lock_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::System,
            theme_mode: ThemeMode::System,
            backdrop: Backdrop::default(),
            tint: crate::theme::DEFAULT_TINT,
            tint_alpha: 150,
            accent: crate::theme::DEFAULT_ACCENT,
            corner_radius: 10.0,
            heading_font: crate::fonts::DEFAULT.into(),
            body_font: crate::fonts::DEFAULT.into(),
            base_font_size: 15.0,
            line_spacing: 1.6,
            paragraph_spacing: 6.0,
            list_width: 248.0,
            auto_lock_minutes: 15,
        }
    }
}

impl Settings {
    pub fn path(exe_dir: &Path) -> PathBuf {
        exe_dir.join("MOSS-Pocket.settings.json")
    }

    /// 读不到或格式不对时一律退回默认值，设置坏了不该让程序打不开。
    pub fn load(exe_dir: &Path) -> Settings {
        std::fs::read(Self::path(exe_dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .map(|mut settings: Settings| {
                settings.sanitize();
                settings
            })
            .unwrap_or_default()
    }

    /// 把越界的值夹回可用范围。
    pub fn sanitize(&mut self) {
        self.corner_radius = self.corner_radius.clamp(0.0, 24.0);
        self.base_font_size = self.base_font_size.clamp(11.0, 28.0);
        self.line_spacing = self.line_spacing.clamp(1.0, 3.0);
        self.paragraph_spacing = self.paragraph_spacing.clamp(0.0, 40.0);
        self.list_width = self.list_width.clamp(180.0, 560.0);
        self.auto_lock_minutes = self.auto_lock_minutes.min(24 * 60);
    }

    /// 写回磁盘。换成本机同目录的临时文件再改名，不会写出半个文件。
    #[allow(dead_code)]
    pub fn save(&self, exe_dir: &Path) -> std::io::Result<()> {
        let path = Self::path(exe_dir);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(&tmp, &path)
    }
}
