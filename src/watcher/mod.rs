use crate::log_info;
use crate::log_warn;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_INFORMATION, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_WATCHER_CHANGED: u32 = WM_APP + 30;

pub struct DirectoryWatcher {
    stop: Option<Arc<AtomicBool>>,
    thread: Option<thread::JoinHandle<()>>,
    last_watched: Option<std::path::PathBuf>,
    last_subtree: bool,
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
    pub fn watch<P: AsRef<Path>>(&mut self, hwnd: HWND, folder: P, watch_subtree: bool) {
        let folder = folder.as_ref().to_path_buf();
        if self.last_watched.as_deref() == Some(folder.as_path()) && self.last_subtree == watch_subtree {
            return;
        }

        // Detener vigilancia anterior
        self.stop_previous();

        self.last_watched = Some(folder.clone());
        self.last_subtree = watch_subtree;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let hwnd_val = hwnd.0 as isize;

        let thread_handle = thread::spawn(move || {
            unsafe {
                let folder_wide: Vec<u16> = folder
                    .to_string_lossy()
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();

                let dir_handle = match CreateFileW(
                    windows::core::PCWSTR(folder_wide.as_ptr()),
                    FILE_LIST_DIRECTORY.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    None,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        log_warn!("No se pudo abrir carpeta para vigilancia (puede no existir): {e:?}");
                        return;
                    }
                };

                log_info!("Vigilancia de directorio iniciada: {:?}", folder);

                // ReadDirectoryChangesW requiere que el búfer receptor esté alineado a DWORD (4 bytes)
                #[repr(C, align(4))]
                struct AlignedBuffer([u8; 4096]);
                let mut buffer = AlignedBuffer([0u8; 4096]);

                loop {
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    let mut bytes_returned: u32 = 0;
                    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE;
                    let subtree = watch_subtree;

                    let res = ReadDirectoryChangesW(
                        dir_handle,
                        buffer.0.as_mut_ptr() as *mut _,
                        buffer.0.len() as u32,
                        subtree,
                        filter,
                        Some(&mut bytes_returned),
                        None,
                        None,
                    );

                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }

                    if res.is_err() || bytes_returned == 0 {
                        break;
                    }

                    // Analizar cambios - solo nos interesan archivos de imagen
                    let mut notify_change = false;
                    let mut offset: usize = 0;
                    while offset < bytes_returned as usize {
                        let info = &*(buffer.0.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION);
                        let action = info.Action;
                        if action == FILE_ACTION_ADDED
                            || action == FILE_ACTION_MODIFIED
                            || action == FILE_ACTION_REMOVED
                            || action == FILE_ACTION_RENAMED_NEW_NAME
                            || action == FILE_ACTION_RENAMED_OLD_NAME
                        {
                            let name_len = (info.FileNameLength as usize) / 2;
                            let name_slice = std::slice::from_raw_parts(info.FileName.as_ptr(), name_len);
                            let name = String::from_utf16_lossy(name_slice);
                            let name_lower = name.to_lowercase();
                            if name_lower.ends_with(".jpg") || name_lower.ends_with(".jpeg") ||
                               name_lower.ends_with(".png") || name_lower.ends_with(".bmp") {
                                notify_change = true;
                            }
                        }
                        if info.NextEntryOffset == 0 {
                            break;
                        }
                        offset += info.NextEntryOffset as usize;
                    }

                    if notify_change {
                        let _ = PostMessageW(HWND(hwnd_val as *mut _), WM_APP_WATCHER_CHANGED, WPARAM(0), LPARAM(0));
                    }
                }

                // Limpieza
                let _ = windows::Win32::Foundation::CloseHandle(dir_handle);
            }
        });

        self.stop = Some(stop);
        self.thread = Some(thread_handle);
    }

    fn stop_previous(&mut self) {
        if let Some(flag) = self.stop.take() {
            flag.store(true, Ordering::Relaxed);
        }
        if let Some(t) = self.thread.take() {
            while !t.is_finished() {
                unsafe {
                    let thread_handle = windows::Win32::Foundation::HANDLE(t.as_raw_handle() as *mut _);
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
