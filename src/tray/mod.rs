use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
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

fn tr(key: &str) -> Vec<u16> { to_wide_chars(&crate::i18n::t(key)) }

/// Gestor del icono de bandeja del sistema (System Tray) y su menú contextual.
pub struct TrayIcon {
    hwnd: HWND,
    is_added: bool,
}

/// Hace que los menús emergentes sigan el tema oscuro/claro de Windows.
/// Usa las funciones sin nombre de uxtheme.dll (ordinales 135 y 136, Windows 10 1903+);
/// si no existen, el menú se queda con el estilo claro de siempre.
fn follow_system_menu_theme() {
    use windows::core::PCSTR;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    unsafe {
        let Ok(uxtheme) = LoadLibraryW(windows::core::w!("uxtheme.dll")) else { return; };
        let set_preferred_app_mode = GetProcAddress(uxtheme, PCSTR(135 as *const u8));
        let flush_menu_themes = GetProcAddress(uxtheme, PCSTR(136 as *const u8));
        if let (Some(set_mode), Some(flush)) = (set_preferred_app_mode, flush_menu_themes) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            const ALLOW_DARK: i32 = 1;
            set_mode(ALLOW_DARK);
            flush();
        }
    }
}

impl TrayIcon {
    pub fn new(hwnd: HWND) -> Self {
        follow_system_menu_theme();
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

        nid.hIcon = crate::resources::app_icon();

        let tip_chars = tr("SWEPR - Wallpaper Selector");
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

            let title_detect = tr("Detection enabled");
            let title_refresh = tr("Refresh gallery");
            let title_folder = tr("Open folder");
            let title_config = tr("Settings...");
            let title_exit = tr("Exit");

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
