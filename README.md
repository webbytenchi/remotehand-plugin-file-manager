# RemoteHand File Manager

Status: **v0.1.0 implementation started**

RemoteHand File Manager is intended to be a programmer- and sysadmin-focused file manager integrated into RemoteHand.

The goal is not to clone Windows Explorer. It should provide fast, predictable navigation and file operations that fit naturally into RemoteHand workflows, especially when combined with RemoteHand Editor, local shells, SSH/SFTP, and future plugin integrations.

Implementation work has started for the first milestone.

The accepted v0.1.0 scope is documented in [V0.1.0-DESIGN.md](V0.1.0-DESIGN.md). The broader notes below remain long-term product direction and are not all part of v0.1.0.

## Product direction

The File Manager should emphasize:

- fast keyboard-friendly navigation
- practical file operations
- local filesystem browsing first
- clean programmer/sysadmin workflows
- cross-platform Windows + Linux design
- strong integration with RemoteHand Editor and terminal workflows
- safe handling of long-running copy/move/delete operations
- generic plugin integration rather than hard-coded plugin dependencies

The initial implementation should stay focused and avoid turning into a full desktop shell replacement.

## Initial scope direction

The first useful version should focus on the **local filesystem**.

Remote SFTP browsing already exists elsewhere in RemoteHand. The File Manager should not duplicate or destabilize that functionality initially.

Potential first-version capabilities:

- browse folders
- single- or dual-pane layout
- editable path / breadcrumb navigation
- Back
- Forward
- Parent
- Refresh
- New Folder
- Rename
- Copy
- Move
- Delete
- Properties
- sorting by name, size, type, and modified time
- hidden-files toggle
- current-directory filtering/search
- bookmarks / favorites
- recent locations
- Copy Path
- Copy Filename
- Open Containing Folder
- Open Terminal Here
- Open in RemoteHand Editor for text/code files when that integration exists

Tabs are also a desirable direction for working across multiple directories.

Example:

```text
C:\Users\Ringo | C:\Projects | Downloads
```

## Single-pane vs dual-pane

A dual-pane workflow is attractive for programmer/sysadmin use because it makes copy/move operations efficient and predictable.

Potential model:

```text
+---------------------------+---------------------------+
| C:\Projects               | D:\Backups               |
+---------------------------+---------------------------+
| remotehand                | releases                  |
| scripts                   | archives                  |
| notes.txt                 |                           |
+---------------------------+---------------------------+
```

The exact default should be decided during design. A single-pane mode may still be useful for smaller layouts.

## File operations

File operations should be treated as proper jobs rather than blocking the UI.

Large copy/move/delete operations should support:

- progress
- current file/path
- bytes completed / total where measurable
- file count where measurable
- cancellation
- useful error reporting
- partial-failure handling
- safe UI behavior while operations continue

Example:

```text
Copying 1,284 files
3.7 GB / 12.1 GB
[Cancel]
```

The plugin should remain responsive during long operations.

## Delete behavior

Deletion should be conservative.

Where supported by the platform:

- normal Delete should use Recycle Bin / Trash
- permanent delete should be a separate explicit action
- permanent delete should require stronger confirmation
- deleting large selections should clearly state what is being removed

The default path should not make irreversible deletion easy by accident.

## Cross-platform direction

The plugin should be designed for **Windows and Linux** from the beginning.

### Windows

Support concepts such as:

- drive roots such as `C:\`, `D:\`
- Windows path rules
- hidden/system attributes where relevant
- Recycle Bin behavior where available
- Windows-specific metadata only when useful

### Linux

Support concepts such as:

- root filesystem `/`
- mounted filesystems
- dotfile hidden behavior
- symlinks
- Unix permissions
- Trash behavior where available
- executable bit and other meaningful metadata

The UI should normalize common concepts while not pretending Windows ACLs and Unix permissions are identical.

## Symlinks and special files

Symlinks must be handled carefully.

The plugin should:

- clearly distinguish links from normal files/folders
- avoid surprising recursive behavior
- avoid following links blindly during recursive delete/copy
- show link targets where practical

Linux special files and other non-regular entries should be treated conservatively rather than assuming every directory entry behaves like a normal file.

## Tabs and session state

Potential future session behavior:

- multiple directory tabs
- remember open tabs
- restore active tab
- remember tab order
- remember each tab's path
- remember view mode and sorting where useful

The exact persistence model should be designed after the basic browser works reliably.

## Search and filtering

Initial search should remain lightweight.

A first version can support filtering the current directory by filename.

Later versions may support recursive search with:

- filename
- extension
- size
- modified date
- simple text-content search where justified

Avoid building a full filesystem indexing service unless real use proves it is necessary.

## Bookmarks and recent locations

Useful programmer/sysadmin conveniences:

- favorite folders
- recent paths
- pinned project directories
- quick access to common locations

These should be stored as plugin-owned preferences.

## RemoteHand Editor integration

One of the key future workflows is:

```text
File Manager -> Open in Editor
Editor -> Show in File Manager
```

This should not be implemented as File Manager hard-coding knowledge of the Editor plugin.

The preferred architectural direction is a generic RemoteHand action/intent system where plugins can advertise or handle capabilities such as:

- `open-file`
- `reveal-path`
- `open-terminal-at-path`

A compatible installed plugin can then handle the action.

This keeps the integration extensible to future plugins.

## Terminal integration

A core workflow should eventually be:

```text
File Manager -> Open Terminal Here
```

The File Manager should pass the selected/current directory through a generic host/plugin action rather than embedding terminal-specific behavior directly.

Likewise, a terminal or other plugin could later request:

```text
Reveal this path in File Manager
```

## SFTP + Editor workflow

A major future integration is remote editing through SFTP.

Desired user experience:

```text
SFTP -> Open in Editor -> edit -> Save -> write back to same remote path
```

The user should not need to manually:

1. download the file
2. edit it locally
3. upload it again

The complexity should remain behind the integration layer.

### Provider-style document model

A preferred architecture is for Editor documents to have a source/provider abstraction.

Conceptually:

```text
Editor
  |
  +-- provider.read
  |
  +-- local recovery buffer
  |
  +-- provider.write
```

Potential providers:

- local filesystem
- SFTP
- future remote filesystem providers

The Editor should not need to understand SFTP credentials, transport, or remote filesystem semantics.

The SFTP side owns remote-file access.

The Editor owns:

- text editing
- recovery
- save UX
- document state

## SFTP remote-save safety

When a file is opened from SFTP and edited in Editor, the future integration should aim to:

- remember the originating connection/profile reference
- remember the remote path
- retain a local recovery copy until remote save succeeds
- detect whether the remote file changed since it was opened where practical
- warn before overwriting a remote file that appears to have changed
- preserve permissions where possible
- use a safer temporary-upload + rename strategy where the server supports it
- keep the user's edits recoverable if the connection becomes unavailable
- allow retry
- eventually support Save As to another local or remote destination

If the remote write fails, the edited content must not be lost.

## Generic provider direction

The File Manager and Editor should eventually benefit from generic resource/provider contracts rather than pairwise hard-coded integrations.

Examples of useful generic concepts:

- open resource
- read resource
- write resource
- reveal resource
- open terminal at location
- describe source/location

These contracts should be designed only when concrete plugin implementations require them.

Do not add speculative RemoteHand core APIs merely to make the architecture look complete.

## Local versus remote file management

For the initial File Manager:

**Local filesystem first.**

Remote SFTP remains in the existing SFTP surface.

A unified local/remote file-browser abstraction may be considered later only if real use demonstrates that it improves workflows without creating unnecessary complexity.

## Potential later features

After the local browsing and operations foundation is stable:

- dual-pane workflow
- directory tabs
- drag/drop where reliable
- batch rename
- archive create/extract
- checksum calculation
- directory size calculation
- recursive search
- file previews
- quick image preview
- text preview
- permission editing
- compare directories
- remote providers
- richer file-operation queue

These are deliberately deferred.

## Non-goals for early versions

Avoid initially:

- full Windows Explorer replacement
- full desktop shell integration
- cloud storage client
- filesystem indexing daemon
- disk partition management
- disk formatting
- undelete/recovery tooling
- duplicate-file cleaner
- destructive bulk automation
- archive-manager replacement
- hard-coded knowledge of every RemoteHand plugin

The goal is a reliable RemoteHand-native file manager for daily programmer/admin work.

## Architecture rule

RemoteHand core owns generic infrastructure such as:

- plugin runtime
- plugin-scoped storage
- generic file access permissions/APIs where needed
- panels/navigation
- sidebar contribution plumbing
- generic actions/intents if and when proven necessary

RemoteHand File Manager owns:

- directory navigation
- selection
- file-operation semantics
- bookmarks
- recent locations
- sorting/filtering
- progress/cancellation UX
- file-manager presentation
- platform-specific file-management behavior

RemoteHand Editor owns text-editing semantics.

SFTP owns remote filesystem transport and remote-file semantics.

Do not move filesystem-management semantics into RemoteHand core.

## Open design questions

Before implementation begins, decide:

- single-pane, dual-pane, or switchable layout
- initial table/list presentation
- file-operation job model
- cancellation semantics
- recycle-bin/trash implementation per OS
- symlink behavior
- permission model
- session/tab restoration
- exact generic action/intent contract for Editor and terminal integration
- whether provider abstractions belong in RemoteHand core or remain plugin-side until more than one plugin proves the need
- how local file change notifications should work
- large-directory virtualization strategy
- directory-size calculation policy
- maximum preview/file-size limits

## Development status

**v0.1.0 implementation started.**

The first milestone is explicitly scoped in [V0.1.0-DESIGN.md](V0.1.0-DESIGN.md). Development should proceed in small, testable slices while preserving the broader architecture and deferred-feature boundaries documented here.
