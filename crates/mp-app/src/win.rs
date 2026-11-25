//! Windows 专属调用：磨砂玻璃材质、系统语言、本地时间、错误提示框。
//! 全部手写系统接口声明，不额外引依赖。

use crate::settings::Backdrop;
use std::ffi::c_void;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

type HWND = *mut c_void;

#[link(name = "user32")]
extern "system" {
    fn FindWindowExW(parent: HWND, after: HWND, class: *const u16, title: *const u16) -> HWND;
    fn GetWindowThreadProcessId(hwnd: HWND, process_id: *mut u32) -> u32;
    fn SetWindowCompositionAttribute(hwnd: HWND, data: *mut CompositionData) -> i32;
    fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, kind: u32) -> i32;
    fn OpenClipboard(hwnd: HWND) -> i32;
    fn CloseClipboard() -> i32;
    fn GetClipboardData(format: u32) -> *mut c_void;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
    fn GetClientRect(hwnd: HWND, rect: *mut ClientRect) -> i32;
    fn ScreenToClient(hwnd: HWND, point: *mut ClientPoint) -> i32;
    fn IsZoomed(hwnd: HWND) -> i32;
}

type SubclassProc = unsafe extern "system" fn(HWND, u32, usize, isize, usize, usize) -> isize;

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(hwnd: HWND, callback: SubclassProc, id: usize, data: usize) -> i32;
    fn RemoveWindowSubclass(hwnd: HWND, callback: SubclassProc, id: usize) -> i32;
    fn DefSubclassProc(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> isize;
}

#[derive(Default)]
#[repr(C)]
struct ClientRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct ClientPoint {
    x: i32,
    y: i32,
}

#[link(name = "shell32")]
extern "system" {
    fn DragQueryFileW(drop: *mut c_void, index: u32, buffer: *mut u16, max_chars: u32) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetUserDefaultUILanguage() -> u16;
    fn GetCurrentProcessId() -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        creation: u32,
        flags: u32,
        template: HWND,
    ) -> HWND;
    fn FileTimeToSystemTime(file_time: *const i64, system_time: *mut SystemTime) -> i32;
    fn SystemTimeToTzSpecificLocalTime(
        time_zone: *const c_void,
        utc: *const SystemTime,
        local: *mut SystemTime,
    ) -> i32;
    fn CloseHandle(handle: HWND) -> i32;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: HWND, attribute: u32, value: *const c_void, size: u32) -> i32;
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    color: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

#[repr(C)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// 系统界面语言是不是中文。
pub fn system_is_chinese() -> bool {
    // 主语言标识 0x04 是中文。
    unsafe { (GetUserDefaultUILanguage() & 0x3FF) == 0x04 }
}

/// 弹一个系统错误提示框。用在窗口都起不来的时候。
pub fn show_error_box(text: &str, caption: &str) {
    const MB_ICONERROR: u32 = 0x0000_0010;
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(text).as_ptr(),
            wide(caption).as_ptr(),
            MB_ICONERROR,
        );
    }
}

/// 把 Unix 毫秒时间戳换算成本地的 (日期, 时间)。
pub fn local_date_time(unix_millis: u64) -> (String, String) {
    // FILETIME starts at 1601; reject timestamps outside its signed range.
    let Some(file_time) = unix_millis.checked_add(11_644_473_600_000)
        .and_then(|value| value.checked_mul(10_000))
        .and_then(|value| i64::try_from(value).ok()) else {
        return (String::from("????-??-??"), String::from("??:??"));
    };
    let mut utc: SystemTime = unsafe { std::mem::zeroed() };
    let mut local: SystemTime = unsafe { std::mem::zeroed() };
    unsafe {
        if FileTimeToSystemTime(&file_time, &mut utc) == 0 {
            return (String::from("????-??-??"), String::from("??:??"));
        }
        if SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            local = utc;
        }
    }
    (
        format!("{:04}-{:02}-{:02}", local.year, local.month, local.day),
        format!("{:02}:{:02}", local.hour, local.minute),
    )
}

/// 剪贴板里有没有从资源管理器复制的文件；有就返回这些文件的路径。
///
/// 只在用户于本软件里按粘贴时调用一次，读的是文件路径列表，不读剪贴板里的其他内容。
pub fn clipboard_files() -> Vec<std::path::PathBuf> {
    const CF_HDROP: u32 = 15;
    let mut paths = Vec::new();
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP) == 0 || OpenClipboard(std::ptr::null_mut()) == 0 {
            return paths;
        }
        let drop = GetClipboardData(CF_HDROP);
        if !drop.is_null() {
            let count = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
            for index in 0..count {
                let length = DragQueryFileW(drop, index, std::ptr::null_mut(), 0) as usize;
                let mut buffer = vec![0u16; length + 1];
                let copied = DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32) as usize;
                if copied > 0 && copied <= length {
                    paths.push(std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..copied])));
                }
            }
        }
        CloseClipboard();
    }
    paths
}

/// 系统剪贴板每变一次，这个数就加一。用它判断「复制完一块之后，剪贴板有没有被别处复制的内容换掉」。
pub fn clipboard_sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// 独占着一个锁文件的句柄。进程退出时系统自动释放。
pub struct InstanceLock(HWND);

/// 独占数据目录里的锁文件，防止两份程序同时写同一个库。已经被别人占着时返回 None。
///
/// 文件以「不和别人共享」的方式打开并一直持有到进程退出，所以哪怕程序崩溃，锁也会随进程一起放开。
pub fn lock_instance(path: &std::path::Path) -> Option<InstanceLock> {
    // GENERIC_READ，不共享，不存在就创建，普通文件属性。
    const GENERIC_READ: u32 = 0x8000_0000;
    const OPEN_ALWAYS: u32 = 4;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(name.as_ptr(), GENERIC_READ, 0, std::ptr::null(), OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut())
    };
    // 失败时返回的是 -1。
    (handle as isize != -1).then_some(InstanceLock(handle))
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// 给窗口套磨砂玻璃材质。窗口由 eframe 创建、句柄不对外暴露，所以按标题找到属于本进程的那个。
#[derive(Default)]
pub struct BackdropApplier {
    hwnd: Option<HWND>,
    applied: Option<(Backdrop, bool, bool)>,
    chrome: Option<(u16, u16)>,
}

impl BackdropApplier {
    /// 让系统接管无边框窗口顶部拖动与边缘缩放，控件区域仍由 egui 处理。
    pub fn install_chrome(&mut self, title: &str, height: u16, control_width: u16) -> bool {
        if self.chrome == Some((height, control_width)) {
            return true;
        }
        let Some(hwnd) = self.find_window(title) else {
            return false;
        };
        let data = (usize::from(height) << 16) | usize::from(control_width);
        unsafe {
            if SetWindowSubclass(hwnd, chrome_proc, 1, data) == 0 {
                return false;
            }
            const DWMWA_BORDER_COLOR: u32 = 34;
            const DWMWA_COLOR_NONE: u32 = 0xffff_fffe;
            DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&DWMWA_COLOR_NONE as *const u32).cast(), 4);
        }
        self.chrome = Some((height, control_width));
        true
    }

    /// 返回桌面合成器是否接受了材质，失败时由应用铺上主题底色。
    pub fn apply(&mut self, title: &str, backdrop: Backdrop, dark: bool) -> bool {
        if let Some((previous, previous_dark, available)) = self.applied {
            if (previous, previous_dark) == (backdrop, dark) {
                return available;
            }
        }
        let Some(hwnd) = self.find_window(title) else {
            return false;
        };
        const SYSTEM_BACKDROP: u32 = 38;
        const IMMERSIVE_DARK_MODE: u32 = 20;
        const MICA: i32 = 2;
        const SYSTEM_ACRYLIC: i32 = 3;
        const MICA_ALT: i32 = 4;
        accent_policy(hwnd, 0, 0);
        dwm_attribute(hwnd, SYSTEM_BACKDROP, 1);
        dwm_attribute(hwnd, IMMERSIVE_DARK_MODE, i32::from(dark));
        let available = match backdrop {
            Backdrop::Opaque | Backdrop::Transparent => true,
            Backdrop::Acrylic => {
                // 颜色格式为 AABBGGRR。非零透明度让 DWM 保留后面的模糊层。
                let color = if dark { 0x381b_1e1b } else { 0x38f5_f9f8 };
                accent_policy(hwnd, 4, color) || dwm_attribute(hwnd, SYSTEM_BACKDROP, SYSTEM_ACRYLIC)
            }
            Backdrop::Mica => dwm_attribute(hwnd, SYSTEM_BACKDROP, MICA_ALT)
                || dwm_attribute(hwnd, SYSTEM_BACKDROP, MICA),
            Backdrop::SystemAcrylic => dwm_attribute(hwnd, SYSTEM_BACKDROP, SYSTEM_ACRYLIC),
        };
        self.applied = Some((backdrop, dark, available));
        available
    }

    fn find_window(&mut self, title: &str) -> Option<HWND> {
        if let Some(hwnd) = self.hwnd {
            return Some(hwnd);
        }
        let title = wide(title);
        let own_pid = unsafe { GetCurrentProcessId() };
        let mut after = std::ptr::null_mut();
        loop {
            let hwnd = unsafe { FindWindowExW(std::ptr::null_mut(), after, std::ptr::null(), title.as_ptr()) };
            if hwnd.is_null() {
                return None;
            }
            let mut owner = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
            if owner == own_pid {
                self.hwnd = Some(hwnd);
                return Some(hwnd);
            }
            after = hwnd;
        }
    }
}

fn accent_policy(hwnd: HWND, state: u32, color: u32) -> bool {
    let mut policy = AccentPolicy { state, flags: 0, color, animation: 0 };
    let mut data = CompositionData {
        attribute: 19,
        data: (&mut policy as *mut AccentPolicy).cast(),
        size: std::mem::size_of::<AccentPolicy>(),
    };
    (unsafe { SetWindowCompositionAttribute(hwnd, &mut data) }) != 0
}

fn dwm_attribute(hwnd: HWND, attribute: u32, value: i32) -> bool {
    (unsafe { DwmSetWindowAttribute(hwnd, attribute, (&value as *const i32).cast(), 4) }) >= 0
}

unsafe extern "system" fn chrome_proc(hwnd: HWND, message: u32, wparam: usize, lparam: isize, id: usize, data: usize) -> isize {
    const WM_NCHITTEST: u32 = 0x0084;
    const WM_NCDESTROY: u32 = 0x0082;
    const HTCLIENT: isize = 1;
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(hwnd, chrome_proc, id) };
    }
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message != WM_NCHITTEST || result != HTCLIENT {
        return result;
    }
    let mut point = ClientPoint { x: i32::from(lparam as i16), y: i32::from((lparam >> 16) as i16) };
    let mut bounds = ClientRect::default();
    if unsafe { ScreenToClient(hwnd, &mut point) } == 0 || unsafe { GetClientRect(hwnd, &mut bounds) } == 0 {
        return result;
    }
    let height = (data >> 16) as i32;
    let control_width = (data & 0xffff) as i32;
    chrome_hit_test(point, bounds, height, control_width, unsafe { IsZoomed(hwnd) != 0 })
}

fn chrome_hit_test(point: ClientPoint, bounds: ClientRect, height: i32, control_width: i32, maximized: bool) -> isize {
    const HTCLIENT: isize = 1;
    const HTCAPTION: isize = 2;
    let ClientPoint { x, y } = point;
    if x < 0 || y < 0 || x >= bounds.right || y >= bounds.bottom {
        return HTCLIENT;
    }
    if !maximized {
        let edge = (height / 6).max(4);
        let horizontal = if x < edge { -1 } else if x >= bounds.right - edge { 1 } else { 0 };
        let vertical = if y < edge { -1 } else if y >= bounds.bottom - edge { 1 } else { 0 };
        match (horizontal, vertical) {
            (-1, -1) => return 13, // HTTOPLEFT
            (1, -1) => return 14, // HTTOPRIGHT
            (-1, 1) => return 16, // HTBOTTOMLEFT
            (1, 1) => return 17, // HTBOTTOMRIGHT
            (-1, 0) => return 10, // HTLEFT
            (1, 0) => return 11, // HTRIGHT
            (0, -1) => return 12, // HTTOP
            (0, 1) => return 15, // HTBOTTOM
            _ => {}
        }
    }
    if y < height && x < bounds.right - control_width { HTCAPTION } else { HTCLIENT }
}

#[cfg(test)]
mod tests {
    #[test]
    fn frameless_title_area_leaves_window_controls_in_the_client() {
        let hit = |x, y, maximized| {
            let bounds = super::ClientRect { right: 1000, bottom: 740, ..Default::default() };
            super::chrome_hit_test(super::ClientPoint { x, y }, bounds, 32, 110, maximized)
        };
        assert_eq!(hit(500, 17, false), 2); // HTCAPTION
        assert_eq!(hit(973, 17, false), 1); // HTCLIENT
        assert_eq!(hit(500, 100, false), 1);
        assert_eq!(hit(998, 738, false), 17); // HTBOTTOMRIGHT
        assert_eq!(hit(500, 2, true), 2);
        assert_eq!(hit(998, 738, true), 1);
    }

    #[test]
    fn out_of_range_timestamps_do_not_overflow() {
        assert_eq!(super::local_date_time(u64::MAX), ("????-??-??".into(), "??:??".into()));
    }

    #[test]
    #[ignore = "requires Windows desktop composition with system backdrops"]
    fn native_material_switches_on_an_unshown_window() {
        use super::{wide, Backdrop, BackdropApplier, HWND};
        use std::ffi::c_void;
        use std::ptr::null_mut;

        #[link(name = "user32")]
        extern "system" {
            fn CreateWindowExW(
                extended_style: u32, class: *const u16, title: *const u16, style: u32,
                x: i32, y: i32, width: i32, height: i32, parent: HWND, menu: HWND,
                instance: HWND, parameter: *mut c_void,
            ) -> HWND;
            fn DestroyWindow(hwnd: HWND) -> i32;
        }
        #[link(name = "dwmapi")]
        extern "system" {
            fn DwmGetWindowAttribute(hwnd: HWND, attribute: u32, value: *mut c_void, size: u32) -> i32;
        }

        struct HiddenWindow(HWND);
        impl Drop for HiddenWindow {
            fn drop(&mut self) {
                unsafe { DestroyWindow(self.0); }
            }
        }

        let title = "MOSS Pocket native QA material";
        let window = HiddenWindow(unsafe {
            CreateWindowExW(0, wide("STATIC").as_ptr(), wide(title).as_ptr(), 0x00cf0000,
                0, 0, 720, 480, null_mut(), null_mut(), null_mut(), null_mut())
        });
        assert!(!window.0.is_null(), "cannot create a hidden Win32 window");
        let mut material = BackdropApplier::default();
        assert!(material.install_chrome(title, 32, 110));
        for dark in [false, true] {
            for (backdrop, allowed) in [
                (Backdrop::Mica, &[2, 4][..]),
                (Backdrop::SystemAcrylic, &[3][..]),
                (Backdrop::Opaque, &[1][..]),
                (Backdrop::Acrylic, &[1, 3][..]),
                (Backdrop::Transparent, &[1][..]),
            ] {
                assert!(material.apply(title, backdrop, dark), "DWM rejected {backdrop:?}, dark={dark}");
                let mut actual = 0i32;
                let hr = unsafe { DwmGetWindowAttribute(window.0, 38, (&mut actual as *mut i32).cast(), 4) };
                assert_eq!(hr, 0, "DWM attribute read failed for {backdrop:?}");
                assert!(allowed.contains(&actual), "{backdrop:?}: DWM reported {actual}");
            }
        }
    }
}

#[repr(C)]
struct MemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> HWND;
    fn GetProcessTimes(
        process: HWND,
        creation: *mut i64,
        exit: *mut i64,
        kernel: *mut i64,
        user: *mut i64,
    ) -> i32;
    fn GetSystemTimePreciseAsFileTime(now: *mut i64);
    fn K32GetProcessMemoryInfo(process: HWND, counters: *mut MemoryCounters, size: u32) -> i32;
}

/// 进程创建到现在过了多少毫秒。自测用。
pub fn millis_since_process_start() -> f64 {
    unsafe {
        let (mut created, mut exited, mut kernel, mut user) = (0i64, 0i64, 0i64, 0i64);
        GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user);
        let mut now_filetime = 0i64;
        GetSystemTimePreciseAsFileTime(&mut now_filetime);
        // FILETIME 的单位是 100 纳秒。
        (now_filetime - created) as f64 / 10_000.0
    }
}

/// 进程的工作集和私有提交，单位 KB。自测用。
pub fn memory_kb() -> (usize, usize) {
    unsafe {
        let mut counters: MemoryCounters = std::mem::zeroed();
        counters.cb = std::mem::size_of::<MemoryCounters>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) == 0 {
            return (0, 0);
        }
        (counters.working_set_size / 1024, counters.private_usage / 1024)
    }
}
