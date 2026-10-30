use std::path::Path;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, POINT, RECT,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};

/// Convierte una cadena Rust a una secuencia UTF-16 terminada en null.
pub fn to_wide_chars(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Crea un PCWSTR a partir de un slice UTF-16 que debe estar terminado en null.
pub fn as_pcwstr(wide: &[u16]) -> PCWSTR {
    PCWSTR::from_raw(wide.as_ptr())
}

/// Obtiene el nombre de clase de una ventana dada.
pub fn get_window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let len = unsafe {
        GetClassNameW(hwnd, &mut buffer)
    };
    if len > 0 {
        String::from_utf16_lossy(&buffer[..len as usize])
    } else {
        String::new()
    }
}

/// Obtiene el texto o título de una ventana dada.
pub fn get_window_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    let len = unsafe {
        GetWindowTextW(hwnd, &mut buffer)
    };
    if len > 0 {
        String::from_utf16_lossy(&buffer[..len as usize])
    } else {
        String::new()
    }
}

/// Obtiene el PID y el nombre del ejecutable del proceso dueño de la ventana.
pub fn get_window_process_info(hwnd: HWND) -> (u32, String) {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }

    if pid == 0 {
        return (0, "<desconocido>".to_string());
    }

    let process_name = unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut name_buf = [0u16; 1024];
            let mut size = name_buf.len() as u32;
            let result = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0),
                PWSTR(name_buf.as_mut_ptr()),
                &mut size,
            );
            let _ = CloseHandle(handle);

            if result.is_ok() && size > 0 {
                let full_path = String::from_utf16_lossy(&name_buf[..size as usize]);
                Path::new(&full_path)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or(full_path)
            } else {
                "<desconocido>".to_string()
            }
        } else {
            "<acceso denegado>".to_string()
        }
    };

    (pid, process_name)
}

/// Obtiene el HWND de la ventana actualmente en primer plano.
pub fn get_current_foreground_window() -> HWND {
    unsafe {
        GetForegroundWindow()
    }
}

/// Obtiene las coordenadas absolutas actuales del cursor del mouse en pantalla.
pub fn get_cursor_position() -> Option<POINT> {
    let mut pt = POINT::default();
    let ok = unsafe {
        GetCursorPos(&mut pt)
    };
    if ok.is_ok() {
        Some(pt)
    } else {
        None
    }
}

/// Comprueba si alguno de los botones principales del mouse (izquierdo, derecho, central) está presionado.
/// Utilizado para descartar activaciones durante arrastres de iconos o selecciones en el escritorio.
pub fn is_any_mouse_button_down() -> bool {
    unsafe {
        let l_down = (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0;
        let r_down = (GetAsyncKeyState(VK_RBUTTON.0 as i32) as u16 & 0x8000) != 0;
        let m_down = (GetAsyncKeyState(VK_MBUTTON.0 as i32) as u16 & 0x8000) != 0;
        l_down || r_down || m_down
    }
}

/// Comprueba si el usuario se encuentra en un estado que prohíbe notificaciones (juego D3D fullscreen, presentación o modo ocupado).
pub fn is_notification_allowed() -> bool {
    unsafe {
        match SHQueryUserNotificationState() {
            Ok(state) => match state {
                QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE | QUNS_BUSY => false,
                _ => true,
            },
            Err(_) => true,
        }
    }
}

/// Obtiene el identificador HMONITOR y los límites RECT del monitor donde se ubica un punto dado.
pub fn get_monitor_rect_from_point(pt: POINT) -> Option<(HMONITOR, RECT)> {
    unsafe {
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        if hmon.is_invalid() {
            return None;
        }

        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            Some((hmon, mi.rcMonitor))
        } else {
            None
        }
    }
}

/// Garantiza la ejecución de una única instancia de la aplicación mediante un Mutex con nombre.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    pub fn try_create(name: &str) -> Result<Option<Self>, String> {
        let name_wide = to_wide_chars(name);
        unsafe {
            let handle = CreateMutexW(None, true, as_pcwstr(&name_wide))
                .map_err(|e| format!("Error en CreateMutexW: {e}"))?;

            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Ok(None);
            }

            Ok(Some(Self { handle }))
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            if !self.handle.is_invalid() {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}
