# Changelog

All notable changes to RemoteHand File Manager are documented here.

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
