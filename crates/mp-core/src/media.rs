//! 媒体类型的判定：按文件扩展名把文件归到图片、音频、视频。

/// 三种媒体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Audio,
    Video,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac", "wma"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "ts", "mpg", "mpeg", "3gp"];

/// 认不认得这个文件；认得的话归哪一类。
pub fn classify(file_name: &str) -> Option<MediaKind> {
    let extension = file_name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase())?;
    let known = |list: &[&str]| list.contains(&extension.as_str());
    if known(IMAGE_EXTENSIONS) {
        Some(MediaKind::Image)
    } else if known(AUDIO_EXTENSIONS) {
        Some(MediaKind::Audio)
    } else if known(VIDEO_EXTENSIONS) {
        Some(MediaKind::Video)
    } else {
        None
    }
}

/// 所有认得的扩展名，给文件选择框的筛选器用。
pub fn extensions() -> impl Iterator<Item = &'static str> {
    IMAGE_EXTENSIONS.iter().chain(AUDIO_EXTENSIONS).chain(VIDEO_EXTENSIONS).copied()
}

impl MediaKind {
    /// 该类媒体的扩展名，给分类文件选择框的筛选器用。
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            MediaKind::Image => IMAGE_EXTENSIONS,
            MediaKind::Audio => AUDIO_EXTENSIONS,
            MediaKind::Video => VIDEO_EXTENSIONS,
        }
    }

    /// 文件的 MIME 类型，交给播放器和解码器时用。
    pub fn mime(self, file_name: &str) -> &'static str {
        let extension = file_name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
        match self {
            MediaKind::Image => match extension.as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                "bmp" => "image/bmp",
                _ => "image/png",
            },
            MediaKind::Audio => match extension.as_str() {
                "mp3" => "audio/mpeg",
                "wav" => "audio/wav",
                "flac" => "audio/flac",
                "ogg" | "oga" => "audio/ogg",
                "opus" => "audio/opus",
                "m4a" => "audio/mp4",
                "aac" => "audio/aac",
                _ => "audio/x-ms-wma",
            },
            MediaKind::Video => match extension.as_str() {
                "webm" => "video/webm",
                "mov" => "video/quicktime",
                "mkv" => "video/x-matroska",
                "avi" => "video/x-msvideo",
                "wmv" => "video/x-ms-wmv",
                "mpg" | "mpeg" => "video/mpeg",
                "3gp" => "video/3gpp",
                "flv" => "video/x-flv",
                "ts" => "video/mp2t",
                _ => "video/mp4",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_follows_extension_case_insensitively() {
        assert_eq!(classify("photo.PNG"), Some(MediaKind::Image));
        assert_eq!(classify("a.b.JPEG"), Some(MediaKind::Image));
        assert_eq!(classify("song.flac"), Some(MediaKind::Audio));
        assert_eq!(classify("clip.MKV"), Some(MediaKind::Video));
        assert_eq!(classify("notes.txt"), None);
        assert_eq!(classify("noextension"), None);
        assert_eq!(classify(".hidden"), None);
        assert_eq!(classify("archive.zip"), None);
    }

    #[test]
    fn mime_types_are_specific_when_known() {
        assert_eq!(MediaKind::Image.mime("a.jpg"), "image/jpeg");
        assert_eq!(MediaKind::Image.mime("a.png"), "image/png");
        assert_eq!(MediaKind::Audio.mime("a.m4a"), "audio/mp4");
        assert_eq!(MediaKind::Video.mime("a.webm"), "video/webm");
        assert_eq!(MediaKind::Video.mime("a.mov"), "video/quicktime");
        assert_eq!(MediaKind::Video.mime("a.mp4"), "video/mp4");
        assert_eq!(MediaKind::Video.mime("a.flv"), "video/x-flv");
        assert_eq!(MediaKind::Video.mime("a.ts"), "video/mp2t");
    }
}
