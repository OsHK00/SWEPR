use std::fs::{create_dir_all, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{GetGuiResources, GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, GET_GUI_RESOURCES_FLAGS};
use crate::{log_error, log_info};

static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Inicializa el sistema de logs escribiendo en %LOCALAPPDATA%\swepr\log.txt.
/// Si el directorio no existe, lo crea.
pub fn init() -> Result<PathBuf, String> {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .map_err(|e| format!("No se pudo leer %LOCALAPPDATA%: {e}"))?;

    let dir = PathBuf::from(local_app_data).join("swepr");
    create_dir_all(&dir)
        .map_err(|e| format!("Error al crear directorio de logs {:?}: {e}", dir))?;

    let log_path = dir.join("log.txt");
    // Rotacion simple: si el log supera 5 MB, se trunca para evitar crecimiento indefinido
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 5 * 1024 * 1024 {
            let _ = File::create(&log_path);
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

/// Registra el uso de recursos del proceso: memoria GDI/USER en uso, working set y handles.
pub fn log_resource_usage() {
    unsafe {
        let pid = GetCurrentProcessId();
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);

        if let Ok(handle) = process {
            let gui_handles = GetGuiResources(handle, GET_GUI_RESOURCES_FLAGS(0));

            let mut counters: PROCESS_MEMORY_COUNTERS = Default::default();
            let ok = GetProcessMemoryInfo(handle, &mut counters, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32);

            if ok.is_ok() {
                let working_set_mb = counters.WorkingSetSize / (1024 * 1024);
                log_info!(
                    "Recursos: PID {pid} | Handles GDI/USER: {gui_handles} | WorkingSet: {working_set_mb} MB"
                );
            } else {
                log_error!("GetProcessMemoryInfo falló para PID {pid}");
            }
            let _ = CloseHandle(handle);
        } else {
            log_error!("No se pudo abrir proceso para medir recursos (PID: {pid})");
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log::write_log("INFO", &format!($($arg)*))
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
