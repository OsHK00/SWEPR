use crate::config::AppConfig;
use crate::detector::focus::{evaluate_window_focus, FocusDetector, WM_APP_FOREGROUND_CHANGED};
use crate::detector::mouse::MouseDetector;
use crate::detector::zones::{ZoneDetector, ZONE_TIMER_ID};
use crate::log_error;
use crate::log_info;
use crate::log_resource_usage;
use crate::log_warn;
use crate::panel;
use crate::scanner::{Scanner, WM_APP_SCAN_COMPLETE};
use crate::thumbs::{ThumbnailCache, WM_APP_THUMB_READY};
use crate::tray::{TrayIcon, IDM_EXIT, IDM_OPEN_FOLDER, IDM_REFRESH_GALLERY, IDM_SETTINGS, IDM_TOGGLE_DETECTION, WM_APP_TRAY_NOTIFICATION};
use crate::config::window;
use crate::wallpaper::{WallpaperManager, WallpaperStyle};
use crate::watcher::{DirectoryWatcher, WM_APP_WATCHER_CHANGED};
use crate::winutil::{as_pcwstr, to_wide_chars};

// Mensaje para aplicar wallpaper desde el panel
pub const WM_APP_APPLY_WALLPAPER: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 40;

// WM_APP para solicitar generación de miniaturas
pub const WM_APP_GEN_THUMBS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 41;

// WM_APP para notificar cambio de configuración
pub const WM_APP_CONFIG_CHANGED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 42;

// Timer periódico para medición de recursos
pub const RESOURCE_TIMER_ID: usize = 9002;

// Función para solicitar aplicación de wallpaper desde el panel
pub fn request_apply_wallpaper(path: &std::path::PathBuf) {
    // Encontrar la ventana principal y enviar PostMessage
    // Por simplicidad, guardamos el path en un static y enviamos el mensaje
    PENDING_WALLPAPER_PATH.lock().unwrap().replace(path.clone());
    // Buscar HWND de la ventana principal - la guardamos en un static
    if let Some(hwnd_val) = MAIN_HWND.lock().ok().and_then(|g| *g) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                windows::Win32::Foundation::HWND(hwnd_val as *mut _),
                WM_APP_APPLY_WALLPAPER,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    }
}

static PENDING_WALLPAPER_PATH: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
pub static MAIN_HWND: Mutex<Option<isize>> = Mutex::new(None);

pub fn set_main_hwnd(hwnd: windows::Win32::Foundation::HWND) {
    if let Ok(mut g) = MAIN_HWND.lock() {
        *g = Some(hwnd.0 as isize);
    }
}
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetWindowLongPtrW, TranslateMessage, UnregisterClassW, GWLP_USERDATA, MSG, WNDCLASSW,
    CS_HREDRAW, CS_VREDRAW, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_ENDSESSION, WM_INPUT, WM_LBUTTONUP, WM_POWERBROADCAST, WM_QUERYENDSESSION,
    WM_RBUTTONUP, WM_TIMER, WS_OVERLAPPED,
};

const WINDOW_CLASS_NAME: &str = "SWEPR_MainWindowClass";
const WINDOW_TITLE: &str = "SWEPR_BackgroundHost";

const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;

/// Estado mutable de la aplicación accesible desde el procedimiento de ventana.
use std::sync::{Arc, Mutex};
use crate::scanner::ScannedImage;
const WATCHER_DEBOUNCE_TIMER_ID: usize = 9001;

pub struct AppState {
    pub config: Arc<Mutex<AppConfig>>,
    pub mouse_detector: MouseDetector,
    pub zone_detector: ZoneDetector,
    pub tray_icon: TrayIcon,
    pub is_desktop_focused: bool,
    pub taskbar_created_msg: u32,
    pub wallpaper_manager: WallpaperManager,
    pub scanner: Scanner,
    pub scanned_images: Arc<Mutex<Vec<ScannedImage>>>,
    pub last_scan_count: usize,
    pub thumb_cache: ThumbnailCache,
    pub directory_watcher: DirectoryWatcher,
}

/// Procedimiento de ventana principal para el despacho de mensajes Win32.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;

    if !state_ptr.is_null() {
        let state = &mut *state_ptr;

        // Reinicio del explorador de Windows: restaurar el icono en la bandeja y reevaluar foco
        if msg == state.taskbar_created_msg && state.taskbar_created_msg != 0 {
            log_info!("Mensaje TaskbarCreated detectado (explorer.exe reiniciado). Restaurando icono de bandeja...");
            let _ = state.tray_icon.re_add();

            let fg = crate::winutil::get_current_foreground_window();
            let is_desktop = evaluate_window_focus(fg);
            state.is_desktop_focused = is_desktop;
            if state.config.lock().unwrap().detection_enabled && is_desktop {
                let _ = state.mouse_detector.register();
            } else {
                let _ = state.mouse_detector.unregister();
                state.zone_detector.reset();
            }
            return LRESULT(0);
        }

        match msg {
            WM_APP_TRAY_NOTIFICATION => {
                let mouse_msg = (lparam.0 & 0xFFFF) as u32;
                if mouse_msg == WM_RBUTTONUP || mouse_msg == WM_CONTEXTMENU {
                    let selected = state.tray_icon.show_context_menu(state.config.lock().unwrap().detection_enabled);
                    if let Some(cmd_id) = selected {
                        match cmd_id {
                            IDM_TOGGLE_DETECTION => {
                                let mut config = state.config.lock().unwrap();
                                config.detection_enabled = !config.detection_enabled;
                                log_info!(
                                    "Acción de menú de bandeja: Detección cambiada a: {}",
                                    if config.detection_enabled { "ACTIVADA" } else { "DESACTIVADA" }
                                );
                                let detection_enabled = config.detection_enabled;
                                let _ = config.save();
                                drop(config);

                                if detection_enabled {
                                    if state.is_desktop_focused {
                                        let _ = state.mouse_detector.register();
                                    }
                                } else {
                                    let _ = state.mouse_detector.unregister();
                                    state.zone_detector.reset();
                                }
                            }
                            IDM_REFRESH_GALLERY => {
                                log_info!("Acción de menú de bandeja: 'Actualizar galería' seleccionada. Iniciando escaneo...");
                                let config = state.config.lock().unwrap();
                                let by_date = config.sort_by.eq_ignore_ascii_case("date");
                                let wallpaper_dir = config.wallpaper_dir.clone();
                                let scan_subfolders = config.scan_subfolders;
                                drop(config);
                                state.scanner.scan_folder(
                                    hwnd,
                                    &wallpaper_dir,
                                    scan_subfolders,
                                    by_date,
                                    state.scanned_images.clone(),
                                );
                            }
                            IDM_OPEN_FOLDER => {
                                let wallpaper_dir = state.config.lock().unwrap().wallpaper_dir.clone();
                                log_info!(
                                    "Acción de menú de bandeja: 'Abrir carpeta' seleccionada. Carpeta: {:?}",
                                    wallpaper_dir
                                );
                                let _ = std::process::Command::new("explorer")
                                    .arg(&wallpaper_dir)
                                    .spawn();
                            }
                            IDM_SETTINGS => {
                                log_info!("Acción de menú de bandeja: 'Configuración...' seleccionada.");
                                window::show_config_window(state.config.clone());
                            }
                            IDM_EXIT => {
                                log_info!("Acción de menú de bandeja: 'Salir' seleccionada por el usuario.");
                                let _ = DestroyWindow(hwnd);
                            }
                            _ => {}
                        }
                    }
                } else if mouse_msg == WM_LBUTTONUP {
                    // Clic izquierdo en icono de bandeja reservado
                }
                return LRESULT(0);
            }
            WM_APP_FOREGROUND_CHANGED => {
                let focused_hwnd = HWND(wparam.0 as *mut _);
                let is_desktop = evaluate_window_focus(focused_hwnd);

                if is_desktop && !state.tray_icon.is_added() {
                    let _ = state.tray_icon.add();
                }

                if is_desktop != state.is_desktop_focused {
                    state.is_desktop_focused = is_desktop;
                    if state.config.lock().unwrap().detection_enabled && is_desktop {
                        let _ = state.mouse_detector.register();
                    } else {
                        let _ = state.mouse_detector.unregister();
                        state.zone_detector.reset();
                    }
                }
                return LRESULT(0);
            }
            WM_INPUT => {
                if state.config.lock().unwrap().detection_enabled && state.is_desktop_focused {
                    if let Some(pt) = crate::winutil::get_cursor_position() {
                        state.zone_detector.on_mouse_move(pt);
                    }
                }
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            // WM_TIMER se maneja en el match anterior con debounce + zonas
            WM_APP_SCAN_COMPLETE => {
                state.last_scan_count = wparam.0 as usize;
                log_info!("Escaneo completado - {} imágenes listas en memoria.", state.last_scan_count);
                panel::set_draw_context(state.thumb_cache.clone(), state.scanned_images.clone());
                panel::request_repaint();
                // Generar miniaturas en segundo plano
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        hwnd,
                        WM_APP_GEN_THUMBS,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                }
                return LRESULT(0);
            }
            WM_APP_THUMB_READY => {
                if panel::is_panel_visible() {
                    panel::request_repaint();
                }
                return LRESULT(0);
            }
            WM_APP_WATCHER_CHANGED => {
                // Debounce: cancelar timer anterior si existe y programar uno nuevo
                unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, WATCHER_DEBOUNCE_TIMER_ID); }
                unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(hwnd, WATCHER_DEBOUNCE_TIMER_ID, 400, None); }
                return LRESULT(0);
            }
            WM_APP_APPLY_WALLPAPER => {
                // Aplicar wallpaper solicitado por el panel
                if let Some(path) = PENDING_WALLPAPER_PATH.lock().ok().and_then(|mut g| g.take()) {
                    log_info!("Aplicando wallpaper desde panel: {:?}", path);
                    let fit_mode = state.config.lock().unwrap().fit_mode.clone();
                    let style = WallpaperStyle::from_str(&fit_mode);
                    match state.wallpaper_manager.set_wallpaper(&path, None, style) {
                        Ok(_) => {
                            log_info!("Wallpaper aplicado correctamente.");
                            crate::panel::set_current_wallpaper(&path);
                        }
                        Err(e) => log_error!("Error al aplicar wallpaper: {e}"),
                    }
                }
                return LRESULT(0);
            }
            WM_APP_GEN_THUMBS => {
                // Generar miniaturas para todas las imágenes escaneadas en un solo hilo
                let thumb_size = state.config.lock().unwrap().thumb_size_value() as i32;
                let images: Vec<std::path::PathBuf> = state.scanned_images.lock().ok()
                    .map(|g| g.iter().map(|img| img.path.clone()).collect())
                    .unwrap_or_default();
                let hwnd_val = hwnd.0 as isize;
                let cache = state.thumb_cache.clone();
                std::thread::spawn(move || {
                    // Inicializar COM una sola vez para todo el lote de miniaturas del hilo
                    unsafe {
                        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
                        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                        for path in images {
                            crate::thumbs::request_thumbnail(
                                windows::Win32::Foundation::HWND(hwnd_val as *mut _),
                                &path,
                                thumb_size,
                                cache.clone(),
                            );
                        }
                        if hr.is_ok() {
                            CoUninitialize();
                        }
                    }
                });
                return LRESULT(0);
            }
            WM_APP_CONFIG_CHANGED => {
                log_info!("Configuración cambiada, limpiando caché y reescaneando...");
                // Limpiar caché de miniaturas
                state.thumb_cache.clear();
                // Limpiar imágenes escaneadas
                if let Ok(mut g) = state.scanned_images.lock() {
                    g.clear();
                }
                let (by_date, wallpaper_dir, scan_subfolders, detection_enabled, zones) = {
                    let config = state.config.lock().unwrap();
                    (
                        config.sort_by.eq_ignore_ascii_case("date"),
                        config.wallpaper_dir.clone(),
                        config.scan_subfolders,
                        config.detection_enabled,
                        config.zones.clone(),
                    )
                };
                state.zone_detector.update_config(zones);
                state.scanner.scan_folder(
                    hwnd,
                    &wallpaper_dir,
                    scan_subfolders,
                    by_date,
                    state.scanned_images.clone(),
                );
                // Reiniciar watcher en la nueva carpeta (si cambió)
                state.directory_watcher.watch(hwnd, &wallpaper_dir, scan_subfolders);
                if detection_enabled && state.is_desktop_focused {
                    let _ = state.mouse_detector.register();
                } else {
                    let _ = state.mouse_detector.unregister();
                    state.zone_detector.reset();
                }
                return LRESULT(0);
            }
            WM_TIMER => {
                if wparam.0 == ZONE_TIMER_ID {
                    state.zone_detector.on_timer(wparam.0);
                    return LRESULT(0);
                }
                if wparam.0 == WATCHER_DEBOUNCE_TIMER_ID {
                    unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, WATCHER_DEBOUNCE_TIMER_ID); }
                    log_info!("Watcher: cambios detectados en carpeta, reescaneando...");
                    let config = state.config.lock().unwrap();
                    let by_date = config.sort_by.eq_ignore_ascii_case("date");
                    let wallpaper_dir = config.wallpaper_dir.clone();
                    let scan_subfolders = config.scan_subfolders;
                    drop(config);
                    state.scanner.scan_folder(
                        hwnd,
                        &wallpaper_dir,
                        scan_subfolders,
                        by_date,
                        state.scanned_images.clone(),
                    );
                    return LRESULT(0);
                }
                if wparam.0 == RESOURCE_TIMER_ID {
                    log_resource_usage();
                    return LRESULT(0);
                }
            }
            WM_DISPLAYCHANGE => {
                log_info!("WM_DISPLAYCHANGE recibido: cambios en monitores o resolución.");
                state.zone_detector.reset();
                return LRESULT(0);
            }
            WM_POWERBROADCAST => {
                match wparam.0 as u32 {
                    PBT_APMSUSPEND => {
                        log_info!("WM_POWERBROADCAST: Suspensión detectada. Desactivando detectores.");
                        let _ = state.mouse_detector.unregister();
                        state.zone_detector.reset();
                        state.is_desktop_focused = false;
                    }
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                        log_info!("WM_POWERBROADCAST: Reanudación detectada. Reevaluando foco...");
                        let fg = crate::winutil::get_current_foreground_window();
                        let is_desktop = evaluate_window_focus(fg);
                        state.is_desktop_focused = is_desktop;
                        if state.config.lock().unwrap().detection_enabled && is_desktop {
                            let _ = state.mouse_detector.register();
                        }
                    }
                    _ => {}
                }
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            WM_QUERYENDSESSION => {
                log_info!("WM_QUERYENDSESSION recibido (cierre de sesión o apagado). Confirmando salida inmediata.");
                // Retornar 1 (TRUE) confirma al sistema operativo que SWEPR está listo para terminar sin colgar la sesión
                return LRESULT(1);
            }
            WM_ENDSESSION => {
                let is_ending = wparam.0 != 0;
                log_info!("WM_ENDSESSION recibido (finalizando sesión: {}).", is_ending);
                if is_ending {
                    state.tray_icon.remove();
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
            }
            _ => {}
        }
    }

    match msg {
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub struct App {
    hwnd: HWND,
    hinstance: HINSTANCE,
    class_name_wide: Vec<u16>,
    _focus_detector: FocusDetector,
    _state: Box<AppState>,
}

impl App {
    /// Inicializa la configuración, la ventana oculta, el icono de bandeja y los detectores.
    pub fn new() -> Result<Self, String> {
        let config = AppConfig::load_or_default();

        if let Err(e) = crate::config::autostart::set_autostart(config.autostart) {
            crate::log_warn!("Fallo al sincronizar autoarranque inicial: {e}");
        }

        unsafe {
            let hinstance = GetModuleHandleW(None)
                .map_err(|e| format!("Fallo al obtener HINSTANCE: {e}"))?;
            let hinstance = HINSTANCE(hinstance.0);

            let class_name_wide = to_wide_chars(WINDOW_CLASS_NAME);
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance,
                lpszClassName: as_pcwstr(&class_name_wide),
                ..Default::default()
            };

            let atom = RegisterClassW(&wc);
            if atom == 0 {
                return Err("Fallo al registrar la clase de ventana WNDCLASSW".into());
            }

            let title_wide = to_wide_chars(WINDOW_TITLE);
            let hwnd = CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&class_name_wide),
                as_pcwstr(&title_wide),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                hinstance,
                None,
            ).map_err(|e| format!("Fallo al crear la ventana principal: {e}"))?;

            crate::app::set_main_hwnd(hwnd);
            log_info!("Ventana principal creada con éxito. HWND: {:?}", hwnd);

            // Registro del mensaje TaskbarCreated para reaccionar al reinicio de explorer.exe
            let taskbar_name = to_wide_chars("TaskbarCreated");
            let taskbar_created_msg = RegisterWindowMessageW(as_pcwstr(&taskbar_name));
            log_info!("Mensaje TaskbarCreated registrado (ID: {taskbar_created_msg})");

            // Creación y registro del icono en la bandeja del sistema
            let mut tray_icon = TrayIcon::new(hwnd);
            tray_icon.add()?;

            let mut mouse_detector = MouseDetector::new(hwnd);
            let zone_detector = ZoneDetector::new(hwnd, config.zones.clone());

            let wallpaper_manager = match WallpaperManager::new() {
                Ok(wm) => wm,
                Err(e) => {
                    log_error!("Fallo al inicializar WallpaperManager: {e}");
                    return Err(format!("Fallo al inicializar wallpaper: {e}"));
                }
            };
            if let Ok(current_wp) = wallpaper_manager.get_current_wallpaper() {
                log_info!("Fondo de pantalla actual del sistema detectado: {:?}", current_wp);
                crate::panel::set_current_wallpaper(&current_wp);
            }

            let initial_fg = crate::winutil::get_current_foreground_window();
            log_info!("Evaluando ventana inicial en primer plano:");
            let is_desktop_focused = evaluate_window_focus(initial_fg);

            if config.detection_enabled && is_desktop_focused {
                let _ = mouse_detector.register();
            }

            let mut directory_watcher = DirectoryWatcher::new();
            if config.wallpaper_dir.exists() {
                directory_watcher.watch(hwnd, &config.wallpaper_dir, config.scan_subfolders);
            } else {
                log_warn!("Carpeta de wallpapers no existe: {:?}", config.wallpaper_dir);
            }


            let config = Arc::new(Mutex::new(config));
            let mut state = Box::new(AppState {
                config: config.clone(),
                mouse_detector,
                zone_detector,
                tray_icon,
                is_desktop_focused,
                taskbar_created_msg,
                wallpaper_manager,
                scanner: Scanner::new(),
                scanned_images: Arc::new(Mutex::new(Vec::new())),
                last_scan_count: 0,
                thumb_cache: ThumbnailCache::new(),
                directory_watcher,
            });
            // Propagar thumb_cache y scanned_images al zone_detector
            state.zone_detector.thumb_cache = Some(state.thumb_cache.clone());
            state.zone_detector.scanned_images = Some(state.scanned_images.clone());


            // Auto-arranque: escanear la galería configurada al iniciar el programa
            {
                let config_guard = state.config.lock().unwrap();
                let by_date = config_guard.sort_by.eq_ignore_ascii_case("date");
                let wallpaper_dir = config_guard.wallpaper_dir.clone();
                let scan_subfolders = config_guard.scan_subfolders;
                drop(config_guard);
                state.scanner.scan_folder(
                    hwnd,
                    &wallpaper_dir,
                    scan_subfolders,
                    by_date,
                    state.scanned_images.clone(),
                );
            }

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *state as *mut AppState) as isize);

            let focus_detector = FocusDetector::new(hwnd)?;

            Ok(Self {
                hwnd,
                hinstance,
                class_name_wide,
                _focus_detector: focus_detector,
                _state: state,
            })
        }
    }

    /// Ejecuta el bucle de mensajes principal de Windows hasta recibir WM_QUIT.
    pub fn run_message_loop(&self) -> i32 {
        log_info!("Iniciando bucle de mensajes GetMessageW...");
        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        log_info!("Bucle de mensajes terminado. Código de salida: {}", msg.wParam.0);
        msg.wParam.0 as i32
    }

    #[allow(dead_code)]
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_invalid() {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                let _ = DestroyWindow(self.hwnd);
            }
            let _ = UnregisterClassW(as_pcwstr(&self.class_name_wide), self.hinstance);
        }
        log_info!("Recursos de la aplicación y clase de ventana liberados.");
    }
}
