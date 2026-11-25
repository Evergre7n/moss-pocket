//! 媒体的导入与缓存。导入和图片解码都放到后台线程，界面不会因为一个大视频卡住。
//!
//! 流程：界面线程说「导入这个文件」→ 后台线程加密入库、必要时解码出尺寸和预览图 →
//! 结果排队回来 → 界面线程在下一帧把这一块插进笔记。

use crate::audio::AudioEngine;
use crate::i18n::{self, Tr};
use crate::stream::StreamServer;
use eframe::egui;
use image::ImageDecoder;
use mp_core::media::{classify, MediaKind};
use mp_core::model::{Block, BlockId, ImageBlock, MediaBlock};
use mp_core::util::random_block_id;
use mp_core::vault::BlobHandle;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufReader, Cursor, Read};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// 预览图最长边的上限，用来控制显存占用。原图原样存库，不重新编码。
const PREVIEW_MAX_SIDE: u32 = 1600;
/// 最多同时留多少张预览图，超了淘汰最久没显示的。
const PREVIEW_CACHE_LIMIT: usize = 24;

/// 一次导入的来源。
#[derive(Debug, Clone)]
pub enum ImportSource {
    File(PathBuf),
    /// 已经在内存里的内容：剪贴板里复制的图片就是这样。
    Memory { name: String, bytes: Vec<u8> },
}

pub struct ImportJob {
    pub source: ImportSource,
    pub anchor: Option<BlockId>,
    pub anchor_revision: u64,
    pub note_id: String,
    pub recording: bool,
}

/// 后台线程交回界面的结果。
pub enum MediaEvent {
    Imported {
        note_id: String,
        anchor: Option<BlockId>,
        anchor_revision: u64,
        block: Block,
        preview: Option<Arc<egui::ColorImage>>,
        recording: bool,
    },
    ImageReady { blob_id: String, image: Arc<egui::ColorImage> },
    ImageFailed { blob_id: String, message: String },
    ImportFailed { message: String, retry: Option<ImportJob> },
}

pub struct MediaHub {
    blob: Option<BlobHandle>,
    /// 界面语言。后台线程报失败时按它写。
    tr: Tr,
    sender: Sender<MediaEvent>,
    inbox: Receiver<MediaEvent>,
    /// 正在后台解码的图片。
    loading: HashSet<String>,
    failed_previews: HashSet<String>,
    queued_imports: Vec<ImportJob>,
    active_imports: usize,
    pending_recordings: usize,
    previews: HashMap<String, (egui::TextureHandle, [usize; 2])>,
    /// 按最近使用的顺序记着缓存里的图片，用来淘汰。
    recent: VecDeque<String>,
    audio: Option<AudioEngine>,
    stream: Option<StreamServer>,
}

impl MediaHub {
    pub fn new(blob: BlobHandle, tr: Tr) -> Self {
        let (sender, inbox) = channel();
        Self {
            blob: Some(blob),
            tr,
            sender,
            inbox,
            loading: HashSet::new(),
            failed_previews: HashSet::new(),
            queued_imports: Vec::new(),
            active_imports: 0,
            pending_recordings: 0,
            previews: HashMap::new(),
            recent: VecDeque::new(),
            audio: None,
            stream: None,
        }
    }

    /// 取回后台交来的全部结果。每帧调一次。
    pub fn poll(&mut self) -> Vec<MediaEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.inbox.try_recv() {
            match &event {
                MediaEvent::Imported { recording, .. } => {
                    self.active_imports = self.active_imports.saturating_sub(1);
                    self.pending_recordings = self.pending_recordings.saturating_sub(usize::from(*recording));
                }
                MediaEvent::ImportFailed { retry, .. } => {
                    self.active_imports = self.active_imports.saturating_sub(1);
                    self.pending_recordings = self.pending_recordings.saturating_sub(usize::from(retry.is_some()));
                }
                MediaEvent::ImageReady { blob_id, .. } => { self.loading.remove(blob_id); }
                MediaEvent::ImageFailed { blob_id, .. } => {
                    self.loading.remove(blob_id);
                    self.failed_previews.insert(blob_id.clone());
                }
            }
            events.push(event);
        }
        self.start_queued_imports();
        if let Some(audio) = &mut self.audio {
            audio.poll();
        }
        events
    }

    pub fn pending_import_count(&self) -> usize {
        self.active_imports + self.queued_imports.len()
    }

    pub fn recording_import_pending(&self) -> bool {
        self.pending_recordings > 0
    }

    pub fn has_background_work(&self) -> bool {
        self.pending_import_count() > 0 || !self.loading.is_empty()
    }

    pub fn preview_failed(&self, blob_id: &str) -> bool {
        self.failed_previews.contains(blob_id)
    }

    /// 按提交顺序导入，每项完成后单独回传结果。
    pub fn import_batch(&mut self, jobs: Vec<ImportJob>) {
        self.pending_recordings += jobs.iter().filter(|job| job.recording).count();
        self.queued_imports.extend(jobs);
        self.start_queued_imports();
    }

    fn start_queued_imports(&mut self) {
        if self.active_imports != 0 || self.queued_imports.is_empty() {
            return;
        }
        let Some(blob) = self.blob.clone() else {
            return;
        };
        let jobs = std::mem::take(&mut self.queued_imports);
        self.active_imports = jobs.len();
        let sender = self.sender.clone();
        let tr = self.tr;
        std::thread::spawn(move || {
            for job in jobs {
                let event = match import_in_background(&blob, &job.source, tr) {
                    Ok((block, preview)) => MediaEvent::Imported {
                        note_id: job.note_id,
                        anchor: job.anchor,
                        anchor_revision: job.anchor_revision,
                        block,
                        preview,
                        recording: job.recording,
                    },
                    Err(message) => MediaEvent::ImportFailed { message, retry: job.recording.then_some(job) },
                };
                if sender.send(event).is_err() {
                    return;
                }
            }
        });
    }

    /// 要显示某张图片。缓存里有就给；没有就安排后台解码，这一帧先画占位。
    pub fn preview(&mut self, blob_id: &str) -> Option<egui::TextureHandle> {
        if let Some((handle, _)) = self.previews.get(blob_id) {
            let handle = handle.clone();
            self.touch(blob_id);
            return Some(handle);
        }
        if self.failed_previews.contains(blob_id) {
            return None;
        }
        if self.loading.insert(blob_id.to_string()) {
            let blob = self.blob.clone()?;
            let sender = self.sender.clone();
            let tr = self.tr;
            let wanted = blob_id.to_string();
            std::thread::spawn(move || {
                let event = match blob.read(&wanted) {
                    Ok(bytes) => match decode_preview(&bytes) {
                        Ok((_, image)) => MediaEvent::ImageReady { blob_id: wanted, image: Arc::new(image) },
                        Err(_) => MediaEvent::ImageFailed { message: i18n::image_unreadable(tr, &wanted), blob_id: wanted },
                    },
                    Err(error) => MediaEvent::ImageFailed { blob_id: wanted, message: tr.core_error(&error) },
                };
                let _ = sender.send(event);
            });
        }
        None
    }

    /// 后台解码好的图片送到显卡上，进缓存。
    pub fn store_preview(&mut self, ctx: &egui::Context, blob_id: &str, image: &egui::ColorImage) {
        let size = image.size;
        let handle = ctx.load_texture(format!("preview-{blob_id}"), image.clone(), egui::TextureOptions::LINEAR);
        self.previews.insert(blob_id.to_string(), (handle, size));
        self.loading.remove(blob_id);
        self.failed_previews.remove(blob_id);
        self.touch(blob_id);
        while self.previews.len() > PREVIEW_CACHE_LIMIT {
            let Some(oldest) = self.recent.pop_front() else {
                break;
            };
            self.previews.remove(&oldest);
        }
    }

    /// 记下这张图刚刚被显示过。
    fn touch(&mut self, blob_id: &str) {
        self.recent.retain(|id| id != blob_id);
        self.recent.push_back(blob_id.to_string());
    }

    fn audio(&mut self, ctx: &egui::Context) -> Option<&AudioEngine> {
        if self.audio.is_none() {
            let blob = self.blob.clone()?;
            self.audio = Some(AudioEngine::start(blob, self.tr, ctx));
        }
        self.audio.as_ref()
    }

    pub fn play_audio(&mut self, ctx: &egui::Context, blob_id: &str, mime: &str) {
        if let Some(audio) = self.audio(ctx) {
            audio.send(crate::audio::AudioCommand::Play { blob_id: blob_id.to_string(), mime: mime.to_string() });
        }
    }

    pub fn pause_audio(&mut self) {
        self.audio.as_ref().map(|audio| audio.send(crate::audio::AudioCommand::Pause));
    }

    pub fn resume_audio(&mut self) {
        self.audio.as_ref().map(|audio| audio.send(crate::audio::AudioCommand::Resume));
    }

    pub fn stop_audio(&mut self) {
        self.audio.as_ref().map(|audio| audio.send(crate::audio::AudioCommand::Stop));
    }

    pub fn seek_audio(&mut self, seconds: f32) {
        self.audio.as_ref().map(|audio| audio.send(crate::audio::AudioCommand::Seek(seconds)));
    }

    pub fn audio_state(&self) -> Option<crate::audio::AudioState> {
        self.audio.as_ref().map(|audio| audio.state.clone())
    }

    pub fn take_audio_error(&mut self) -> Option<String> {
        self.audio.as_mut().and_then(|audio| audio.last_error.take())
    }

    /// 窗内播放器的解密流地址；首次播放时启动服务。
    pub fn video_url(&mut self, blob_id: &str, file_name: &str) -> Option<String> {
        if self.stream.as_ref().is_none_or(|server| !server.is_running()) {
            let blob = self.blob.clone()?;
            self.stream = StreamServer::start(blob).ok();
        }
        self.stream.as_ref().map(|server| server.url(blob_id, file_name))
    }

    pub fn stop_video_stream(&mut self) {
        self.stream = None;
    }

    pub fn read_blob(&self, blob_id: &str) -> Result<Vec<u8>, String> {
        self.blob
            .as_ref()
            .ok_or_else(|| self.tr.err_vault_locked().to_string())?
            .read(blob_id)
            .map_err(|error| self.tr.core_error(&error))
    }
}

/// 在后台线程里完成一次导入。失败原因用界面语言 `tr` 写。
fn import_in_background(
    blob: &BlobHandle,
    source: &ImportSource,
    tr: Tr,
) -> Result<(Block, Option<Arc<egui::ColorImage>>), String> {
    let (name, kind, reader): (String, MediaKind, Box<dyn Read + Send + '_>) = match source {
        ImportSource::File(path) => {
            let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
            let kind = classify(&name).ok_or_else(|| i18n::unsupported_file(tr, &name))?;
            let file = std::fs::File::open(path)
                .map_err(|error| i18n::cannot_open_file(tr, &name, &error.to_string()))?;
            (name, kind, Box::new(BufReader::new(file)))
        }
        ImportSource::Memory { name, bytes } => {
            let name = PathBuf::from(name).file_name().map(|part| part.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
            let kind = classify(&name).ok_or_else(|| i18n::unsupported_file(tr, &name))?;
            (name, kind, Box::new(Cursor::new(bytes)))
        }
    };

    let (blob_id, size) = blob.import(reader).map_err(|error| tr.core_error(&error))?;
    let id = random_block_id().unwrap_or(size);
    let mime = kind.mime(&name).to_string();

    match kind {
        MediaKind::Image => {
            // 读回来解一次，拿到尺寸和预览图。解不了就当导入失败，留下的文件下次解锁时会被清掉。
            let decoded = blob.read(&blob_id).ok().and_then(|bytes| decode_preview(&bytes).ok());
            match decoded {
                Some(([width, height], image)) => Ok((
                    Block::Image(ImageBlock { id, blob_id, width: width as u32, height: height as u32, display_width: None }),
                    Some(Arc::new(image)),
                )),
                None => Err(i18n::image_unreadable(tr, &name)),
            }
        }
        MediaKind::Audio => {
            let duration_secs = crate::audio::probe_duration(blob, &blob_id, &mime, tr);
            Ok((Block::Audio(MediaBlock { id, blob_id, mime, duration_secs, file_name: name, size }), None))
        }
        MediaKind::Video => Ok((Block::Video(MediaBlock { id, blob_id, file_name: name, mime, size, duration_secs: None }), None)),
    }
}

/// 解码一张图片，缩到最长边不超过上限。返回 (原始宽高, 预览图)。
/// 会按照片里记录的拍摄方向摆正。
pub fn decode_preview(bytes: &[u8]) -> Result<([usize; 2], egui::ColorImage), String> {
    let mut decoder = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .into_decoder()
        .map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
    let full_size = [image.width() as usize, image.height() as usize];
    if full_size.contains(&0) {
        // 这里的说明只是内部诊断，不会直接给用户看：调用方按界面语言另写。
        return Err("image has zero size".into());
    }
    let scaled = if full_size[0] as u32 > PREVIEW_MAX_SIDE || full_size[1] as u32 > PREVIEW_MAX_SIDE {
        image.thumbnail(PREVIEW_MAX_SIDE, PREVIEW_MAX_SIDE)
    } else {
        image
    };
    let rgba = scaled.to_rgba8();
    let pixels = rgba
        .pixels()
        .map(|pixel| egui::Color32::from_rgba_unmultiplied(pixel[0], pixel[1], pixel[2], pixel[3]))
        .collect();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok((full_size, egui::ColorImage::new(size, pixels)))
}

/// 把界面上的一张图（比如剪贴板里的截图）编成 PNG。
pub fn encode_png(image: &egui::ColorImage) -> Result<Vec<u8>, String> {
    let [width, height] = image.size;
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    let rgba = image::RgbaImage::from_raw(width as u32, height as u32, bytes).ok_or_else(|| "invalid image size".to_string())?;
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(out)
}

/// 文件大小的可读写法，按十进制算（1 KB = 1000 字节）。
pub fn format_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < units.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", units[unit])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_audio_engine_does_not_keep_media_polling_active() {
        let path = std::env::temp_dir().join(format!("moss-media-idle-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = mp_core::vault::Vault::create(&path, "test1234").unwrap();
        let mut media = MediaHub::new(vault.blob_handle(), Tr(crate::i18n::Lang::En));
        let ctx = egui::Context::default();
        assert!(media.audio(&ctx).is_some());
        assert!(!media.has_background_work());

        drop(media);
        drop(vault);
        std::fs::remove_dir_all(path).unwrap();
    }

    /// 造一张两色相间的小 PNG。
    pub fn sample_png(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = image::RgbaImage::new(width, height);
        for (x, y, pixel) in rgba.enumerate_pixels_mut() {
            *pixel = if (x + y) % 2 == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) };
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(rgba).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn preview_keeps_small_images_exact_and_shrinks_large_ones() {
        let (size, preview) = decode_preview(&sample_png(40, 24)).unwrap();
        assert_eq!(size, [40, 24]);
        assert_eq!(preview.size, [40, 24]);
        assert_eq!(preview.pixels[0], egui::Color32::from_rgb(255, 0, 0));
        assert_eq!(preview.pixels[1], egui::Color32::from_rgb(0, 0, 255));

        // 长边超过上限的按比例缩小，比例不变。
        let (size, preview) = decode_preview(&sample_png(4000, 2000)).unwrap();
        assert_eq!(size, [4000, 2000]);
        assert_eq!(preview.size, [PREVIEW_MAX_SIDE as usize, 800]);
    }

    #[test]
    fn garbage_is_rejected_instead_of_panicking() {
        assert!(decode_preview(b"not an image").is_err());
        assert!(decode_preview(&[]).is_err());
    }

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1000), "1.0 KB");
        assert_eq!(format_size(12_345_678), "12.3 MB");
        assert_eq!(format_size(2_500_000_000), "2.5 GB");
        assert_eq!(format_size(999_999_999_999_999), "1000 TB");
    }

    #[test]
    fn clipboard_image_round_trips_through_png() {
        let image = egui::ColorImage::new([3, 2], vec![egui::Color32::from_rgb(10, 200, 30); 6]);
        let png = encode_png(&image).unwrap();
        let (size, back) = decode_preview(&png).unwrap();
        assert_eq!(size, [3, 2]);
        assert_eq!(back.pixels, vec![egui::Color32::from_rgb(10, 200, 30); 6]);
    }
}
