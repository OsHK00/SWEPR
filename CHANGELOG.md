# Changelog

This file tracks user-facing changes between releases. Commits do not imply a new release.

## Unreleased

### Added

- "SWEPR canvas" wallpaper change mode (Settings > Wallpaper > Wallpaper change): SWEPR draws the wallpaper itself behind the desktop icons with Direct2D, so changes start instantly with a smooth fade synchronized with the display, also on each monitor separately. The Windows wallpaper is updated silently in the background, so Task View, accent colors, and other apps see the same image and the desktop stays correct when SWEPR closes. The canvas is recreated automatically if Explorer restarts or monitors change, and SWEPR falls back to Windows if it cannot attach. The previous Windows modes (instant or with fade) are still available.
- Canvas transitions: Fade, Slide, Zoom in, Wipe, Circle, or a random effect each time, with a duration from 0.3 to 1.5 seconds.
- When a transition ends, the canvas asks Direct2D and the graphics driver to release cached memory.
- Notch panel mode (Settings > Thumbnail panel > Panel mode): a black island attached to the top edge of the screen, like a phone notch. It drops in as a small pill and expands into a gallery with the image name, its position, and a row of thumbnails where the centered one is larger. Move with the mouse wheel or the arrow keys, click a side image to center it, and click the centered one (or press Enter) to apply it. It opens from the top center of the screen or with the panel shortcut. It is drawn with Direct2D with smooth edges, a soft shadow, and real transparency, and everything it uses is released when it closes.
- Pro mode styles (Settings > Thumbnail panel > Pro style), drawn with Direct2D over the dimmed desktop:
  - Showcase: slanted cards where the focused one widens and shows its name, position, and Apply, Favorite, Explorer, and Remove buttons. Cards fade out toward the screen edges and cast soft shadows.
  - Fan: cards spread in an arc that turns as you move through the gallery.
  - Mosaic: a full-screen grid. Hovering a card focuses it, the arrow keys and Page Up/Down move through it, and the wheel scrolls.
  - Frame: the focused image fills almost the whole screen inside a light frame. A box in the top-left corner has search, the name, the color chips, and the actions, the thumbnails cascade in the bottom-right corner, and a column on the right has the filters and arrows.
  - Classic carousel: the previous Pro carousel.
- Filters in the Pro styles: All, Favorites, and New, plus color chips based on the dominant color of each thumbnail. Typing filters by name in any style; Esc clears the search, and Tab switches the filter.
- Color chips in the Notch header: click one to show only the images of that dominant color, click it again to show all. The selected image stays in view when it matches.
- Notch notice (Settings > Thumbnail panel): when the wallpaper changes from a shortcut, the tray, an automatic change, or a panel (once the panel has closed), the notch drops in with a thumbnail and the name of the new wallpaper for about two seconds. It stays while the mouse is over it, a click dismisses it, and it is not shown during full-screen games or presentations. It works with every panel mode.
- Animated wallpapers (Settings > Wallpaper, SWEPR canvas only): GIF and animated WebP files play on the desktop behind the icons. The first frame appears at once with the configured transition, and the animation starts as soon as all frames are ready. The animation pauses during full-screen games or presentations, when a maximized window covers its monitor, while the session is locked, and with battery saver on. Frames are kept in memory within a budget of 160 MB (larger animations are scaled down), and only one frame at a time is on the GPU. Without the canvas, or with the option off, the first frame is shown.
- Desktop clock (Settings > Desktop clock, SWEPR canvas only): a clock drawn behind the desktop icons, in four styles: Large (big thin numbers with the date), Pill (a translucent capsule), Analog (a round face with an accent-colored second hand), Greeting (good morning, afternoon, or evening above the time), and Stacked (bold hours above the minutes, with the seconds fainter below). It can sit in the center, in any corner, or halfway down the left or right edge, in three sizes, with a 24-hour or AM/PM format, optional seconds and date, and on every monitor or only the main one. Its color adapts to the wallpaper: dark ink over light areas and white over dark ones, with a soft shadow. It only redraws when the minute (or the second) changes, and not while the desktop is hidden by a full-screen app or a locked session.
- Light theme (Settings > General > Theme: Same as Windows, Dark, or Light). It covers Settings, the Removed images window, the low-power strip, the Notch and its notice, and the Pro styles. The Frame style turns dark with the dark theme. The accent color uses the variant Windows uses on each background.
- Option to hide wallpaper names (Settings > Thumbnail panel) in the Notch, its notice, and the Pro styles. Search by name still works.
- Desktop preview (Settings > Wallpaper, SWEPR canvas only): while you browse in the Notch or a Pro style, the focused image is shown on the desktop behind the icons after a short pause, with the configured canvas transition. If you close the panel without applying, the desktop goes back to the current wallpaper with the same transition. The Windows wallpaper is not changed by a preview.

### Changed

- Settings use the Windows accent color for checkboxes, focused fields, and the Save button, and the Notch and Pro styles use it to mark the current wallpaper. The Thumbnail panel, General, and Wallpaper cards are aligned, with the new Pro style, notch notice, and desktop preview options.
- The Settings window is wider, with a new Desktop clock card next to the Wallpaper card. The General card has the theme selector and the option to hide wallpaper names.

### Fixed

- SWEPR canvas on Windows 11 24H2 and later: transitions were not visible (the new image only appeared when the transition ended) and the desktop could flicker between the old and the new image. Every frame is now shown, and the canvas moves back behind the icons as soon as Explorer puts its own wallpaper layer on top, instead of up to a second later.

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
