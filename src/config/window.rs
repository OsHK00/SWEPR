use crate::config::AppConfig;
use crate::log_info;
use crate::winutil::{as_pcwstr, to_wide_chars};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
    CS_HREDRAW, CS_VREDRAW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
    GetDlgItem, SendMessageW, SetWindowTextW,
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
const IDC_THUMB_CHK_64: i32 = 1010;
const IDC_THUMB_CHK_128: i32 = 1011;
const IDC_THUMB_CHK_256: i32 = 1012;
const IDC_THUMB_CHK_512: i32 = 1013;

static CONFIG_HWND: Mutex<Option<isize>> = Mutex::new(None);
static CONFIG_STATE: Mutex<Option<Arc<Mutex<AppConfig>>>> = Mutex::new(None);
static PENDING_FOLDER_PATH: Mutex<Option<String>> = Mutex::new(None);

fn hmenu(id: i32) -> windows::Win32::UI::WindowsAndMessaging::HMENU {
    windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut _)
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
            let x = 20i32;
            let mut y = 20i32;
            let w = 440i32;
            let h = 24i32;

            // Carpeta label
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("STATIC")),
                as_pcwstr(&to_wide_chars("Carpeta de fondos:")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE,
                x, y, w, 16, hwnd, windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()), None, None,
            );
            y += 20;
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("EDIT")),
                as_pcwstr(&to_wide_chars("")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WS_BORDER,
                x, y, w - 80, h, hwnd, hmenu(IDC_FOLDER_EDIT), None, None,
            );
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Examinar...")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE ,
                x + w - 70, y, 70, h, hwnd, hmenu(IDC_FOLDER_BTN), None, None,
            );
            y += 35;

            // Detección activada
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Detección activada")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(3),
                x, y, w, h, hwnd, hmenu(IDC_DETECTION_CHK), None, None,
            );
            y += 30;

            // Subcarpetas
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Escanear subcarpetas")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(3),
                x, y, w, h, hwnd, hmenu(IDC_SUBFOLDERS_CHK), None, None,
            );
            y += 30;

            // Autoarranque
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Autoarranque")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(3),
                x, y, w, h, hwnd, hmenu(IDC_AUTOSTART_CHK), None, None,
            );
            y += 40;

            // Calidad / Detalle de miniatura (grupo de radios)
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("STATIC")),
                as_pcwstr(&to_wide_chars("Calidad de miniaturas (ahorro de RAM):")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE,
                x, y, w, 16, hwnd, windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()), None, None,
            );
            y += 20;
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Pequena (64 px) — Menor uso de memoria")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(4),
                x, y, w, 24, hwnd, hmenu(IDC_THUMB_CHK_64), None, None,
            );
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Estandar (128 px) — Recomendado")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(4),
                x, y + 24, w, 24, hwnd, hmenu(IDC_THUMB_CHK_128), None, None,
            );
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Grande (256 px) — Mayor nitidez")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(4),
                x, y + 48, w, 24, hwnd, hmenu(IDC_THUMB_CHK_256), None, None,
            );
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Muy grande (512 px) — Maximo detalle")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(4),
                x, y + 72, w, 24, hwnd, hmenu(IDC_THUMB_CHK_512), None, None,
            );
            y += 110;

            // Nota sobre guardado
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("STATIC")),
                as_pcwstr(&to_wide_chars("El cambio de calidad de miniatura se aplica al guardar.")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE,
                x, y, w, 16, hwnd, windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()), None, None,
            );
            y += 24;

            // Botones
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Guardar")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE ,
                x, y, 90, 30, hwnd, hmenu(IDC_APPLY_BTN), None, None,
            );
            let _ = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&to_wide_chars("BUTTON")),
                as_pcwstr(&to_wide_chars("Cancelar")),
                windows::Win32::UI::WindowsAndMessaging::WS_CHILD | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE ,
                x + 100, y, 90, 30, hwnd, hmenu(IDC_CANCEL_BTN), None, None,
            );

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
                                let _ = dialog.SetTitle(windows::core::PCWSTR(to_wide_chars("Seleccionar carpeta de fondos").as_ptr()));
                                
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
            lpszClassName: as_pcwstr(&class_name_wide),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        // Establecer CONFIG_STATE ANTES de crear la ventana (WM_CREATE lo necesita)
        if let Ok(mut g) = CONFIG_STATE.lock() { *g = Some(config.clone()); }

        let hwnd = match CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            as_pcwstr(&class_name_wide),
            as_pcwstr(&to_wide_chars("SWEPR - Configuración")),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
            100, 100, 540, 440,
            None, None, hinstance, None,
        ) {
            Ok(h) => h,
            Err(_) => return,
        };

        if let Ok(mut g) = CONFIG_HWND.lock() { *g = Some(hwnd.0 as isize); }
        log_info!("Ventana de configuración mostrada");
    }
}
