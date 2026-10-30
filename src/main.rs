#![windows_subsystem = "windows"]

mod app;
mod config;
mod detector;
mod log;
mod panel;
mod scanner;
mod thumbs;
mod tray;
mod wallpaper;
mod watcher;
mod winutil;

use app::App;
use log::log_resource_usage;
use winutil::SingleInstance;

const MUTEX_NAME: &str = "Local\\SWEPR_SingleInstance_Mutex";

fn main() {
    // Inicialización del sistema de logs
    let log_path = match log::init() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error al inicializar el logger: {e}");
            return;
        }
    };

    let pid = std::process::id();
    log_info!("==================================================");
    log_info!("SWEPR iniciando | PID: {} | Log: {:?}", pid, log_path);
    log_resource_usage();

    // Garantizar instancia única mediante Mutex del sistema
    let _instance = match SingleInstance::try_create(MUTEX_NAME) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            log_warn!("Ya existe otra instancia de SWEPR en ejecución. Terminando proceso actual.");
            return;
        }
        Err(e) => {
            log_error!("Error al verificar instancia única: {e}");
            return;
        }
    };

    // Creación de la aplicación, configuración, bandeja y detectores
    let app = match App::new() {
        Ok(app) => app,
        Err(e) => {
            log_error!("Error fatal al inicializar la aplicación: {e}");
            return;
        }
    };

    // Bucle de mensajes principal
    let exit_code = app.run_message_loop();

    log_info!("SWEPR finalizado limpiamente con código de salida: {exit_code}");
    log_info!("==================================================");
}
