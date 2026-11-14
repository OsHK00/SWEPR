# Changelog

This file tracks user-facing changes between releases. Commits do not imply a new release.

## Unreleased

### Added

- "SWEPR canvas" wallpaper change mode (Settings > Wallpaper > Wallpaper change): SWEPR draws the wallpaper itself behind the desktop icons with Direct2D, so changes start instantly with a smooth fade synchronized with the display, also on each monitor separately. The Windows wallpaper is updated silently in the background, so Task View, accent colors, and other apps see the same image and the desktop stays correct when SWEPR closes. The canvas is recreated automatically if Explorer restarts or monitors change, and SWEPR falls back to Windows if it cannot attach. The previous Windows modes (instant or with fade) are still available.
- Canvas transitions: Fade, Slide, Zoom in, Wipe, Circle, or a random effect each time, with a duration from 0.3 to 1.5 seconds.
- When a transition ends, the canvas asks Direct2D and the graphics driver to release cached memory.

## [1.2.1]

### Added

- Buttons on the hovered thumbnail or carousel card: add to or remove from favorites, and remove from the gallery.
- Remove from gallery: the image is hidden from SWEPR but its file is not deleted. Settings > Removed images lists them and brings them back. This replaces "Move to Recycle Bin" in the thumbnail menu.
- "Different wallpaper on each monitor": next, random, and automatic changes put a different image on each monitor, and choosing an image in the panel changes only the monitor where the panel is.

### Fixed

- Hover animations in the carousel and the strip were choppy when moving the mouse quickly. Animations now advance by elapsed time and no longer stall while the mouse moves, and the panel no longer repaints on every mouse movement.
- In Pro (carousel) mode, the options that only apply to the Low power strip (panel position and scrollbar) can no longer be changed.

## [1.2.0]

### Added

- Windows installer (Inno Setup). It installs for the current user without administrator rights, and the setup of a newer version updates the installed one in place: it closes SWEPR, keeps the settings, and starts SWEPR again. Updates can run silently with `/VERYSILENT`. The uninstaller removes the startup entry and asks whether to delete the settings.
- Pro mode can prioritize smooth navigation by preloading all large carousel cards, or prioritize lower memory use by keeping only nearby cards cached.
- Thumbnail context menu (right click or the Menu key): set as wallpaper, set on this monitor only, set as lock screen, add to or remove from favorites, show in Explorer, and move to the Recycle Bin.
- Favorites, marked with a gold star in the panel and the carousel.
- Automatic wallpaper changes at an interval (5 minutes to daily), optionally in random order and only among favorites. Skipped during full-screen games and presentations.
- Random order: the Next shortcut picks a random wallpaper and Previous goes back to the last one.
- Tray menu: next, random, and back to the previous wallpaper. Left-clicking the tray icon opens the panel.
- Settings for the fit mode (including Span across all monitors) and the gallery order (name or date).
- Support for JFIF, WebP, GIF, and TIFF images.
- New images stay marked as new after restarting SWEPR.
- The executable shows its version in its file properties.

### Fixed

- The folder watcher stopped for good when many files changed at once, when the folder did not exist yet, or when it was renamed or deleted. It now rescans after an overflow and keeps retrying until the folder is available.
- Overlapping scans could replace the gallery with an older result (for example, after changing the folder during a long scan).
- Junctions or links that loop back to a parent folder no longer hang scanning.
- Settings are saved atomically, and a damaged settings file is kept as `config.corrupt.json` instead of being silently replaced by defaults.
- Pressing Next or Previous quickly could apply the same wallpaper twice while the fade was running.
- The current wallpaper outline is shown even if Windows reports its path with different letter case.
- Edited images now get a new thumbnail.
- The Settings window warns when the folder does not exist or settings cannot be saved, opens only one folder dialog at a time, and rescales when moved to a monitor with a different DPI.
- Saving settings only reloads what changed instead of rescanning and rebuilding all thumbnails.

### Improved

- The log keeps the previous session as `log.old.txt` instead of being cleared, and crashes are written to the log.
- Translations are only loaded from beside the executable.

## [1.1.0]

### Improved

- Redesigned the settings window: sections shown as cards with titles, dark drop-down lists, checkboxes and buttons drawn by SWEPR (with hover and pressed states), a highlighted Save button, and a title bar that matches the window. The thumbnail size is now a drop-down list, and the window is shorter so it fits on 1080p screens at 150% scaling.
- The settings window uses the Windows interface font and scales with the display DPI; the thumbnail panel also scales with the DPI of its monitor.
- Clearer thumbnail states: white outline for the focused wallpaper, blue outline for the current one, and a green dot for new ones.
- The thumbnail panel only draws the visible thumbnails.
- The tray menu follows the Windows dark or light theme.
- Thumbnails use about 25% less memory, are generated by a single background worker without duplicate work, and are released when their files are deleted.
- Faster folder scanning, especially when sorting by date.
- Less logging and fewer system calls in release builds; the resource log now reports GDI objects and private memory correctly.

### Added

- Panel mode setting: Low power (the thumbnail strip) or Pro, a carousel of slanted wallpaper cards in the middle of the screen with the focused card enlarged. Pro loads large cards only for the visible wallpapers and releases them 30 seconds after the panel closes.
- A message in the panel when the wallpaper folder has no images.
- Thumbnail loading setting: All in advance (default) or On demand, which keeps only thumbnails near the visible ones in memory (at most 200).
- Keyboard shortcuts can be changed in Settings and apply as soon as you save, without restarting SWEPR. Shortcuts can use the Win key; combinations without Ctrl, Alt, or Win get Ctrl + Alt added, Backspace turns a shortcut off, and two shortcuts cannot share the same combination.
- Panel navigation wraps around: after the last wallpaper comes the first one, and the other way around, with the arrow keys and with the mouse wheel.
- Wallpaper changes use the native Windows fade (on by default; "Fade when changing wallpaper" in Settings). If the fade is unavailable, the wallpaper is applied instantly as before.

## [1.0.0]

### Features

- Browse and apply wallpapers from a folder, with optional subfolder scanning.
- Open the thumbnail panel from screen edge sensors or configurable keyboard shortcuts.
- Position the panel at the top, left, or right, with adjustable spacing in the configuration file.
- Navigate wallpapers with keyboard shortcuts.
- Automatically refresh the gallery when its folder changes.
- Highlight newly discovered wallpapers and fit varied image aspect ratios into thumbnail frames.
- Choose English or Spanish, with support for additional translation files.
