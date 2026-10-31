use crate::winutil::{as_pcwstr, to_wide_chars};
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{LoadIconW, HICON, IDI_APPLICATION};

pub fn app_icon() -> HICON {
    unsafe {
        let fallback = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        let Ok(module) = GetModuleHandleW(None) else { return fallback; };
        let icon_name = to_wide_chars("IDI_SWEPR");
        LoadIconW(HINSTANCE(module.0), as_pcwstr(&icon_name)).unwrap_or(fallback)
    }
}
