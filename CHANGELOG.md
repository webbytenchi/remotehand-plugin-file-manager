# Changelog

## v0.2.2 - 2026-09-29

### Changed

- Copy Path and Copy Filename now use RemoteHand's generic `clipboard.write` host API exclusively.
- Removed browser clipboard fallbacks from the sandboxed plugin UI.

### Compatibility

- Plugin API remains **v1**.
- Existing browsing, file operations, and Open in Editor behavior are unchanged.

### Verification

- Rust tests passed with **15 tests and 0 failures**.
- Windows package build passed.
- Manual Windows acceptance confirmed Copy Path and Copy Filename through the host clipboard API.

All notable changes to RemoteHand File Manager are documented here.

## v0.2.1 - 2026-09-28

### Fixed

- Corrected the executable Plugin API handshake version so it matches the installed v0.2.1 manifest.
- Fixed the `plugin hello version does not match installed manifest` startup error from v0.2.0.

### Verification

- Added a regression test that checks the runtime plugin version matches `plugin.json`.

## v0.2.0 - 2026-09-28

### Integration

- Added **Open in Editor** for supported text/code files when RemoteHand Editor is installed and enabled.
- Added **Open with System App** for regular files, using the operating system's registered/default application.
- Reopening a file through **Open in Editor** reuses the existing Editor tab when that file is already open.
- Editor availability is refreshed whenever the context menu opens on a supported file, so enabling or disabling Editor is reflected immediately.
- The final **Open in Editor** action revalidates the selected file and routes the path through RemoteHand's generic plugin-panel host API.

### Compatibility

- Requires **RemoteHand v1.2.20 or later** for the `plugins.open` host permission/API.
- **Open in Editor** requires **RemoteHand Editor v0.2.0 or later**.
- Existing File Manager browsing and file-operation behavior remains unchanged.

### Verification

- Added targeted tests for text/code classification, Editor availability reporting, and Editor panel routing.
- Manual Windows acceptance confirmed system-app opening, Editor opening, existing-tab reuse, and immediate enable/disable availability updates.

## v0.1.0

Initial release.

### Browser

- Added single-pane and dual-pane local filesystem browsing.
- Added independent pane navigation with Back, Forward, Parent, Refresh, breadcrumbs, editable paths, and mouse side-button navigation.
- Added persisted layout, pane paths, and active-pane state.
- Added file, folder, and symlink presentation.

### Selection and operations

- Added desktop-style single, toggle, range, additive-range, and select-all behavior.
- Added New Folder and Rename.
- Added Copy, Cut, Paste, and Move.
- Added direct Copy to Other Pane and Move to Other Pane actions.
- Added Recycle Bin / Trash deletion.
- Added Copy Path and Copy Filename, including multi-selection support.

### Jobs and progress

- Added background Copy and Move jobs.
- Added live recursive scan totals.
- Added item and byte progress.
- Added cancellation where safe.
- Added active-job restoration when returning to the File Manager panel.
- Added background Delete scanning and consistent Delete status.

### Properties

- Added read-only Properties.
- Added cancellable live folder scanning for file count, folder count, links, and aggregate size.

### Drag-and-drop

- Added internal drag-and-drop between panes and onto folders.
- Added explicit Copy here / Move here / Cancel choice.
- Reused the existing Copy/Move job engine for drag-and-drop transfers.

### UI

- Added File Manager right-click context menu.
- Added themed toolbar and filesystem icons.
- Added destination highlighting for drag-and-drop.
- Suppressed the native browser/WebView context menu outside editable text fields.

### Safety

- Added conservative symlink handling.
- Added destination conflict validation before Copy/Move starts.
- Added partial-copy cleanup for ordinary cancellation/failure paths.
- Kept permanent delete out of the initial release.
