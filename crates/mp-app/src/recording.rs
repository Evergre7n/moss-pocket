//! 从系统默认麦克风采集声音，在内存中生成单声道 16 位 PCM WAV。

use crate::i18n::{Lang, Tr};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use std::fmt::Display;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WAV_HEADER_BYTES: usize = 44;
const PCM_BYTES_PER_FRAME: usize = 2;
const MAX_SECONDS: usize = 5 * 60;
const MAX_PCM_BYTES: usize = 32 * 1024 * 1024;
const NO_INPUT_TIMEOUT: Duration = Duration::from_secs(5);

// 采集流关闭后，仍在退出的回调可能短暂持有这个引用。
type SharedCapture = Arc<Mutex<Option<Capture>>>;

pub struct Recorder(Input);

enum Input {
    Microphone {
        // 先释放流，再释放缓冲。Windows 后端会在释放流时等待采集回调退出。
        stream: cpal::Stream,
        capture: SharedCapture,
        started_at: Instant,
        tr: Tr,
    },
    #[cfg(test)]
    Synthetic {
        wav: Vec<u8>,
        elapsed: Duration,
    },
}

impl Recorder {
    pub fn start(tr: Tr) -> Result<Self, String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| {
                localized(
                    tr,
                    "系统没有可用的默认麦克风。",
                    "No default microphone is available.",
                )
                .to_owned()
            })?;
        let supported = device.default_input_config().map_err(|error| {
            details(
                tr,
                "无法读取默认麦克风设置",
                "Could not read the default microphone settings",
                error,
            )
        })?;
        let config = supported.config();
        if config.channels == 0 {
            return Err(localized(
                tr,
                "麦克风返回了无效的声道数。",
                "The microphone reported an invalid channel count.",
            )
            .to_owned());
        }
        let capture = Arc::new(Mutex::new(Some(Capture::new(config.sample_rate, tr)?)));
        let data = Arc::clone(&capture);
        let stream = match supported.sample_format() {
            SampleFormat::I8 => input_stream::<i8>(&device, config, data),
            SampleFormat::I16 => input_stream::<i16>(&device, config, data),
            SampleFormat::I24 => input_stream::<cpal::I24>(&device, config, data),
            SampleFormat::I32 => input_stream::<i32>(&device, config, data),
            SampleFormat::I64 => input_stream::<i64>(&device, config, data),
            SampleFormat::U8 => input_stream::<u8>(&device, config, data),
            SampleFormat::U16 => input_stream::<u16>(&device, config, data),
            SampleFormat::U24 => input_stream::<cpal::U24>(&device, config, data),
            SampleFormat::U32 => input_stream::<u32>(&device, config, data),
            SampleFormat::U64 => input_stream::<u64>(&device, config, data),
            SampleFormat::F32 => input_stream::<f32>(&device, config, data),
            SampleFormat::F64 => input_stream::<f64>(&device, config, data),
            format => {
                return Err(details(
                    tr,
                    "麦克风的采样格式不受支持",
                    "Unsupported microphone sample format",
                    format,
                ));
            }
        }
        .map_err(|error| {
            details(tr, "无法打开默认麦克风", "Could not open the default microphone", error)
        })?;
        let started_at = Instant::now();
        stream.play().map_err(|error| {
            details(tr, "无法启动默认麦克风", "Could not start the default microphone", error)
        })?;

        Ok(Self(Input::Microphone {
            stream,
            capture,
            started_at,
            tr,
        }))
    }

    pub fn elapsed(&self) -> Duration {
        match &self.0 {
            Input::Microphone { capture, .. } => capture.lock().map_or(Duration::ZERO, |capture| {
                capture.as_ref().map_or(Duration::ZERO, |capture| {
                    let seconds = capture.frame_count() as f64 / f64::from(capture.sample_rate);
                    Duration::from_secs_f64(seconds)
                })
            }),
            #[cfg(test)]
            Input::Synthetic { elapsed, .. } => *elapsed,
        }
    }

    pub fn error(&self) -> Option<String> {
        match &self.0 {
            Input::Microphone {
                capture,
                started_at,
                tr,
                ..
            } => match capture.lock() {
                Ok(mut capture) => capture.as_mut().and_then(|capture| {
                    capture.check_timeout(*started_at, Instant::now());
                    capture.error.clone()
                }),
                Err(_) => Some(buffer_unavailable(*tr)),
            },
            #[cfg(test)]
            Input::Synthetic { .. } => None,
        }
    }

    /// 达到时长或字节上限时，调用 `stop_with_error` 保存已录制的内容。
    pub fn reached_limit(&self) -> bool {
        match &self.0 {
            Input::Microphone { capture, .. } => capture
                .lock()
                .is_ok_and(|capture| capture.as_ref().is_some_and(Capture::is_full)),
            #[cfg(test)]
            Input::Synthetic { elapsed, .. } => elapsed.as_secs() >= MAX_SECONDS as u64,
        }
    }

    /// 停止采集并交出 WAV 与中断提示。直接丢弃 `Recorder` 则取消录音。
    pub fn stop_with_error(self) -> Result<(Vec<u8>, Option<String>), String> {
        match self.0 {
            Input::Microphone {
                stream,
                capture,
                started_at,
                tr,
            } => {
                // 先取走录音，后续回调即使继续执行也不会改写已结束的录音。
                let stop_requested_at = Instant::now();
                let mut capture = capture
                    .lock()
                    .map_err(|_| buffer_unavailable(tr))?
                    .take()
                    .ok_or_else(|| buffer_unavailable(tr))?;
                drop(stream);
                capture.check_timeout(started_at, stop_requested_at);
                capture.finish()
            }
            #[cfg(test)]
            Input::Synthetic { wav, .. } => Ok((wav, None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn synthetic(wav: Vec<u8>, elapsed: Duration) -> Self {
        Self(Input::Synthetic { wav, elapsed })
    }
}

fn localized(tr: Tr, zh: &'static str, en: &'static str) -> &'static str {
    match tr.0 {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

fn details(tr: Tr, zh: &'static str, en: &'static str, detail: impl Display) -> String {
    let message = localized(tr, zh, en);
    format!("{message} ({detail})")
}

fn buffer_unavailable(tr: Tr) -> String {
    localized(
        tr,
        "无法读取麦克风录音缓冲。",
        "Could not read the microphone recording buffer.",
    )
    .to_owned()
}

fn input_stream<T: SizedSample>(
    device: &cpal::Device,
    config: StreamConfig,
    capture: SharedCapture,
) -> Result<cpal::Stream, cpal::Error>
where
    f64: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let error_capture = Arc::clone(&capture);
    device.build_input_stream(
        config,
        move |samples: &[T], _| {
            let Ok(mut capture) = capture.lock() else {
                return;
            };
            let Some(capture) = capture.as_mut() else {
                return;
            };
            capture.append(samples, channels);
        },
        move |error| {
            use cpal::ErrorKind::{DeviceChanged, RealtimeDenied, Xrun};
            if matches!(error.kind(), DeviceChanged | RealtimeDenied | Xrun) {
                return;
            }
            let Ok(mut capture) = error_capture.lock() else {
                return;
            };
            let Some(capture) = capture.as_mut() else {
                return;
            };
            if !capture.is_full() && capture.error.is_none() {
                capture.error = Some(details(
                    capture.tr,
                    "麦克风录音中断",
                    "Microphone recording was interrupted",
                    error,
                ));
            }
        },
        Some(Duration::from_secs(5)),
    )
}

struct Capture {
    // 直接把 PCM 写进最终的 WAV 缓冲，停止录音时只补写文件头。
    wav: Vec<u8>,
    sample_rate: u32,
    max_wav_len: usize,
    error: Option<String>,
    last_input: Option<Instant>,
    tr: Tr,
}

impl Capture {
    fn new(sample_rate: u32, tr: Tr) -> Result<Self, String> {
        if sample_rate == 0 || sample_rate.checked_mul(PCM_BYTES_PER_FRAME as u32).is_none() {
            return Err(localized(
                tr,
                "麦克风返回了无效的采样率。",
                "The microphone reported an invalid sample rate.",
            )
            .to_owned());
        }
        let bytes_per_second = sample_rate as usize * PCM_BYTES_PER_FRAME;
        let max_pcm_len = bytes_per_second
            .saturating_mul(MAX_SECONDS)
            .min(MAX_PCM_BYTES);
        let mut wav = Vec::new();
        wav.try_reserve_exact(WAV_HEADER_BYTES + bytes_per_second.min(max_pcm_len))
            .map_err(|error| {
                details(tr, "无法分配录音缓冲", "Could not allocate the recording buffer", error)
            })?;
        wav.resize(WAV_HEADER_BYTES, 0);
        Ok(Self {
            wav,
            sample_rate,
            max_wav_len: WAV_HEADER_BYTES + max_pcm_len,
            error: None,
            last_input: None,
            tr,
        })
    }

    fn frame_count(&self) -> usize {
        (self.wav.len() - WAV_HEADER_BYTES) / PCM_BYTES_PER_FRAME
    }

    fn is_full(&self) -> bool {
        self.wav.len() == self.max_wav_len
    }

    fn check_timeout(&mut self, started_at: Instant, now: Instant) {
        let last_input = self.last_input.unwrap_or(started_at);
        if self.error.is_none()
            && !self.is_full()
            && now.duration_since(last_input) >= NO_INPUT_TIMEOUT
        {
            self.error = Some(
                localized(
                    self.tr,
                    "麦克风连续 5 秒未返回音频采样。",
                    "The microphone did not send audio for five seconds.",
                )
                .to_owned(),
            );
        }
    }

    fn append<T: SizedSample>(&mut self, samples: &[T], channels: usize)
    where
        f64: FromSample<T>,
    {
        if self.error.is_some() || self.is_full() {
            return;
        }
        let remaining_frames = (self.max_wav_len - self.wav.len()) / PCM_BYTES_PER_FRAME;
        let frames = (samples.len() / channels).min(remaining_frames);
        if frames == 0 {
            return;
        }
        self.last_input = Some(Instant::now());
        let required = self.wav.len() + frames * PCM_BYTES_PER_FRAME;
        let capacity = self.wav.capacity();
        if required > capacity {
            let target = capacity.saturating_mul(2).max(required).min(self.max_wav_len);
            if let Err(error) = self.wav.try_reserve_exact(target - self.wav.len()) {
                self.error = Some(details(
                    self.tr,
                    "无法扩展录音缓冲",
                    "Could not expand the recording buffer",
                    error,
                ));
                return;
            }
        }

        for frame in samples.chunks_exact(channels).take(frames) {
            let sum: f64 = frame
                .iter()
                .map(|sample| {
                    let sample = sample.to_sample::<f64>();
                    if sample.is_finite() {
                        sample.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                })
                .sum();
            let mono = sum / channels as f64;
            let pcm = (mono * 32_768.0).round() as i16;
            self.wav.extend_from_slice(&pcm.to_le_bytes());
        }
    }

    fn finish(mut self) -> Result<(Vec<u8>, Option<String>), String> {
        if self.frame_count() == 0 {
            return Err(self.error.unwrap_or_else(|| {
                localized(self.tr, "没有收到麦克风采样。", "No microphone samples were received.")
                    .to_owned()
            }));
        }
        let pcm_bytes = (self.wav.len() - WAV_HEADER_BYTES) as u32;
        let byte_rate = self.sample_rate * PCM_BYTES_PER_FRAME as u32;
        self.wav[0..4].copy_from_slice(b"RIFF");
        self.wav[4..8].copy_from_slice(&(36 + pcm_bytes).to_le_bytes());
        self.wav[8..16].copy_from_slice(b"WAVEfmt ");
        self.wav[16..20].copy_from_slice(&16u32.to_le_bytes());
        self.wav[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
        self.wav[22..24].copy_from_slice(&1u16.to_le_bytes()); // Mono
        self.wav[24..28].copy_from_slice(&self.sample_rate.to_le_bytes());
        self.wav[28..32].copy_from_slice(&byte_rate.to_le_bytes());
        self.wav[32..34].copy_from_slice(&(PCM_BYTES_PER_FRAME as u16).to_le_bytes());
        self.wav[34..36].copy_from_slice(&16u16.to_le_bytes());
        self.wav[36..40].copy_from_slice(b"data");
        self.wav[40..44].copy_from_slice(&pcm_bytes.to_le_bytes());
        Ok((self.wav, self.error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const EN: Tr = Tr(Lang::En);

    #[test]
    fn stereo_float_input_becomes_mono_pcm_wav() {
        let mut capture = Capture::new(48_000, EN).unwrap();
        capture.append(&[0.25f32, 0.75, 1.5, 1.0, -1.0, -1.5, f32::NAN, 0.0], 2);
        let (wav, error) = capture.finish().unwrap();
        assert!(error.is_none());
        assert_eq!(
            &wav[..WAV_HEADER_BYTES],
            b"RIFF\x2c\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\
              \x80\xbb\x00\x00\x00\x77\x01\x00\x02\x00\x10\x00data\x08\x00\x00\x00"
        );
        assert_eq!(
            &wav[WAV_HEADER_BYTES..],
            &[0x00, 0x40, 0xff, 0x7f, 0x00, 0x80, 0x00, 0x00]
        );
    }

    #[test]
    fn unsigned_input_uses_the_midpoint_as_silence() {
        let mut capture = Capture::new(44_100, EN).unwrap();
        capture.append(&[0u8, 128, 255], 1);
        let (wav, _) = capture.finish().unwrap();
        assert_eq!(
            &wav[WAV_HEADER_BYTES..],
            &[0x00, 0x80, 0x00, 0x00, 0x00, 0x7f]
        );
    }

    #[test]
    fn full_capture_keeps_only_complete_frames_and_stays_playable() {
        let mut capture = Capture::new(1, EN).unwrap();
        capture.append(&[10i16; 2 * MAX_SECONDS - 1], 2);
        assert_eq!(capture.frame_count(), MAX_SECONDS - 1);
        assert!(!capture.is_full());
        capture.append(&[20i16, 20, 30, 30], 2);
        assert!(capture.is_full());
        capture.append(&[40i16, 40], 2);
        let (wav, _) = capture.finish().unwrap();
        assert_eq!(wav.len(), WAV_HEADER_BYTES + 2 * MAX_SECONDS);
        assert_eq!(&wav[wav.len() - 2..], &20i16.to_le_bytes());
    }

    #[test]
    fn high_sample_rate_is_also_limited_by_bytes() {
        let capture = Capture::new(192_000, EN).unwrap();
        assert_eq!(capture.max_wav_len, WAV_HEADER_BYTES + MAX_PCM_BYTES);
    }

    #[test]
    fn empty_capture_returns_an_error() {
        assert!(Capture::new(0, EN).is_err());
        assert!(Capture::new(48_000, EN).unwrap().finish().is_err());
        let mut capture = Capture::new(48_000, EN).unwrap();
        capture.error = Some("device disconnected".to_owned());
        assert_eq!(capture.finish().unwrap_err(), "device disconnected");
    }

    #[test]
    fn interrupted_capture_keeps_available_frames() {
        let mut capture = Capture::new(48_000, EN).unwrap();
        capture.append(&[42i16], 1);
        capture.error = Some("device disconnected".to_owned());
        capture.append(&[43i16], 1);
        let (wav, error) = capture.finish().unwrap();
        assert_eq!(error.as_deref(), Some("device disconnected"));
        assert_eq!(&wav[WAV_HEADER_BYTES..], &42i16.to_le_bytes());
    }

    #[test]
    fn no_input_times_out_with_a_localized_message() {
        let start = Instant::now();
        let mut capture = Capture::new(48_000, Tr(Lang::Zh)).unwrap();
        capture.check_timeout(start, start + Duration::from_secs(4));
        assert!(capture.error.is_none());
        capture.check_timeout(start, start + NO_INPUT_TIMEOUT);
        assert_eq!(
            capture.finish().unwrap_err(),
            "麦克风连续 5 秒未返回音频采样。"
        );
    }

    #[test]
    fn stalled_input_retains_audio_with_a_timeout_message() {
        let start = Instant::now();
        let mut capture = Capture::new(48_000, EN).unwrap();
        capture.append(&[42i16], 1);
        let last_input = capture.last_input.unwrap();
        capture.check_timeout(start, last_input + NO_INPUT_TIMEOUT);
        let (wav, error) = capture.finish().unwrap();
        assert_eq!(&wav[WAV_HEADER_BYTES..], &42i16.to_le_bytes());
        assert_eq!(
            error.as_deref(),
            Some("The microphone did not send audio for five seconds.")
        );
    }
}
