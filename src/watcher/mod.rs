use crate::log_info;
use crate::log_warn;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_WATCHER_CHANGED: u32 = WM_APP + 30;

/// Cada cuánto se reintenta abrir la carpeta si no existe o se perdió.
const RETRY_INTERVAL: Duration = Duration::from_secs(2);
/// Tamaño del búfer de notificaciones (el máximo útil en carpetas locales y de red).
const BUFFER_SIZE: usize = 64 * 1024;

pub struct DirectoryWatcher {
    stop: Option<Arc<AtomicBool>>,
    thread: Option<thread::JoinHandle<()>>,
    last_watched: Option<PathBuf>,
    last_subtree: bool,
}

/// Espera `duration` en pasos cortos para poder detenerse enseguida.
fn sleep_unless_stopped(stop: &AtomicBool, duration: Duration) {
    let step = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < duration && !stop.load(Ordering::Relaxed) {
        thread::sleep(step);
        waited += step;
    }
}

fn notify(hwnd_val: isize) {
    unsafe { let _ = PostMessageW(HWND(hwnd_val as *mut _), WM_APP_WATCHER_CHANGED, WPARAM(0), LPARAM(0)); }
}

/// ¿Alguna de las notificaciones del búfer afecta a la galería?
unsafe fn buffer_has_relevant_change(buffer: &[u8], bytes: usize, watch_subtree: bool) -> bool {
    let mut offset: usize = 0;
    while offset + std::mem::size_of::<FILE_NOTIFY_INFORMATION>() <= bytes {
        let info = &*(buffer.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION);
        let action = info.Action;
        if action == FILE_ACTION_ADDED
            || action == FILE_ACTION_MODIFIED
            || action == FILE_ACTION_REMOVED
            || action == FILE_ACTION_RENAMED_NEW_NAME
            || action == FILE_ACTION_RENAMED_OLD_NAME
        {
            let name_len = (info.FileNameLength as usize) / 2;
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(info.FileName.as_ptr(), name_len));
            let path = Path::new(&name);
            // Sin extensión casi siempre es una carpeta: con subcarpetas, también cuenta.
            let is_folder_like = path.extension().is_none() && action != FILE_ACTION_MODIFIED;
            if crate::scanner::is_supported_image(path) || (watch_subtree && is_folder_like) {
                return true;
            }
        }
        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }
    false
}

/// Vigila la carpeta hasta que se pida parar. Si la carpeta no existe o desaparece,
/// reintenta cada poco y avisa en cuanto vuelve a estar disponible.
fn watch_loop(folder: PathBuf, watch_subtree: bool, stop: Arc<AtomicBool>, hwnd_val: isize) {
    let folder_wide: Vec<u16> = folder.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    #[repr(C, align(4))]
    struct AlignedBuffer([u8; BUFFER_SIZE]);
    let mut buffer = Box::new(AlignedBuffer([0u8; BUFFER_SIZE]));
    let mut warned_missing = false;
    let mut had_handle = false;

    while !stop.load(Ordering::Relaxed) {
        let dir_handle: HANDLE = match unsafe {
            CreateFileW(
                windows::core::PCWSTR(folder_wide.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        } {
            Ok(handle) => handle,
            Err(e) => {
                if !warned_missing {
                    log_warn!("No se pudo abrir la carpeta para vigilarla ({e}); se reintentará: {:?}", folder);
                    warned_missing = true;
                }
                sleep_unless_stopped(&stop, RETRY_INTERVAL);
                continue;
            }
        };
        if warned_missing || had_handle {
            // La carpeta volvió (o se recuperó la vigilancia): puede haber cambios que no vimos.
            notify(hwnd_val);
        }
        warned_missing = false;
        had_handle = true;
        log_info!("Vigilancia de directorio iniciada: {:?}", folder);

        let mut filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE;
        if watch_subtree {
            filter |= FILE_NOTIFY_CHANGE_DIR_NAME;
        }
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let mut bytes_returned: u32 = 0;
            let res = unsafe {
                ReadDirectoryChangesW(
                    dir_handle,
                    buffer.0.as_mut_ptr() as *mut _,
                    buffer.0.len() as u32,
                    watch_subtree,
                    filter,
                    Some(&mut bytes_returned),
                    None,
                    None,
                )
            };
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if let Err(e) = res {
                // Carpeta borrada, renombrada o unidad desconectada: volver a abrirla.
                log_warn!("Vigilancia interrumpida ({e}); se reintentará: {:?}", folder);
                break;
            }
            if bytes_returned == 0 {
                // Desbordamiento: hubo más cambios de los que caben. Reescanear todo.
                log_info!("Watcher: demasiados cambios a la vez, se reescanea la carpeta.");
                notify(hwnd_val);
                continue;
            }
            if unsafe { buffer_has_relevant_change(&buffer.0, bytes_returned as usize, watch_subtree) } {
                notify(hwnd_val);
            }
        }
        unsafe { let _ = windows::Win32::Foundation::CloseHandle(dir_handle); }
        sleep_unless_stopped(&stop, RETRY_INTERVAL);
    }
}

impl DirectoryWatcher {
    pub fn new() -> Self {
        Self {
            stop: None,
            thread: None,
            last_watched: None,
            last_subtree: false,
        }
    }

    /// Vigila cambios en la carpeta. Envía WM_APP_WATCHER_CHANGED a hwnd ante cambios relevantes.
    /// Si la carpeta ya estaba vigilada con el mismo modo de subcarpetas, no hace nada.
    /// La carpeta puede no existir todavía: se empieza a vigilar en cuanto aparezca.
    pub fn watch<P: AsRef<Path>>(&mut self, hwnd: HWND, folder: P, watch_subtree: bool) {
        let folder = folder.as_ref().to_path_buf();
        if self.last_watched.as_deref() == Some(folder.as_path()) && self.last_subtree == watch_subtree && self.thread.is_some() {
            return;
        }

        self.stop_previous();

        self.last_watched = Some(folder.clone());
        self.last_subtree = watch_subtree;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let hwnd_val = hwnd.0 as isize;
        self.thread = Some(thread::spawn(move || watch_loop(folder, watch_subtree, stop_clone, hwnd_val)));
        self.stop = Some(stop);
    }

    fn stop_previous(&mut self) {
        if let Some(flag) = self.stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        if let Some(t) = self.thread.take() {
            while !t.is_finished() {
                unsafe {
                    let thread_handle = HANDLE(t.as_raw_handle() as *mut _);
                    let _ = windows::Win32::System::IO::CancelSynchronousIo(thread_handle);
                }
                thread::sleep(Duration::from_millis(10));
            }
            let _ = t.join();
        }
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        self.stop_previous();
    }
}
