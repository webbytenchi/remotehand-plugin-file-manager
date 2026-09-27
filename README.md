# RemoteHand File Manager

RemoteHand File Manager is a local filesystem browser and file-operation plugin for RemoteHand.

Version **0.2.0** adds file-opening integration on top of the established local-file-management foundation.

## Requirements

RemoteHand File Manager v0.2.0 requires **RemoteHand v1.2.20 or later** for the generic `plugins.open` host capability.

**Open in Editor** additionally requires **RemoteHand Editor v0.2.0 or later** to receive and open the selected file path.

## v0.2.0 highlights

- Open supported text/code files directly in RemoteHand Editor
- Reuse the existing Editor tab when the selected file is already open
- Show **Open in Editor** only while Editor is installed and enabled
- Refresh Editor availability when the file context menu opens
- Open regular files with the operating system's registered/default application

## File Manager foundation

The v0.1.0 browsing and file-operation foundation remains included:

- Single-pane and dual-pane layouts
- Persisted pane paths, layout, and active pane
- Independent Back, Forward, Parent, Refresh, and breadcrumb/path navigation
- Details view with Name, Type, Size, and Modified columns
- Single and multi-selection
- New Folder
- Rename
- Copy, Cut, Paste, and Move
- Copy to Other Pane / Move to Other Pane
- Internal drag-and-drop between panes and onto folders
- Explicit Copy here / Move here choice for drag-and-drop
- Recycle Bin / Trash deletion
- Read-only Properties
- Live folder Properties scanning with file/folder counts and total size
- Copy Path / Copy Filename
- Right-click context menu
- Symlink-aware browsing and conservative recursive handling
- Background Copy, Move, and Delete jobs
- Live item and byte discovery while scanning
- Transfer progress and cancellation where safe
- Job status restoration when returning to the File Manager panel

## File-operation behavior

Copy and Move use the same background job engine whether they are started from the toolbar, context menu, dual-pane actions, clipboard workflow, or internal drag-and-drop.

Large operations first scan the selected content, then report item and byte progress in the File Manager job bar.

Delete uses Recycle Bin / Trash semantics. Its pre-delete scan can be cancelled. Once the operating system begins the Recycle Bin / Trash operation, cancellation is disabled rather than pretending the operation can be safely interrupted.

## Drag-and-drop

File Manager supports **internal File Manager drag-and-drop**:

- between panes
- onto folders
- single or multiple selected items

A drop asks whether to **Copy here**, **Move here**, or cancel.

External operating-system drag-and-drop, such as dragging between Windows Explorer and RemoteHand, remains deferred.

## Properties

Properties are read-only.

For files, metadata is displayed immediately.

For folders, File Manager performs a cancellable background scan and progressively reports:

- file count
- folder count
- link count when present
- total size

Closing or dismissing the Properties dialog stops the scan.

## Keyboard and mouse navigation

Supported navigation includes:

- Alt+Left — Back
- Alt+Right — Forward
- Alt+Up — Parent
- F5 — Refresh
- Ctrl+L — edit current path
- Ctrl+A — select all
- Ctrl+C — copy
- Ctrl+X — cut
- Ctrl+V — paste
- Ctrl+Shift+N — new folder
- F2 — rename
- Delete — move to Recycle Bin / Trash
- Alt+Enter — Properties
- mouse side buttons — Back / Forward

## Safety and filesystem handling

File Manager treats recursive filesystem operations conservatively:

- symlinks are not blindly followed during recursive copy, move, delete, or folder-size scans
- unsupported filesystem entries fail safely
- conflicting destinations are rejected before Copy/Move starts
- normal Delete uses Recycle Bin / Trash rather than permanent removal

## Platform direction

File Manager has been accepted primarily on Windows.

The plugin is designed to remain portable to Linux, including platform-appropriate path, symlink, and Trash behavior. Linux acceptance and packaging can be expanded independently without moving file-manager semantics into RemoteHand core.

## Deferred

The first release intentionally does not include:

- external OS drag-and-drop
- directory tabs
- folder tree sidebar
- batch rename
- permanent delete
- archive create/extract
- checksums
- permission editing
- directory comparison
- rich previews
- recursive filesystem search/indexing
- SFTP integration
- generic cross-plugin action/intent contracts
- column sorting controls
- hidden-files toggle

These can be added in later milestones without destabilizing the v0.1.0 foundation.

## Architecture

RemoteHand core owns generic plugin infrastructure.

RemoteHand File Manager owns:

- directory navigation
- selection
- filesystem operations
- progress and cancellation behavior
- File Manager presentation
- platform-specific file-management semantics

The plugin does not require RemoteHand core to hard-code File Manager behavior.

See [V0.1.0-DESIGN.md](V0.1.0-DESIGN.md) for the original foundation design and [CHANGELOG.md](CHANGELOG.md) for release history.
