use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};
use crate::log_info;
use crate::winutil::{as_pcwstr, to_wide_chars};

const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "SWEPR";

/// Configura o elimina la entrada de autoarranque en el registro del usuario actual (HKCU).
pub fn set_autostart(enable: bool) -> Result<(), String> {
    let subkey_wide = to_wide_chars(RUN_SUBKEY);
    let value_name_wide = to_wide_chars(VALUE_NAME);

    if enable {
        let current_exe = std::env::current_exe()
            .map_err(|e| format!("No se pudo obtener la ruta del ejecutable: {e}"))?;
        let cmd = format!("\"{}\"", current_exe.display());
        let cmd_wide = to_wide_chars(&cmd);

        unsafe {
            let mut hkey = HKEY::default();
            let err = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                as_pcwstr(&subkey_wide),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if err != ERROR_SUCCESS {
                return Err(format!("RegOpenKeyExW falló con código: {:?}", err));
            }

            let slice_bytes = std::slice::from_raw_parts(
                cmd_wide.as_ptr() as *const u8,
                cmd_wide.len() * std::mem::size_of::<u16>(),
            );

            let set_err = RegSetValueExW(
                hkey,
                as_pcwstr(&value_name_wide),
                0,
                REG_SZ,
                Some(slice_bytes),
            );
            let _ = RegCloseKey(hkey);

            if set_err != ERROR_SUCCESS {
                return Err(format!("RegSetValueExW falló con código: {:?}", set_err));
            }
        }
        log_info!("Autoarranque habilitado en HKCU\\{}: {}", RUN_SUBKEY, cmd);
    } else {
        unsafe {
            let mut hkey = HKEY::default();
            let err = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                as_pcwstr(&subkey_wide),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if err == ERROR_SUCCESS {
                let _ = RegDeleteValueW(hkey, as_pcwstr(&value_name_wide));
                let _ = RegCloseKey(hkey);
            }
        }
        log_info!("Autoarranque deshabilitado en HKCU\\{}", RUN_SUBKEY);
    }

    Ok(())
}

/// Comprueba si SWEPR se encuentra registrado en el autoarranque de HKCU.
#[allow(dead_code)]
pub fn is_autostart_configured() -> bool {
    let subkey_wide = to_wide_chars(RUN_SUBKEY);
    let value_name_wide = to_wide_chars(VALUE_NAME);

    unsafe {
        let mut hkey = HKEY::default();
        let err = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            as_pcwstr(&subkey_wide),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        );
        if err != ERROR_SUCCESS {
            return false;
        }

        let query_err = RegQueryValueExW(
            hkey,
            as_pcwstr(&value_name_wide),
            None,
            None,
            None,
            None,
        );
        let _ = RegCloseKey(hkey);

        query_err == ERROR_SUCCESS
    }
}
