//! MOSS-Pocket 的平台无关核心。不依赖任何界面库，可以在命令行和测试里单独运行。
//!
//! 模块划分：
//! - [`crypto`]：密钥派生、主密钥包装、分块加密
//! - [`model`]：笔记与块的数据模型
//! - [`vault`]：磁盘上的库文件格式与读写
//! - [`media`]：按文件名判定图片、音频、视频
//! - [`spans`]、[`editor`]：行内样式区间与块式编辑器的纯逻辑
//! - [`find`]：笔记内查找与替换清单

pub mod crypto;
pub mod editor;
pub mod error;
pub mod find;
pub mod media;
pub mod model;
pub mod spans;
pub mod util;
pub mod vault;

pub use error::{CoreError, Result};
