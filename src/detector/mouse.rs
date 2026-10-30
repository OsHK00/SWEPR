use crate::log_info;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::{
    RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK, RIDEV_REMOVE,
};

/// Administrador de registro y baja de Raw Input para eventos del mouse.
pub struct MouseDetector {
    hwnd: HWND,
    is_registered: bool,
}

impl MouseDetector {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            is_registered: false,
        }
    }

    /// Registra la recepción de eventos Raw Input del mouse con RIDEV_INPUTSINK.
    /// Solo debe invocarse cuando el escritorio tiene el foco.
    pub fn register(&mut self) -> Result<(), String> {
        if self.is_registered {
            return Ok(());
        }

        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01, // Generic Desktop Controls
            usUsage: 0x02,     // Mouse
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: self.hwnd,
        };

        unsafe {
            RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32)
                .map_err(|e| format!("Fallo al registrar Raw Input (RIDEV_INPUTSINK): {e}"))?;
        }

        self.is_registered = true;
        log_info!("Raw Input registrado (escritorio con foco) | HWND: {:?}", self.hwnd);
        Ok(())
    }

    /// Desregistra Raw Input del mouse liberando el sumidero de entrada.
    /// Invocado de inmediato cuando el escritorio pierde el foco.
    pub fn unregister(&mut self) -> Result<(), String> {
        if !self.is_registered {
            return Ok(());
        }

        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_REMOVE,
            hwndTarget: HWND(std::ptr::null_mut()),
        };

        unsafe {
            let _ = RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
        }

        self.is_registered = false;
        log_info!("Raw Input desregistrado (foco perdido en el escritorio)");
        Ok(())
    }

    #[allow(dead_code)]
    #[inline]
    pub fn is_registered(&self) -> bool {
        self.is_registered
    }
}

impl Drop for MouseDetector {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}
