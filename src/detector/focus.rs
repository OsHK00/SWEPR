use std::sync::atomic::{AtomicUsize, Ordering};
use crate::{log_debug, log_info};
use crate::winutil::{get_window_class_name, get_window_process_info, get_window_text};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

/// Mensaje personalizado de la aplicación para notificar un cambio de ventana en primer plano.
pub const WM_APP_FOREGROUND_CHANGED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

static TARGET_HWND: AtomicUsize = AtomicUsize::new(0);

/// Determina si una clase de ventana corresponde al entorno de escritorio de Windows.
/// Clases de escritorio reconocidas:
/// - "Progman": Program Manager (escritorio tradicional).
/// - "WorkerW": Worker Window (escritorio moderno con fondo activo o transiciones).
/// - "Shell_TrayWnd": Barra de tareas principal de Windows.
pub fn is_desktop_window(class_name: &str) -> bool {
    matches!(class_name, "Progman" | "WorkerW" | "Shell_TrayWnd")
}

/// Callback invocado por el sistema operativo cuando cambia la ventana en primer plano.
/// Realiza el mínimo trabajo posible y delega con PostMessage al hilo principal.
unsafe extern "system" fn win_event_proc(
    _h_win_event_hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        let target = TARGET_HWND.load(Ordering::Relaxed);
        if target != 0 {
            let _ = PostMessageW(
                HWND(target as *mut _),
                WM_APP_FOREGROUND_CHANGED,
                WPARAM(hwnd.0 as usize),
                LPARAM(0),
            );
        }
    }
}

/// Gestor de ciclo de vida del hook de foco.
pub struct FocusDetector {
    hook: HWINEVENTHOOK,
}

impl FocusDetector {
    /// Instala el hook SetWinEventHook escuchando únicamente EVENT_SYSTEM_FOREGROUND.
    pub fn new(target_hwnd: HWND) -> Result<Self, String> {
        TARGET_HWND.store(target_hwnd.0 as usize, Ordering::Relaxed);

        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };

        if hook.is_invalid() {
            return Err("Fallo al registrar SetWinEventHook(EVENT_SYSTEM_FOREGROUND)".into());
        }

        log_info!("SetWinEventHook(EVENT_SYSTEM_FOREGROUND) instalado correctamente.");
        Ok(Self { hook })
    }
}

impl Drop for FocusDetector {
    fn drop(&mut self) {
        TARGET_HWND.store(0, Ordering::Relaxed);
        if !self.hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.hook);
            }
            log_info!("SetWinEventHook(EVENT_SYSTEM_FOREGROUND) desinstalado limpiamente.");
        }
    }
}

/// Evalúa la ventana en primer plano dada, registra sus propiedades en el log y devuelve si es escritorio.
pub fn evaluate_window_focus(hwnd: HWND) -> bool {
    let class_name = get_window_class_name(hwnd);
    let is_desktop = is_desktop_window(&class_name);

    // Título y proceso solo sirven para el log: en release no se consultan.
    if cfg!(debug_assertions) {
        let title = get_window_text(hwnd);
        let (pid, proc_name) = get_window_process_info(hwnd);
        log_debug!(
            "Foco cambiado -> Clase: \"{}\" | Título: \"{}\" | Proceso: \"{}\" (PID: {}) | ¿Escritorio con foco?: {}",
            class_name,
            title,
            proc_name,
            pid,
            if is_desktop { "SÍ" } else { "NO" }
        );
    }

    is_desktop
}
