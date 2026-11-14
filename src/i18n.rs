use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

static ACTIVE_LANGUAGE: Mutex<Option<String>> = Mutex::new(None);
static TRANSLATIONS: Mutex<Option<(String, HashMap<String, String>)>> = Mutex::new(None);

fn locale_name() -> String {
    let mut name = [0u16; 85];
    let length = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut name) };
    if length > 0 {
        String::from_utf16_lossy(&name[..length as usize - 1]).replace('_', "-").to_lowercase()
    } else {
        std::env::var("LANG").unwrap_or_default().split('.').next().unwrap_or("").replace('_', "-").to_lowercase()
    }
}

fn language_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(mut dir) = exe.parent().map(PathBuf::from) {
            for _ in 0..4 {
                directories.push(dir.join("languages"));
                if !dir.pop() { break; }
            }
        }
    }
    // Solo junto al ejecutable (y sus carpetas superiores, para `cargo run`): el directorio de
    // trabajo depende de cómo se lanzó SWEPR y podría cargar traducciones ajenas.
    directories
}

fn load_translations(language: &str) -> HashMap<String, String> {
    let locale = if language == "auto" { locale_name() } else { language.to_lowercase() };
    let base = locale.split('-').next().unwrap_or("es");
    let candidates = ["es.json".to_string(), "en.json".to_string(), format!("{base}.json"), format!("{locale}.json")];
    let mut translations = HashMap::new();
    for directory in language_directories().into_iter().rev() {
        for filename in &candidates {
            if let Ok(contents) = std::fs::read_to_string(directory.join(filename)) {
                if let Ok(parsed) = serde_json::from_str::<HashMap<String, String>>(&contents) {
                    translations.extend(parsed);
                }
            }
        }
    }
    translations
}

pub fn set_language(language: String) {
    if let Ok(mut active) = ACTIVE_LANGUAGE.lock() {
        *active = Some(language);
    }
}

pub fn available_languages() -> Vec<(String, String)> {
    let mut languages = std::collections::BTreeMap::new();
    for directory in language_directories().into_iter().rev() {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") { continue; }
                let Some(code) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_lowercase) else { continue; };
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    let name = serde_json::from_str::<serde_json::Value>(&contents).ok()
                        .and_then(|value| value.get("language_name")?.as_str().map(str::to_string))
                        .unwrap_or_else(|| code.clone());
                    languages.insert(code, name);
                }
            }
        }
    }
    languages.into_iter().collect()
}

pub fn t(key: &str) -> String {
    let language = ACTIVE_LANGUAGE.lock().ok().and_then(|value| value.clone()).unwrap_or_else(|| "auto".into());
    if let Ok(mut cache) = TRANSLATIONS.lock() {
        if cache.as_ref().map(|(cached_language, _)| cached_language != &language).unwrap_or(true) {
            let translations = load_translations(&language);
            *cache = Some((language, translations));
        }
        if let Some((_, translations)) = cache.as_ref() {
            return translations.get(key).cloned().unwrap_or_else(|| key.to_string());
        }
    }
    key.to_string()
}
