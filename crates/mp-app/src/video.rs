//! Windows Media Foundation playback in a child of the editor's native window.

use eframe::egui;
use raw_window_handle::Win32WindowHandle;
use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::core::{s, w, BSTR, HRESULT, Interface};
use windows::Win32::Foundation::{
    FreeLibrary, E_INVALIDARG, E_POINTER, HMODULE, HWND, LPARAM, LRESULT, PROPERTYKEY, RECT,
    RPC_E_CHANGED_MODE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFMediaEngine, IMFMediaEngineClassFactory,
    IMFMediaEngineEx, IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MFNETSOURCE_CACHEENABLED,
    MFSTARTUP_FULL, MF_VERSION,
    MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_EVENT, MF_MEDIA_ENGINE_EVENT_CANPLAY,
    MF_MEDIA_ENGINE_EVENT_DURATIONCHANGE, MF_MEDIA_ENGINE_EVENT_ENDED, MF_MEDIA_ENGINE_EVENT_ERROR,
    MF_MEDIA_ENGINE_EVENT_FORMATCHANGE, MF_MEDIA_ENGINE_EVENT_LOADEDDATA,
    MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA, MF_MEDIA_ENGINE_EVENT_PAUSE, MF_MEDIA_ENGINE_EVENT_PLAYING,
    MF_MEDIA_ENGINE_EVENT_SEEKED, MF_MEDIA_ENGINE_EVENT_STREAMRENDERINGERROR,
    MF_MEDIA_ENGINE_PLAYBACK_HWND, MF_MEDIA_ENGINE_READY_HAVE_CURRENT_DATA,
    MF_MEDIA_ENGINE_SOURCE_RESOLVER_CONFIG_STORE,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::Variant::VT_I4;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PSCreateMemoryPropertyStore};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetClientRect, SetWindowPos, ShowWindow, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WM_ERASEBKGND, WM_NCDESTROY, WM_PAINT,
    WS_CHILD, WS_CLIPSIBLINGS, WS_DISABLED,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoState {
    Loading,
    Playing,
    Paused,
    Ended,
}

pub struct VideoSnapshot {
    pub state: VideoState,
    pub position: f64,
    pub duration: Option<f64>,
    pub aspect_ratio: Option<f32>,
    pub error: Option<String>,
}

pub struct VideoPlayer {
    // Field order keeps the native window and COM runtime alive through Shutdown and Release.
    engine: PlaybackEngine,
    display: IMFMediaEngineEx,
    surface: VideoSurface,
    _media_foundation: MediaFoundation,
    _apartment: ComApartment,
    ctx: egui::Context,
    event_error: Arc<AtomicU64>,
    loaded: Cell<bool>,
}

impl VideoPlayer {
    pub fn open(
        parent: Win32WindowHandle,
        url: &str,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let apartment = ComApartment::enter().map_err(system_error)?;
        let media_foundation = MediaFoundation::start().map_err(system_error)?;
        let surface = VideoSurface::create(HWND(parent.hwnd.get() as *mut c_void))
            .map_err(system_error)?;
        let event_error = Arc::new(AtomicU64::new(0));
        let notify: IMFMediaEngineNotify = PlayerEvents {
            ctx: ctx.clone(),
            error: Arc::clone(&event_error),
        }
        .into();
        let engine = media_foundation
            .create_engine(&notify, surface.hwnd)
            .map_err(system_error)?;
        let display = engine.0.cast().map_err(system_error)?;
        unsafe {
            engine.0.SetAutoPlay(true).map_err(system_error)?;
            engine.0.SetSource(&BSTR::from(url)).map_err(system_error)?;
        }
        surface.install_painter(&display).map_err(system_error)?;
        Ok(Self {
            engine,
            display,
            surface,
            _media_foundation: media_foundation,
            _apartment: apartment,
            ctx: ctx.clone(),
            event_error,
            loaded: Cell::new(false),
        })
    }

    pub fn snapshot(&self) -> VideoSnapshot {
        let engine = &self.engine.0;
        let ready = unsafe { engine.GetReadyState() };
        self.loaded.set(
            self.loaded.get() || ready >= MF_MEDIA_ENGINE_READY_HAVE_CURRENT_DATA.0 as u16,
        );

        let event_error = self.event_error.load(Ordering::Relaxed);
        let error = if event_error != 0 {
            let kind = (event_error >> 32) as u32;
            let code = event_error as u32;
            Some(format!("Media Foundation ({kind}, 0x{code:08X})"))
        } else if self.loaded.get() && !unsafe { engine.HasVideo() }.as_bool() {
            Some("Media Foundation (no video stream)".into())
        } else {
            None
        };

        let state = if !self.loaded.get() {
            VideoState::Loading
        } else if unsafe { engine.IsEnded() }.as_bool() {
            VideoState::Ended
        } else if unsafe { engine.IsPaused() }.as_bool() {
            VideoState::Paused
        } else {
            VideoState::Playing
        };
        if state == VideoState::Playing && error.is_none() {
            self.ctx.request_repaint_after(Duration::from_millis(100));
        }
        let duration = unsafe { engine.GetDuration() };
        let duration = (duration.is_finite() && duration > 0.0).then_some(duration);
        let position = unsafe { engine.GetCurrentTime() }.max(0.0);
        let position = if position.is_finite() { position } else { 0.0 };
        let mut width = 0;
        let mut height = 0;
        let aspect_ratio = unsafe { engine.GetVideoAspectRatio(Some(&mut width), Some(&mut height)) }
            .ok()
            .and_then(|()| (width != 0 && height != 0).then(|| width as f32 / height as f32));

        VideoSnapshot {
            state,
            position,
            duration,
            aspect_ratio,
            error,
        }
    }

    /// Set the video rectangle in egui points relative to the editor's client origin.
    pub fn set_bounds(&self, rect: Option<egui::Rect>, pixels_per_point: f32) -> Result<(), String> {
        let Some(rect) = rect.filter(|_| self.loaded.get()) else {
            self.surface.hide();
            return Ok(());
        };
        let mut client = RECT::default();
        unsafe { GetClientRect(self.surface.parent, &mut client) }.map_err(system_error)?;
        let Some(bounds) = client_pixels(rect, pixels_per_point, client) else {
            self.surface.hide();
            return Ok(());
        };
        if self.surface.bounds.get() == Some(bounds) {
            return Ok(());
        }

        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        let dest = RECT {
            right: width,
            bottom: height,
            ..Default::default()
        };
        let placed = unsafe {
            SetWindowPos(
                self.surface.hwnd,
                None,
                bounds.left,
                bounds.top,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .and_then(|()| self.display.UpdateVideoStream(None, Some(&dest), None))
        };
        if let Err(error) = placed {
            self.surface.hide();
            return Err(system_error(error));
        }
        let _ = unsafe { ShowWindow(self.surface.hwnd, SW_SHOWNOACTIVATE) };
        self.surface.bounds.set(Some(bounds));
        Ok(())
    }

    pub fn toggle(&self) -> Result<(), String> {
        let engine = &self.engine.0;
        unsafe {
            if engine.IsEnded().as_bool() {
                engine.SetCurrentTime(0.0).and_then(|()| engine.Play())
            } else if engine.IsPaused().as_bool() {
                engine.Play()
            } else {
                engine.Pause()
            }
        }
        .map_err(system_error)
    }

    pub fn stop(&self) -> Result<(), String> {
        let engine = &self.engine.0;
        if !unsafe { engine.IsPaused() }.as_bool() {
            unsafe { engine.Pause() }.map_err(system_error)?;
        }
        self.seek(0.0)
    }

    pub fn seek(&self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() {
            return Err(system_error(windows::core::Error::from_hresult(E_INVALIDARG)));
        }
        let duration = unsafe { self.engine.0.GetDuration() };
        let seconds = seconds.max(0.0);
        let seconds = if duration.is_finite() && duration >= 0.0 {
            seconds.min(duration)
        } else {
            seconds
        };
        unsafe { self.engine.0.SetCurrentTime(seconds) }.map_err(system_error)
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.surface.hide();
        let _ = unsafe { RemoveWindowSubclass(self.surface.hwnd, Some(video_subclass), 1) };
    }
}

fn client_pixels(rect: egui::Rect, scale: f32, client: RECT) -> Option<RECT> {
    if !rect.is_finite() || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    // Round inwards to keep the native window inside the egui layout at fractional DPI.
    let pixels = RECT {
        left: ((rect.left() * scale).ceil() as i32).clamp(client.left, client.right),
        top: ((rect.top() * scale).ceil() as i32).clamp(client.top, client.bottom),
        right: ((rect.right() * scale).floor() as i32).clamp(client.left, client.right),
        bottom: ((rect.bottom() * scale).floor() as i32).clamp(client.top, client.bottom),
    };
    (pixels.right > pixels.left && pixels.bottom > pixels.top).then_some(pixels)
}

fn system_error(error: windows::core::Error) -> String {
    // The source URL carries a bearer token. Keep operating-system error details to the code.
    format!("Windows 0x{:08X}", error.code().0 as u32)
}

struct VideoSurface {
    parent: HWND,
    hwnd: HWND,
    bounds: Cell<Option<RECT>>,
}

impl VideoSurface {
    fn create(parent: HWND) -> windows::core::Result<Self> {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                // The disabled surface leaves mouse and keyboard input with the editor.
                WS_CHILD | WS_CLIPSIBLINGS | WS_DISABLED,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                None,
                None,
            )
        }?;
        Ok(Self {
            parent,
            hwnd,
            bounds: Cell::new(None),
        })
    }

    fn install_painter(&self, display: &IMFMediaEngineEx) -> windows::core::Result<()> {
        // The player removes this subclass before releasing the interface.
        unsafe { SetWindowSubclass(self.hwnd, Some(video_subclass), 1, display.as_raw() as usize) }
            .ok()
    }

    fn hide(&self) {
        if self.bounds.take().is_some() {
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }
}

unsafe extern "system" fn video_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        match message {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut paint);
                let raw = data as *mut c_void;
                if let Some(display) = IMFMediaEngineEx::from_raw_borrowed(&raw) {
                    let _ = display.UpdateVideoStream(None, None, None);
                }
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(video_subclass), subclass);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

impl Drop for VideoSurface {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

struct PlaybackEngine(IMFMediaEngine);

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        let _ = unsafe { self.0.Shutdown() };
    }
}

#[windows::core::implement(IMFMediaEngineNotify)]
struct PlayerEvents {
    ctx: egui::Context,
    error: Arc<AtomicU64>,
}

impl IMFMediaEngineNotify_Impl for PlayerEvents_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> windows::core::Result<()> {
        match MF_MEDIA_ENGINE_EVENT(event as i32) {
            MF_MEDIA_ENGINE_EVENT_ERROR => {
                // The callback can run on a Media Foundation worker. It only publishes status.
                let failure = (u64::from((param1 as u32).max(1)) << 32) | u64::from(param2);
                self.error.store(failure, Ordering::Relaxed);
            }
            MF_MEDIA_ENGINE_EVENT_STREAMRENDERINGERROR => {
                let _ = self.error.compare_exchange(
                    0,
                    u64::from(event) << 32,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
            MF_MEDIA_ENGINE_EVENT_CANPLAY
            | MF_MEDIA_ENGINE_EVENT_DURATIONCHANGE
            | MF_MEDIA_ENGINE_EVENT_ENDED
            | MF_MEDIA_ENGINE_EVENT_FORMATCHANGE
            | MF_MEDIA_ENGINE_EVENT_LOADEDDATA
            | MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA
            | MF_MEDIA_ENGINE_EVENT_PAUSE
            | MF_MEDIA_ENGINE_EVENT_PLAYING
            | MF_MEDIA_ENGINE_EVENT_SEEKED => {}
            _ => return Ok(()),
        }
        self.ctx.request_repaint();
        Ok(())
    }
}

struct ComApartment(bool);

impl ComApartment {
    fn enter() -> windows::core::Result<Self> {
        let status = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if status == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        status.ok()?;
        Ok(Self(true))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct MediaLibrary(HMODULE);

impl Drop for MediaLibrary {
    fn drop(&mut self) {
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

struct MediaFoundation {
    shutdown: unsafe extern "system" fn() -> HRESULT,
    create_attributes: unsafe extern "system" fn(*mut Option<IMFAttributes>, u32) -> HRESULT,
    _library: MediaLibrary,
}

impl MediaFoundation {
    fn start() -> windows::core::Result<Self> {
        // Delay loading Media Foundation so Windows N can still open the app without the codec pack.
        let library = MediaLibrary(unsafe {
            LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)?
        });
        let startup: unsafe extern "system" fn(u32, u32) -> HRESULT;
        let shutdown: unsafe extern "system" fn() -> HRESULT;
        let create_attributes: unsafe extern "system" fn(*mut Option<IMFAttributes>, u32) -> HRESULT;
        unsafe {
            startup = std::mem::transmute(
                GetProcAddress(library.0, s!("MFStartup"))
                    .ok_or_else(windows::core::Error::from_thread)?,
            );
            shutdown = std::mem::transmute(
                GetProcAddress(library.0, s!("MFShutdown"))
                    .ok_or_else(windows::core::Error::from_thread)?,
            );
            create_attributes = std::mem::transmute(
                GetProcAddress(library.0, s!("MFCreateAttributes"))
                    .ok_or_else(windows::core::Error::from_thread)?,
            );
            startup(MF_VERSION, MFSTARTUP_FULL).ok()?;
        }
        Ok(Self {
            shutdown,
            create_attributes,
            _library: library,
        })
    }

    fn create_engine(
        &self,
        notify: &IMFMediaEngineNotify,
        hwnd: HWND,
    ) -> windows::core::Result<PlaybackEngine> {
        unsafe {
            let mut attributes = None;
            (self.create_attributes)(&mut attributes, 3).ok()?;
            let attributes = attributes.ok_or(windows::core::Error::from_hresult(E_POINTER))?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, notify)?;
            attributes.SetUINT64(&MF_MEDIA_ENGINE_PLAYBACK_HWND, hwnd.0 as usize as u64)?;
            attributes.SetUnknown(&MF_MEDIA_ENGINE_SOURCE_RESOLVER_CONFIG_STORE, &no_cache_store()?)?;
            let factory: IMFMediaEngineClassFactory = CoCreateInstance(
                &CLSID_MFMediaEngineClassFactory,
                None,
                CLSCTX_INPROC_SERVER,
            )?;
            factory.CreateInstance(0, &attributes).map(PlaybackEngine)
        }
    }
}

fn no_cache_store() -> windows::core::Result<IPropertyStore> {
    unsafe {
        let mut raw = std::ptr::null_mut();
        PSCreateMemoryPropertyStore(&IPropertyStore::IID, &mut raw)?;
        if raw.is_null() {
            return Err(windows::core::Error::from_hresult(E_POINTER));
        }
        let store = IPropertyStore::from_raw(raw);
        let key = PROPERTYKEY {
            fmtid: MFNETSOURCE_CACHEENABLED,
            pid: 0,
        };
        // The documented Boolean representation is a VT_I4 LONG; the zero value disables caching.
        let mut value = PROPVARIANT::default();
        (*value.Anonymous.Anonymous).vt = VT_I4;
        store.SetValue(&key, &value)?;
        store.Commit()?;
        Ok(store)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        let _ = unsafe { (self.shutdown)() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_bounds_stay_inside_egui_and_parent_at_fractional_dpi() {
        let client = RECT {
            right: 801,
            bottom: 602,
            ..Default::default()
        };
        let surface = egui::Rect::from_min_max(egui::pos2(14.2, 28.1), egui::pos2(710.3, 100.7));
        assert_eq!(
            client_pixels(surface, 1.25, client),
            Some(RECT {
                left: 18,
                top: 36,
                right: 801,
                bottom: 125,
            })
        );
        let outside = surface.translate(egui::vec2(0.0, 700.0));
        assert_eq!(client_pixels(outside, 1.25, client), None);
        assert_eq!(client_pixels(surface, f32::NAN, client), None);
    }
}
