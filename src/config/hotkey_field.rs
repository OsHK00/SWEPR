//! Campo para capturar un atajo de teclado, dibujado a mano con la paleta oscura.
//! Guarda el atajo en el mismo formato que el control de atajos de Windows
//! (`HotkeyBinding::to_hotkey_value`): tecla en el byte bajo y modificadores en el alto.

use crate::config::{HOTKEYF_ALT, HOTKEYF_CONTROL, HOTKEYF_EXT, HOTKEYF_SHIFT, HOTKEYF_WIN};
use crate::winutil::{as_pcwstr, to_wide_chars};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint, HGDIOBJ, InvalidateRect,
    SelectObject, SetBkMode, SetTextColor, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyNameTextW, GetKeyState, MapVirtualKeyW, SetFocus, MAPVK_VK_TO_VSC};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, GetWindowLongPtrW, RegisterClassW, SetWindowLongPtrW, GWLP_USERDATA, IDC_IBEAM, LoadCursorW,
    WM_GETDLGCODE, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_PAINT, WM_SETFOCUS, WM_SYSKEYDOWN, WNDCLASSW,
};

pub const CLASS_NAME: &str = "SWEPR_HotkeyField";
/// Mismos mensajes que el control de atajos de Windows.
pub const HKM_SETHOTKEY: u32 = 0x0401;
pub const HKM_GETHOTKEY: u32 = 0x0402;

use super::theme::{COLOR_MUTED as COLOR_PLACEHOLDER, COLOR_TEXT};
const TEXT_PADDING: i32 = 10;

pub fn register_class(hinstance: windows::Win32::Foundation::HINSTANCE) {
    let class_name = to_wide_chars(CLASS_NAME);
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(field_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_IBEAM).unwrap_or_default(),
            lpszClassName: as_pcwstr(&class_name),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
    }
}

fn key_label(code: u32) -> String {
    match code {
        0x30..=0x39 | 0x41..=0x5A => char::from(code as u8).to_string(),
        0x70..=0x87 => format!("F{}", code - 0x70 + 1),
        0x25 => "←".into(),
        0x26 => "↑".into(),
        0x27 => "→".into(),
        0x28 => "↓".into(),
        _ => unsafe {
            // Nombre que da el teclado instalado ("Inicio", "Supr", "Num 4"...).
            let scan = MapVirtualKeyW(code, MAPVK_VK_TO_VSC);
            let extended = matches!(code, 0x21..=0x2E) as i32;
            let mut buffer = [0u16; 64];
            let len = GetKeyNameTextW(((scan as i32) << 16) | (extended << 24), &mut buffer);
            if len > 0 { String::from_utf16_lossy(&buffer[..len as usize]) } else { format!("0x{code:02X}") }
        },
    }
}

/// Texto del atajo, p. ej. "Ctrl + Alt + ←". Vacío si no hay tecla.
pub fn hotkey_text(value: u16) -> String {
    let code = (value & 0xFF) as u32;
    if code == 0 { return String::new(); }
    let flags = value >> 8;
    let mut parts: Vec<String> = Vec::new();
    if flags & HOTKEYF_WIN != 0 { parts.push("Win".into()); }
    if flags & HOTKEYF_CONTROL != 0 { parts.push("Ctrl".into()); }
    if flags & HOTKEYF_ALT != 0 { parts.push("Alt".into()); }
    if flags & HOTKEYF_SHIFT != 0 { parts.push("Shift".into()); }
    parts.push(key_label(code));
    parts.join(" + ")
}

fn is_modifier_key(code: u32) -> bool {
    matches!(code, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn key_down(code: i32) -> bool {
    unsafe { GetKeyState(code) < 0 }
}

/// Atajo para una tecla pulsada con los modificadores que estén presionados.
fn capture(code: u32) -> u16 {
    let mut flags = 0u16;
    if key_down(0x11) { flags |= HOTKEYF_CONTROL; }
    if key_down(0x12) { flags |= HOTKEYF_ALT; }
    if key_down(0x10) { flags |= HOTKEYF_SHIFT; }
    if key_down(0x5B) || key_down(0x5C) { flags |= HOTKEYF_WIN; }
    // Un atajo global sin Ctrl, Alt ni Win bloquearía esa tecla en todo Windows: se añade Ctrl+Alt.
    if flags & (HOTKEYF_CONTROL | HOTKEYF_ALT | HOTKEYF_WIN) == 0 {
        flags |= HOTKEYF_CONTROL | HOTKEYF_ALT;
    }
    (flags << 8) | code as u16
}

unsafe extern "system" fn field_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        HKM_SETHOTKEY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (wparam.0 as u16 & !(HOTKEYF_EXT << 8)) as isize);
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        HKM_GETHOTKEY => LRESULT(GetWindowLongPtrW(hwnd, GWLP_USERDATA) & 0xFFFF),
        WM_GETDLGCODE => LRESULT(0x0004), // DLGC_WANTALLKEYS
        WM_LBUTTONDOWN => {
            let _ = SetFocus(hwnd);
            LRESULT(0)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let code = wparam.0 as u32;
            if is_modifier_key(code) { return LRESULT(0); }
            let no_modifiers = !key_down(0x11) && !key_down(0x12) && !key_down(0x10) && !key_down(0x5B) && !key_down(0x5C);
            // Retroceso o Supr solos dejan el atajo vacío (desactivado).
            let value = if no_modifiers && matches!(code, 0x08 | 0x2E) { 0 } else { capture(code) };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, value as isize);
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let focused = windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
            super::theme::draw_field(dc, &rect, focused, false);

            let value = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) & 0xFFFF) as u16;
            let text = hotkey_text(value);
            let (text, color) = if text.is_empty() {
                (crate::i18n::t("None"), COLOR_PLACEHOLDER)
            } else {
                (text, COLOR_TEXT)
            };
            let font = super::window::ui_font();
            let old_font = (font != 0).then(|| SelectObject(dc, HGDIOBJ(font as *mut _)));
            let _ = SetBkMode(dc, TRANSPARENT);
            let _ = SetTextColor(dc, color);
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            let mut text_rect = RECT { left: rect.left + super::window::px(TEXT_PADDING), right: rect.right - super::window::px(TEXT_PADDING), ..rect };
            let _ = DrawTextW(dc, &mut wide, &mut text_rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
            if let Some(old) = old_font { SelectObject(dc, old); }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
