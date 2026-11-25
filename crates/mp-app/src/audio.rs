//! 音频播放。输出流和播放器住在一个专门的线程上，界面线程只发命令、收状态。
//!
//! 这样解码和声卡不会拖住界面；锁屏时会话连同引擎一起丢弃，线程随之退出。
//! 解密是在读取时按块进行的，硬盘和内存里都不会出现整段明文。

use crate::i18n::{self, Tr};
use eframe::egui;
use mp_core::vault::BlobHandle;
use rodio::Source;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 已打开的加密音频文件。
pub(crate) type OpenBlob = mp_core::vault::BlobReader<std::io::BufReader<std::fs::File>>;
type AudioSource = Box<dyn Source + Send>;

/// 界面发给播放线程的命令。
#[derive(Debug, Clone)]
pub enum AudioCommand {
    Play { blob_id: String, mime: String },
    Pause,
    Resume,
    Stop,
    /// 跳到第几秒。
    Seek(f32),
}

/// 播放器的当前状态。正在播放时大约每十分之一秒更新进度。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioState {
    /// 正在播放（或暂停中）的那一块的媒体标识。
    pub blob_id: Option<String>,
    pub playing: bool,
    /// 播放队列中仍有内容，用户可以从当前进度继续。
    pub paused: bool,
    pub position: f32,
    pub duration: Option<f32>,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            blob_id: None,
            playing: false,
            paused: false,
            position: 0.0,
            duration: None,
        }
    }
}

struct AudioEvent {
    state: AudioState,
    error: Option<String>,
}

/// 音频引擎。整个应用里只有一个；不许克隆，免得出现两个线程同时出声。
pub struct AudioEngine {
    sender: Sender<AudioCommand>,
    inbox: Receiver<AudioEvent>,
    pub state: AudioState,
    /// 最近一次失败的原因，界面取走后清空。
    pub last_error: Option<String>,
}

impl AudioEngine {
    /// 启动播放线程。在收到播放命令之前不会发出任何声音。失败原因用界面语言 `tr` 写。
    pub fn start(blob: BlobHandle, tr: Tr, ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self::with_output(blob, tr, move || ctx.request_repaint(), move || {
            let sink = rodio::DeviceSinkBuilder::open_default_sink()
                .map_err(|error| i18n::audio_device_failed(tr, &error.to_string()))?;
            Ok((rodio::Player::connect_new(sink.mixer()), sink))
        })
    }

    fn with_output<T>(
        blob: BlobHandle,
        tr: Tr,
        wake: impl Fn() + Send + 'static,
        open_output: impl Fn() -> Result<(rodio::Player, T), String> + Send + 'static,
    ) -> Self {
        let (sender, commands) = channel::<AudioCommand>();
        let (events, inbox) = channel::<AudioEvent>();
        std::thread::spawn(move || run(blob, commands, events, tr, wake, open_output));
        Self {
            sender,
            inbox,
            state: AudioState::default(),
            last_error: None,
        }
    }

    pub fn send(&self, command: AudioCommand) {
        let _ = self.sender.send(command);
    }

    /// 收下播放线程发来的状态。
    pub fn poll(&mut self) {
        while let Ok(event) = self.inbox.try_recv() {
            self.state = event.state;
            if let Some(error) = event.error {
                self.last_error = Some(error);
            }
        }
    }
}

// 发送端随引擎一起丢弃时，播放线程会在下一次等待命令时发现通道已断并退出。

fn run<T>(
    blob: BlobHandle,
    commands: Receiver<AudioCommand>,
    events: Sender<AudioEvent>,
    tr: Tr,
    wake: impl Fn(),
    open_output: impl Fn() -> Result<(rodio::Player, T), String>,
) {
    let mut output = None;
    let mut state = AudioState::default();

    loop {
        let command = if state.playing {
            match commands.recv_timeout(PROGRESS_INTERVAL) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            let Ok(command) = commands.recv() else { break };
            Some(command)
        };
        let command_finished = command.is_some();
        let previous = state.clone();
        let mut error = None;
        match command {
            Some(AudioCommand::Play { blob_id, mime }) => {
                output = None;
                state = AudioState::default();
                match decode(&blob, &blob_id, &mime, tr).and_then(|(decoder, total)| {
                    let (player, sink) = open_output()?;
                    player.append(decoder);
                    player.play();
                    Ok((player, sink, total))
                }) {
                    Ok((player, sink, total)) => {
                        output = Some((player, sink));
                        state.blob_id = Some(blob_id);
                        state.duration = total.map(|value| value.as_secs_f32());
                    }
                    Err(message) => error = Some(message),
                }
            }
            Some(AudioCommand::Stop) => {
                output = None;
                state = AudioState::default();
            }
            command => {
                if let Some((player, _)) = &output {
                    match command {
                        Some(AudioCommand::Pause) => player.pause(),
                        Some(AudioCommand::Resume) => player.play(),
                        Some(AudioCommand::Seek(seconds)) => {
                            match Duration::try_from_secs_f32(seconds.max(0.0)) {
                                Ok(position) if seconds.is_finite() && player.try_seek(position).is_ok() => {}
                                _ => error = Some(tr.err_audio_seek().into()),
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some((player, _)) = &output {
            let has_audio = !player.empty();
            state.paused = has_audio && player.is_paused();
            state.playing = has_audio && !state.paused;
            state.position = if has_audio { player.get_pos().as_secs_f32() }
                else { state.duration.unwrap_or(previous.position) };
            if !has_audio {
                output = None;
            }
        }
        if command_finished || state != previous {
            if events.send(AudioEvent { state: state.clone(), error }).is_err() {
                break;
            }
            wake();
        }
    }
}

/// 打开一个加密的音频文件并造出解码器。不涉及声卡，所以测试里也能用。
fn decode(blob: &BlobHandle, blob_id: &str, mime: &str, tr: Tr) -> Result<(AudioSource, Option<Duration>), String> {
    let reader = blob.open(blob_id).map_err(|error| tr.core_error(&error))?;
    let byte_len = reader.len();
    let decoder = rodio::Decoder::builder()
        .with_data(reader)
        .with_seekable(true)
        .with_byte_len(byte_len)
        .with_mime_type(mime)
        .build();
    let decoder: AudioSource = match decoder {
        Ok(decoder) => Box::new(decoder),
        #[cfg(windows)]
        Err(_) if mime == "audio/x-ms-wma" => {
            let reader = blob.open(blob_id).map_err(|error| tr.core_error(&error))?;
            Box::new(crate::native_audio::NativeAudio::open(reader)
                .map_err(|error| i18n::audio_decode_failed(tr, &error.to_string()))?)
        }
        #[cfg(windows)]
        Err(_) if matches!(mime, "audio/ogg" | "audio/opus") => {
            let reader = blob.open(blob_id).map_err(|error| tr.core_error(&error))?;
            Box::new(crate::native_audio::OpusSource::open(reader)
                .map_err(|error| i18n::audio_decode_failed(tr, &error.to_string()))?)
        }
        Err(error) => return Err(i18n::audio_decode_failed(tr, &error.to_string())),
    };
    let duration = decoder.total_duration();
    Ok((decoder, duration))
}

/// 这段音频的时长（秒），导入时在后台线程调用，给界面显示用。
/// 解不了的格式或读不出时长的格式返回 None，不影响文件入库。
pub fn probe_duration(blob: &BlobHandle, blob_id: &str, mime: &str, tr: Tr) -> Option<f64> {
    let (_, duration) = decode(blob, blob_id, mime, tr).ok()?;
    duration.map(|value| value.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mp_core::vault::Vault;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn seekable_m4a_can_be_probed_and_played_from_an_encrypted_blob() {
        let root = std::env::temp_dir().join(format!("moss-aac-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = Vault::create(&root, "media-aac").unwrap();
        let bytes = include_bytes!("../tests/fixtures/short-aac.m4a");
        let (id, _) = vault.import_blob(&bytes[..]).unwrap();
        let tr = Tr(crate::i18n::Lang::En);
        let blob = vault.blob_handle();
        assert!(probe_duration(&blob, &id, "audio/mp4", tr).is_some_and(|duration| duration > 0.1));
        let (mut decoder, _) = decode(&blob, &id, "audio/mp4", tr).unwrap();
        assert!(decoder.any(|sample| sample.abs() > 0.01));
        drop(decoder);
        drop(vault);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn poll_next(audio: &mut AudioEngine, wakes: &Receiver<()>) {
        wakes.recv_timeout(Duration::from_secs(2)).unwrap();
        audio.poll();
    }

    #[cfg(windows)]
    #[test]
    fn fallback_decodes_and_seeks_encrypted_audio_fixtures() {
        let root = std::env::temp_dir().join(format!("moss-audio-fallback-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = Vault::create(&root, "media-native").unwrap();
        let blob = vault.blob_handle();
        let tr = Tr(crate::i18n::Lang::En);
        let mut failures = Vec::new();
        let fixtures: [(&str, &str, &[u8]); 2] = [
            ("WMA2/ASF", "audio/x-ms-wma", include_bytes!("../tests/fixtures/short-asf-wma.wma")),
            ("Opus/Ogg", "audio/opus", include_bytes!("../tests/fixtures/short-ogg-opus.opus")),
        ];
        for (format, mime, bytes) in fixtures {
            let (id, _) = blob.import(bytes).unwrap();
            let Ok((mut source, duration)) = decode(&blob, &id, mime, tr).map_err(|error| {
                failures.push(format!("{format}: {error}"));
            }) else { continue };
            assert!(duration.is_some_and(|duration| duration > Duration::from_millis(500)), "{format}");
            assert!(source.by_ref().take(16_000).any(|sample| sample.abs() > 0.01), "{format}");
            source.try_seek(Duration::from_millis(400)).unwrap();
            assert!(source.by_ref().take(16_000).any(|sample| sample.abs() > 0.01), "{format}");
            source.try_seek(Duration::from_secs(100)).unwrap();
            assert!(source.next().is_none(), "{format}");
            source.try_seek(Duration::ZERO).unwrap();
            assert!(source.take(16_000).any(|sample| sample.abs() > 0.01), "{format}");
        }
        let (id, _) = blob.import(&b"unsupported audio bytes"[..]).unwrap();
        assert!(decode(&blob, &id, "audio/x-ms-wma", tr).is_err());
        assert!(decode(&blob, &id, "audio/opus", tr).is_err());
        drop(blob);
        drop(vault);
        std::fs::remove_dir_all(root).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }

    fn assert_idle(wakes: &Receiver<()>) {
        assert_eq!(wakes.recv_timeout(PROGRESS_INTERVAL * 3), Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn audio_notifies_commands_and_sleeps_while_paused_finished_or_stopped() {
        let path = std::env::temp_dir().join(format!("moss-audio-events-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = Vault::create(&path, "test1234").unwrap();
        let blob = vault.blob_handle();
        let (blob_id, _) = blob.import(crate::scenarios::wav_bytes(800).as_slice()).unwrap();
        let play = AudioCommand::Play { blob_id, mime: "audio/wav".into() };

        // Test decoder and player together, consuming samples without an OS output device.
        let sources = Arc::new(Mutex::new(None));
        let output = sources.clone();
        let failed_once = AtomicBool::new(false);
        let (wake, wakes) = channel();
        let mut audio = AudioEngine::with_output(blob, Tr(crate::i18n::Lang::En), move || {
            let _ = wake.send(());
        }, move || {
            if !failed_once.swap(true, Ordering::Relaxed) {
                return Err("test output unavailable".into());
            }
            let (player, queue) = rodio::Player::new();
            *output.lock().unwrap() = Some(queue);
            Ok((player, ()))
        });

        audio.send(play.clone());
        poll_next(&mut audio, &wakes);
        assert_eq!(audio.last_error.take().as_deref(), Some("test output unavailable"));
        assert!(!audio.state.playing);
        assert_idle(&wakes);

        audio.send(play.clone());
        poll_next(&mut audio, &wakes);
        assert!(audio.state.playing);
        audio.send(AudioCommand::Pause);
        poll_next(&mut audio, &wakes);
        assert!(audio.state.paused);
        assert_idle(&wakes);

        audio.send(AudioCommand::Resume);
        poll_next(&mut audio, &wakes);
        assert!(audio.state.playing);
        let mut queue = sources.lock().unwrap().take().unwrap();
        for _ in (&mut queue).take(400) {}
        poll_next(&mut audio, &wakes);
        assert!(audio.state.playing && audio.state.position > 0.0);
        for _ in (&mut queue).take(1600) {}
        poll_next(&mut audio, &wakes);
        assert!(!audio.state.playing && !audio.state.paused);
        assert_eq!(Some(audio.state.position), audio.state.duration);
        assert_idle(&wakes);

        audio.send(play);
        poll_next(&mut audio, &wakes);
        assert!(audio.state.playing);
        audio.send(AudioCommand::Stop);
        poll_next(&mut audio, &wakes);
        assert_eq!(audio.state, AudioState::default());
        assert_idle(&wakes);

        drop(audio);
        drop(vault);
        std::fs::remove_dir_all(path).unwrap();
    }
}
