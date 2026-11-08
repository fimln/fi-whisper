//! Integrasi Win32: single instance, foreground target, Unicode SendInput, clipboard, tray, hotkey.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HANDLE, HWND, LPARAM, LRESULT,
    RECT, WAIT_OBJECT_0, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcessId, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetWindowLongPtrW, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IsZoomed, SWP_FRAMECHANGED, SWP_NOMOVE, SetWindowLongPtrW,
    WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WS_CAPTION, WS_THICKFRAME,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsWindow,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

pub fn configure_window(hwnd: HWND) {
    // Keep native sizing and DWM rounding while drawing the entire caption ourselves.
    unsafe {
        if SetWindowSubclass(hwnd, Some(window_frame), 1, 0) != 0 {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                style | (WS_THICKFRAME | WS_CAPTION) as isize,
            );
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let round = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&round as *const i32).cast(),
                std::mem::size_of_val(&round) as u32,
            );
        }
    }
    center_window(hwnd);
}

unsafe extern "system" fn window_frame(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        if msg == WM_NCCALCSIZE && w != 0 {
            return 0;
        }
        if msg == WM_NCHITTEST && IsZoomed(hwnd) == 0 {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect) != 0 {
                let x = (l as u16 as i16) as i32;
                let y = ((l >> 16) as u16 as i16) as i32;
                let edge = (8 * GetDpiForWindow(hwnd) / 96).max(1) as i32;
                if let Some(hit) = resize_edge(x, y, rect, edge) {
                    return hit as LRESULT;
                }
            }
        }
        if msg == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(window_frame), id);
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

fn resize_edge(x: i32, y: i32, r: RECT, edge: i32) -> Option<u32> {
    if x < r.left || x >= r.right || y < r.top || y >= r.bottom {
        return None;
    }
    match (
        x < r.left + edge,
        x >= r.right - edge,
        y < r.top + edge,
        y >= r.bottom - edge,
    ) {
        (true, _, true, _) => Some(HTTOPLEFT),
        (_, true, true, _) => Some(HTTOPRIGHT),
        (true, _, _, true) => Some(HTBOTTOMLEFT),
        (_, true, _, true) => Some(HTBOTTOMRIGHT),
        (true, _, _, _) => Some(HTLEFT),
        (_, true, _, _) => Some(HTRIGHT),
        (_, _, true, _) => Some(HTTOP),
        (_, _, _, true) => Some(HTBOTTOM),
        _ => None,
    }
}

pub fn center_window(hwnd: HWND) {
    let mut rect = RECT::default();
    let mut monitor = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // Use physical pixels and the work area so DPI and the taskbar are accounted for.
    unsafe {
        let primary = MonitorFromWindow(std::ptr::null_mut(), MONITOR_DEFAULTTOPRIMARY);
        if GetWindowRect(hwnd, &mut rect) == 0 || GetMonitorInfoW(primary, &mut monitor) == 0 {
            return;
        }
        let work = monitor.rcWork;
        let x = work.left + ((work.right - work.left - (rect.right - rect.left)) / 2).max(0);
        let y = work.top + ((work.bottom - work.top - (rect.bottom - rect.top)) / 2).max(0);
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

const CF_UNICODETEXT: u32 = 13;
const BATCH_CHARS: usize = 32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// --- Single instance ---

const ACTIVATE_EVENT: &str = "Local\\fi-whisper.activate";

pub struct InstanceGuard(HANDLE, HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
        unsafe { CloseHandle(self.1) };
    }
}

/// Mutex bernama di namespace sesi pengguna. `None` jika instance lain sudah berjalan.
pub fn single_instance() -> Option<InstanceGuard> {
    let event_name = wide(ACTIVATE_EVENT);
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr()) };
    let name = wide("Local\\fi-whisper.single-instance");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        // Gagal membuat mutex bukan bukti ada instance lain; jalankan tanpa guard.
        return Some(InstanceGuard(handle, event));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { SetEvent(event) };
        unsafe { CloseHandle(event) };
        unsafe { CloseHandle(handle) };
        return None;
    }
    Some(InstanceGuard(handle, event))
}

pub struct Activation {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Activation {
    pub fn new(on_open: impl Fn() + Send + 'static) -> Option<Self> {
        let name = wide(ACTIVATE_EVENT);
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if event.is_null() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let event_id = event as isize;
        let thread = std::thread::Builder::new()
            .name("fi-whisper-activate".into())
            .spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    if unsafe { WaitForSingleObject(event_id as HANDLE, 100) } == WAIT_OBJECT_0
                        && !stopping.load(Ordering::SeqCst)
                    {
                        on_open();
                    }
                }
                unsafe { CloseHandle(event_id as HANDLE) };
            });
        match thread {
            Ok(thread) => Some(Self {
                stop,
                thread: Some(thread),
            }),
            Err(_) => {
                unsafe { CloseHandle(event) };
                None
            }
        }
    }
}

impl Drop for Activation {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// --- Target foreground ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub hwnd: isize,
    pub pid: u32,
}

fn window_target(hwnd: HWND) -> Option<Target> {
    if hwnd.is_null() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    Some(Target {
        hwnd: hwnd as isize,
        pid,
    })
}

fn is_shell(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    matches!(
        class.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

/// Jendela aplikasi lain yang sedang fokus. Desktop, taskbar, dan fi-whisper sendiri ditolak.
pub fn foreground_target() -> Option<Target> {
    let hwnd = unsafe { GetForegroundWindow() };
    let target = window_target(hwnd)?;
    (target.pid != unsafe { GetCurrentProcessId() } && !is_shell(hwnd)).then_some(target)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    NoTarget,
    TargetChanged,
}

/// Keputusan murni: boleh mengetik hanya ke target awal yang masih fokus.
pub fn may_insert(start: Option<Target>, now: Option<Target>) -> Result<Target, Blocked> {
    let start = start.ok_or(Blocked::NoTarget)?;
    if now == Some(start) {
        Ok(start)
    } else {
        Err(Blocked::TargetChanged)
    }
}

fn current_target_valid(target: Target) -> Option<Target> {
    let hwnd = target.hwnd as HWND;
    if unsafe { IsWindow(hwnd) } == 0 {
        return None;
    }
    window_target(unsafe { GetForegroundWindow() })
}

fn modifiers_down() -> bool {
    [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inserted {
    Done,
    /// Tidak ada karakter yang dikirim.
    Blocked(String),
    /// Sebagian karakter terkirim lalu dihentikan; jangan diulang.
    Partial {
        sent: usize,
        total: usize,
    },
}

/// Ketik `text` ke target dengan KEYEVENTF_UNICODE. Foreground diperiksa sebelum setiap batch.
pub fn insert_text(target: Target, text: &str, cancelled: &AtomicBool) -> Inserted {
    let deadline = Instant::now() + Duration::from_millis(1500);
    while modifiers_down() {
        if cancelled.load(Ordering::SeqCst) {
            return Inserted::Blocked("Aplikasi ditutup.".into());
        }
        if Instant::now() > deadline {
            return Inserted::Blocked("Tombol modifier masih ditekan.".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    insert_batches(
        target,
        text,
        cancelled,
        current_target_valid,
        |inputs| unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            ) as usize
        },
    )
}

fn insert_batches(
    target: Target,
    text: &str,
    cancelled: &AtomicBool,
    mut foreground: impl FnMut(Target) -> Option<Target>,
    mut send: impl FnMut(&[INPUT]) -> usize,
) -> Inserted {
    // Teks satu paragraf; baris baru tidak dikirim sebagai Enter.
    let chars: Vec<char> = text
        .chars()
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect();
    let total = chars.len();
    let mut sent = 0;
    for batch in chars.chunks(BATCH_CHARS) {
        if cancelled.load(Ordering::SeqCst) {
            return if sent == 0 {
                Inserted::Blocked("Aplikasi ditutup.".into())
            } else {
                Inserted::Partial { sent, total }
            };
        }
        if let Err(reason) = may_insert(Some(target), foreground(target)) {
            return if sent == 0 {
                Inserted::Blocked(match reason {
                    Blocked::NoTarget => "Target tidak tersedia.".into(),
                    Blocked::TargetChanged => "Jendela tujuan berubah atau tertutup.".into(),
                })
            } else {
                Inserted::Partial { sent, total }
            };
        }
        let inputs = unicode_inputs(batch);
        let accepted = send(&inputs);
        if accepted != inputs.len() {
            // Event yang sudah diterima tidak bisa ditarik kembali.
            return if sent == 0 && accepted == 0 {
                Inserted::Blocked(
                    "Windows menolak input (mungkin aplikasi tujuan berjalan sebagai administrator)."
                        .into(),
                )
            } else {
                Inserted::Partial { sent, total }
            };
        }
        sent += batch.len();
        if sent < total {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Inserted::Done
}

/// Keydown/keyup per unit UTF-16; surrogate pair tetap berurutan dalam satu batch.
pub fn unicode_inputs(chars: &[char]) -> Vec<INPUT> {
    let mut buf = [0u16; 2];
    let mut inputs = Vec::with_capacity(chars.len() * 4);
    for c in chars {
        for &unit in c.encode_utf16(&mut buf).iter() {
            for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
                inputs.push(INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: unit,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                });
            }
        }
    }
    inputs
}

/// Pelepasan satu kali saat masuk tray dalam keadaan idle.
pub fn trim_idle_memory() {
    unsafe {
        windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
        );
    }
}

// --- Clipboard eksplisit ---

/// Salin teks dan tandai agar tidak masuk Clipboard History/cloud sync serta diabaikan
/// pemantau clipboard yang menghormati format pengecualian.
pub fn copy_text(owner: isize, text: &str) -> Result<(), String> {
    unsafe {
        if OpenClipboard(owner as HWND) == 0 {
            return Err("Clipboard sedang dipakai aplikasi lain.".into());
        }
        let result = (|| {
            if EmptyClipboard() == 0 {
                return Err("Clipboard tidak dapat dikosongkan.".to_owned());
            }
            set_data(CF_UNICODETEXT, &wide(text))?;
            for name in ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"] {
                let format = RegisterClipboardFormatW(wide(name).as_ptr());
                set_data(format, &[0u16, 0u16])?; // DWORD 0
            }
            let format = RegisterClipboardFormatW(
                wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr(),
            );
            set_data(format, &[0u16, 0u16])
        })();
        CloseClipboard();
        result
    }
}

unsafe fn set_data(format: u32, data: &[u16]) -> Result<(), String> {
    let bytes = std::mem::size_of_val(data);
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem.is_null() {
            return Err("Memori clipboard tidak cukup.".into());
        }
        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            GlobalFree(mem);
            return Err("Memori clipboard tidak dapat dikunci.".into());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        GlobalUnlock(mem);
        if SetClipboardData(format, mem).is_null() {
            GlobalFree(mem);
            return Err("Teks tidak dapat disalin.".into());
        }
    }
    Ok(())
}

// --- Hotkey global ---

type HotkeyCallback = dyn Fn() + Send + Sync;
// Setter dependency memakai OnceCell; handler tetap hanya meneruskan ke pemilik yang masih hidup.
static HOTKEY_CALLBACK: Mutex<Option<Weak<HotkeyCallback>>> = Mutex::new(None);

fn hotkey_event(e: GlobalHotKeyEvent) {
    let callback = HOTKEY_CALLBACK
        .lock()
        .unwrap()
        .as_ref()
        .and_then(Weak::upgrade);
    if e.state() == HotKeyState::Pressed
        && let Some(callback) = callback
    {
        callback();
    }
}

/// Harus dibuat di thread utama (message loop winit).
pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    pub active: Option<(HotKey, String)>,
    _callback: Arc<HotkeyCallback>,
}

impl Hotkeys {
    pub fn new(on_press: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| format!("Shortcut global tidak tersedia: {e}"))?;
        let callback: Arc<HotkeyCallback> = Arc::new(on_press);
        *HOTKEY_CALLBACK.lock().unwrap() = Some(Arc::downgrade(&callback));
        GlobalHotKeyEvent::set_event_handler(Some(hotkey_event));
        Ok(Self {
            manager,
            active: None,
            _callback: callback,
        })
    }

    /// Daftarkan shortcut baru; shortcut lama dilepas hanya jika yang baru berhasil.
    pub fn set(&mut self, text: &str) -> Result<(), String> {
        let hotkey: HotKey = text
            .trim()
            .parse()
            .map_err(|e| format!("Format shortcut tidak dikenali: {e}"))?;
        if self.active.as_ref().is_some_and(|(h, _)| *h == hotkey) {
            return Ok(());
        }
        self.manager.register(hotkey).map_err(|_| {
            format!(
                "Shortcut {} sudah dipakai aplikasi lain atau ditolak Windows.",
                text.trim()
            )
        })?;
        if let Some((old, _)) = self.active.take() {
            let _ = self.manager.unregister(old);
        }
        self.active = Some((hotkey, text.trim().to_owned()));
        Ok(())
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        if let Some((hotkey, _)) = self.active.take() {
            let _ = self.manager.unregister(hotkey);
        }
    }
}

// --- Tray ---

pub enum TrayAction {
    Open,
    Cancel,
    Quit,
}

pub struct Tray {
    items: [MenuItem; 3],
    _icon: TrayIcon,
    _callback: Arc<TrayCallback>,
}

struct TrayCallback {
    ids: [MenuId; 3],
    action: Box<dyn Fn(TrayAction) + Send + Sync>,
}

static TRAY_CALLBACK: Mutex<Option<Weak<TrayCallback>>> = Mutex::new(None);

fn menu_event(e: MenuEvent) {
    let callback = TRAY_CALLBACK
        .lock()
        .unwrap()
        .as_ref()
        .and_then(Weak::upgrade);
    if let Some(callback) = callback {
        if e.id == callback.ids[0] {
            (callback.action)(TrayAction::Open)
        } else if e.id == callback.ids[1] {
            (callback.action)(TrayAction::Cancel)
        } else if e.id == callback.ids[2] {
            (callback.action)(TrayAction::Quit)
        }
    }
}

fn tray_event(e: TrayIconEvent) {
    let callback = TRAY_CALLBACK
        .lock()
        .unwrap()
        .as_ref()
        .and_then(Weak::upgrade);
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = e
        && let Some(callback) = callback
    {
        (callback.action)(TrayAction::Open)
    }
}

/// Buat ikon tray. Handler dipanggil dari message loop thread utama.
pub fn create_tray(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Tray, String> {
    let open = MenuItem::new("Buka fi-whisper", true, None);
    let cancel = MenuItem::new("Batalkan rekaman/proses", true, None);
    let quit = MenuItem::new("Keluar", true, None);
    let ids: [MenuId; 3] = [open.id().clone(), cancel.id().clone(), quit.id().clone()];
    let menu = Menu::with_items(&[&open, &cancel, &PredefinedMenuItem::separator(), &quit])
        .map_err(|e| e.to_string())?;
    let icon = TrayIconBuilder::new()
        .with_tooltip("fi-whisper")
        .with_icon(tray_icon_image())
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| format!("Ikon tray gagal dibuat: {e}"))?;
    let callback = Arc::new(TrayCallback {
        ids,
        action: Box::new(on_action),
    });
    *TRAY_CALLBACK.lock().unwrap() = Some(Arc::downgrade(&callback));
    MenuEvent::set_event_handler(Some(menu_event));
    TrayIconEvent::set_event_handler(Some(tray_event));
    Ok(Tray {
        items: [open, cancel, quit],
        _icon: icon,
        _callback: callback,
    })
}

/// Ikon tray dari logo aplikasi (Windows menskalakan ke ukuran tray).
fn tray_icon_image() -> tray_icon::Icon {
    let n = crate::theme::LOGO_SIZE as u32;
    tray_icon::Icon::from_rgba(crate::theme::LOGO_RGBA.to_vec(), n, n).expect("ikon tray valid")
}

/// HKCU Run tidak membutuhkan hak administrator dan berjalan saat login pengguna.
pub fn set_auto_startup(enabled: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::*;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let command = wide(&format!("\"{}\" --tray", exe.display()));
    let mut key = std::ptr::null_mut();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if result != 0 {
        return Err(format!("Auto-startup gagal: {result}"));
    }
    let result = unsafe {
        let result = if enabled {
            RegSetValueExW(
                key,
                wide("fi-whisper").as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(key, wide("fi-whisper").as_ptr())
        };
        RegCloseKey(key);
        result
    };
    if result == 0 || (!enabled && result == 2) {
        Ok(())
    } else {
        Err(format!("Auto-startup gagal: {result}"))
    }
}

impl Tray {
    pub fn set_english(&self, english: bool) {
        let labels = if english {
            ["Open fi-whisper", "Cancel recording/processing", "Quit"]
        } else {
            ["Buka fi-whisper", "Batalkan rekaman/proses", "Keluar"]
        };
        for (item, label) in self.items.iter().zip(labels) {
            item.set_text(label);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn resize_edges_include_all_corners_and_negative_screen_coordinates() {
        use super::*;
        let rect = RECT {
            left: -800,
            top: -600,
            right: -100,
            bottom: 50,
        };
        for (x, y, hit) in [
            (-800, -600, HTTOPLEFT),
            (-101, -600, HTTOPRIGHT),
            (-800, 49, HTBOTTOMLEFT),
            (-101, 49, HTBOTTOMRIGHT),
            (-800, -300, HTLEFT),
            (-101, -300, HTRIGHT),
            (-400, -600, HTTOP),
            (-400, 49, HTBOTTOM),
        ] {
            assert_eq!(resize_edge(x, y, rect, 12), Some(hit));
        }
        assert_eq!(resize_edge(-400, -300, rect, 12), None);
        assert_eq!(resize_edge(-801, -300, rect, 12), None);
    }
    use super::*;

    #[test]
    fn callbacks_release_targets_and_hotkeys_can_be_recreated() {
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        for expected in 1..=2 {
            let target = calls.clone();
            let hotkeys = Hotkeys::new(move || {
                target.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
            hotkey_event(GlobalHotKeyEvent {
                id: 0,
                state: HotKeyState::Pressed,
            });
            assert_eq!(calls.load(Ordering::SeqCst), expected);
            drop(hotkeys);
            assert_eq!(Arc::strong_count(&calls), 1);
        }
        let target = calls.clone();
        let callback = Arc::new(TrayCallback {
            ids: [
                MenuId::new("open"),
                MenuId::new("cancel"),
                MenuId::new("quit"),
            ],
            action: Box::new(move |_| {
                target.fetch_add(1, Ordering::SeqCst);
            }),
        });
        *TRAY_CALLBACK.lock().unwrap() = Some(Arc::downgrade(&callback));
        menu_event(MenuEvent {
            id: MenuId::new("open"),
        });
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        drop(callback);
        menu_event(MenuEvent {
            id: MenuId::new("open"),
        });
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(Arc::strong_count(&calls), 1);
    }

    #[test]
    fn shutdown_blocks_insertion_before_sending_input() {
        let result = insert_text(Target { hwnd: 0, pid: 0 }, "uji", &AtomicBool::new(true));
        assert_eq!(result, Inserted::Blocked("Aplikasi ditutup.".into()));
    }

    #[test]
    fn changed_window_stops_between_batches() {
        let target = Target { hwnd: 1, pid: 2 };
        let mut checks = 0;
        let mut sends = 0;
        let result = insert_batches(
            target,
            &"x".repeat(BATCH_CHARS + 1),
            &AtomicBool::new(false),
            |_| {
                checks += 1;
                (checks == 1).then_some(target)
            },
            |inputs| {
                sends += 1;
                inputs.len()
            },
        );
        assert_eq!(sends, 1);
        assert_eq!(
            result,
            Inserted::Partial {
                sent: BATCH_CHARS,
                total: BATCH_CHARS + 1
            }
        );
    }

    #[test]
    fn partial_send_is_never_retried_and_shutdown_stops_next_batch() {
        let target = Target { hwnd: 1, pid: 2 };
        let cancelled = AtomicBool::new(false);
        let mut sends = 0;
        let result = insert_batches(
            target,
            "abc",
            &cancelled,
            |_| Some(target),
            |_| {
                sends += 1;
                1
            },
        );
        assert_eq!(sends, 1);
        assert_eq!(result, Inserted::Partial { sent: 0, total: 3 });
        let result = insert_batches(
            target,
            &"x".repeat(BATCH_CHARS + 1),
            &cancelled,
            |_| Some(target),
            |inputs| {
                cancelled.store(true, Ordering::SeqCst);
                inputs.len()
            },
        );
        assert_eq!(
            result,
            Inserted::Partial {
                sent: BATCH_CHARS,
                total: BATCH_CHARS + 1
            }
        );
    }

    #[test]
    fn insertion_requires_same_target() {
        let a = Target { hwnd: 1, pid: 10 };
        let b = Target { hwnd: 2, pid: 10 };
        assert_eq!(may_insert(Some(a), Some(a)), Ok(a));
        assert_eq!(may_insert(Some(a), Some(b)), Err(Blocked::TargetChanged));
        assert_eq!(may_insert(Some(a), None), Err(Blocked::TargetChanged));
        assert_eq!(may_insert(None, Some(a)), Err(Blocked::NoTarget));
    }

    #[test]
    fn unicode_inputs_keep_surrogate_pairs() {
        let inputs = unicode_inputs(&['é', '😀']);
        // é = 1 unit, 😀 = 2 unit; masing-masing keydown + keyup.
        assert_eq!(inputs.len(), 6);
        let units: Vec<u16> = inputs
            .iter()
            .map(|i| unsafe { i.Anonymous.ki.wScan })
            .collect();
        assert_eq!(units, [0xE9, 0xE9, 0xD83D, 0xD83D, 0xDE00, 0xDE00]);
        assert!(inputs.iter().all(|i| unsafe { i.Anonymous.ki.wVk } == 0));
        let flags: Vec<u32> = inputs
            .iter()
            .map(|i| unsafe { i.Anonymous.ki.dwFlags })
            .collect();
        assert_eq!(flags[0], KEYEVENTF_UNICODE);
        assert_eq!(flags[1], KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
    }
}
