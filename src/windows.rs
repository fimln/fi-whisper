//! Integrasi Win32: single instance, foreground target, Unicode SendInput, clipboard, tray, hotkey.

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HANDLE, HWND,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
    MB_ICONINFORMATION, MB_OK, MessageBoxW, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow,
    ShowWindow,
};

const CF_UNICODETEXT: u32 = 13;
const BATCH_CHARS: usize = 32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// --- Single instance ---

pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Mutex bernama di namespace sesi pengguna. `None` jika instance lain sudah berjalan.
pub fn single_instance() -> Option<InstanceGuard> {
    let name = wide("Local\\fi-whisper.single-instance");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        // Gagal membuat mutex bukan bukti ada instance lain; jalankan tanpa guard.
        return Some(InstanceGuard(handle));
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        return None;
    }
    Some(InstanceGuard(handle))
}

pub fn message(text: &str) {
    let (text, caption) = (wide(text), wide("fi-whisper"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        )
    };
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
pub fn insert_text(target: Target, text: &str) -> Inserted {
    let deadline = Instant::now() + Duration::from_millis(1500);
    while modifiers_down() {
        if Instant::now() > deadline {
            return Inserted::Blocked("Tombol modifier masih ditekan.".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Teks satu paragraf; baris baru tidak dikirim sebagai Enter.
    let chars: Vec<char> = text
        .chars()
        .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect();
    let total = chars.len();
    let mut sent = 0;
    for batch in chars.chunks(BATCH_CHARS) {
        if let Err(reason) = may_insert(Some(target), current_target_valid(target)) {
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
        let accepted = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            )
        };
        if accepted as usize != inputs.len() {
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

// --- Jendela utama ---

pub fn show_window(hwnd: isize) {
    let hwnd = hwnd as HWND;
    unsafe {
        ShowWindow(
            hwnd,
            if IsIconic(hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        SetForegroundWindow(hwnd);
    }
}

/// Sembunyikan langsung lewat Win32. `ViewportCommand::Visible(false)` tidak dipakai karena winit
/// menyimpan status visible sendiri: setelah jendela ditampilkan lagi oleh `show_window`, perintah
/// hide berikutnya dianggap no-op dan tombol close tampak tidak bekerja.
pub fn hide_window(hwnd: isize) {
    unsafe {
        ShowWindow(hwnd as HWND, SW_HIDE);
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

/// Harus dibuat di thread utama (message loop winit).
pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    pub active: Option<(HotKey, String)>,
}

impl Hotkeys {
    pub fn new(on_press: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| format!("Shortcut global tidak tersedia: {e}"))?;
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state() == HotKeyState::Pressed {
                on_press();
            }
        }));
        Ok(Self {
            manager,
            active: None,
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
        GlobalHotKeyEvent::set_event_handler(None::<fn(GlobalHotKeyEvent)>);
    }
}

// --- Tray ---

pub enum TrayAction {
    Open,
    Cancel,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
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
    let on_action = std::sync::Arc::new(on_action);
    let menu_action = on_action.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == ids[0] {
            menu_action(TrayAction::Open)
        } else if e.id == ids[1] {
            menu_action(TrayAction::Cancel)
        } else if e.id == ids[2] {
            menu_action(TrayAction::Quit)
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            on_action(TrayAction::Open)
        }
    }));
    Ok(Tray { _icon: icon })
}

impl Drop for Tray {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    }
}

/// Ikon tray dari logo aplikasi (Windows menskalakan ke ukuran tray).
fn tray_icon_image() -> tray_icon::Icon {
    let n = crate::theme::LOGO_SIZE as u32;
    tray_icon::Icon::from_rgba(crate::theme::LOGO_RGBA.to_vec(), n, n).expect("ikon tray valid")
}

#[cfg(test)]
mod tests {
    use super::*;

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
