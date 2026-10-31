<div align="center">
    <img src="resources/logo.png" title="SWEPR" alt="SWEPR logo" width="256" />
    <h1>SWEPR</h1>
    <p>A lightweight Windows desktop wallpaper selector.</p>
</div>

SWEPR runs in the system tray and lets you browse a folder of images, preview them in a thumbnail panel, and apply a wallpaper without opening a separate desktop app.

## Features

- Scans a wallpaper folder for JPG, JPEG, PNG, and BMP images, with optional subfolder scanning.
- Opens a thumbnail panel from screen edge sensors or a keyboard shortcut. The panel can be positioned at the top, left, or right edge of the screen.
- Highlights newly discovered images in green and places them at the beginning of the gallery.
- Applies a selected wallpaper and lets you move through the collection with keyboard shortcuts.
- Provides tray menu actions for refreshing the gallery, opening the wallpaper folder, changing settings, and exiting.
- Supports English and Spanish translations. The language can be selected in Settings; additional translation files can be added to the `languages` folder.
- Offers settings for the wallpaper folder, subfolder scanning, Windows startup, panel position, scrollbar visibility, and thumbnail size.

## Using SWEPR

Start SWEPR and open its context menu from the system tray to access Settings and other actions. Choose the folder containing your wallpapers in Settings. The gallery refreshes when files in the folder change.

Default shortcuts:

| Shortcut | Action |
| --- | --- |
| `Ctrl` + `Alt` + `P` | Open the thumbnail panel |
| `Ctrl` + `Alt` + `Left` | Apply the previous wallpaper |
| `Ctrl` + `Alt` + `Right` | Apply the next wallpaper |

Settings are stored at `%LOCALAPPDATA%\swepr\config.json`. Shortcut bindings can be customized there. The language defaults to the Windows display language, or can be selected from the Settings window.

## Languages

Translation files are JSON files in `languages`, named with a language code such as `fr.json` or `fr-CA.json`. Use the keys in `en.json` and set `language_name` to the display name shown in the language selector. Place the file in the `languages` folder beside `swepr.exe`; restart SWEPR for it to appear in Settings. Missing translation keys are displayed in English.

## Project layout

```text
.
├── languages/              Translation files and language instructions
│   ├── en.json              English interface text
│   ├── es.json              Spanish interface text
│   └── README.md            Instructions for adding translations
├── resources/
│   ├── logo.png             Project logo shown in this README
│   ├── swepr.ico            Application icon for Windows
│   └── swepr.rc             Windows icon resource definition
├── src/
│   ├── app.rs               Application startup, events, and coordination
│   ├── config/
│   │   ├── mod.rs           Settings model and JSON persistence
│   │   ├── window.rs        Settings window
│   │   └── autostart.rs     Windows startup registration
│   ├── detector/
│   │   ├── mod.rs           Detector coordination
│   │   ├── zones.rs         Screen edge sensor zones
│   │   ├── mouse.rs         Mouse input detection
│   │   └── focus.rs         Desktop focus detection
│   ├── i18n.rs              Loading and selection of translations
│   ├── log.rs               Application logging
│   ├── panel/mod.rs         Thumbnail panel and its interactions
│   ├── resources.rs         Embedded application icon loader
│   ├── scanner/mod.rs       Wallpaper folder scanning
│   ├── thumbs/mod.rs        Thumbnail creation and cache
│   ├── tray/mod.rs          System tray icon and context menu
│   ├── wallpaper/mod.rs    Windows wallpaper operations
│   ├── watcher/mod.rs       Wallpaper folder change monitoring
│   ├── winutil/mod.rs       Shared Windows utility functions
│   └── main.rs              Program entry point and module declarations
├── app.manifest             Windows application manifest
├── build.rs                 Build-time Windows manifest setup
├── Cargo.toml               Rust package metadata and dependencies
└── Cargo.lock               Locked Rust dependency versions
```

## Build

SWEPR requires Windows, the Rust stable MSVC toolchain, and the Microsoft C++ build tools with a Windows SDK (for the linker).

From the repository directory, run:

```powershell
cargo build --release
```

The executable is created at `target\release\swepr.exe`. Keep the `languages` folder beside the executable so translations can be loaded when distributing or moving the application.
