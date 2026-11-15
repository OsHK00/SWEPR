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
            Ok(state) => !matches!(state, QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE | QUNS_BUSY),
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

/// Compara rutas como lo hace Windows: sin distinguir mayúsculas y con cualquier separador.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| path.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    normalize(a) == normalize(b)
}

/// Número pseudoaleatorio en `0..bound` (xorshift sembrado con la hora; basta para el modo aleatorio).
pub fn random_below(bound: usize) -> usize {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0);
    if bound <= 1 { return 0; }
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        x = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    (x % bound as u64) as usize
}

/// Acento por defecto de Windows 11 sobre fondo oscuro (#4CC2FF).
pub const DEFAULT_ACCENT: (u8, u8, u8) = (0x4C, 0xC2, 0xFF);
/// Acento por defecto de Windows 11 sobre fondo claro (#005FB8).
pub const DEFAULT_ACCENT_ON_LIGHT: (u8, u8, u8) = (0x00, 0x5F, 0xB8);

/// Tema elegido en la configuración: 0 = el de Windows, 1 = oscuro, 2 = claro.
static THEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

pub fn set_theme(choice: u8) { THEME.store(choice, std::sync::atomic::Ordering::Relaxed); }

/// ¿Se pinta la interfaz en claro? Con "el de Windows", lo que diga el sistema para las apps.
pub fn light_theme() -> bool {
    match THEME.load(std::sync::atomic::Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => system_light(),
    }
}

/// Modo de las apps de Windows (claro u oscuro), releído cada pocos segundos.
fn system_light() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    if let Ok(guard) = CACHE.lock() {
        if let Some((read, light)) = *guard {
            if read.elapsed() < Duration::from_secs(5) { return light; }
        }
    }
    let light = unsafe {
        use windows::core::w;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
        let mut value = 0u32;
        let mut size = 4u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        )
        .is_ok()
            && value != 0
    };
    if let Ok(mut guard) = CACHE.lock() { *guard = Some((Instant::now(), light)); }
    light
}

/// Color de acento del sistema como (r, g, b): la variante clara sobre el tema oscuro y la
/// oscura sobre el claro, como hace Windows. Se relee cada pocos segundos por si cambia.
pub fn accent_rgb() -> (u8, u8, u8) {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    type Rgb = (u8, u8, u8);
    type Cached = Option<(Instant, (Rgb, Rgb))>;
    static CACHE: Mutex<Cached> = Mutex::new(None);
    let pick = |(on_dark, on_light): (Rgb, Rgb)| if light_theme() { on_light } else { on_dark };
    if let Ok(guard) = CACHE.lock() {
        if let Some((read, pair)) = *guard {
            if read.elapsed() < Duration::from_secs(5) { return pick(pair); }
        }
    }
    let pair = read_accent().unwrap_or((DEFAULT_ACCENT, DEFAULT_ACCENT_ON_LIGHT));
    if let Ok(mut guard) = CACHE.lock() { *guard = Some((Instant::now(), pair)); }
    pick(pair)
}

/// (acento sobre oscuro, acento sobre claro).
type AccentPair = ((u8, u8, u8), (u8, u8, u8));

fn read_accent() -> Option<AccentPair> {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD};
    unsafe {
        // Paleta de 8 colores RGBA; la segunda entrada es el acento "claro 2" y la quinta, "oscuro 1".
        let mut palette = [0u8; 32];
        let mut size = palette.len() as u32;
        let read = RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(palette.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if read.is_ok() && size >= 20 {
            return Some(((palette[4], palette[5], palette[6]), (palette[16], palette[17], palette[18])));
        }
        // Sin paleta: el acento base (ABGR).
        let mut value = 0u32;
        let mut size = 4u32;
        let read = RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\DWM"),
            w!("AccentColor"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        );
        let base = (value as u8, (value >> 8) as u8, (value >> 16) as u8);
        read.is_ok().then_some((base, base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_ignores_case_and_separators() {
        assert!(same_path(Path::new(r"C:\Fotos\A.jpg"), Path::new("c:/fotos/a.JPG")));
        assert!(same_path(Path::new(r"C:\Fotos\"), Path::new(r"C:\fotos")));
        assert!(!same_path(Path::new(r"C:\Fotos\a.jpg"), Path::new(r"C:\Fotos\b.jpg")));
    }

    #[test]
    fn random_below_stays_in_range() {
        for _ in 0..1000 {
            assert!(random_below(7) < 7);
        }
        assert_eq!(random_below(1), 0);
        assert_eq!(random_below(0), 0);
    }
}
