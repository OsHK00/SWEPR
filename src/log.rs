use std::fs::{create_dir_all, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS, GR_USEROBJECTS};
use crate::{log_error, log_info};

static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Inicializa el sistema de logs escribiendo en %LOCALAPPDATA%\swepr\log.txt.
/// Si el directorio no existe, lo crea.
pub fn init() -> Result<PathBuf, String> {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .map_err(|e| format!("No se pudo leer %LOCALAPPDATA%: {e}"))?;

    let dir = PathBuf::from(local_app_data).join("swepr");
    create_dir_all(&dir)
        .map_err(|e| format!("Error al crear directorio de logs {:?}: {e}", dir))?;

    let log_path = dir.join("log.txt");
    // Rotación: si el log supera 5 MB pasa a log.old.txt (se conserva la sesión anterior).
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > MAX_LOG_BYTES {
            let old_path = dir.join("log.old.txt");
            if std::fs::rename(&log_path, &old_path).is_err() {
                let _ = File::create(&log_path);
            }
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("Error al abrir {:?}: {e}", log_path))?;

    let mut guard = LOG_FILE.lock().map_err(|e| format!("Error al bloquear Mutex de log: {e}"))?;
    *guard = Some(file);

    Ok(log_path)
}

/// Obtiene la marca de tiempo local actual en formato YYYY-MM-DD HH:MM:SS.mmm.
fn current_timestamp() -> String {
    let st = unsafe {
        GetLocalTime()
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

/// Escribe una línea de log con marca de tiempo y nivel, asegurando un flush inmediato a disco.
pub fn write_log(level: &str, message: &str) {
    let timestamp = current_timestamp();
    let line = format!("[{timestamp}] [{level}] {message}\n");

    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(ref mut file) = *guard {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

/// Registra el uso de recursos del proceso: objetos GDI/USER, working set y memoria privada.
pub fn log_resource_usage() {
    unsafe {
        let process = GetCurrentProcess();
        let gdi = GetGuiResources(process, GR_GDIOBJECTS);
        let user = GetGuiResources(process, GR_USEROBJECTS);
        let mut counters: PROCESS_MEMORY_COUNTERS = Default::default();
        if GetProcessMemoryInfo(process, &mut counters, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32).is_ok() {
            log_info!(
                "Recursos: objetos GDI {gdi} | USER {user} | WorkingSet: {} MB | Privada: {} MB",
                counters.WorkingSetSize / (1024 * 1024),
                counters.PagefileUsage / (1024 * 1024)
            );
        } else {
            log_error!("GetProcessMemoryInfo falló");
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log::write_log("INFO", &format!($($arg)*))
    };
}

/// Detalle para depurar (cada miniatura, cada cambio de foco...). Solo existe en builds de
/// desarrollo: en release no formatea ni escribe nada.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            $crate::log::write_log("DEBUG", &format!($($arg)*))
        }
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::log::write_log("WARN", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::log::write_log("ERROR", &format!($($arg)*))
    };
}
