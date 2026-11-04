use crate::config::{AppConfig, PanelMode, PanelSide};
use crate::log_info;
use crate::winutil::{as_pcwstr, to_wide_chars};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{BOOL, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, FillRect, HBRUSH, HDC, SetBkColor, SetBkMode, SetTextColor, OPAQUE, TRANSPARENT};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
    CS_HREDRAW, CS_VREDRAW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
    GetDlgItem, SendMessageW, SetWindowTextW, ShowWindow, SW_SHOW,
};

const CONFIG_CLASS_NAME: &str = "SWEPR_ConfigWindowClass";

// Paleta oscura de la ventana (COLORREF en formato 0x00BBGGRR).
const COLOR_WINDOW_BG: COLORREF = COLORREF(0x00242424);
const COLOR_FIELD_BG: COLORREF = COLORREF(0x00383838);
const COLOR_TEXT: COLORREF = COLORREF(0x00F0F0F0);

const IDC_FOLDER_EDIT: i32 = 1001;
const IDC_FOLDER_BTN: i32 = 1002;
const IDC_DETECTION_CHK: i32 = 1003;
const IDC_SUBFOLDERS_CHK: i32 = 1004;
const IDC_AUTOSTART_CHK: i32 = 1005;
const IDC_APPLY_BTN: i32 = 1006;
const IDC_CANCEL_BTN: i32 = 1007;
const IDC_EXIT_BTN: i32 = 1008;
const IDC_THUMB_CHK_64: i32 = 1010;
const IDC_THUMB_CHK_128: i32 = 1011;
const IDC_THUMB_CHK_256: i32 = 1012;
const IDC_THUMB_CHK_512: i32 = 1013;
const IDC_PANEL_SIDE_COMBO: i32 = 1014;
const IDC_SCROLLBAR_CHK: i32 = 1015;
const IDC_LANGUAGE_COMBO: i32 = 1016;
const IDC_SECTION_GENERAL: i32 = 1017;
const IDC_SECTION_PANEL: i32 = 1018;
const IDC_SECTION_THUMBNAILS: i32 = 1019;
const IDC_SECTION_FOLDER: i32 = 1020;
const IDC_PANEL_MODE_COMBO: i32 = 1021;

static CONFIG_HWND: Mutex<Option<isize>> = Mutex::new(None);
static CONFIG_STATE: Mutex<Option<Arc<Mutex<AppConfig>>>> = Mutex::new(None);
static PENDING_FOLDER_PATH: Mutex<Option<String>> = Mutex::new(None);
static LANGUAGE_OPTIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CONFIG_BG_BRUSH: Mutex<Option<isize>> = Mutex::new(None);
static CONFIG_FIELD_BRUSH: Mutex<Option<isize>> = Mutex::new(None);
/// DPI de la ventana; el layout se escribe en píxeles a 96 DPI y se escala con `px`.
static CONFIG_DPI: Mutex<u32> = Mutex::new(96);
/// Fuente de mensajes del sistema (Segoe UI), compartida por todos los controles.
static CONFIG_FONT: Mutex<Option<isize>> = Mutex::new(None);

fn px(value: i32) -> i32 {
    let dpi = CONFIG_DPI.lock().map(|dpi| *dpi).unwrap_or(96);
    value * dpi as i32 / 96
}

/// Crea la fuente de interfaz del sistema al tamaño del DPI actual.
unsafe fn create_ui_font(dpi: u32) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS};
    let mut metrics = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    let ok = windows::Win32::UI::HiDpi::SystemParametersInfoForDpi(
        SPI_GETNONCLIENTMETRICS.0,
        metrics.cbSize,
        Some(&mut metrics as *mut _ as *mut std::ffi::c_void),
        0,
        dpi,
    );
    if ok.is_err() { return 0; }
    windows::Win32::Graphics::Gdi::CreateFontIndirectW(&metrics.lfMessageFont).0 as isize
}

fn hmenu(id: i32) -> windows::Win32::UI::WindowsAndMessaging::HMENU {
    windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut _)
}

fn tr(key: &str) -> Vec<u16> { to_wide_chars(&crate::i18n::t(key)) }

unsafe fn config_brush(field: bool) -> HBRUSH {
    let storage = if field { &CONFIG_FIELD_BRUSH } else { &CONFIG_BG_BRUSH };
    if let Ok(mut stored) = storage.lock() {
        if let Some(handle) = *stored { return HBRUSH(handle as *mut _); }
        let color = if field { COLOR_FIELD_BG } else { COLOR_WINDOW_BG };
        let brush = CreateSolidBrush(color);
        *stored = Some(brush.0 as isize);
        brush
    } else {
        HBRUSH::default()
    }
}

unsafe fn add_control(hwnd: HWND, class: &str, text: &[u16], id: i32, x: i32, y: i32, width: i32, height: i32, extra_style: u32) -> HWND {
    let style = windows::Win32::UI::WindowsAndMessaging::WS_CHILD
        | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE
        | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(extra_style);
    let control = CreateWindowExW(
        windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
        as_pcwstr(&to_wide_chars(class)),
        as_pcwstr(text),
        style,
        px(x), px(y), px(width), px(height), hwnd, hmenu(id), None, None,
    ).unwrap_or_default();
    if let Some(font) = CONFIG_FONT.lock().ok().and_then(|font| *font) {
        let _ = SendMessageW(control, windows::Win32::UI::WindowsAndMessaging::WM_SETFONT, WPARAM(font as usize), LPARAM(0));
    }
    control
}

unsafe fn create_config_controls(hwnd: HWND) {
    const TAB_STOP: u32 = 0x0001_0000;
    const AUTO_HORIZONTAL_SCROLL: u32 = 0x0080;
    const CHECKBOX: u32 = 3;
    const RADIO_BUTTON: u32 = 4;
    const GROUP_BOX: u32 = 7;
    const COMBO_STYLE: u32 = 3 | 0x0020_0000;

    add_control(hwnd, "BUTTON", &tr("Wallpaper folder:"), IDC_SECTION_FOLDER, 12, 8, 620, 76, GROUP_BOX);
    add_control(hwnd, "EDIT", &to_wide_chars(""), IDC_FOLDER_EDIT, 24, 40, 462, 28, TAB_STOP | AUTO_HORIZONTAL_SCROLL | 0x0080_0000);
    add_control(hwnd, "BUTTON", &tr("Browse..."), IDC_FOLDER_BTN, 496, 40, 122, 28, TAB_STOP);

    add_control(hwnd, "BUTTON", &tr("General"), IDC_SECTION_GENERAL, 12, 92, 620, 140, GROUP_BOX);
    add_control(hwnd, "BUTTON", &tr("Detection enabled"), IDC_DETECTION_CHK, 24, 116, 286, 28, CHECKBOX | TAB_STOP);
    add_control(hwnd, "BUTTON", &tr("Scan subfolders"), IDC_SUBFOLDERS_CHK, 24, 148, 286, 28, CHECKBOX | TAB_STOP);
    add_control(hwnd, "BUTTON", &tr("Start with Windows"), IDC_AUTOSTART_CHK, 24, 180, 286, 28, CHECKBOX | TAB_STOP);
    add_control(hwnd, "STATIC", &tr("Language:"), 0, 338, 112, 270, 18, 0);
    let language_combo = add_control(hwnd, "COMBOBOX", &to_wide_chars(""), IDC_LANGUAGE_COMBO, 338, 134, 270, 130, COMBO_STYLE | TAB_STOP);
    let mut options = vec!["auto".to_string()];
    let automatic = tr("Automatic (Windows)");
    let _ = SendMessageW(language_combo, windows::Win32::UI::WindowsAndMessaging::CB_ADDSTRING, WPARAM(0), LPARAM(automatic.as_ptr() as isize));
    for (code, name) in crate::i18n::available_languages() {
        let name = to_wide_chars(&name);
        let _ = SendMessageW(language_combo, windows::Win32::UI::WindowsAndMessaging::CB_ADDSTRING, WPARAM(0), LPARAM(name.as_ptr() as isize));
        options.push(code);
    }
    if let Ok(mut stored) = LANGUAGE_OPTIONS.lock() { *stored = options; }

    add_control(hwnd, "BUTTON", &tr("Thumbnail panel"), IDC_SECTION_PANEL, 12, 240, 620, 130, GROUP_BOX);
    add_control(hwnd, "STATIC", &tr("Panel mode:"), 0, 24, 260, 270, 18, 0);
    let mode_combo = add_control(hwnd, "COMBOBOX", &to_wide_chars(""), IDC_PANEL_MODE_COMBO, 24, 281, 270, 100, COMBO_STYLE | TAB_STOP);
    for item in ["Low power", "Pro (carousel)"] {
        let item = tr(item);
        let _ = SendMessageW(mode_combo, windows::Win32::UI::WindowsAndMessaging::CB_ADDSTRING, WPARAM(0), LPARAM(item.as_ptr() as isize));
    }
    add_control(hwnd, "STATIC", &tr("Thumbnail panel position:"), 0, 338, 260, 270, 18, 0);
    let panel_combo = add_control(hwnd, "COMBOBOX", &to_wide_chars(""), IDC_PANEL_SIDE_COMBO, 338, 281, 270, 100, COMBO_STYLE | TAB_STOP);
    for item in ["Top", "Left", "Right"] {
        let item = tr(item);
        let _ = SendMessageW(panel_combo, windows::Win32::UI::WindowsAndMessaging::CB_ADDSTRING, WPARAM(0), LPARAM(item.as_ptr() as isize));
    }
    add_control(hwnd, "BUTTON", &tr("Show scrollbar"), IDC_SCROLLBAR_CHK, 24, 320, 286, 28, CHECKBOX | TAB_STOP);

    add_control(hwnd, "BUTTON", &tr("Thumbnail size"), IDC_SECTION_THUMBNAILS, 12, 378, 620, 142, GROUP_BOX);
    add_control(hwnd, "BUTTON", &tr("Small (64 px)"), IDC_THUMB_CHK_64, 24, 408, 280, 30, RADIO_BUTTON | TAB_STOP);
    add_control(hwnd, "BUTTON", &tr("Standard (128 px)"), IDC_THUMB_CHK_128, 318, 408, 290, 30, RADIO_BUTTON | TAB_STOP);
    add_control(hwnd, "BUTTON", &tr("Large (256 px)"), IDC_THUMB_CHK_256, 24, 446, 280, 30, RADIO_BUTTON | TAB_STOP);
    add_control(hwnd, "BUTTON", &tr("Maximum (512 px)"), IDC_THUMB_CHK_512, 318, 446, 290, 30, RADIO_BUTTON | TAB_STOP);

    add_control(hwnd, "STATIC", &tr("Smaller thumbnails use less memory. Changes apply after saving."), 0, 24, 526, 580, 20, 0);
    add_control(hwnd, "BUTTON", &tr("Cancel"), IDC_CANCEL_BTN, 410, 560, 96, 32, TAB_STOP);
    add_control(hwnd, "BUTTON", &tr("Save"), IDC_APPLY_BTN, 518, 560, 96, 32, TAB_STOP | 1);
}

fn selected_panel_mode(combo: HWND) -> usize {
    unsafe { SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize }
}

/// La barra de desplazamiento solo existe en el modo de bajo consumo.
unsafe fn update_mode_dependent_controls(hwnd: HWND) {
    let pro = GetDlgItem(hwnd, IDC_PANEL_MODE_COMBO).map(|combo| selected_panel_mode(combo) == 1).unwrap_or(false);
    if let Ok(chk) = GetDlgItem(hwnd, IDC_SCROLLBAR_CHK) {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(chk, !pro);
    }
}

/// Marca exclusivamente un radio de miniatura (BS_RADIOBUTTON) como seleccionado.
unsafe fn set_thumb_radio_checked(hwnd: HWND, checked_id: i32, checked: bool) {
    for id in [IDC_THUMB_CHK_64, IDC_THUMB_CHK_128, IDC_THUMB_CHK_256, IDC_THUMB_CHK_512] {
        if let Ok(chk) = GetDlgItem(hwnd, id) {
            if !chk.is_invalid() {
                let state = if id == checked_id && checked { 1usize } else { 0usize };
                let _ = SendMessageW(
                    chk,
                    windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK,
                    windows::Win32::Foundation::WPARAM(state),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    }
}

unsafe extern "system" fn config_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        _ if msg == windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND => {
            let dc = HDC(wparam.0 as *mut _);
            let mut rect = windows::Win32::Foundation::RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
            let _ = FillRect(dc, &rect, config_brush(false));
            LRESULT(1)
        }
        _ if msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORSTATIC
            || msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORBTN =>
        {
            let dc = HDC(wparam.0 as *mut _);
            let _ = SetTextColor(dc, COLOR_TEXT);
            let _ = SetBkMode(dc, TRANSPARENT);
            LRESULT(config_brush(false).0 as isize)
        }
        _ if msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLOREDIT
            || msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORLISTBOX =>
        {
            let dc = HDC(wparam.0 as *mut _);
            let _ = SetTextColor(dc, COLOR_TEXT);
            let _ = SetBkColor(dc, COLOR_FIELD_BG);
            let _ = SetBkMode(dc, OPAQUE);
            LRESULT(config_brush(true).0 as isize)
        }
        _ if msg == windows::Win32::UI::WindowsAndMessaging::WM_USER + 100 => {
            // Actualizar el edit con la ruta seleccionada (sin usar puntero colgante)
            if let Ok(mut g) = PENDING_FOLDER_PATH.lock() {
                if let Some(s) = g.take() {
                    if let Ok(edit_hwnd) = GetDlgItem(hwnd, IDC_FOLDER_EDIT) {
                        if !edit_hwnd.is_invalid() {
                            let wide = to_wide_chars(&s);
                            let _ = SetWindowTextW(edit_hwnd, as_pcwstr(&wide));
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_CREATE => {
            log_info!("Ventana de configuración creada");
            create_config_controls(hwnd);

            // Cargar valores de config
            if let Some(config_arc) = CONFIG_STATE.lock().ok().and_then(|g| g.clone()) {
                if let Ok(config) = config_arc.lock() {
                    if let Ok(edit_hwnd) = GetDlgItem(hwnd, IDC_FOLDER_EDIT) {
                        if !edit_hwnd.is_invalid() {
                            let path_str = config.wallpaper_dir.to_string_lossy();
                            let wide = to_wide_chars(&path_str);
                            let _ = SetWindowTextW(edit_hwnd, as_pcwstr(&wide));
                        }
                    }
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_LANGUAGE_COMBO) {
                        let options = LANGUAGE_OPTIONS.lock().map(|value| value.clone()).unwrap_or_default();
                        let selected = options.iter().position(|code| code == &config.language).unwrap_or(0);
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selected), LPARAM(0));
                    }
                    // Checkboxes
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_DETECTION_CHK) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if config.detection_enabled { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_SUBFOLDERS_CHK) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if config.scan_subfolders { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_AUTOSTART_CHK) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if config.autostart { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_PANEL_SIDE_COMBO) {
                        if !combo.is_invalid() {
                            let selection = match config.panel_side {
                                PanelSide::Top => 0,
                                PanelSide::Left => 1,
                                PanelSide::Right => 2,
                            };
                            let _ = SendMessageW(
                                combo,
                                windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL,
                                WPARAM(selection),
                                LPARAM(0),
                            );
                        }
                    }
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_PANEL_MODE_COMBO) {
                        let selection = if config.panel_mode == PanelMode::Pro { 1 } else { 0 };
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selection), LPARAM(0));
                    }
                    update_mode_dependent_controls(hwnd);
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_SCROLLBAR_CHK) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if config.show_scrollbar { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    // Detalle de miniatura (grupo de radios)
                    let thumb_size = config.thumb_size_value();
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_THUMB_CHK_64) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if thumb_size == 64 { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_THUMB_CHK_128) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if thumb_size == 128 { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_THUMB_CHK_256) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if thumb_size == 256 { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_THUMB_CHK_512) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if thumb_size == 512 { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                }
            }

            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as i32;
            match cmd {
                IDC_FOLDER_BTN => {
                    log_info!("Botón 'Examinar...' presionado");
                    // Abrir IFileOpenDialog en un hilo STA separado
                    let hwnd_val = hwnd.0 as isize;
                    std::thread::spawn(move || {
                        unsafe {
                            use windows::Win32::UI::Shell::{IFileOpenDialog, FOS_PICKFOLDERS, FOS_FORCEFILESYSTEM, FileOpenDialog, SIGDN_FILESYSPATH};
                            use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
                            
                            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                            
                            if let Ok(dialog) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) {
                                let options = dialog.GetOptions().unwrap_or_default();
                                let _ = dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
                                let dialog_title = tr("Select wallpaper folder");
                                let _ = dialog.SetTitle(windows::core::PCWSTR(dialog_title.as_ptr()));
                                
                                let hwnd = windows::Win32::Foundation::HWND(hwnd_val as *mut _);
                                if dialog.Show(hwnd).is_ok() {
                                    if let Ok(item) = dialog.GetResult() {
                                        if let Ok(path_ptr) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                                            let path_str = path_ptr.to_string().unwrap_or_default();
                                            log_info!("Carpeta seleccionada: {}", path_str);
                                            
                                            // Guardar la ruta en static y notificar al hilo principal (sin puntero colgante)
                                            if let Ok(mut g) = PENDING_FOLDER_PATH.lock() {
                                                *g = Some(path_str.clone());
                                            }
                                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                                hwnd,
                                                windows::Win32::UI::WindowsAndMessaging::WM_USER + 100,
                                                windows::Win32::Foundation::WPARAM(0),
                                                windows::Win32::Foundation::LPARAM(0),
                                            );
                                        }
                                    }
                                }
                            }
                            CoUninitialize();
                        }
                    });
                }
                IDC_APPLY_BTN => {
                    log_info!("Botón 'Aplicar' presionado");
                    if let Some(config_arc) = CONFIG_STATE.lock().ok().and_then(|g| g.clone()) {
                        if let Ok(mut config) = config_arc.lock() {
                            // Leer carpeta del edit
                            if let Ok(edit_hwnd) = GetDlgItem(hwnd, IDC_FOLDER_EDIT) {
                                if !edit_hwnd.is_invalid() {
                                    // Leer texto
                                    let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW(edit_hwnd);
                                    if len > 0 {
                                        let mut buf = vec![0u16; (len + 1) as usize];
                                        let read = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(edit_hwnd, &mut buf);
                                        if read > 0 {
                                            let s = String::from_utf16_lossy(&buf[..read as usize]);
                                            config.wallpaper_dir = std::path::PathBuf::from(s);
                                        }
                                    }
                                }
                            }
                            // Checkboxes
                            if let Ok(chk) = GetDlgItem(hwnd, IDC_DETECTION_CHK) {
                                if !chk.is_invalid() {
                                    use windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK;
                                    let checked = SendMessageW(chk, BM_GETCHECK, WPARAM(0), LPARAM(0));
                                    config.detection_enabled = checked.0 == 1;
                                }
                            }
                            if let Ok(chk) = GetDlgItem(hwnd, IDC_SUBFOLDERS_CHK) {
                                if !chk.is_invalid() {
                                    use windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK;
                                    let checked = SendMessageW(chk, BM_GETCHECK, WPARAM(0), LPARAM(0));
                                    config.scan_subfolders = checked.0 == 1;
                                }
                            }
                            if let Ok(chk) = GetDlgItem(hwnd, IDC_AUTOSTART_CHK) {
                                if !chk.is_invalid() {
                                    use windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK;
                                    let checked = SendMessageW(chk, BM_GETCHECK, WPARAM(0), LPARAM(0));
                                    config.autostart = checked.0 == 1;
                                }
                            }
                            // Detalle de miniatura (grupo de radios)
                            let mut thumb_size: Option<u32> = None;
                            for (id, size) in [
                                (IDC_THUMB_CHK_64, 64u32),
                                (IDC_THUMB_CHK_128, 128u32),
                                (IDC_THUMB_CHK_256, 256u32),
                                (IDC_THUMB_CHK_512, 512u32),
                            ] {
                                if let Ok(chk) = GetDlgItem(hwnd, id) {
                                    if !chk.is_invalid() {
                                        use windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK;
                                        let checked = SendMessageW(chk, BM_GETCHECK, WPARAM(0), LPARAM(0));
                                        if checked.0 == 1 {
                                            thumb_size = Some(size);
                                        }
                                    }
                                }
                            }
                            if let Some(size) = thumb_size {
                                config.thumb_size = size;
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_PANEL_SIDE_COMBO) {
                                let selected = SendMessageW(
                                    combo,
                                    windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL,
                                    WPARAM(0),
                                    LPARAM(0),
                                ).0 as usize;
                                config.panel_side = match selected {
                                    1 => PanelSide::Left,
                                    2 => PanelSide::Right,
                                    _ => PanelSide::Top,
                                };
                                config.zones.active_zones = config.panel_side.active_zones();
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_PANEL_MODE_COMBO) {
                                config.panel_mode = if selected_panel_mode(combo) == 1 { PanelMode::Pro } else { PanelMode::LowPower };
                            }
                            if let Ok(chk) = GetDlgItem(hwnd, IDC_SCROLLBAR_CHK) {
                                use windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK;
                                config.show_scrollbar = SendMessageW(chk, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1;
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_LANGUAGE_COMBO) {
                                let selected = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize;
                                if let Some(language) = LANGUAGE_OPTIONS.lock().ok().and_then(|options| options.get(selected).cloned()) {
                                    config.language = language;
                                }
                            }
                            crate::i18n::set_language(config.language.clone());
                            let autostart_enabled = config.autostart;
                            let _ = config.save();
                            if let Err(e) = crate::config::autostart::set_autostart(autostart_enabled) {
                                crate::log_warn!("No se pudo sincronizar autoarranque: {e}");
                            }
                            log_info!("Configuración guardada");
                        }
                    }
                    // Notificar al main window que la config cambió
                    if let Some(main_hwnd) = crate::app::MAIN_HWND.lock().ok().and_then(|g| *g) {
                        unsafe {
                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                windows::Win32::Foundation::HWND(main_hwnd as *mut _),
                                crate::app::WM_APP_CONFIG_CHANGED,
                                windows::Win32::Foundation::WPARAM(0),
                                windows::Win32::Foundation::LPARAM(0),
                            );
                        }
                    }
                    let _ = DestroyWindow(hwnd);
                }
                IDC_THUMB_CHK_64 => {
                    log_info!("Radio miniatura: Pequena (64 px) seleccionada");
                    unsafe { set_thumb_radio_checked(hwnd, IDC_THUMB_CHK_64, true); }
                }
                IDC_THUMB_CHK_128 => {
                    log_info!("Radio miniatura: Estandar (128 px) seleccionada");
                    unsafe { set_thumb_radio_checked(hwnd, IDC_THUMB_CHK_128, true); }
                }
                IDC_THUMB_CHK_256 => {
                    log_info!("Radio miniatura: Grande (256 px) seleccionada");
                    unsafe { set_thumb_radio_checked(hwnd, IDC_THUMB_CHK_256, true); }
                }
                IDC_THUMB_CHK_512 => {
                    log_info!("Radio miniatura: Muy grande (512 px) seleccionada");
                    unsafe { set_thumb_radio_checked(hwnd, IDC_THUMB_CHK_512, true); }
                }
                IDC_PANEL_MODE_COMBO => {
                    // CBN_SELCHANGE
                    if (wparam.0 >> 16) & 0xFFFF == 1 {
                        update_mode_dependent_controls(hwnd);
                    }
                }
                IDC_CANCEL_BTN => {
                    let _ = DestroyWindow(hwnd);
                }
                IDC_EXIT_BTN => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Ok(mut g) = CONFIG_HWND.lock() { *g = None; }
            if let Some(font) = CONFIG_FONT.lock().ok().and_then(|mut font| font.take()) {
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(font as *mut _));
            }
            log_info!("Ventana de configuración destruida");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn show_config_window(config: Arc<Mutex<AppConfig>>) {
    unsafe {
        if let Ok(guard) = CONFIG_HWND.lock() {
            if guard.is_some() {
                return;
            }
        }
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => HINSTANCE(h.0),
            Err(_) => return,
        };
        let class_name_wide = to_wide_chars(CONFIG_CLASS_NAME);
        let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(config_wnd_proc),
            hInstance: hinstance,
            hIcon: crate::resources::app_icon(),
            hbrBackground: config_brush(false),
            lpszClassName: as_pcwstr(&class_name_wide),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        // Establecer CONFIG_STATE ANTES de crear la ventana (WM_CREATE lo necesita)
        if let Ok(mut g) = CONFIG_STATE.lock() { *g = Some(config.clone()); }
        let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem().max(96);
        if let Ok(mut g) = CONFIG_DPI.lock() { *g = dpi; }
        if let Ok(mut g) = CONFIG_FONT.lock() { *g = Some(create_ui_font(dpi)).filter(|font| *font != 0); }

        let hwnd = match CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            as_pcwstr(&class_name_wide),
            as_pcwstr(&tr("SWEPR - Settings")),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            px(100), px(100), px(660), px(640),
            None, None, hinstance, None,
        ) {
            Ok(h) => h,
            Err(_) => return,
        };

        let use_dark_titlebar = BOOL(1);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &use_dark_titlebar as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
        if let Ok(mut g) = CONFIG_HWND.lock() { *g = Some(hwnd.0 as isize); }
        let _ = ShowWindow(hwnd, SW_SHOW);
        log_info!("Ventana de configuración mostrada");
    }
}
