//! Ventana "Imágenes quitadas": lista las imágenes quitadas de la galería y permite
//! devolverlas. Los cambios se guardan y se aplican al momento (la galería se reescanea).

use super::window::{px, ui_font};
use crate::config::AppConfig;
use crate::log_info;
use crate::winutil::{as_pcwstr, to_wide_chars};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{FillRect, SetBkColor, SetBkMode, SetTextColor, HDC, OPAQUE, TRANSPARENT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetDlgItem, RegisterClassW, SendMessageW, SetForegroundWindow, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, SW_SHOW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_CAPTION,
    WS_CLIPCHILDREN, WS_OVERLAPPED, WS_SYSMENU,
};

const CLASS_NAME: &str = "SWEPR_HiddenWallpapersClass";
const IDC_LIST: i32 = 2001;
const IDC_RESTORE: i32 = 2002;
const IDC_RESTORE_ALL: i32 = 2003;
const IDC_CLOSE: i32 = 2004;
const IDC_EMPTY: i32 = 2005;

const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_GETSELCOUNT: u32 = 0x0190;
const LB_GETSELITEMS: u32 = 0x0191;
const LBS_NOTIFY: u32 = 0x0001;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
const LBS_EXTENDEDSEL: u32 = 0x0800;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_HSCROLL: u32 = 0x0010_0000;
const LB_SETHORIZONTALEXTENT: u32 = 0x0194;

const CLIENT_W: i32 = 640;
const CLIENT_H: i32 = 400;

struct HiddenState {
    config: Arc<Mutex<AppConfig>>,
    owner: HWND,
}

// Solo lo usa el hilo de interfaz (como el resto de ventanas de SWEPR).
thread_local! {
    static STATE: std::cell::RefCell<Option<HiddenState>> = const { std::cell::RefCell::new(None) };
    static WINDOW: std::cell::Cell<Option<HWND>> = const { std::cell::Cell::new(None) };
}

fn tr(key: &str) -> Vec<u16> { to_wide_chars(&crate::i18n::t(key)) }

fn config() -> Option<Arc<Mutex<AppConfig>>> {
    STATE.with(|state| state.borrow().as_ref().map(|s| s.config.clone()))
}

unsafe fn add_control(hwnd: HWND, class: &str, text: &[u16], id: i32, rect: (i32, i32, i32, i32), style: u32) -> HWND {
    let (x, y, w, h) = rect;
    let control = CreateWindowExW(
        windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
        as_pcwstr(&to_wide_chars(class)),
        as_pcwstr(text),
        windows::Win32::UI::WindowsAndMessaging::WS_CHILD
            | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE
            | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style),
        px(x), px(y), px(w), px(h),
        hwnd,
        windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut _),
        None,
        None,
    ).unwrap_or_default();
    let font = ui_font();
    if font != 0 {
        let _ = SendMessageW(control, WM_SETFONT, WPARAM(font as usize), LPARAM(0));
    }
    control
}

/// Vuelve a llenar la lista con las imágenes quitadas.
unsafe fn refresh_list(hwnd: HWND) {
    let excluded = config().map(|config| crate::app::lock(&config).excluded.clone()).unwrap_or_default();
    let Ok(list) = GetDlgItem(hwnd, IDC_LIST) else { return; };
    let _ = SendMessageW(list, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
    for path in &excluded {
        let text = to_wide_chars(&path.to_string_lossy());
        let _ = SendMessageW(list, LB_ADDSTRING, WPARAM(0), LPARAM(text.as_ptr() as isize));
    }
    // Rutas largas: barra horizontal.
    let longest = excluded.iter().map(|p| p.to_string_lossy().chars().count()).max().unwrap_or(0) as i32;
    let _ = SendMessageW(list, LB_SETHORIZONTALEXTENT, WPARAM((longest * px(8)) as usize), LPARAM(0));
    let empty = excluded.is_empty();
    for id in [IDC_RESTORE, IDC_RESTORE_ALL] {
        if let Ok(button) = GetDlgItem(hwnd, id) {
            let _ = EnableWindow(button, !empty);
        }
    }
    if let Ok(note) = GetDlgItem(hwnd, IDC_EMPTY) {
        let _ = ShowWindow(note, if empty { SW_SHOW } else { windows::Win32::UI::WindowsAndMessaging::SW_HIDE });
    }
}

/// Devuelve a la galería las imágenes en `indices` (o todas con `None`).
unsafe fn restore(hwnd: HWND, indices: Option<Vec<usize>>) {
    let Some(config) = config() else { return; };
    {
        let mut config = crate::app::lock(&config);
        let before = config.excluded.len();
        match indices {
            Some(indices) => {
                let mut index = 0;
                config.excluded.retain(|_| {
                    let keep = !indices.contains(&index);
                    index += 1;
                    keep
                });
            }
            None => config.excluded.clear(),
        }
        log_info!("Imágenes devueltas a la galería: {}", before - config.excluded.len());
        if let Err(e) = config.save() {
            crate::log_error!("No se pudo guardar la configuración: {e}");
        }
    }
    // La ventana principal reescanea al ver que cambió la lista de quitadas.
    if let Some(main) = crate::app::MAIN_HWND.lock().ok().and_then(|g| *g) {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(HWND(main as *mut _), crate::app::WM_APP_CONFIG_CHANGED, WPARAM(0), LPARAM(0));
    }
    refresh_list(hwnd);
}

unsafe fn selected_indices(hwnd: HWND) -> Vec<usize> {
    let Ok(list) = GetDlgItem(hwnd, IDC_LIST) else { return Vec::new(); };
    let count = SendMessageW(list, LB_GETSELCOUNT, WPARAM(0), LPARAM(0)).0;
    if count <= 0 { return Vec::new(); }
    let mut items = vec![0i32; count as usize];
    let read = SendMessageW(list, LB_GETSELITEMS, WPARAM(items.len()), LPARAM(items.as_mut_ptr() as isize)).0;
    items.truncate(read.max(0) as usize);
    items.into_iter().map(|i| i as usize).collect()
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use super::theme::{brush, COLOR_FIELD_BG, COLOR_LABEL, COLOR_TEXT, COLOR_WINDOW_BG};
    match msg {
        WM_CREATE => {
            add_control(hwnd, "STATIC", &tr("These images are not shown in the gallery. Their files were not deleted."), 0, (16, 12, 608, 36), 0);
            add_control(hwnd, "LISTBOX", &to_wide_chars(""), IDC_LIST, (16, 52, 608, 280),
                LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_EXTENDEDSEL | WS_VSCROLL | WS_HSCROLL | 0x0001_0000);
            add_control(hwnd, "STATIC", &tr("No images have been removed from the gallery."), IDC_EMPTY, (32, 70, 576, 24), 0);
            super::theme::style_button(add_control(hwnd, "BUTTON", &tr("Restore selected"), IDC_RESTORE, (16, 348, 180, 34), 0x0001_0000), false, false);
            super::theme::style_button(add_control(hwnd, "BUTTON", &tr("Restore all"), IDC_RESTORE_ALL, (204, 348, 150, 34), 0x0001_0000), false, false);
            super::theme::style_button(add_control(hwnd, "BUTTON", &tr("Close"), IDC_CLOSE, (514, 348, 110, 34), 0x0001_0000), true, false);
            refresh_list(hwnd);
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND => {
            let mut rect = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
            let _ = FillRect(HDC(wparam.0 as *mut _), &rect, brush(COLOR_WINDOW_BG));
            LRESULT(1)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORSTATIC | windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORBTN => {
            let dc = HDC(wparam.0 as *mut _);
            let _ = SetTextColor(dc, COLOR_LABEL);
            let _ = SetBkMode(dc, TRANSPARENT);
            LRESULT(brush(COLOR_WINDOW_BG).0 as isize)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORLISTBOX => {
            let dc = HDC(wparam.0 as *mut _);
            let _ = SetTextColor(dc, COLOR_TEXT);
            let _ = SetBkColor(dc, COLOR_FIELD_BG);
            let _ = SetBkMode(dc, OPAQUE);
            LRESULT(brush(COLOR_FIELD_BG).0 as isize)
        }
        WM_COMMAND => {
            match (wparam.0 & 0xFFFF) as i32 {
                IDC_RESTORE => {
                    let selected = selected_indices(hwnd);
                    if !selected.is_empty() {
                        restore(hwnd, Some(selected));
                    }
                }
                IDC_RESTORE_ALL => restore(hwnd, None),
                IDC_CLOSE => { let _ = DestroyWindow(hwnd); }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // Devolver el control a la ventana de configuración.
            if let Some(owner) = STATE.with(|state| state.borrow_mut().take()).map(|s| s.owner) {
                let _ = EnableWindow(owner, true);
                let _ = SetForegroundWindow(owner);
            }
            WINDOW.with(|w| w.set(None));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Abre la ventana sobre `owner` (la de configuración), que queda desactivada mientras tanto.
pub fn show(owner: HWND, config: Arc<Mutex<AppConfig>>) {
    unsafe {
        if let Some(existing) = WINDOW.with(|w| w.get()) {
            let _ = SetForegroundWindow(existing);
            return;
        }
        let Ok(module) = GetModuleHandleW(None) else { return; };
        let hinstance = HINSTANCE(module.0);
        let class_name = to_wide_chars(CLASS_NAME);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hIcon: crate::resources::app_icon(),
            lpszClassName: as_pcwstr(&class_name),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        STATE.with(|state| *state.borrow_mut() = Some(HiddenState { config, owner }));

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
        let mut rect = RECT { left: 0, top: 0, right: px(CLIENT_W), bottom: px(CLIENT_H) };
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(owner).max(96);
        let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut rect, style, false, windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0), dpi);
        let mut owner_rect = RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(owner, &mut owner_rect);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let x = owner_rect.left + ((owner_rect.right - owner_rect.left) - width) / 2;
        let y = owner_rect.top + ((owner_rect.bottom - owner_rect.top) - height) / 2;

        let Ok(hwnd) = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            as_pcwstr(&class_name),
            as_pcwstr(&tr("Removed images")),
            style,
            x, y, width, height,
            owner, None, hinstance, None,
        ) else {
            STATE.with(|state| *state.borrow_mut() = None);
            return;
        };
        let dark = windows::Win32::Foundation::BOOL(1);
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&dark) as u32,
        );
        let caption = COLOR_CAPTION;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_CAPTION_COLOR,
            &caption as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&caption) as u32,
        );
        WINDOW.with(|w| w.set(Some(hwnd)));
        // Modal respecto a la configuración.
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

const COLOR_CAPTION: windows::Win32::Foundation::COLORREF = super::theme::COLOR_WINDOW_BG;
