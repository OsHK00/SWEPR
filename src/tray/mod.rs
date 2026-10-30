use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, LoadIconW, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, IDI_APPLICATION, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_NULL,
};
use crate::{log_info, log_warn};
use crate::winutil::{as_pcwstr, get_cursor_position, to_wide_chars};

pub const WM_APP_TRAY_NOTIFICATION: u32 = WM_APP + 2;

pub const IDM_TOGGLE_DETECTION: usize = 1002;
pub const IDM_REFRESH_GALLERY: usize = 1003;
pub const IDM_OPEN_FOLDER: usize = 1004;
pub const IDM_SETTINGS: usize = 1005;
pub const IDM_EXIT: usize = 1008;

/// Gestor del icono de bandeja del sistema (System Tray) y su menú contextual.
pub struct TrayIcon {
    hwnd: HWND,
    is_added: bool,
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            is_added: false,
        }
    }

    /// Agrega el icono de la aplicación al área de notificación de la barra de tareas.
    pub fn add(&mut self) -> Result<(), String> {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY_NOTIFICATION,
            ..Default::default()
        };

        unsafe {
            let hicon = LoadIconW(None, IDI_APPLICATION)
                .map_err(|e| format!("Fallo al cargar IDI_APPLICATION: {e}"))?;
            nid.hIcon = hicon;
        }

        let tip_chars = to_wide_chars("SWEPR - Selector de Fondos");
        let copy_len = tip_chars.len().min(nid.szTip.len());
        nid.szTip[..copy_len].copy_from_slice(&tip_chars[..copy_len]);

        unsafe {
            let ok = Shell_NotifyIconW(NIM_ADD, &nid);
            if !ok.as_bool() {
                let err = windows::Win32::Foundation::GetLastError();
                log_warn!(
                    "No se pudo registrar el icono en la bandeja (código: {:?}). Se reintentará con TaskbarCreated.",
                    err
                );
                self.is_added = false;
                return Ok(());
            }
        }

        self.is_added = true;
        log_info!("Icono de bandeja agregado correctamente al área de notificación.");
        Ok(())
    }

    /// Retira el icono del área de notificación.
    pub fn remove(&mut self) {
        if !self.is_added {
            return;
        }

        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        };

        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }

        self.is_added = false;
        log_info!("Icono de bandeja retirado limpiamente.");
    }

    /// Vuelve a agregar el icono (invocado tras reiniciar explorer.exe / TaskbarCreated).
    pub fn re_add(&mut self) -> Result<(), String> {
        self.is_added = false;
        self.add()
    }

    #[inline]
    pub fn is_added(&self) -> bool {
        self.is_added
    }

    /// Despliega el menú contextual Win32 nativo en la posición actual del cursor.
    pub fn show_context_menu(&self, detection_enabled: bool) -> Option<usize> {
        unsafe {
            let hmenu = match CreatePopupMenu() {
                Ok(menu) => menu,
                Err(e) => {
                    log_warn!("Error al crear menú de bandeja: {e}");
                    return None;
                }
            };

            let title_detect = to_wide_chars("Detección activada");
            let title_refresh = to_wide_chars("Actualizar galería");
            let title_folder = to_wide_chars("Abrir carpeta");
            let title_config = to_wide_chars("Configuración...");
            let title_exit = to_wide_chars("Salir");

            let det_flags = if detection_enabled {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING | MF_UNCHECKED
            };
            let _ = AppendMenuW(hmenu, det_flags, IDM_TOGGLE_DETECTION, as_pcwstr(&title_detect));
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_REFRESH_GALLERY, as_pcwstr(&title_refresh));
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_OPEN_FOLDER, as_pcwstr(&title_folder));
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_SETTINGS, as_pcwstr(&title_config));
            let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(hmenu, MF_STRING, IDM_EXIT, as_pcwstr(&title_exit));

            let pt = get_cursor_position().unwrap_or_default();

            let _ = SetForegroundWindow(self.hwnd);

            let selected = TrackPopupMenu(
                hmenu,
                TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_RETURNCMD,
                pt.x,
                pt.y,
                0,
                self.hwnd,
                None,
            );

            let _ = PostMessageW(self.hwnd, WM_NULL, WPARAM(0), LPARAM(0));

            let _ = DestroyMenu(hmenu);

            if selected.0 > 0 {
                Some(selected.0 as usize)
            } else {
                None
            }
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.remove();
    }
}
