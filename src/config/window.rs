use crate::config::{AppConfig, ApplyMode, ClockPosition, ClockSize, ClockStyle, FitMode, Transition, PanelMode, PanelSide, ProPriority, ProStyle, SortOrder, Theme, ThumbLoading};
use crate::log_info;
use crate::winutil::{as_pcwstr, to_wide_chars};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Graphics::Gdi::{FillRect, HDC, SetBkColor, SetBkMode, SetTextColor, OPAQUE, TRANSPARENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
    CS_HREDRAW, CS_VREDRAW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WS_CAPTION, WS_CLIPCHILDREN, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
    GetDlgItem, SendMessageW, SetWindowTextW, ShowWindow, SW_SHOW,
};

const CONFIG_CLASS_NAME: &str = "SWEPR_ConfigWindowClass";


const IDC_FOLDER_EDIT: i32 = 1001;
const IDC_FOLDER_BTN: i32 = 1002;
const IDC_DETECTION_CHK: i32 = 1003;
const IDC_SUBFOLDERS_CHK: i32 = 1004;
const IDC_AUTOSTART_CHK: i32 = 1005;
const IDC_APPLY_BTN: i32 = 1006;
const IDC_CANCEL_BTN: i32 = 1007;
const IDC_EXIT_BTN: i32 = 1008;
const IDC_THUMB_SIZE_COMBO: i32 = 1010;
const IDC_FOOTER_NOTE: i32 = 1011;
const IDC_PANEL_SIDE_COMBO: i32 = 1014;
const IDC_SCROLLBAR_CHK: i32 = 1015;
const IDC_LANGUAGE_COMBO: i32 = 1016;
const IDC_PANEL_MODE_COMBO: i32 = 1021;
const IDC_THUMB_LOADING_COMBO: i32 = 1022;
/// Cambio de fondo: Windows sin fundido, Windows con fundido o lienzo de SWEPR.
const IDC_APPLY_MODE_COMBO: i32 = 1023;
const IDC_TRANSITION_COMBO: i32 = 1035;
const IDC_TRANSITION_EFFECT_COMBO: i32 = 1036;
const IDC_PRO_PRIORITY_COMBO: i32 = 1024;
const IDC_HOTKEY_PANEL: i32 = 1025;
const IDC_HOTKEY_PREVIOUS: i32 = 1026;
const IDC_HOTKEY_NEXT: i32 = 1027;
const IDC_FIT_MODE_COMBO: i32 = 1028;
const IDC_SORT_COMBO: i32 = 1029;
const IDC_ROTATION_COMBO: i32 = 1030;
const IDC_SHUFFLE_CHK: i32 = 1031;
const IDC_FAVORITES_ONLY_CHK: i32 = 1032;
const IDC_PER_MONITOR_CHK: i32 = 1033;
const IDC_HIDDEN_BTN: i32 = 1034;
const IDC_PRO_STYLE_COMBO: i32 = 1037;
const IDC_NOTCH_NOTIFY_CHK: i32 = 1038;
const IDC_LIVE_PREVIEW_CHK: i32 = 1039;
const IDC_THEME_COMBO: i32 = 1040;
const IDC_SHOW_NAMES_CHK: i32 = 1041;
const IDC_ANIMATED_CHK: i32 = 1042;
const IDC_CLOCK_CHK: i32 = 1043;
const IDC_CLOCK_STYLE_COMBO: i32 = 1044;
const IDC_CLOCK_POSITION_COMBO: i32 = 1045;
const IDC_CLOCK_SIZE_COMBO: i32 = 1046;
const IDC_CLOCK_24H_CHK: i32 = 1047;
const IDC_CLOCK_SECONDS_CHK: i32 = 1048;
const IDC_CLOCK_DATE_CHK: i32 = 1049;
const IDC_CLOCK_MAIN_CHK: i32 = 1050;
/// Controles del reloj que dependen de tenerlo activado.
const CLOCK_OPTIONS: [i32; 7] = [
    IDC_CLOCK_STYLE_COMBO, IDC_CLOCK_POSITION_COMBO, IDC_CLOCK_SIZE_COMBO, IDC_CLOCK_24H_CHK, IDC_CLOCK_SECONDS_CHK, IDC_CLOCK_DATE_CHK, IDC_CLOCK_MAIN_CHK,
];

/// Mensaje propio: el hilo del diálogo "Examinar" terminó (con o sin carpeta elegida).
const WM_APP_FOLDER_PICKED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 100;
const WM_DPICHANGED: u32 = 0x02E0;

use super::hotkey_field::{HKM_GETHOTKEY, HKM_SETHOTKEY};

static CONFIG_HWND: Mutex<Option<isize>> = Mutex::new(None);
static CONFIG_STATE: Mutex<Option<Arc<Mutex<AppConfig>>>> = Mutex::new(None);
static PENDING_FOLDER_PATH: Mutex<Option<String>> = Mutex::new(None);
/// Hay un diálogo "Examinar" abierto (solo se permite uno).
static BROWSE_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LANGUAGE_OPTIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// DPI de la ventana; el layout se escribe en píxeles a 96 DPI y se escala con `px`.
static CONFIG_DPI: Mutex<u32> = Mutex::new(96);
/// Fuente de mensajes del sistema (Segoe UI), compartida por todos los controles.
static CONFIG_FONT: Mutex<Option<isize>> = Mutex::new(None);
/// Misma fuente en seminegrita y algo mayor, para los títulos de sección.
static CONFIG_TITLE_FONT: Mutex<Option<isize>> = Mutex::new(None);

pub(super) fn px(value: i32) -> i32 {
    let dpi = CONFIG_DPI.lock().map(|dpi| *dpi).unwrap_or(96);
    value * dpi as i32 / 96
}

/// Crea la fuente de interfaz del sistema al tamaño del DPI actual; con `title`, la de los títulos.
unsafe fn create_ui_font(dpi: u32, title: bool) -> isize {
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
    let mut font = metrics.lfMessageFont;
    if title {
        font.lfWeight = 600;
        font.lfHeight = font.lfHeight * 115 / 100;
    }
    windows::Win32::Graphics::Gdi::CreateFontIndirectW(&font).0 as isize
}

fn hmenu(id: i32) -> windows::Win32::UI::WindowsAndMessaging::HMENU {
    windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut _)
}

fn tr(key: &str) -> Vec<u16> { to_wide_chars(&crate::i18n::t(key)) }

/// Crea un control hijo con la posición y el tamaño dados a 96 DPI (se escalan con `px`).
#[allow(clippy::too_many_arguments)]
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

/// Tarjetas de la ventana (título, x, y, ancho, alto) en píxeles a 96 DPI.
/// Las de cada columna empiezan y acaban a la misma altura.
const SECTIONS: [(&str, i32, i32, i32, i32); 6] = [
    ("Wallpaper folder:", 12, 12, 1220, 86),
    ("Thumbnail panel", 12, 110, 620, 304),
    ("General", 12, 426, 620, 178),
    ("Wallpaper", 644, 110, 288, 494),
    ("Desktop clock", 944, 110, 288, 494),
    ("Keyboard shortcuts", 12, 616, 1220, 106),
];
/// Marco del campo de la carpeta (el EDIT va dentro, sin borde propio).
const FOLDER_FIELD: (i32, i32, i32, i32) = (28, 52, 1056, 32);
const CLIENT_W: i32 = 1244;
const CLIENT_H: i32 = 782;
const COL_1: i32 = 28;
const COL_2: i32 = 334;
const COL_3: i32 = 660;
const COL_W: i32 = 282;
const COL_3_W: i32 = 256;
const COL_4: i32 = 960;
const THUMB_SIZES: [u32; 4] = [64, 128, 256, 512];
/// Intervalos de rotación automática en minutos (0 = desactivada).
const ROTATION_MINUTES: [u32; 8] = [0, 5, 15, 30, 60, 180, 720, 1440];
/// Duraciones de la transición del lienzo, en milisegundos.
const TRANSITION_MS: [u32; 4] = [300, 600, 1000, 1500];

unsafe fn create_config_controls(hwnd: HWND) {
    use super::theme::{style_button, style_checkbox, style_combo};
    const TAB_STOP: u32 = 0x0001_0000;
    const AUTO_HORIZONTAL_SCROLL: u32 = 0x0080;
    const CHECKBOX: u32 = 3;
    const COMBO_STYLE: u32 = 3 | 0x0020_0000;
    let add_combo = |id: i32, x: i32, y: i32, width: i32, items: &[Vec<u16>]| {
        let combo = add_control(hwnd, "COMBOBOX", &to_wide_chars(""), id, x, y, width, 200, COMBO_STYLE | TAB_STOP);
        for item in items {
            let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_ADDSTRING, WPARAM(0), LPARAM(item.as_ptr() as isize));
        }
        style_combo(combo);
        combo
    };
    let add_checkbox_w = |text: &str, id: i32, x: i32, y: i32, width: i32| {
        style_checkbox(add_control(hwnd, "BUTTON", &tr(text), id, x, y, width, 24, CHECKBOX | TAB_STOP));
    };
    let add_checkbox = |text: &str, id: i32, x: i32, y: i32| add_checkbox_w(text, id, x, y, COL_W);
    let label = |text: Vec<u16>, x: i32, y: i32, width: i32| { add_control(hwnd, "STATIC", &text, 0, x, y, width, 18, 0); };
    let with_colon = |key: &str| to_wide_chars(&format!("{}:", crate::i18n::t(key).trim_end_matches(':')));

    // Carpeta
    let (fx, fy, fw, fh) = FOLDER_FIELD;
    add_control(hwnd, "EDIT", &to_wide_chars(""), IDC_FOLDER_EDIT, fx + 10, fy + (fh - 18) / 2, fw - 20, 18, TAB_STOP | AUTO_HORIZONTAL_SCROLL);
    style_button(add_control(hwnd, "BUTTON", &tr("Browse..."), IDC_FOLDER_BTN, 1094, fy, 122, fh, TAB_STOP), false, true);

    // Panel de miniaturas
    // Izquierda: el panel y su posición; derecha: lo propio del modo Pro.
    label(tr("Panel mode:"), COL_1, 150, COL_W);
    add_combo(IDC_PANEL_MODE_COMBO, COL_1, 170, COL_W, &[tr("Low power"), tr("Pro (gallery)"), tr("Notch (top island)")]);
    label(tr("Pro style:"), COL_2, 150, COL_W);
    // En el orden de ProStyle::ALL.
    add_combo(IDC_PRO_STYLE_COMBO, COL_2, 170, COL_W, &[tr("Showcase"), tr("Fan"), tr("Mosaic"), tr("Frame"), tr("Classic carousel")]);
    label(tr("Thumbnail panel position:"), COL_1, 212, COL_W);
    add_combo(IDC_PANEL_SIDE_COMBO, COL_1, 232, COL_W, &[tr("Top"), tr("Left"), tr("Right")]);
    label(tr("Pro mode priority:"), COL_2, 212, COL_W);
    add_combo(IDC_PRO_PRIORITY_COMBO, COL_2, 232, COL_W, &[tr("Smooth (higher memory use)"), tr("Performance (lower memory use)")]);
    label(with_colon("Thumbnail size"), COL_1, 274, COL_W);
    add_combo(IDC_THUMB_SIZE_COMBO, COL_1, 294, COL_W, &[tr("Small (64 px)"), tr("Standard (128 px)"), tr("Large (256 px)"), tr("Maximum (512 px)")]);
    label(tr("Thumbnail loading:"), COL_2, 274, COL_W);
    add_combo(IDC_THUMB_LOADING_COMBO, COL_2, 294, COL_W, &[tr("All in advance"), tr("On demand (less memory)")]);
    add_checkbox("Show scrollbar", IDC_SCROLLBAR_CHK, COL_1, 340);
    add_checkbox("Notch notice when the wallpaper changes", IDC_NOTCH_NOTIFY_CHK, COL_2, 340);
    add_checkbox("Show wallpaper names", IDC_SHOW_NAMES_CHK, COL_1, 370);

    // General
    add_checkbox("Detection enabled", IDC_DETECTION_CHK, COL_1, 466);
    add_checkbox("Scan subfolders", IDC_SUBFOLDERS_CHK, COL_1, 496);
    add_checkbox("Start with Windows", IDC_AUTOSTART_CHK, COL_1, 526);
    label(tr("Language:"), COL_2, 466, COL_W);
    let mut options = vec!["auto".to_string()];
    let mut language_items = vec![tr("Automatic (Windows)")];
    for (code, name) in crate::i18n::available_languages() {
        language_items.push(to_wide_chars(&name));
        options.push(code);
    }
    add_combo(IDC_LANGUAGE_COMBO, COL_2, 486, COL_W, &language_items);
    label(tr("Theme:"), COL_2, 528, COL_W);
    // En el orden de Theme::ALL.
    add_combo(IDC_THEME_COMBO, COL_2, 548, COL_W, &[tr("Same as Windows"), tr("Dark"), tr("Light")]);
    if let Ok(mut stored) = LANGUAGE_OPTIONS.lock() { *stored = options; }

    // Fondo de pantalla
    label(tr("Fit:"), COL_3, 150, COL_3_W);
    add_combo(IDC_FIT_MODE_COMBO, COL_3, 170, COL_3_W, &[tr("Fill"), tr("Fit"), tr("Stretch"), tr("Center"), tr("Tile"), tr("Span (all monitors)")]);
    label(tr("Gallery order:"), COL_3, 212, COL_3_W);
    add_combo(IDC_SORT_COMBO, COL_3, 232, COL_3_W, &[tr("Name"), tr("Date (newest first)")]);
    label(tr("Change wallpaper automatically:"), COL_3, 274, COL_3_W);
    add_combo(IDC_ROTATION_COMBO, COL_3, 294, COL_3_W, &[
        tr("Never"), tr("Every 5 minutes"), tr("Every 15 minutes"), tr("Every 30 minutes"),
        tr("Every hour"), tr("Every 3 hours"), tr("Every 12 hours"), tr("Every day"),
    ]);
    label(tr("Wallpaper change:"), COL_3, 336, COL_3_W);
    add_combo(IDC_APPLY_MODE_COMBO, COL_3, 356, COL_3_W, &[tr("Windows, instant"), tr("Windows, fade"), tr("SWEPR canvas")]);
    // Efecto y duración de la transición del lienzo (en el orden de Transition::ALL).
    add_combo(IDC_TRANSITION_EFFECT_COMBO, COL_3, 398, 172, &[
        tr("Fade"), tr("Slide"), tr("Zoom in"), tr("Wipe"), tr("Circle"), tr("Random effect"),
    ]);
    add_combo(IDC_TRANSITION_COMBO, COL_3 + 180, 398, COL_3_W - 180, &[tr("0.3 s"), tr("0.6 s"), tr("1 s"), tr("1.5 s")]);
    add_checkbox_w("Preview on the desktop", IDC_LIVE_PREVIEW_CHK, COL_3, 432, COL_3_W);
    add_checkbox_w("Random order", IDC_SHUFFLE_CHK, COL_3, 462, COL_3_W);
    add_checkbox_w("Rotate only favorites", IDC_FAVORITES_ONLY_CHK, COL_3, 488, COL_3_W);
    add_checkbox_w("Different wallpaper on each monitor", IDC_PER_MONITOR_CHK, COL_3, 514, COL_3_W);
    add_checkbox_w("Play animated and video wallpapers", IDC_ANIMATED_CHK, COL_3, 548, COL_3_W);

    // Reloj del escritorio (en el orden de ClockStyle::ALL, ClockPosition::ALL y ClockSize::ALL).
    add_checkbox_w("Show a clock on the desktop", IDC_CLOCK_CHK, COL_4, 150, COL_3_W);
    label(tr("Clock style:"), COL_4, 186, COL_3_W);
    add_combo(IDC_CLOCK_STYLE_COMBO, COL_4, 206, COL_3_W, &[tr("Large"), tr("Pill"), tr("Analog"), tr("Greeting"), tr("Stacked"), tr("Collage")]);
    label(tr("Position:"), COL_4, 248, COL_3_W);
    add_combo(IDC_CLOCK_POSITION_COMBO, COL_4, 268, COL_3_W, &[tr("In the center"), tr("Top left"), tr("Top right"), tr("On the left"), tr("On the right"), tr("Bottom left"), tr("Bottom right")]);
    label(tr("Size:"), COL_4, 310, COL_3_W);
    add_combo(IDC_CLOCK_SIZE_COMBO, COL_4, 330, COL_3_W, &[tr("Small"), tr("Medium"), tr("Big")]);
    add_checkbox_w("24-hour format", IDC_CLOCK_24H_CHK, COL_4, 376, COL_3_W);
    add_checkbox_w("Show seconds", IDC_CLOCK_SECONDS_CHK, COL_4, 406, COL_3_W);
    add_checkbox_w("Show the date", IDC_CLOCK_DATE_CHK, COL_4, 436, COL_3_W);
    add_checkbox_w("Only on the main monitor", IDC_CLOCK_MAIN_CHK, COL_4, 466, COL_3_W);
    label(tr("Needs the SWEPR canvas."), COL_4, 510, COL_3_W);
    style_button(add_control(hwnd, "BUTTON", &tr("Removed images..."), IDC_HIDDEN_BTN, COL_1, 562, COL_W - 30, 30, TAB_STOP), false, true);

    // Atajos de teclado
    let hotkeys = CONFIG_STATE.lock().ok().and_then(|g| g.clone())
        .and_then(|config| config.lock().ok().map(|config| config.hotkeys.clone()))
        .unwrap_or_default();
    for (index, (text, id, binding)) in [
        ("Open panel:", IDC_HOTKEY_PANEL, &hotkeys.open_panel),
        ("Previous wallpaper:", IDC_HOTKEY_PREVIOUS, &hotkeys.previous_wallpaper),
        ("Next wallpaper:", IDC_HOTKEY_NEXT, &hotkeys.next_wallpaper),
    ].into_iter().enumerate() {
        let x = COL_1 + index as i32 * 404;
        label(tr(text), x, 656, 380);
        let control = add_control(hwnd, super::hotkey_field::CLASS_NAME, &to_wide_chars(""), id, x, 676, 380, 32, TAB_STOP);
        let _ = SendMessageW(control, HKM_SETHOTKEY, WPARAM(binding.to_hotkey_value() as usize), LPARAM(0));
    }

    // Pie
    const SS_CENTERIMAGE: u32 = 0x0200;
    add_control(hwnd, "STATIC", &tr("Changes apply after saving."), IDC_FOOTER_NOTE, 16, 736, 972, 32, SS_CENTERIMAGE);
    style_button(add_control(hwnd, "BUTTON", &tr("Cancel"), IDC_CANCEL_BTN, 1004, 736, 102, 32, TAB_STOP), false, false);
    style_button(add_control(hwnd, "BUTTON", &tr("Save"), IDC_APPLY_BTN, 1114, 736, 102, 32, TAB_STOP), true, false);
}

/// Fondo de la ventana: tarjetas de cada sección y el marco del campo de la carpeta.
unsafe fn paint_background(hwnd: HWND, dc: HDC) {
    use windows::Win32::Foundation::RECT;
    let mut client = RECT::default();
    let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
    let _ = FillRect(dc, &client, super::theme::brush(super::theme::color_window_bg()));
    let title_font = CONFIG_TITLE_FONT.lock().ok().and_then(|g| *g).unwrap_or(0);
    for (title, x, y, width, height) in SECTIONS {
        let rect = RECT { left: px(x), top: px(y), right: px(x + width), bottom: px(y + height) };
        super::theme::draw_card(dc, &rect, crate::i18n::t(title).trim_end_matches(':'), title_font);
    }
    let (fx, fy, fw, fh) = FOLDER_FIELD;
    let field = RECT { left: px(fx), top: px(fy), right: px(fx + fw), bottom: px(fy + fh) };
    super::theme::draw_field(dc, &field, false, false);
}

/// Fuente de la interfaz (0 si no hay), para los controles dibujados a mano.
pub(super) fn ui_font() -> isize {
    CONFIG_FONT.lock().ok().and_then(|g| *g).unwrap_or(0)
}

/// Tamaño exterior de la ventana para que el área de cliente sea CLIENT_W x CLIENT_H.
unsafe fn window_size(dpi: u32) -> (i32, i32) {
    let mut rect = windows::Win32::Foundation::RECT { left: 0, top: 0, right: px(CLIENT_W), bottom: px(CLIENT_H) };
    let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
        &mut rect,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
        false,
        windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
        dpi,
    );
    (rect.right - rect.left, rect.bottom - rect.top)
}

fn post_suspend_hotkeys(suspend: bool) {
    if let Some(main_hwnd) = crate::app::MAIN_HWND.lock().ok().and_then(|g| *g) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                HWND(main_hwnd as *mut _),
                crate::app::WM_APP_SUSPEND_HOTKEYS,
                WPARAM(suspend as usize),
                LPARAM(0),
            );
        }
    }
}

/// Atajos de los controles, o None si dos coinciden.
unsafe fn read_hotkeys(hwnd: HWND) -> Option<crate::config::HotkeyConfig> {
    use crate::config::HotkeyBinding;
    let read = |id: i32| -> HotkeyBinding {
        let value = GetDlgItem(hwnd, id).map(|control| SendMessageW(control, HKM_GETHOTKEY, WPARAM(0), LPARAM(0)).0 as u16).unwrap_or(0);
        HotkeyBinding::from_hotkey_value(value)
    };
    let hotkeys = crate::config::HotkeyConfig {
        open_panel: read(IDC_HOTKEY_PANEL),
        previous_wallpaper: read(IDC_HOTKEY_PREVIOUS),
        next_wallpaper: read(IDC_HOTKEY_NEXT),
    };
    let all = [&hotkeys.open_panel, &hotkeys.previous_wallpaper, &hotkeys.next_wallpaper];
    for (i, a) in all.iter().enumerate() {
        if all[i + 1..].iter().any(|b| !a.is_empty() && a.same_as(b)) {
            return None;
        }
    }
    Some(hotkeys)
}

/// Índice seleccionado de un combo.
fn combo_selection(combo: HWND) -> usize {
    unsafe { SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize }
}

/// La barra de desplazamiento solo existe en el modo de bajo consumo.
unsafe fn update_mode_dependent_controls(hwnd: HWND) {
    let mode = GetDlgItem(hwnd, IDC_PANEL_MODE_COMBO).map(|combo| PanelMode::from_index(combo_selection(combo))).unwrap_or_default();
    let pro = mode == PanelMode::Pro;
    let strip = mode == PanelMode::LowPower;
    if let Ok(chk) = GetDlgItem(hwnd, IDC_SCROLLBAR_CHK) {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(chk, strip);
    }
    if let Ok(combo) = GetDlgItem(hwnd, IDC_PANEL_SIDE_COMBO) {
        // El carrusel va en el centro y el notch arriba: la posición solo vale para la tira.
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(combo, strip);
    }
    for id in [IDC_PRO_PRIORITY_COMBO, IDC_PRO_STYLE_COMBO] {
        if let Ok(combo) = GetDlgItem(hwnd, id) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(combo, pro);
        }
    }
    // "Extender" ya usa una sola imagen para todos los monitores.
    let span = GetDlgItem(hwnd, IDC_FIT_MODE_COMBO)
        .map(|combo| FitMode::ALL.get(combo_selection(combo)) == Some(&FitMode::Span))
        .unwrap_or(false);
    if let Ok(chk) = GetDlgItem(hwnd, IDC_PER_MONITOR_CHK) {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(chk, !span);
    }
    // La duración solo se usa con el lienzo.
    let canvas = GetDlgItem(hwnd, IDC_APPLY_MODE_COMBO).map(|combo| combo_selection(combo) == 2).unwrap_or(false);
    // Transición y vista previa solo existen con el lienzo.
    for id in [IDC_TRANSITION_EFFECT_COMBO, IDC_TRANSITION_COMBO, IDC_LIVE_PREVIEW_CHK, IDC_ANIMATED_CHK, IDC_CLOCK_CHK] {
        if let Ok(combo) = GetDlgItem(hwnd, id) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(combo, canvas);
        }
    }
    // Las opciones del reloj, con el reloj activado.
    let clock = canvas
        && GetDlgItem(hwnd, IDC_CLOCK_CHK)
            .map(|chk| SendMessageW(chk, windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1)
            .unwrap_or(false);
    for id in CLOCK_OPTIONS {
        if let Ok(control) = GetDlgItem(hwnd, id) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(control, clock);
        }
    }
}

/// Texto completo de un control.
unsafe fn window_text(control: HWND) -> String {
    let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW(control);
    if len <= 0 { return String::new(); }
    let mut buf = vec![0u16; (len + 1) as usize];
    let read = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(control, &mut buf);
    String::from_utf16_lossy(&buf[..read.max(0) as usize])
}

/// Aviso modal sobre la ventana de configuración.
unsafe fn show_message(hwnd: HWND, text: &str) {
    let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
        hwnd,
        as_pcwstr(&to_wide_chars(text)),
        as_pcwstr(&tr("SWEPR - Settings")),
        windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
    );
}

/// La ventana pasó a un monitor con otro DPI: nuevas fuentes y controles reescalados.
unsafe fn rescale_for_dpi(hwnd: HWND, new_dpi: u32, suggested: &windows::Win32::Foundation::RECT) {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindow, GetWindowRect, MoveWindow, SetWindowPos, GW_CHILD, GW_HWNDNEXT, SWP_NOACTIVATE, SWP_NOZORDER, WM_SETFONT};
    let new_dpi = new_dpi.max(96);
    let old_dpi = CONFIG_DPI.lock().map(|mut dpi| std::mem::replace(&mut *dpi, new_dpi)).unwrap_or(96);
    if old_dpi == new_dpi { return; }
    log_info!("Configuración: DPI {old_dpi} -> {new_dpi}");
    // Las fuentes viejas se borran cuando los controles ya usan las nuevas.
    let mut old_fonts = Vec::new();
    for (storage, title) in [(&CONFIG_FONT, false), (&CONFIG_TITLE_FONT, true)] {
        if let Ok(mut font) = storage.lock() {
            old_fonts.extend(font.take());
            *font = Some(create_ui_font(new_dpi, title)).filter(|f| *f != 0);
        }
    }
    let scale = |value: i32| (value as i64 * new_dpi as i64 / old_dpi as i64) as i32;
    let font = ui_font();
    let mut child = GetWindow(hwnd, GW_CHILD).unwrap_or_default();
    while !child.is_invalid() {
        let mut rect = windows::Win32::Foundation::RECT::default();
        let _ = GetWindowRect(child, &mut rect);
        let mut points = [
            windows::Win32::Foundation::POINT { x: rect.left, y: rect.top },
            windows::Win32::Foundation::POINT { x: rect.right, y: rect.bottom },
        ];
        windows::Win32::Graphics::Gdi::MapWindowPoints(None, hwnd, &mut points);
        let mut class = [0u16; 32];
        let len = GetClassNameW(child, &mut class).max(0) as usize;
        let is_combo = String::from_utf16_lossy(&class[..len]).eq_ignore_ascii_case("ComboBox");
        // La altura de un desplegable incluye su lista: se mantiene la de creación.
        let height = if is_combo { px(200) } else { scale(points[1].y - points[0].y) };
        let _ = MoveWindow(child, scale(points[0].x), scale(points[0].y), scale(points[1].x - points[0].x), height, false);
        if font != 0 {
            let _ = SendMessageW(child, WM_SETFONT, WPARAM(font as usize), LPARAM(0));
        }
        child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
    }
    for old in old_fonts {
        let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(old as *mut _));
    }
    let _ = SetWindowPos(
        hwnd, None, suggested.left, suggested.top, suggested.right - suggested.left, suggested.bottom - suggested.top,
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
}

unsafe extern "system" fn config_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        _ if msg == windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND => {
            paint_background(hwnd, HDC(wparam.0 as *mut _));
            LRESULT(1)
        }
        _ if msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORSTATIC
            || msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORBTN =>
        {
            use super::theme::{brush, color_card_bg, color_label, color_muted, color_window_bg};
            let dc = HDC(wparam.0 as *mut _);
            // Todo va sobre una tarjeta salvo la nota del pie.
            let footer = windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(HWND(lparam.0 as *mut _)) == IDC_FOOTER_NOTE;
            let _ = SetTextColor(dc, if footer { color_muted() } else { color_label() });
            let _ = SetBkMode(dc, TRANSPARENT);
            LRESULT(brush(if footer { color_window_bg() } else { color_card_bg() }).0 as isize)
        }
        _ if msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLOREDIT
            || msg == windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLORLISTBOX =>
        {
            use super::theme::{brush, color_field_bg, color_text};
            let dc = HDC(wparam.0 as *mut _);
            let _ = SetTextColor(dc, color_text());
            let _ = SetBkColor(dc, color_field_bg());
            let _ = SetBkMode(dc, OPAQUE);
            LRESULT(brush(color_field_bg()).0 as isize)
        }
        WM_APP_FOLDER_PICKED => {
            BROWSE_OPEN.store(false, std::sync::atomic::Ordering::Release);
            if let Ok(button) = GetDlgItem(hwnd, IDC_FOLDER_BTN) {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(button, true);
            }
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
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_APPLY_MODE_COMBO) {
                        let selection = match (config.apply_mode, config.wallpaper_fade) {
                            (ApplyMode::Canvas, _) => 2,
                            (ApplyMode::Windows, true) => 1,
                            (ApplyMode::Windows, false) => 0,
                        };
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selection), LPARAM(0));
                    }
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_TRANSITION_EFFECT_COMBO) {
                        let selection = Transition::ALL.iter().position(|t| *t == config.transition).unwrap_or(0);
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selection), LPARAM(0));
                    }
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_TRANSITION_COMBO) {
                        // La más parecida a la guardada (puede venir escrita a mano en el JSON).
                        let selection = TRANSITION_MS.iter().enumerate()
                            .min_by_key(|(_, ms)| ms.abs_diff(config.transition_ms))
                            .map(|(index, _)| index)
                            .unwrap_or(1);
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selection), LPARAM(0));
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
                        let selection = config.panel_mode.index();
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selection), LPARAM(0));
                    }
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_PRO_PRIORITY_COMBO) {
                        let selection = if config.pro_priority == ProPriority::Performance { 1 } else { 0 };
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selection), LPARAM(0));
                    }
                    update_mode_dependent_controls(hwnd);
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_THUMB_LOADING_COMBO) {
                        let selection = if config.thumb_loading == ThumbLoading::OnDemand { 1 } else { 0 };
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selection), LPARAM(0));
                    }
                    if let Ok(chk) = GetDlgItem(hwnd, IDC_SCROLLBAR_CHK) {
                        if !chk.is_invalid() {
                            use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
                            let check = if config.show_scrollbar { 1usize } else { 0usize };
                            let _ = SendMessageW(chk, BM_SETCHECK, WPARAM(check), LPARAM(0));
                        }
                    }
                    if let Ok(combo) = GetDlgItem(hwnd, IDC_THUMB_SIZE_COMBO) {
                        // Un tamaño escrito a mano en el JSON que no esté en la lista deja el desplegable vacío y no se cambia.
                        let selected = THUMB_SIZES.iter().position(|size| *size == config.thumb_size_value()).map_or(usize::MAX, |index| index);
                        let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(selected), LPARAM(0));
                    }
                    let set_combo = |id: i32, index: Option<usize>| {
                        if let Ok(combo) = GetDlgItem(hwnd, id) {
                            let _ = SendMessageW(combo, windows::Win32::UI::WindowsAndMessaging::CB_SETCURSEL, WPARAM(index.unwrap_or(usize::MAX)), LPARAM(0));
                        }
                    };
                    let set_check = |id: i32, checked: bool| {
                        if let Ok(chk) = GetDlgItem(hwnd, id) {
                            let _ = SendMessageW(chk, windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK, WPARAM(checked as usize), LPARAM(0));
                        }
                    };
                    set_combo(IDC_FIT_MODE_COMBO, FitMode::ALL.iter().position(|mode| *mode == config.fit_mode));
                    set_combo(IDC_SORT_COMBO, Some(if config.sort_by == SortOrder::Date { 1 } else { 0 }));
                    // Un intervalo escrito a mano que no esté en la lista deja el desplegable vacío y no se cambia.
                    set_combo(IDC_ROTATION_COMBO, ROTATION_MINUTES.iter().position(|minutes| *minutes == config.rotation_minutes));
                    set_check(IDC_SHUFFLE_CHK, config.shuffle);
                    set_check(IDC_FAVORITES_ONLY_CHK, config.rotation_favorites_only);
                    set_check(IDC_PER_MONITOR_CHK, config.per_monitor);
                    set_combo(IDC_PRO_STYLE_COMBO, Some(config.pro_style.index()));
                    set_check(IDC_NOTCH_NOTIFY_CHK, config.notch_notify);
                    set_check(IDC_LIVE_PREVIEW_CHK, config.live_preview);
                    set_check(IDC_SHOW_NAMES_CHK, config.show_names);
                    set_check(IDC_ANIMATED_CHK, config.animated_wallpapers);
                    set_check(IDC_CLOCK_CHK, config.clock.enabled);
                    set_combo(IDC_CLOCK_STYLE_COMBO, Some(config.clock.style.index()));
                    set_combo(IDC_CLOCK_POSITION_COMBO, Some(config.clock.position.index()));
                    set_combo(IDC_CLOCK_SIZE_COMBO, Some(config.clock.size.index()));
                    set_check(IDC_CLOCK_24H_CHK, config.clock.hour24);
                    set_check(IDC_CLOCK_SECONDS_CHK, config.clock.seconds);
                    set_check(IDC_CLOCK_DATE_CHK, config.clock.date);
                    set_check(IDC_CLOCK_MAIN_CHK, config.clock.main_monitor_only);
                    set_combo(IDC_THEME_COMBO, Some(config.theme.index()));
                    update_mode_dependent_controls(hwnd);
                }
            }

            LRESULT(0)
        }
        WM_DPICHANGED => {
            rescale_for_dpi(hwnd, (wparam.0 & 0xFFFF) as u32, &*(lparam.0 as *const windows::Win32::Foundation::RECT));
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = (wparam.0 & 0xFFFF) as i32;
            match cmd {
                IDC_FOLDER_BTN => {
                    // Solo un diálogo a la vez: el botón queda desactivado hasta que se cierre.
                    if BROWSE_OPEN.swap(true, std::sync::atomic::Ordering::AcqRel) {
                        return LRESULT(0);
                    }
                    log_info!("Botón 'Examinar...' presionado");
                    if let Ok(button) = GetDlgItem(hwnd, IDC_FOLDER_BTN) {
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(button, false);
                    }
                    // El hilo principal es MTA: el diálogo necesita un hilo STA propio.
                    let hwnd_val = hwnd.0 as isize;
                    std::thread::spawn(move || {
                        unsafe {
                            use windows::Win32::UI::Shell::{IFileOpenDialog, FOS_PICKFOLDERS, FOS_FORCEFILESYSTEM, FileOpenDialog, SIGDN_FILESYSPATH};
                            use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};

                            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                            let hwnd = HWND(hwnd_val as *mut _);
                            if let Ok(dialog) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) {
                                let options = dialog.GetOptions().unwrap_or_default();
                                let _ = dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
                                let dialog_title = tr("Select wallpaper folder");
                                let _ = dialog.SetTitle(windows::core::PCWSTR(dialog_title.as_ptr()));
                                if dialog.Show(hwnd).is_ok() {
                                    if let Ok(item) = dialog.GetResult() {
                                        if let Ok(path_ptr) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                                            let path_str = path_ptr.to_string().unwrap_or_default();
                                            CoTaskMemFree(Some(path_ptr.0 as *const _));
                                            log_info!("Carpeta seleccionada: {}", path_str);
                                            if let Ok(mut g) = PENDING_FOLDER_PATH.lock() {
                                                *g = Some(path_str);
                                            }
                                        }
                                    }
                                }
                            }
                            // Siempre avisar, aunque se cancele, para volver a activar el botón.
                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd, WM_APP_FOLDER_PICKED, WPARAM(0), LPARAM(0));
                            CoUninitialize();
                        }
                    });
                }
                IDC_APPLY_BTN => {
                    log_info!("Botón 'Aplicar' presionado");
                    let Some(hotkeys) = read_hotkeys(hwnd) else {
                        let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                            hwnd,
                            as_pcwstr(&tr("Two shortcuts use the same key combination.")),
                            as_pcwstr(&tr("SWEPR - Settings")),
                            windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
                        );
                        return LRESULT(0);
                    };
                    // La carpeta debe existir: si no, se avisa y no se guarda nada.
                    let folder_text = GetDlgItem(hwnd, IDC_FOLDER_EDIT).map(|edit| window_text(edit)).unwrap_or_default();
                    let folder_text = folder_text.trim().to_string();
                    if !folder_text.is_empty() && !std::path::Path::new(&folder_text).is_dir() {
                        show_message(hwnd, &crate::i18n::t("The wallpaper folder does not exist. Choose an existing folder."));
                        return LRESULT(0);
                    }
                    let mut save_error = None;
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
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_APPLY_MODE_COMBO) {
                                match combo_selection(combo) {
                                    0 => { config.apply_mode = ApplyMode::Windows; config.wallpaper_fade = false; }
                                    1 => { config.apply_mode = ApplyMode::Windows; config.wallpaper_fade = true; }
                                    2 => config.apply_mode = ApplyMode::Canvas,
                                    _ => {}
                                }
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_TRANSITION_EFFECT_COMBO) {
                                if let Some(transition) = Transition::ALL.get(combo_selection(combo)) {
                                    config.transition = *transition;
                                }
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_TRANSITION_COMBO) {
                                if let Some(ms) = TRANSITION_MS.get(combo_selection(combo)) {
                                    config.transition_ms = *ms;
                                }
                            }
                            if let Ok(chk) = GetDlgItem(hwnd, IDC_AUTOSTART_CHK) {
                                if !chk.is_invalid() {
                                    use windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK;
                                    let checked = SendMessageW(chk, BM_GETCHECK, WPARAM(0), LPARAM(0));
                                    config.autostart = checked.0 == 1;
                                }
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_THUMB_SIZE_COMBO) {
                                if let Some(size) = THUMB_SIZES.get(combo_selection(combo)) {
                                    config.thumb_size = *size;
                                }
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_THUMB_LOADING_COMBO) {
                                config.thumb_loading = if combo_selection(combo) == 1 { ThumbLoading::OnDemand } else { ThumbLoading::Preload };
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
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_PANEL_MODE_COMBO) {
                                config.panel_mode = PanelMode::from_index(combo_selection(combo));
                            }
                            config.zones.active_zones = config.panel_side.active_zones(config.panel_mode);
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_PRO_PRIORITY_COMBO) {
                                config.pro_priority = if combo_selection(combo) == 1 { ProPriority::Performance } else { ProPriority::Smooth };
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
                            config.hotkeys = hotkeys;
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_FIT_MODE_COMBO) {
                                if let Some(mode) = FitMode::ALL.get(combo_selection(combo)) {
                                    config.fit_mode = *mode;
                                }
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_SORT_COMBO) {
                                config.sort_by = if combo_selection(combo) == 1 { SortOrder::Date } else { SortOrder::Name };
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_ROTATION_COMBO) {
                                if let Some(minutes) = ROTATION_MINUTES.get(combo_selection(combo)) {
                                    config.rotation_minutes = *minutes;
                                }
                            }
                            let checked = |id: i32| GetDlgItem(hwnd, id)
                                .map(|chk| SendMessageW(chk, windows::Win32::UI::WindowsAndMessaging::BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1)
                                .unwrap_or(false);
                            config.shuffle = checked(IDC_SHUFFLE_CHK);
                            config.rotation_favorites_only = checked(IDC_FAVORITES_ONLY_CHK);
                            config.per_monitor = checked(IDC_PER_MONITOR_CHK);
                            config.notch_notify = checked(IDC_NOTCH_NOTIFY_CHK);
                            config.live_preview = checked(IDC_LIVE_PREVIEW_CHK);
                            config.show_names = checked(IDC_SHOW_NAMES_CHK);
                            config.animated_wallpapers = checked(IDC_ANIMATED_CHK);
                            config.clock.enabled = checked(IDC_CLOCK_CHK);
                            config.clock.hour24 = checked(IDC_CLOCK_24H_CHK);
                            config.clock.seconds = checked(IDC_CLOCK_SECONDS_CHK);
                            config.clock.date = checked(IDC_CLOCK_DATE_CHK);
                            config.clock.main_monitor_only = checked(IDC_CLOCK_MAIN_CHK);
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_CLOCK_STYLE_COMBO) {
                                config.clock.style = ClockStyle::from_index(combo_selection(combo));
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_CLOCK_POSITION_COMBO) {
                                config.clock.position = ClockPosition::from_index(combo_selection(combo));
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_CLOCK_SIZE_COMBO) {
                                config.clock.size = ClockSize::from_index(combo_selection(combo));
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_THEME_COMBO) {
                                config.theme = Theme::from_index(combo_selection(combo));
                            }
                            if let Ok(combo) = GetDlgItem(hwnd, IDC_PRO_STYLE_COMBO) {
                                config.pro_style = ProStyle::from_index(combo_selection(combo));
                            }
                            let autostart_enabled = config.autostart;
                            if let Err(e) = config.save() {
                                crate::log_error!("No se pudo guardar la configuración: {e}");
                                save_error = Some(e);
                            }
                            if let Err(e) = crate::config::autostart::set_autostart(autostart_enabled) {
                                crate::log_warn!("No se pudo sincronizar autoarranque: {e}");
                            }
                            log_info!("Configuración guardada");
                        }
                    }
                    if let Some(e) = save_error {
                        // Los cambios ya están en memoria y se aplican, pero se perderán al reiniciar.
                        show_message(hwnd, &format!("{}\n\n{e}", crate::i18n::t("Settings could not be saved to disk. They will apply until SWEPR is closed.")));
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
                IDC_HIDDEN_BTN => {
                    if let Some(config) = CONFIG_STATE.lock().ok().and_then(|g| g.clone()) {
                        super::hidden::show(hwnd, config);
                    }
                }
                IDC_THEME_COMBO => {
                    // CBN_SELCHANGE: el tema se ve al momento; si no se guarda, se vuelve al anterior al cerrar.
                    if (wparam.0 >> 16) & 0xFFFF == 1 {
                        if let Ok(combo) = GetDlgItem(hwnd, IDC_THEME_COMBO) {
                            crate::winutil::set_theme(Theme::from_index(combo_selection(combo)).code());
                        }
                        super::theme::apply_window_frame(hwnd);
                        use windows::Win32::Graphics::Gdi::{RedrawWindow, RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE};
                        let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME);
                    }
                }
                IDC_CLOCK_CHK => {
                    // BN_CLICKED
                    if (wparam.0 >> 16) & 0xFFFF == 0 {
                        update_mode_dependent_controls(hwnd);
                    }
                }
                IDC_PANEL_MODE_COMBO | IDC_FIT_MODE_COMBO | IDC_APPLY_MODE_COMBO => {
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
            // El tema guardado (el que se probó en el desplegable se descarta si no se guardó).
            if let Some(theme) = CONFIG_STATE.lock().ok().and_then(|g| g.clone()).and_then(|c| c.lock().ok().map(|c| c.theme)) {
                crate::winutil::set_theme(theme.code());
            }
            // Tras guardar ya está la configuración nueva: los atajos se registran con ella.
            post_suspend_hotkeys(false);
            for storage in [&CONFIG_FONT, &CONFIG_TITLE_FONT] {
                if let Some(font) = storage.lock().ok().and_then(|mut font| font.take()) {
                    let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(font as *mut _));
                }
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
            Err(_) => {
                post_suspend_hotkeys(false);
                return;
            }
        };
        let class_name_wide = to_wide_chars(CONFIG_CLASS_NAME);
        let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(config_wnd_proc),
            hInstance: hinstance,
            hIcon: crate::resources::app_icon(),
            hbrBackground: super::theme::brush(super::theme::color_window_bg()),
            lpszClassName: as_pcwstr(&class_name_wide),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        // Establecer CONFIG_STATE ANTES de crear la ventana (WM_CREATE lo necesita)
        if let Ok(mut g) = CONFIG_STATE.lock() { *g = Some(config.clone()); }
        let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem().max(96);
        if let Ok(mut g) = CONFIG_DPI.lock() { *g = dpi; }
        if let Ok(mut g) = CONFIG_FONT.lock() { *g = Some(create_ui_font(dpi, false)).filter(|font| *font != 0); }
        if let Ok(mut g) = CONFIG_TITLE_FONT.lock() { *g = Some(create_ui_font(dpi, true)).filter(|font| *font != 0); }
        super::hotkey_field::register_class(hinstance);
        // Mientras la ventana esté abierta los atajos no se registran: así el campo de
        // atajos recibe la combinación aunque sea la que ya está en uso.
        post_suspend_hotkeys(true);

        // Que la ventana quepa en el área de trabajo en pantallas bajas con escalado alto.
        let mut work = windows::Win32::Foundation::RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETWORKAREA, 0, Some(&mut work as *mut _ as *mut _),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let (width, height) = window_size(dpi);
        let top = px(100).min(work.bottom - height).max(work.top);

        let hwnd = match CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            as_pcwstr(&class_name_wide),
            as_pcwstr(&tr("SWEPR - Settings")),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            px(100), top, width, height,
            None, None, hinstance, None,
        ) {
            Ok(h) => h,
            Err(_) => {
                post_suspend_hotkeys(false);
                return;
            }
        };

        super::theme::apply_window_frame(hwnd);
        if let Ok(mut g) = CONFIG_HWND.lock() { *g = Some(hwnd.0 as isize); }
        let _ = ShowWindow(hwnd, SW_SHOW);
        log_info!("Ventana de configuración mostrada");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dibuja la ventana de configuración sin mostrarla y la guarda en un BMP
    /// (`SWEPR_SETTINGS_PREVIEW=ruta.bmp cargo test render_settings -- --ignored`).
    #[test]
    #[ignore]
    fn render_settings_preview() {
        if std::env::var("SWEPR_TEST_LIGHT").is_ok() { crate::winutil::set_theme(2); }
        use windows::Win32::Graphics::Gdi::{CreateCompatibleBitmap, CreateCompatibleDC, GetDC, ReleaseDC, SelectObject};
        let path = std::env::var("SWEPR_SETTINGS_PREVIEW").unwrap_or_else(|_| "settings_preview.bmp".into());
        unsafe {
            let hinstance = HINSTANCE(GetModuleHandleW(None).unwrap().0);
            let class_name = to_wide_chars("SWEPR_ConfigPreviewClass");
            let wc = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
                lpfnWndProc: Some(config_wnd_proc),
                hInstance: hinstance,
                lpszClassName: as_pcwstr(&class_name),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            *CONFIG_STATE.lock().unwrap() = Some(Arc::new(Mutex::new(AppConfig::default())));
            // SWEPR_SETTINGS_DPI=144 para ver la escala de 150 %.
            let dpi = std::env::var("SWEPR_SETTINGS_DPI").ok().and_then(|v| v.parse().ok()).unwrap_or(96u32);
            *CONFIG_DPI.lock().unwrap() = dpi;
            *CONFIG_FONT.lock().unwrap() = Some(create_ui_font(dpi, false)).filter(|font| *font != 0);
            *CONFIG_TITLE_FONT.lock().unwrap() = Some(create_ui_font(dpi, true)).filter(|font| *font != 0);
            let (width, height) = window_size(dpi);
            super::super::hotkey_field::register_class(hinstance);
            // Fuera de la pantalla, sin activarse ni aparecer en la barra de tareas.
            let hwnd = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW | windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE,
                as_pcwstr(&class_name), as_pcwstr(&tr("SWEPR - Settings")),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN, -10000, -10000, width, height, None, None, hinstance, None,
            ).unwrap();
            let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE);
            let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
            // Dar tiempo a DWM a componer la ventana antes de capturarla.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
            let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
            while std::time::Instant::now() < deadline {
                while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool() {
                    let _ = windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let mut rect = windows::Win32::Foundation::RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, rect.right, rect.bottom);
            SelectObject(dc, bmp);
            // PW_CLIENTONLY | PW_RENDERFULLCONTENT: tal como lo compone DWM, aunque esté fuera de pantalla.
            let _ = windows::Win32::Storage::Xps::PrintWindow(hwnd, dc, windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(1 | 2));
            crate::panel::save_bmp(dc, bmp, rect.right, rect.bottom, &path);
            ReleaseDC(None, screen);
            let _ = DestroyWindow(hwnd);
        }
    }
}
