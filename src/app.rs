use crate::config::{window, AppConfig, ApplyMode, FitMode, SortOrder, ThumbLoading};
use crate::detector::focus::{evaluate_window_focus, FocusDetector, WM_APP_FOREGROUND_CHANGED};
use crate::detector::mouse::MouseDetector;
use crate::detector::zones::{ZoneDetector, ZONE_TIMER_ID};
use crate::panel;
use crate::scanner::{ScannedImage, Scanner, WM_APP_SCAN_COMPLETE};
use crate::thumbs::{ThumbnailCache, WM_APP_THUMB_READY};
use crate::tray::{
    TrayIcon, IDM_EXIT, IDM_NEXT_WALLPAPER, IDM_OPEN_FOLDER, IDM_RANDOM_WALLPAPER, IDM_REFRESH_GALLERY, IDM_SETTINGS,
    IDM_TOGGLE_DETECTION, IDM_UNDO_WALLPAPER, WM_APP_TRAY_NOTIFICATION,
};
use crate::wallpaper::{WallpaperManager, WallpaperStyle};
use crate::watcher::{DirectoryWatcher, WM_APP_WATCHER_CHANGED};
use crate::winutil::{as_pcwstr, same_path, to_wide_chars};
use crate::{log_error, log_info, log_warn};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindowLongPtrW, KillTimer,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetTimer, SetWindowLongPtrW,
    TranslateMessage, UnregisterClassW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, MSG, WM_APP, WM_CLOSE, WM_CONTEXTMENU,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_HOTKEY, WM_INPUT, WM_LBUTTONUP, WM_POWERBROADCAST,
    WM_QUERYENDSESSION, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

/// Hay acciones pendientes en la cola (`request`).
pub const WM_APP_ACTION: u32 = WM_APP + 40;
/// WM_APP para solicitar generación de miniaturas
pub const WM_APP_GEN_THUMBS: u32 = WM_APP + 41;
/// WM_APP para notificar cambio de configuración
pub const WM_APP_CONFIG_CHANGED: u32 = WM_APP + 42;
/// WPARAM 1: liberar los atajos globales (se están editando en la configuración);
/// WPARAM 0: volver a registrarlos con la configuración guardada.
pub const WM_APP_SUSPEND_HOTKEYS: u32 = WM_APP + 43;

/// En carga bajo demanda, miniaturas que se precargan a cada lado del fondo actual.
const ON_DEMAND_PREFETCH: usize = 30;
/// Fondos anteriores que se recuerdan para "Deshacer".
const HISTORY_LIMIT: usize = 50;

const WINDOW_CLASS_NAME: &str = "SWEPR_MainWindowClass";
const WINDOW_TITLE: &str = "SWEPR_BackgroundHost";

const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;

const WATCHER_DEBOUNCE_TIMER_ID: usize = 9001;
const ROTATION_TIMER_ID: usize = 9002;
/// Volver a crear el lienzo tras reiniciarse el Explorador o cambiar los monitores.
const CANVAS_RESTART_TIMER_ID: usize = 9003;
const HOTKEY_PANEL_ID: i32 = 1;
const HOTKEY_PREVIOUS_ID: i32 = 2;
const HOTKEY_NEXT_ID: i32 = 3;

/// Acciones que el panel (u otras partes) piden a la ventana principal.
#[derive(Debug, Clone)]
pub enum Action {
    /// Aplicar solo al monitor con este rectángulo.
    ApplyToMonitor(PathBuf, RECT),
    /// Elegida en el panel (que está en el monitor `RECT`): a todos, o solo a ese con "uno por pantalla".
    ApplyFromPanel(PathBuf, RECT),
    LockScreen(PathBuf),
    ToggleFavorite(PathBuf),
    /// Mostrar el archivo en el Explorador.
    Reveal(PathBuf),
    /// Quitar de la galería sin borrar el archivo.
    Exclude(PathBuf),
}

static PENDING_ACTIONS: Mutex<VecDeque<Action>> = Mutex::new(VecDeque::new());
pub static MAIN_HWND: Mutex<Option<isize>> = Mutex::new(None);

/// Encola una acción para la ventana principal. Se pueden pedir varias seguidas sin perder ninguna.
pub fn request(action: Action) {
    if let Ok(mut queue) = PENDING_ACTIONS.lock() {
        queue.push_back(action);
    }
    if let Some(hwnd_val) = MAIN_HWND.lock().ok().and_then(|g| *g) {
        unsafe {
            let _ = PostMessageW(HWND(hwnd_val as *mut _), WM_APP_ACTION, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn set_main_hwnd(hwnd: HWND) {
    if let Ok(mut g) = MAIN_HWND.lock() {
        *g = Some(hwnd.0 as isize);
    }
}

fn register_configured_hotkey(hwnd: HWND, id: i32, description: &str, binding: &crate::config::HotkeyBinding) {
    if binding.is_empty() {
        log_info!("Atajo {description} desactivado.");
        return;
    }
    let Some(key) = crate::config::virtual_key_code(&binding.key) else {
        log_warn!("Atajo {description} omitido: tecla desconocida {:?}.", binding.key);
        return;
    };

    let mut modifiers = HOT_KEY_MODIFIERS(0);
    for modifier in &binding.modifiers {
        modifiers |= match modifier.trim().to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => MOD_CONTROL,
            "ALT" => MOD_ALT,
            "SHIFT" => MOD_SHIFT,
            "WIN" | "WINDOWS" => MOD_WIN,
            other => {
                log_warn!("Atajo {description} omitido: modificador desconocido {:?}.", other);
                return;
            }
        };
    }

    if let Err(error) = unsafe { RegisterHotKey(hwnd, id, modifiers, key) } {
        log_warn!("No se pudo registrar el atajo {description} ({:?}+{}): {error}", binding.modifiers, binding.key);
    } else {
        log_info!("Atajo registrado: {description} ({:?}+{})", binding.modifiers, binding.key);
    }
}

unsafe fn unregister_hotkeys(hwnd: HWND) {
    let _ = UnregisterHotKey(hwnd, HOTKEY_PANEL_ID);
    let _ = UnregisterHotKey(hwnd, HOTKEY_PREVIOUS_ID);
    let _ = UnregisterHotKey(hwnd, HOTKEY_NEXT_ID);
}

fn register_hotkeys(hwnd: HWND, hotkeys: &crate::config::HotkeyConfig) {
    unsafe { unregister_hotkeys(hwnd); }
    register_configured_hotkey(hwnd, HOTKEY_PANEL_ID, "open_panel", &hotkeys.open_panel);
    register_configured_hotkey(hwnd, HOTKEY_PREVIOUS_ID, "previous_wallpaper", &hotkeys.previous_wallpaper);
    register_configured_hotkey(hwnd, HOTKEY_NEXT_ID, "next_wallpaper", &hotkeys.next_wallpaper);
}

/// Bloquea un mutex aunque otro hilo haya fallado con él tomado.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Qué elegir con "siguiente" / "anterior".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Previous,
    Next,
    Random,
}

/// Índice de destino dentro de `count` imágenes partiendo de `current`.
fn step_index(step: Step, current: Option<usize>, count: usize, random: impl FnOnce(usize) -> usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (step, current) {
        (Step::Previous, Some(0)) | (Step::Previous, None) => count - 1,
        (Step::Previous, Some(index)) => index - 1,
        (Step::Next, Some(index)) => (index + 1) % count,
        (Step::Next, None) => 0,
        (Step::Random, Some(index)) if count > 1 => {
            // Cualquiera menos la actual.
            let pick = random(count - 1);
            if pick >= index { pick + 1 } else { pick }
        }
        (Step::Random, _) => random(count),
    })
}

/// `count` índices siguiendo `step` desde `current`. En aleatorio no se repiten mientras haya
/// imágenes suficientes, ni se elige la actual.
fn pick_indices(step: Step, current: Option<usize>, len: usize, count: usize, mut random: impl FnMut(usize) -> usize) -> Vec<usize> {
    if len == 0 || count == 0 {
        return Vec::new();
    }
    if step == Step::Random {
        let mut pool: Vec<usize> = (0..len).filter(|index| Some(*index) != current || len == 1).collect();
        // Fisher-Yates parcial.
        let take = count.min(pool.len());
        for i in 0..take {
            let j = i + random(pool.len() - i);
            pool.swap(i, j);
        }
        pool.truncate(take);
        let mut picks = pool.clone();
        while picks.len() < count {
            picks.push(pool[picks.len() % pool.len()]);
        }
        return picks;
    }
    let mut picks = Vec::with_capacity(count);
    let mut position = current;
    for _ in 0..count {
        let Some(index) = step_index(step, position, len, &mut random) else { break; };
        picks.push(index);
        position = Some(index);
    }
    picks
}

/// Estado mutable de la aplicación accesible desde el procedimiento de ventana.
pub struct AppState {
    pub hwnd: HWND,
    pub config: Arc<Mutex<AppConfig>>,
    /// Configuración con la que está funcionando la aplicación ahora (para aplicar solo lo que cambia).
    applied_config: AppConfig,
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
    /// Último fondo aplicado por SWEPR (o el que tenía Windows al arrancar).
    current_wallpaper: Option<PathBuf>,
    /// Fondos anteriores, el más reciente al final.
    history: Vec<PathBuf>,
    /// Fecha de modificación de cada imagen en el escaneo anterior (para regenerar miniaturas editadas).
    last_mtimes: HashMap<PathBuf, SystemTime>,
    /// Lienzo detrás de los iconos (modo de aplicación "Lienzo SWEPR").
    canvas: Option<crate::canvas::Canvas>,
}

impl AppState {
    fn config(&self) -> MutexGuard<'_, AppConfig> {
        lock(&self.config)
    }

    fn rescan(&self) {
        let (wallpaper_dir, scan_subfolders, by_date) = {
            let config = self.config();
            (config.wallpaper_dir.clone(), config.scan_subfolders, config.sort_by == SortOrder::Date)
        };
        self.scanner.scan_folder(self.hwnd, &wallpaper_dir, scan_subfolders, by_date, self.scanned_images.clone());
    }

    /// Activa la detección de bordes solo si está habilitada y el escritorio tiene el foco.
    fn sync_detection(&mut self) {
        if self.config().detection_enabled && self.is_desktop_focused {
            let _ = self.mouse_detector.register();
        } else {
            let _ = self.mouse_detector.unregister();
            self.zone_detector.reset();
        }
    }

    fn restart_rotation_timer(&self) {
        let minutes = self.config().rotation_minutes;
        unsafe {
            let _ = KillTimer(self.hwnd, ROTATION_TIMER_ID);
            if minutes > 0 {
                let ms = minutes.min(24 * 60).saturating_mul(60_000);
                let _ = SetTimer(self.hwnd, ROTATION_TIMER_ID, ms, None);
                log_info!("Rotación automática cada {minutes} min.");
            }
        }
    }

    /// Fondo actual: el que aplicó SWEPR o, si no hay, el que informa Windows.
    fn current_wallpaper(&self) -> Option<PathBuf> {
        self.current_wallpaper.clone().or_else(|| self.wallpaper_manager.get_current_wallpaper().ok())
    }

    /// Imagen a la que llevaría `step`. Con `favorites_only`, solo entre las favoritas (si hay).
    fn pick(&self, step: Step, favorites_only: bool) -> Option<PathBuf> {
        self.pick_many(step, favorites_only, 1).into_iter().next()
    }

    /// `count` imágenes distintas (si hay suficientes) siguiendo `step` desde el fondo actual.
    fn pick_many(&self, step: Step, favorites_only: bool, count: usize) -> Vec<PathBuf> {
        let current = self.current_wallpaper();
        let Ok(images) = self.scanned_images.lock() else { return Vec::new(); };
        let mut candidates: Vec<&ScannedImage> = images.iter().collect();
        if favorites_only && candidates.iter().any(|image| image.is_favorite) {
            candidates.retain(|image| image.is_favorite);
        }
        let current_index = current.as_ref().and_then(|path| candidates.iter().position(|image| same_path(&image.path, path)));
        pick_indices(step, current_index, candidates.len(), count, crate::winutil::random_below)
            .into_iter()
            .map(|index| candidates[index].path.clone())
            .collect()
    }

    /// ¿Hay que poner una imagen distinta en cada monitor? Devuelve los monitores (id y rectángulo).
    fn per_monitor_ids(&self) -> Option<Vec<(String, RECT)>> {
        let (per_monitor, fit_mode) = {
            let config = self.config();
            (config.per_monitor, config.fit_mode)
        };
        if !per_monitor || fit_mode == crate::config::FitMode::Span {
            return None;
        }
        let monitors = self.wallpaper_manager.monitors();
        (monitors.len() > 1).then_some(monitors)
    }

    /// Una imagen distinta para cada monitor (la primera, para el de más a la izquierda).
    fn apply_per_monitor(&mut self, paths: &[PathBuf], monitors: &[(String, RECT)]) {
        let previous = self.current_wallpaper();
        let mut applied = false;
        for (path, (id, rect)) in paths.iter().cycle().zip(monitors) {
            match self.set_wallpaper_on(path, Some((id.clone(), *rect)), false) {
                Ok(()) => applied = true,
                Err(e) => log_error!("Error al aplicar {:?} al monitor {id}: {e}", path),
            }
        }
        if let (true, Some(first)) = (applied, paths.first()) {
            if let Some(previous) = previous.filter(|previous| !same_path(previous, first)) {
                self.push_history(previous);
            }
            for path in paths {
                self.mark_seen(path);
            }
            self.current_wallpaper = Some(first.clone());
            panel::set_current_wallpaper(first);
            panel::request_repaint();
        }
    }

    fn push_history(&mut self, path: PathBuf) {
        self.history.push(path);
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
    }

    /// La imagen ya no es "nueva" (se usó como fondo).
    fn mark_seen(&self, path: &Path) {
        let wallpaper_dir = self.config().wallpaper_dir.clone();
        if let Ok(mut images) = self.scanned_images.lock() {
            if let Some(image) = images.iter_mut().find(|image| same_path(&image.path, path)) {
                if image.is_new {
                    image.is_new = false;
                    crate::scanner::save_gallery_state(&wallpaper_dir, &images);
                }
            }
        }
    }

    /// Pone una imagen en un monitor (`Some`) o en todos, con el modo de aplicación configurado.
    /// Con el lienzo: se dibuja al instante con transición y el fondo real se sincroniza aparte.
    fn set_wallpaper_on(&mut self, path: &Path, monitor: Option<(String, RECT)>, fade_allowed: bool) -> Result<(), String> {
        let (fit_mode, fade, mode, transition_ms, transition) = {
            let config = self.config();
            (config.fit_mode, config.wallpaper_fade, config.apply_mode, config.transition_ms, config.transition)
        };
        let style = WallpaperStyle::from(fit_mode);
        if mode == ApplyMode::Canvas {
            self.ensure_canvas();
            if let Some(canvas) = self.canvas.as_ref() {
                let target = match &monitor {
                    Some((_, rect)) => crate::canvas::Target::Monitor(*rect),
                    None => crate::canvas::Target::All,
                };
                let duration = std::time::Duration::from_millis(transition_ms as u64);
                canvas.show(path, target, fit_mode, duration, transition);
                // Margen para la decodificación además de la transición.
                crate::wallpaper::sync_in_background(path, monitor.map(|(id, _)| id), style, duration + std::time::Duration::from_millis(500));
                return Ok(());
            }
            // Sin lienzo (no se pudo enganchar): se usa Windows.
        }
        self.wallpaper_manager.set_wallpaper(path, monitor.as_ref().map(|(id, _)| id.as_str()), style, fade && fade_allowed)
    }

    /// Arranca el lienzo si el modo lo pide y no está ya en marcha, y le pone los fondos actuales.
    fn ensure_canvas(&mut self) {
        if self.config().apply_mode != ApplyMode::Canvas {
            self.canvas = None;
            return;
        }
        if self.canvas.as_ref().is_some_and(|canvas| canvas.is_alive()) {
            return;
        }
        self.canvas = None;
        match crate::canvas::Canvas::start() {
            Ok(canvas) => {
                self.canvas = Some(canvas);
                self.restore_canvas();
            }
            Err(e) => log_warn!("No se pudo usar el lienzo ({e}); los fondos se aplicarán con Windows."),
        }
    }

    /// Dibuja en el lienzo, sin transición, lo que Windows tiene puesto ahora en cada monitor.
    fn restore_canvas(&self) {
        let Some(canvas) = self.canvas.as_ref() else { return; };
        let fit = self.config().fit_mode;
        let zero = std::time::Duration::ZERO;
        let monitors = self.wallpaper_manager.monitors();
        if fit == FitMode::Span || monitors.is_empty() {
            if let Some(path) = self.current_wallpaper() {
                canvas.show(&path, crate::canvas::Target::All, fit, zero, crate::config::Transition::Fade);
            }
            return;
        }
        for (id, rect) in monitors {
            if let Some(path) = self.wallpaper_manager.wallpaper_of(&id).or_else(|| self.current_wallpaper()) {
                canvas.show(&path, crate::canvas::Target::Monitor(rect), fit, zero, crate::config::Transition::Fade);
            }
        }
    }

    /// Tras reiniciarse el Explorador o cambiar los monitores, el lienzo se rehace un poco después.
    fn schedule_canvas_restart(&mut self, delay_ms: u32) {
        if self.config().apply_mode != ApplyMode::Canvas {
            return;
        }
        self.canvas = None;
        unsafe { let _ = SetTimer(self.hwnd, CANVAS_RESTART_TIMER_ID, delay_ms, None); }
    }

    /// Aplica un fondo. `monitor`: rectángulo del monitor si solo va a uno.
    fn apply(&mut self, path: &Path, monitor: Option<RECT>, record_history: bool) {
        log_info!("Aplicando wallpaper: {:?}", path);
        let monitor = monitor.and_then(|rect| {
            let id = self.wallpaper_manager.monitor_id_for_rect(rect);
            if id.is_none() {
                log_warn!("No se encontró el monitor del panel; se aplica a todos.");
            }
            id.map(|id| (id, rect))
        });
        let previous = self.current_wallpaper();
        match self.set_wallpaper_on(path, monitor, true) {
            Ok(_) => {
                log_info!("Wallpaper aplicado correctamente.");
                if record_history {
                    if let Some(previous) = previous.filter(|previous| !same_path(previous, path)) {
                        self.push_history(previous);
                    }
                }
                // Con un solo monitor cambiado, "actual" sigue siendo el último aplicado.
                self.current_wallpaper = Some(path.to_path_buf());
                self.mark_seen(path);
                panel::set_current_wallpaper(path);
                panel::request_repaint();
            }
            Err(e) => log_error!("Error al aplicar wallpaper: {e}"),
        }
    }

    fn step(&mut self, step: Step, favorites_only: bool) {
        let step = if step == Step::Next && self.config().shuffle { Step::Random } else { step };
        // En modo aleatorio, "anterior" vuelve al fondo de antes.
        if step == Step::Previous && self.config().shuffle && !self.history.is_empty() {
            self.undo();
            return;
        }
        if let Some(monitors) = self.per_monitor_ids() {
            let paths = self.pick_many(step, favorites_only, monitors.len());
            if !paths.is_empty() {
                self.apply_per_monitor(&paths, &monitors);
            }
            return;
        }
        if let Some(path) = self.pick(step, favorites_only) {
            self.apply(&path, None, true);
        }
    }

    fn undo(&mut self) {
        if let Some(path) = self.history.pop() {
            log_info!("Deshacer: volviendo a {:?}", path);
            self.apply(&path, None, false);
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::ApplyToMonitor(path, rect) => self.apply(&path, Some(rect), true),
            Action::ApplyFromPanel(path, rect) => {
                let monitor = self.per_monitor_ids().map(|_| rect);
                self.apply(&path, monitor, true);
            }
            Action::LockScreen(path) => crate::wallpaper::set_lock_screen(&path),
            Action::ToggleFavorite(path) => {
                let (favorite, favorites) = {
                    let mut config = lock(&self.config);
                    let favorite = config.toggle_favorite(&path);
                    if let Err(e) = config.save() {
                        log_error!("No se pudo guardar la favorita: {e}");
                    }
                    (favorite, config.favorites.clone())
                };
                self.applied_config.favorites = favorites;
                if let Ok(mut images) = self.scanned_images.lock() {
                    for image in images.iter_mut().filter(|image| same_path(&image.path, &path)) {
                        image.is_favorite = favorite;
                    }
                }
                log_info!("Favorita {}: {:?}", if favorite { "añadida" } else { "quitada" }, path);
                panel::request_repaint();
            }
            Action::Reveal(path) => {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
            }
            Action::Exclude(path) => {
                let excluded = {
                    let mut config = lock(&self.config);
                    let added = config.exclude(&path);
                    if added {
                        if let Err(e) = config.save() {
                            log_error!("No se pudo guardar la imagen quitada: {e}");
                        }
                    }
                    config.excluded.clone()
                };
                self.applied_config.excluded = excluded;
                if let Ok(mut images) = self.scanned_images.lock() {
                    images.retain(|image| !same_path(&image.path, &path));
                }
                log_info!("Quitada de la galería: {:?}", path);
                // Las escalas del panel dependen del número de imágenes.
                panel::set_draw_context(self.thumb_cache.clone(), self.scanned_images.clone());
            }
        }
    }

    /// Tras un escaneo: marcar favoritas, liberar miniaturas de archivos borrados o editados.
    fn on_scan_complete(&mut self) {
        let config = self.config().clone();
        let mut stale: HashSet<String> = HashSet::new();
        let mut present: HashSet<String> = HashSet::new();
        let mut mtimes = HashMap::new();
        if let Ok(mut images) = self.scanned_images.lock() {
            if !config.excluded.is_empty() {
                images.retain(|image| !config.is_excluded(&image.path));
            }
            for image in images.iter_mut() {
                image.is_favorite = config.is_favorite(&image.path);
                let key = image.path.to_string_lossy().to_string();
                if let (Some(old), Some(new)) = (self.last_mtimes.get(&image.path), image.modified) {
                    if *old != new {
                        stale.insert(key.clone());
                    }
                }
                if let Some(modified) = image.modified {
                    mtimes.insert(image.path.clone(), modified);
                }
                present.insert(key);
            }
        }
        self.last_mtimes = mtimes;
        let removed = self.thumb_cache.retain(|key| present.contains(key) && !stale.contains(key));
        if removed > 0 {
            log_info!("{} miniaturas liberadas (archivos borrados o modificados).", removed);
        }
        if !stale.is_empty() {
            panel::forget_tiles(&stale);
        }
    }

    /// Aplica la configuración guardada, tocando solo lo que cambió.
    fn apply_config_changes(&mut self) {
        let new = self.config().clone();
        let old = std::mem::replace(&mut self.applied_config, new.clone());
        log_info!("Configuración cambiada, aplicando diferencias...");

        crate::i18n::set_language(new.language.clone());
        panel::set_panel_side(new.panel_side);
        panel::set_panel_mode(new.panel_mode);
        panel::set_pro_priority(new.pro_priority);
        panel::set_scrollbar_visible(new.show_scrollbar);
        panel::set_panel_margin(new.panel_margin);
        panel::set_thumbnail_loading(new.thumb_loading == ThumbLoading::OnDemand, new.thumb_size_value() as i32);

        let size_changed = old.thumb_size_value() != new.thumb_size_value();
        if size_changed {
            // Lo pendiente sería del tamaño anterior.
            crate::thumbs::cancel_pending();
            self.thumb_cache.clear();
            panel::clear_carousel_tiles();
        }

        if old.zones != new.zones || old.panel_side != new.panel_side {
            let mut zones = new.zones.clone();
            zones.active_zones = new.panel_side.active_zones();
            self.zone_detector.update_config(zones);
        }

        let rescan = !same_path(&old.wallpaper_dir, &new.wallpaper_dir)
            || old.scan_subfolders != new.scan_subfolders
            || old.sort_by != new.sort_by
            || old.excluded != new.excluded;
        if rescan {
            self.rescan();
        } else if size_changed || old.thumb_loading != new.thumb_loading {
            unsafe { let _ = PostMessageW(self.hwnd, WM_APP_GEN_THUMBS, WPARAM(0), LPARAM(0)); }
        } else {
            panel::request_repaint();
        }
        self.directory_watcher.watch(self.hwnd, &new.wallpaper_dir, new.scan_subfolders);
        self.sync_detection();
        if old.rotation_minutes != new.rotation_minutes {
            self.restart_rotation_timer();
        }
        if old.apply_mode != new.apply_mode {
            log_info!("Modo de aplicación: {:?}", new.apply_mode);
            self.ensure_canvas();
        } else if self.canvas.is_some() && old.fit_mode != new.fit_mode {
            // El ajuste nuevo se ve enseguida en lo que ya está puesto.
            self.restore_canvas();
        }
    }

    fn generate_thumbnails(&self) {
        let (thumb_size, loading) = {
            let config = self.config();
            (config.thumb_size_value() as i32, config.thumb_loading)
        };
        let images: Vec<PathBuf> = self.scanned_images.lock().ok()
            .map(|g| g.iter().map(|img| img.path.clone()).collect())
            .unwrap_or_default();
        // Lo pendiente de un escaneo anterior ya no sirve: se vuelve a pedir lo que falte.
        crate::thumbs::cancel_pending();
        let wanted: &[PathBuf] = match loading {
            ThumbLoading::Preload => &images,
            ThumbLoading::OnDemand => {
                // Solo las cercanas al fondo actual, para que el panel abra sin huecos.
                let center = self.current_wallpaper()
                    .and_then(|path| images.iter().position(|p| same_path(p, &path)))
                    .unwrap_or(0);
                let start = center.saturating_sub(ON_DEMAND_PREFETCH);
                &images[start..(center + ON_DEMAND_PREFETCH).min(images.len())]
            }
        };
        crate::thumbs::request_thumbnails(wanted.iter().map(|p| p.as_path()), thumb_size, &self.thumb_cache, self.hwnd);
    }

    fn handle_tray_menu(&mut self, hwnd: HWND) {
        let detection_enabled = self.config().detection_enabled;
        let Some(cmd_id) = self.tray_icon.show_context_menu(detection_enabled, !self.history.is_empty()) else { return; };
        match cmd_id {
            IDM_TOGGLE_DETECTION => {
                let enabled = {
                    let mut config = self.config();
                    config.detection_enabled = !config.detection_enabled;
                    let _ = config.save();
                    config.detection_enabled
                };
                self.applied_config.detection_enabled = enabled;
                log_info!("Acción de menú de bandeja: Detección {}", if enabled { "ACTIVADA" } else { "DESACTIVADA" });
                self.sync_detection();
            }
            IDM_NEXT_WALLPAPER => self.step(Step::Next, false),
            IDM_RANDOM_WALLPAPER => {
                if let Some(path) = self.pick(Step::Random, false) {
                    self.apply(&path, None, true);
                }
            }
            IDM_UNDO_WALLPAPER => self.undo(),
            IDM_REFRESH_GALLERY => {
                log_info!("Acción de menú de bandeja: 'Actualizar galería'. Iniciando escaneo...");
                self.rescan();
            }
            IDM_OPEN_FOLDER => {
                let wallpaper_dir = self.config().wallpaper_dir.clone();
                log_info!("Acción de menú de bandeja: 'Abrir carpeta' {:?}", wallpaper_dir);
                let _ = std::process::Command::new("explorer").arg(&wallpaper_dir).spawn();
            }
            IDM_SETTINGS => {
                log_info!("Acción de menú de bandeja: 'Configuración...'");
                window::show_config_window(self.config.clone());
            }
            IDM_EXIT => {
                log_info!("Acción de menú de bandeja: 'Salir' seleccionada por el usuario.");
                unsafe { let _ = DestroyWindow(hwnd); }
            }
            _ => {}
        }
    }
}

/// Procedimiento de ventana principal para el despacho de mensajes Win32.
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;

    if !state_ptr.is_null() {
        let state = &mut *state_ptr;

        // Reinicio del explorador de Windows: restaurar el icono en la bandeja y reevaluar foco
        if msg == state.taskbar_created_msg && state.taskbar_created_msg != 0 {
            log_info!("Mensaje TaskbarCreated detectado (explorer.exe reiniciado). Restaurando icono de bandeja...");
            let _ = state.tray_icon.re_add();
            state.is_desktop_focused = evaluate_window_focus(crate::winutil::get_current_foreground_window());
            state.sync_detection();
            // El escritorio nuevo tarda un poco en estar listo.
            state.schedule_canvas_restart(1500);
            return LRESULT(0);
        }

        match msg {
            WM_HOTKEY => {
                match wparam.0 as i32 {
                    HOTKEY_PANEL_ID => panel::show_panel_from_hotkey(),
                    HOTKEY_PREVIOUS_ID => state.step(Step::Previous, false),
                    HOTKEY_NEXT_ID => state.step(Step::Next, false),
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_APP_TRAY_NOTIFICATION => {
                let mouse_msg = (lparam.0 & 0xFFFF) as u32;
                if mouse_msg == WM_RBUTTONUP || mouse_msg == WM_CONTEXTMENU {
                    state.handle_tray_menu(hwnd);
                } else if mouse_msg == WM_LBUTTONUP {
                    // Clic izquierdo en el icono: abrir el panel como con el atajo.
                    panel::show_panel_from_hotkey();
                }
                return LRESULT(0);
            }
            WM_APP_FOREGROUND_CHANGED => {
                let is_desktop = evaluate_window_focus(HWND(wparam.0 as *mut _));
                if is_desktop && !state.tray_icon.is_added() {
                    let _ = state.tray_icon.add();
                }
                if is_desktop != state.is_desktop_focused {
                    state.is_desktop_focused = is_desktop;
                    state.sync_detection();
                }
                return LRESULT(0);
            }
            WM_INPUT => {
                if state.is_desktop_focused && state.config().detection_enabled {
                    if let Some(pt) = crate::winutil::get_cursor_position() {
                        state.zone_detector.on_mouse_move(pt);
                    }
                }
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            WM_APP_SCAN_COMPLETE => {
                state.last_scan_count = wparam.0;
                log_info!("Escaneo completado - {} imágenes listas en memoria.", state.last_scan_count);
                state.on_scan_complete();
                panel::set_draw_context(state.thumb_cache.clone(), state.scanned_images.clone());
                panel::request_repaint();
                let _ = PostMessageW(hwnd, WM_APP_GEN_THUMBS, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            WM_APP_THUMB_READY => {
                if panel::is_panel_visible() {
                    panel::request_repaint();
                }
                return LRESULT(0);
            }
            WM_APP_WATCHER_CHANGED => {
                // Debounce: reprogramar el temporizador con cada aviso.
                let _ = KillTimer(hwnd, WATCHER_DEBOUNCE_TIMER_ID);
                let _ = SetTimer(hwnd, WATCHER_DEBOUNCE_TIMER_ID, 400, None);
                return LRESULT(0);
            }
            WM_APP_ACTION => {
                loop {
                    let Some(action) = PENDING_ACTIONS.lock().ok().and_then(|mut queue| queue.pop_front()) else { break; };
                    state.handle_action(action);
                }
                return LRESULT(0);
            }
            WM_APP_GEN_THUMBS => {
                state.generate_thumbnails();
                return LRESULT(0);
            }
            WM_APP_SUSPEND_HOTKEYS => {
                if wparam.0 == 1 {
                    unregister_hotkeys(hwnd);
                    log_info!("Atajos globales suspendidos mientras se editan.");
                } else {
                    let hotkeys = state.config().hotkeys.clone();
                    register_hotkeys(hwnd, &hotkeys);
                }
                return LRESULT(0);
            }
            WM_APP_CONFIG_CHANGED => {
                state.apply_config_changes();
                return LRESULT(0);
            }
            WM_TIMER => match wparam.0 {
                ZONE_TIMER_ID => {
                    state.zone_detector.on_timer(wparam.0);
                    return LRESULT(0);
                }
                WATCHER_DEBOUNCE_TIMER_ID => {
                    let _ = KillTimer(hwnd, WATCHER_DEBOUNCE_TIMER_ID);
                    log_info!("Watcher: cambios detectados en carpeta, reescaneando...");
                    state.rescan();
                    return LRESULT(0);
                }
                CANVAS_RESTART_TIMER_ID => {
                    let _ = KillTimer(hwnd, CANVAS_RESTART_TIMER_ID);
                    state.ensure_canvas();
                    return LRESULT(0);
                }
                ROTATION_TIMER_ID => {
                    // No cambiar el fondo con un juego o una presentación a pantalla completa.
                    if crate::winutil::is_notification_allowed() {
                        let favorites_only = state.config().rotation_favorites_only;
                        log_info!("Rotación automática: siguiente fondo.");
                        state.step(Step::Next, favorites_only);
                    }
                    return LRESULT(0);
                }
                _ => {}
            },
            WM_DISPLAYCHANGE => {
                log_info!("WM_DISPLAYCHANGE recibido: cambios en monitores o resolución.");
                state.zone_detector.reset();
                state.schedule_canvas_restart(800);
                return LRESULT(0);
            }
            WM_POWERBROADCAST => {
                match wparam.0 as u32 {
                    PBT_APMSUSPEND => {
                        log_info!("WM_POWERBROADCAST: Suspensión detectada. Desactivando detectores.");
                        state.is_desktop_focused = false;
                        state.sync_detection();
                    }
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                        log_info!("WM_POWERBROADCAST: Reanudación detectada. Reevaluando foco...");
                        state.is_desktop_focused = evaluate_window_focus(crate::winutil::get_current_foreground_window());
                        state.sync_detection();
                    }
                    _ => {}
                }
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            WM_QUERYENDSESSION => {
                log_info!("WM_QUERYENDSESSION recibido (cierre de sesión o apagado). Confirmando salida inmediata.");
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
            unregister_hotkeys(hwnd);
            let _ = KillTimer(hwnd, ROTATION_TIMER_ID);
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
    // El orden importa: el hook se quita antes de liberar el estado.
    _focus_detector: FocusDetector,
    _state: Box<AppState>,
}

impl App {
    /// Inicializa la configuración, la ventana oculta, el icono de bandeja y los detectores.
    pub fn new() -> Result<Self, String> {
        let config = AppConfig::load_or_default();
        crate::i18n::set_language(config.language.clone());
        panel::set_panel_side(config.panel_side);
        panel::set_panel_mode(config.panel_mode);
        panel::set_pro_priority(config.pro_priority);
        panel::set_thumbnail_loading(config.thumb_loading == ThumbLoading::OnDemand, config.thumb_size_value() as i32);
        panel::set_scrollbar_visible(config.show_scrollbar);
        panel::set_panel_margin(config.panel_margin);

        if let Err(e) = crate::config::autostart::set_autostart(config.autostart) {
            log_warn!("Fallo al sincronizar autoarranque inicial: {e}");
        }

        unsafe {
            let hinstance = GetModuleHandleW(None).map_err(|e| format!("Fallo al obtener HINSTANCE: {e}"))?;
            let hinstance = HINSTANCE(hinstance.0);

            let class_name_wide = to_wide_chars(WINDOW_CLASS_NAME);
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance,
                hIcon: crate::resources::app_icon(),
                lpszClassName: as_pcwstr(&class_name_wide),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err("Fallo al registrar la clase de ventana WNDCLASSW".into());
            }

            let title_wide = to_wide_chars(WINDOW_TITLE);
            let hwnd = match CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
                as_pcwstr(&class_name_wide),
                as_pcwstr(&title_wide),
                WS_OVERLAPPED,
                0, 0, 0, 0,
                None, None, hinstance, None,
            ) {
                Ok(hwnd) => hwnd,
                Err(e) => {
                    let _ = UnregisterClassW(as_pcwstr(&class_name_wide), hinstance);
                    return Err(format!("Fallo al crear la ventana principal: {e}"));
                }
            };

            // Si algo falla a partir de aquí, se destruye la ventana para no dejar recursos colgando.
            match Self::init_state(hwnd, config) {
                Ok((state, focus_detector)) => Ok(Self {
                    hwnd,
                    hinstance,
                    class_name_wide,
                    _focus_detector: focus_detector,
                    _state: state,
                }),
                Err(e) => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = DestroyWindow(hwnd);
                    let _ = UnregisterClassW(as_pcwstr(&class_name_wide), hinstance);
                    Err(e)
                }
            }
        }
    }

    unsafe fn init_state(hwnd: HWND, config: AppConfig) -> Result<(Box<AppState>, FocusDetector), String> {
        set_main_hwnd(hwnd);
        log_info!("Ventana principal creada con éxito. HWND: {:?}", hwnd);

        register_hotkeys(hwnd, &config.hotkeys);

        // Registro del mensaje TaskbarCreated para reaccionar al reinicio de explorer.exe
        let taskbar_name = to_wide_chars("TaskbarCreated");
        let taskbar_created_msg = RegisterWindowMessageW(as_pcwstr(&taskbar_name));
        log_info!("Mensaje TaskbarCreated registrado (ID: {taskbar_created_msg})");

        let mut tray_icon = TrayIcon::new(hwnd);
        tray_icon.add()?;

        let wallpaper_manager = WallpaperManager::new().map_err(|e| {
            log_error!("Fallo al inicializar WallpaperManager: {e}");
            format!("Fallo al inicializar wallpaper: {e}")
        })?;
        let current_wallpaper = wallpaper_manager.get_current_wallpaper().ok();
        if let Some(current) = &current_wallpaper {
            log_info!("Fondo de pantalla actual del sistema detectado: {:?}", current);
            panel::set_current_wallpaper(current);
        }

        log_info!("Evaluando ventana inicial en primer plano:");
        let is_desktop_focused = evaluate_window_focus(crate::winutil::get_current_foreground_window());

        let mut directory_watcher = DirectoryWatcher::new();
        if !config.wallpaper_dir.exists() {
            log_warn!("Carpeta de wallpapers no existe (se vigilará hasta que aparezca): {:?}", config.wallpaper_dir);
        }
        directory_watcher.watch(hwnd, &config.wallpaper_dir, config.scan_subfolders);

        let zone_detector = ZoneDetector::new(hwnd, config.zones.clone());
        let applied_config = config.clone();
        let mut state = Box::new(AppState {
            hwnd,
            config: Arc::new(Mutex::new(config)),
            applied_config,
            mouse_detector: MouseDetector::new(hwnd),
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
            current_wallpaper,
            history: Vec::new(),
            last_mtimes: HashMap::new(),
            canvas: None,
        });
        state.zone_detector.thumb_cache = Some(state.thumb_cache.clone());
        state.zone_detector.scanned_images = Some(state.scanned_images.clone());
        state.sync_detection();

        // El hook se instala antes de publicar el estado: si falla, el estado nunca llegó a la ventana.
        let focus_detector = FocusDetector::new(hwnd)?;

        // Auto-arranque: escanear la galería configurada al iniciar el programa
        state.rescan();
        state.restart_rotation_timer();
        state.ensure_canvas();

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&mut *state as *mut AppState) as isize);
        Ok((state, focus_detector))
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
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = UnregisterClassW(as_pcwstr(&self.class_name_wide), self.hinstance);
        }
        log_info!("Recursos de la aplicación y clase de ventana liberados.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_index_wraps_around() {
        let never = |_: usize| -> usize { unreachable!() };
        assert_eq!(step_index(Step::Next, Some(2), 3, never), Some(0));
        assert_eq!(step_index(Step::Previous, Some(0), 3, never), Some(2));
        assert_eq!(step_index(Step::Next, None, 3, never), Some(0));
        assert_eq!(step_index(Step::Previous, None, 3, never), Some(2));
        assert_eq!(step_index(Step::Next, Some(0), 0, never), None);
    }

    #[test]
    fn pick_indices_follow_order_and_stay_distinct() {
        let never = |_: usize| -> usize { unreachable!() };
        assert_eq!(pick_indices(Step::Next, Some(3), 5, 3, never), vec![4, 0, 1]);
        assert_eq!(pick_indices(Step::Previous, Some(1), 5, 2, never), vec![0, 4]);
        // Aleatorio: distintas entre sí y distintas de la actual mientras haya suficientes.
        let mut seed = 0usize;
        let picks = pick_indices(Step::Random, Some(2), 6, 4, |bound| { seed += 1; seed % bound });
        assert_eq!(picks.len(), 4);
        assert!(!picks.contains(&2));
        let unique: std::collections::HashSet<_> = picks.iter().collect();
        assert_eq!(unique.len(), 4);
        // Menos imágenes que monitores: se repiten.
        assert_eq!(pick_indices(Step::Next, None, 1, 3, never), vec![0, 0, 0]);
        assert!(pick_indices(Step::Next, None, 0, 2, never).is_empty());
    }

    #[test]
    fn random_step_never_repeats_current() {
        for pick in 0..4 {
            let index = step_index(Step::Random, Some(2), 5, |_| pick).unwrap();
            assert_ne!(index, 2);
            assert!(index < 5);
        }
        assert_eq!(step_index(Step::Random, Some(0), 1, |_| 0), Some(0));
    }
}
