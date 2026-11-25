//! Additional audio codecs that decode from a seekable, encrypted vault reader.

use crate::audio::OpenBlob;
use rodio::{ChannelCount, SampleRate, Source};
use std::ffi::c_void;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::Duration;
use windows::core::{s, w, Error, Interface, PCWSTR, Ref, GUID, HRESULT};
use windows::Win32::Foundation::{
    FreeLibrary, E_FAIL, E_NOTIMPL, E_POINTER, HMODULE, S_FALSE, S_OK, STG_E_ACCESSDENIED,
    STG_E_INVALIDFUNCTION, STG_E_READFAULT, STG_E_SEEKERROR,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFByteStream, IMFMediaType, IMFSourceReader, MFMediaType_Audio, MFSTARTUP_FULL,
    MFAudioFormat_PCM, MF_VERSION, MF_BYTESTREAM_CONTENT_TYPE, MF_BYTESTREAM_ORIGIN_NAME,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToUInt64, PROPVARIANT,
};
use windows::Win32::System::Com::{
    CoDecrementMTAUsage, CoIncrementMTAUsage, ISequentialStream_Impl, IStream, IStream_Impl,
    CO_MTA_USAGE_COOKIE, LOCKTYPE, STATFLAG, STATSTG, STGC, STGTY_STREAM, STREAM_SEEK,
    STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::Variant::VT_I8;

pub(crate) use opus::OpusSource;

const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const TICKS_PER_SECOND: u64 = 10_000_000;

pub(crate) struct NativeAudio {
    reader: IMFSourceReader,
    _runtime: Runtime,
    channels: ChannelCount,
    rate: SampleRate,
    duration: Option<Duration>,
    pcm: Vec<f32>,
    cursor: usize,
    seek_target: Option<i64>,
    ended: bool,
}

// MF objects are thread-safe and work in the MTA. Runtime keeps that apartment
// alive as the Source passes from the audio command thread to the Rodio mixer.
unsafe impl Send for NativeAudio {}

impl NativeAudio {
    pub(crate) fn open(blob: OpenBlob) -> Result<Self, Error> {
        let runtime = Runtime::start()?;
        let stream: IStream = VaultStream(Mutex::new(blob)).into();
        let (reader, channels, rate, duration) = unsafe {
            let mut bytes = None;
            (runtime.byte_stream)(stream.as_raw(), &mut bytes).ok()?;
            let bytes: IMFByteStream = bytes.ok_or(Error::from_hresult(E_POINTER))?;
            let attributes: IMFAttributes = bytes.cast()?;
            attributes.SetString(&MF_BYTESTREAM_ORIGIN_NAME, w!("audio.wma"))?;
            attributes.SetString(&MF_BYTESTREAM_CONTENT_TYPE, w!("audio/x-ms-wma"))?;
            let mut reader = None;
            (runtime.source_reader)(bytes.as_raw(), std::ptr::null_mut(), &mut reader).ok()?;
            let reader: IMFSourceReader = reader.ok_or(Error::from_hresult(E_POINTER))?;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(AUDIO, true)?;
            let mut pcm = None;
            (runtime.media_type)(&mut pcm).ok()?;
            let pcm: IMFMediaType = pcm.ok_or(Error::from_hresult(E_POINTER))?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            reader.SetCurrentMediaType(AUDIO, None, &pcm)?;
            let (channels, rate) = audio_format(&reader)?;
            let duration = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|mut value| {
                    let ticks = PropVariantToUInt64(&value).ok();
                    let _ = PropVariantClear(&mut value);
                    ticks.and_then(|ticks| Duration::try_from_secs_f64(ticks as f64 / TICKS_PER_SECOND as f64).ok())
                });
            (reader, channels, rate, duration)
        };
        let mut source = Self {
            reader,
            _runtime: runtime,
            channels,
            rate,
            duration,
            pcm: Vec::new(),
            cursor: 0,
            seek_target: None,
            ended: false,
        };
        if !source.refill()? {
            return Err(Error::from_hresult(E_FAIL));
        }
        Ok(source)
    }

    fn refill(&mut self) -> Result<bool, Error> {
        while !self.ended {
            let mut flags = 0;
            let mut timestamp = 0;
            let mut sample = None;
            unsafe {
                self.reader.ReadSample(
                    AUDIO, 0, None, Some(&mut flags), Some(&mut timestamp), Some(&mut sample),
                )?;
                if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                    return Err(Error::from_hresult(E_FAIL));
                }
                self.ended = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
                if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0
                    && audio_format(&self.reader)? != (self.channels, self.rate)
                {
                    return Err(Error::from_hresult(E_FAIL));
                }
                let Some(sample) = sample else { continue };
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut data = std::ptr::null_mut();
                let mut len = 0;
                buffer.Lock(&mut data, None, Some(&mut len))?;
                let sample_bytes = std::mem::size_of::<i16>();
                self.pcm.clear();
                if len != 0 && !data.is_null() && len as usize % sample_bytes == 0 {
                    let bytes = std::slice::from_raw_parts(data, len as usize);
                    self.pcm.extend(bytes.chunks_exact(sample_bytes).map(|word| {
                        f32::from(i16::from_le_bytes([word[0], word[1]])) / 32_768.0
                    }));
                }
                buffer.Unlock()?;
                let skip = self.seek_target.map_or(0, |target| {
                    let ticks = target.saturating_sub(timestamp).max(0) as u128;
                    let frames = (ticks * u128::from(self.rate.get())) / u128::from(TICKS_PER_SECOND);
                    let samples = frames * u128::from(self.channels.get());
                    usize::try_from(samples).unwrap_or(usize::MAX)
                });
                self.cursor = skip.min(self.pcm.len());
                if self.cursor < self.pcm.len() {
                    self.seek_target = None;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

unsafe fn audio_format(reader: &IMFSourceReader) -> Result<(ChannelCount, SampleRate), Error> {
    unsafe {
        let media = reader.GetCurrentMediaType(AUDIO)?;
        if media.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE)? != 16 {
            return Err(Error::from_hresult(E_FAIL));
        }
        let channels = media.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?;
        let channels = u16::try_from(channels).ok().and_then(ChannelCount::new)
            .ok_or(Error::from_hresult(E_FAIL))?;
        let rate = SampleRate::new(media.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?)
            .ok_or(Error::from_hresult(E_FAIL))?;
        Ok((channels, rate))
    }
}

impl Iterator for NativeAudio {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.cursor >= self.pcm.len() && !self.refill().unwrap_or(false) {
            self.ended = true;
            return None;
        }
        let sample = self.pcm[self.cursor];
        self.cursor += 1;
        Some(sample)
    }
}

impl Source for NativeAudio {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        if self.duration.is_some_and(|duration| position >= duration) {
            self.pcm.clear();
            self.cursor = 0;
            self.seek_target = None;
            self.ended = true;
            return Ok(());
        }
        let ticks = (position.as_nanos() / 100).min(i64::MAX as u128) as i64;
        let mut target = PROPVARIANT::default();
        unsafe {
            let value = &mut *target.Anonymous.Anonymous;
            value.vt = VT_I8;
            value.Anonymous.hVal = ticks;
            self.reader.SetCurrentPosition(&GUID::zeroed(), &target).map_err(|error| {
                rodio::source::SeekError::Other(std::sync::Arc::new(error))
            })?;
        }
        self.pcm.clear();
        self.cursor = 0;
        self.ended = false;
        self.seek_target = Some(ticks);
        Ok(())
    }
}

#[windows::core::implement(IStream)]
struct VaultStream(Mutex<OpenBlob>);

impl ISequentialStream_Impl for VaultStream_Impl {
    fn Read(&self, target: *mut c_void, count: u32, actual: *mut u32) -> HRESULT {
        if count != 0 && target.is_null() {
            return E_POINTER;
        }
        unsafe {
            if !actual.is_null() {
                *actual = 0;
            }
        }
        if count == 0 {
            return S_OK;
        }
        let Ok(mut reader) = self.0.lock() else { return STG_E_READFAULT };
        let count = count as usize;
        let mut chunk = [0; 8192];
        let mut filled = 0;
        while filled < count {
            let remaining = (count - filled).min(chunk.len());
            match reader.read(&mut chunk[..remaining]) {
                Ok(0) => break,
                Ok(read) => {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            chunk.as_ptr(), target.cast::<u8>().add(filled), read,
                        );
                    }
                    filled += read;
                }
                Err(_) => return STG_E_READFAULT,
            }
        }
        unsafe {
            if !actual.is_null() {
                *actual = filled as u32;
            }
        }
        if filled == count { S_OK } else { S_FALSE }
    }

    fn Write(&self, _: *const c_void, _: u32, _: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for VaultStream_Impl {
    fn Seek(&self, offset: i64, origin: STREAM_SEEK, actual: *mut u64) -> Result<(), Error> {
        let from = match origin {
            STREAM_SEEK_SET if offset >= 0 => SeekFrom::Start(offset as u64),
            STREAM_SEEK_CUR => SeekFrom::Current(offset),
            STREAM_SEEK_END => SeekFrom::End(offset),
            _ => return Err(Error::from_hresult(STG_E_INVALIDFUNCTION)),
        };
        let mut reader = self.0.lock().map_err(|_| Error::from_hresult(STG_E_SEEKERROR))?;
        let pos = reader.seek(from).map_err(|_| Error::from_hresult(STG_E_SEEKERROR))?;
        unsafe {
            if !actual.is_null() {
                *actual = pos;
            }
        }
        Ok(())
    }

    fn Stat(&self, stat: *mut STATSTG, _: &STATFLAG) -> Result<(), Error> {
        if stat.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        let reader = self.0.lock().map_err(|_| Error::from_hresult(STG_E_READFAULT))?;
        unsafe {
            *stat = STATSTG {
                r#type: STGTY_STREAM.0 as u32,
                cbSize: reader.len(),
                ..Default::default()
            };
        }
        Ok(())
    }

    fn SetSize(&self, _: u64) -> Result<(), Error> {
        Err(Error::from_hresult(STG_E_ACCESSDENIED))
    }

    fn CopyTo(&self, _: Ref<IStream>, _: u64, _: *mut u64, _: *mut u64) -> Result<(), Error> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn Commit(&self, _: &STGC) -> Result<(), Error> {
        Ok(())
    }

    fn Revert(&self) -> Result<(), Error> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn LockRegion(&self, _: u64, _: u64, _: &LOCKTYPE) -> Result<(), Error> {
        Err(Error::from_hresult(STG_E_INVALIDFUNCTION))
    }

    fn UnlockRegion(&self, _: u64, _: u64, _: u32) -> Result<(), Error> {
        Err(Error::from_hresult(STG_E_INVALIDFUNCTION))
    }

    fn Clone(&self) -> Result<IStream, Error> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
}

// Resolve Media Foundation lazily so a missing Windows Media Feature Pack only
// disables these audio codecs.
struct Module(HMODULE);

impl Module {
    fn open(name: PCWSTR) -> Result<Self, Error> {
        unsafe { LoadLibraryExW(name, None, LOAD_LIBRARY_SEARCH_SYSTEM32).map(Self) }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

struct Apartment(CO_MTA_USAGE_COOKIE);

impl Drop for Apartment {
    fn drop(&mut self) {
        let _ = unsafe { CoDecrementMTAUsage(self.0) };
    }
}

struct Runtime {
    shutdown: unsafe extern "system" fn() -> HRESULT,
    byte_stream: unsafe extern "system" fn(*mut c_void, *mut Option<IMFByteStream>) -> HRESULT,
    source_reader: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut Option<IMFSourceReader>) -> HRESULT,
    media_type: unsafe extern "system" fn(*mut Option<IMFMediaType>) -> HRESULT,
    _readwrite: Module,
    _platform: Module,
    _apartment: Apartment,
}

impl Runtime {
    fn start() -> Result<Self, Error> {
        let apartment = Apartment(unsafe { CoIncrementMTAUsage()? });
        let platform = Module::open(w!("mfplat.dll"))?;
        let readwrite = Module::open(w!("mfreadwrite.dll"))?;
        let startup: unsafe extern "system" fn(u32, u32) -> HRESULT;
        unsafe {
            startup = std::mem::transmute(GetProcAddress(platform.0, s!("MFStartup"))
                .ok_or_else(Error::from_thread)?);
            let shutdown = std::mem::transmute(GetProcAddress(platform.0, s!("MFShutdown"))
                .ok_or_else(Error::from_thread)?);
            let byte_stream = std::mem::transmute(GetProcAddress(platform.0, s!("MFCreateMFByteStreamOnStream"))
                .ok_or_else(Error::from_thread)?);
            let media_type = std::mem::transmute(GetProcAddress(platform.0, s!("MFCreateMediaType"))
                .ok_or_else(Error::from_thread)?);
            let source_reader = std::mem::transmute(GetProcAddress(readwrite.0, s!("MFCreateSourceReaderFromByteStream"))
                .ok_or_else(Error::from_thread)?);
            startup(MF_VERSION, MFSTARTUP_FULL).ok()?;
            Ok(Self {
                shutdown, byte_stream, source_reader, media_type,
                _readwrite: readwrite, _platform: platform, _apartment: apartment,
            })
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = unsafe { (self.shutdown)() };
    }
}

mod opus {
    use super::{ChannelCount, Duration, OpenBlob, Read, SampleRate, Seek, SeekFrom, Source};
    use std::io;
    use std::ptr::NonNull;
    use std::sync::Arc;
    use symphonia::core::{
        codecs::CODEC_TYPE_OPUS,
        errors::Error,
        formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
        io::{MediaSource, MediaSourceStream},
        meta::MetadataOptions,
        probe::Hint,
    };

    const RATE: u32 = 48_000;
    const MAX_FRAME: usize = 5_760;
    const SEEK_PREROLL: u64 = 3_840;

    struct Decoder(NonNull<opusic_sys::OpusMSDecoder>);

    // Ownership of the codec state is exclusive and follows the Rodio source.
    unsafe impl Send for Decoder {}

    impl Decoder {
        fn new(header: &[u8], channels: usize) -> Result<Self, Error> {
            if !header.starts_with(b"OpusHead") || header.len() < 19 {
                return Err(Error::DecodeError("invalid Opus identification packet"));
            }
            let (streams, coupled, mapping) = match header[18] {
                0 if (1..=2).contains(&channels) => (1, channels as u8 - 1, &[0, 1][..channels]),
                1 if header.len() >= 21 + channels => {
                    (header[19], header[20], &header[21..21 + channels])
                }
                _ => return Err(Error::Unsupported("unsupported Opus channel mapping")),
            };
            let mut status = opusic_sys::OPUS_OK;
            let decoder = unsafe {
                opusic_sys::opus_multistream_decoder_create(
                    RATE as i32,
                    channels as i32,
                    i32::from(streams),
                    i32::from(coupled),
                    mapping.as_ptr(),
                    &mut status,
                )
            };
            let decoder = Self(NonNull::new(decoder)
                .ok_or(Error::DecodeError("cannot initialize Opus decoder"))?);
            if status != opusic_sys::OPUS_OK {
                return Err(Error::DecodeError("cannot initialize Opus decoder"));
            }
            let gain = i32::from(i16::from_le_bytes([header[16], header[17]]));
            if gain != 0 {
                let status = unsafe {
                    opusic_sys::opus_multistream_decoder_ctl(
                        decoder.0.as_ptr(), opusic_sys::OPUS_SET_GAIN_REQUEST, gain,
                    )
                };
                if status != opusic_sys::OPUS_OK {
                    return Err(Error::DecodeError("cannot apply Opus header gain"));
                }
            }
            Ok(decoder)
        }

        fn decode(&mut self, packet: &[u8], samples: &mut [f32]) -> Result<usize, Error> {
            if packet.is_empty() {
                return Err(Error::DecodeError("empty Opus packet"));
            }
            let bytes = i32::try_from(packet.len())
                .map_err(|_| Error::DecodeError("Opus packet too large"))?;
            let frames = unsafe {
                opusic_sys::opus_multistream_decode_float(
                    self.0.as_ptr(), packet.as_ptr(), bytes, samples.as_mut_ptr(),
                    MAX_FRAME as i32, 0,
                )
            };
            usize::try_from(frames).map_err(|_| Error::DecodeError("invalid Opus packet"))
        }

        fn reset(&mut self) -> Result<(), Error> {
            let status = unsafe {
                opusic_sys::opus_multistream_decoder_ctl(self.0.as_ptr(), opusic_sys::OPUS_RESET_STATE)
            };
            if status == opusic_sys::OPUS_OK {
                Ok(())
            } else {
                Err(Error::DecodeError("cannot reset Opus decoder"))
            }
        }
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            unsafe { opusic_sys::opus_multistream_decoder_destroy(self.0.as_ptr()) };
        }
    }

    pub(crate) struct OpusSource {
        format: Box<dyn FormatReader>,
        decoder: Decoder,
        track: u32,
        start: u64,
        duration: Option<Duration>,
        channels: ChannelCount,
        pcm: Vec<f32>,
        cursor: usize,
        end: usize,
        seek_target: Option<u64>,
        ended: bool,
    }

    impl OpusSource {
        pub(crate) fn open(blob: OpenBlob) -> Result<Self, Error> {
            let media = MediaSourceStream::new(Box::new(SeekableBlob(blob)), Default::default());
            let options = FormatOptions { enable_gapless: true, ..Default::default() };
            let probed = symphonia::default::get_probe().format(
                Hint::new().with_extension("ogg"), media, &options, &MetadataOptions::default(),
            )?;
            let track = probed.format.tracks().iter()
                .find(|track| track.codec_params.codec == CODEC_TYPE_OPUS)
                .ok_or(Error::Unsupported("no Opus track"))?;
            let params = &track.codec_params;
            let track = track.id;
            let time_base = params.time_base.ok_or(Error::Unsupported("missing Opus time base"))?;
            let duration = params.n_frames.map(|frames| Duration::from(time_base.calc_time(frames)));
            let start = params.start_ts;
            let channels = params.channels.and_then(|channels| u16::try_from(channels.count()).ok())
                .and_then(ChannelCount::new)
                .ok_or(Error::Unsupported("invalid Opus channels"))?;
            let channel_count = usize::from(channels.get());
            let header = params.extra_data.as_deref().ok_or(Error::Unsupported("missing Opus header"))?;
            let decoder = Decoder::new(header, channel_count)?;
            let mut source = Self {
                format: probed.format,
                decoder,
                track,
                start,
                duration,
                channels,
                pcm: vec![0.0; MAX_FRAME * channel_count],
                cursor: 0,
                end: 0,
                seek_target: None,
                ended: false,
            };
            if !source.refill()? {
                return Err(Error::DecodeError("empty Opus track"));
            }
            Ok(source)
        }

        fn refill(&mut self) -> Result<bool, Error> {
            loop {
                let packet = match self.format.next_packet() {
                    Ok(packet) => packet,
                    Err(Error::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                        return Ok(false);
                    }
                    Err(error) => return Err(error),
                };
                if packet.track_id() != self.track {
                    continue;
                }
                let frames = self.decoder.decode(&packet.data, &mut self.pcm)?;
                let channels = usize::from(self.channels.get());
                self.end = frames.saturating_sub(packet.trim_end() as usize) * channels;
                // Ogg Opus granules are measured in samples at 48 kHz.
                let skip = self.seek_target.map_or(0, |target| {
                    usize::try_from(target.saturating_sub(packet.ts())).unwrap_or(usize::MAX)
                });
                self.cursor = (packet.trim_start() as usize).saturating_add(skip)
                    .saturating_mul(channels).min(self.end);
                if self.cursor < self.end {
                    self.seek_target = None;
                    return Ok(true);
                }
            }
        }
    }

    impl Iterator for OpusSource {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.ended {
                return None;
            }
            if self.cursor >= self.end && !self.refill().unwrap_or(false) {
                self.ended = true;
                return None;
            }
            let sample = self.pcm[self.cursor];
            self.cursor += 1;
            Some(sample)
        }
    }

    impl Source for OpusSource {
        fn current_span_len(&self) -> Option<usize> { None }
        fn channels(&self) -> ChannelCount { self.channels }
        fn sample_rate(&self) -> SampleRate { SampleRate::new(RATE).unwrap() }
        fn total_duration(&self) -> Option<Duration> { self.duration }

        fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
            if self.duration.is_some_and(|duration| position >= duration) {
                self.end = 0;
                self.cursor = 0;
                self.seek_target = None;
                self.ended = true;
                return Ok(());
            }
            let frames = position.as_nanos() * u128::from(RATE) / 1_000_000_000;
            let target = self.start.saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
            // RFC 7845 recommends decoding 80 ms before emitting audio after a seek.
            let preroll = target.saturating_sub(SEEK_PREROLL).max(self.start);
            self.format.seek(
                SeekMode::Accurate,
                SeekTo::TimeStamp { ts: preroll, track_id: self.track },
            ).map_err(|error| rodio::source::SeekError::Other(Arc::new(error)))?;
            self.decoder.reset().map_err(|error| rodio::source::SeekError::Other(Arc::new(error)))?;
            self.end = 0;
            self.cursor = 0;
            self.seek_target = Some(target);
            self.ended = false;
            Ok(())
        }
    }

    struct SeekableBlob(OpenBlob);

    impl Read for SeekableBlob {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> { self.0.read(buffer) }
    }

    impl Seek for SeekableBlob {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> { self.0.seek(position) }
    }

    impl MediaSource for SeekableBlob {
        fn is_seekable(&self) -> bool { true }
        fn byte_len(&self) -> Option<u64> { Some(self.0.len()) }
    }
}
