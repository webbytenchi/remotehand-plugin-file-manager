use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::UNIX_EPOCH;

const PLUGIN_ID: &str = "remotehand.file-manager";
const PLUGIN_VERSION: &str = "0.1.0";
const API_VERSION: u32 = 1;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const UI_STATE_KEY: &str = "ui-state-v1";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginHello<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    plugin_id: &'a str,
    api_version: u32,
    plugin_version: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostReady {
    #[serde(rename = "type")]
    message_type: String,
    plugin_id: String,
    api_version: u32,
    #[serde(default, rename = "grantedPermissions")]
    _granted_permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostRequest {
    #[serde(rename = "type")]
    message_type: String,
    request_id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginResponse {
    #[serde(rename = "type")]
    message_type: &'static str,
    request_id: u64,
    ok: bool,
    result: Value,
    error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginHostRequest<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    request_id: u64,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostResponse {
    #[serde(rename = "type")]
    message_type: String,
    request_id: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UiState {
    #[serde(default)]
    dual_pane: bool,
    #[serde(default)]
    left_path: Option<String>,
    #[serde(default)]
    right_path: Option<String>,
    #[serde(default = "default_active_pane")]
    active_pane: String,
}

fn default_active_pane() -> String {
    "left".to_string()
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            dual_pane: false,
            left_path: None,
            right_path: None,
            active_pane: default_active_pane(),
        }
    }
}

fn normalize_ui_state(mut state: UiState) -> UiState {
    if state.active_pane != "left" && state.active_pane != "right" {
        state.active_pane = default_active_pane();
    }
    state.left_path = state
        .left_path
        .and_then(|path| (!path.trim().is_empty() && path.len() <= 32768).then_some(path));
    state.right_path = state
        .right_path
        .and_then(|path| (!path.trim().is_empty() && path.len() <= 32768).then_some(path));
    state
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DirectoryEntry {
    name: String,
    path: String,
    kind: EntryKind,
    link_target_kind: Option<EntryKind>,
    broken_link: bool,
    size: Option<u64>,
    modified_ms: Option<u64>,
    hidden: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}


#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobSnapshot {
    id: u64,
    kind: String,
    state: String,
    sources: Vec<String>,
    destination: String,
    bytes_total: u64,
    bytes_done: u64,
    items_total: u64,
    items_done: u64,
    cancellable: bool,
    error: Option<String>,
}

struct JobControl {
    snapshot: Mutex<JobSnapshot>,
    cancel: AtomicBool,
}

#[derive(Default)]
struct JobManager {
    next_id: AtomicU64,
    jobs: Mutex<BTreeMap<u64, Arc<JobControl>>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PropertyScanSnapshot {
    id: u64,
    state: String,
    path: String,
    files: u64,
    folders: u64,
    links: u64,
    bytes: u64,
    cancellable: bool,
    error: Option<String>,
}

struct PropertyScanControl {
    snapshot: Mutex<PropertyScanSnapshot>,
    cancel: AtomicBool,
}

#[derive(Default)]
struct PropertyScanManager {
    next_id: AtomicU64,
    scans: Mutex<BTreeMap<u64, Arc<PropertyScanControl>>>,
}

impl PropertyScanManager {
    fn start(&self, path: &Path) -> Result<u64, String> {
        let path = ensure_regular_parent(path)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let control = Arc::new(PropertyScanControl {
            snapshot: Mutex::new(PropertyScanSnapshot {
                id,
                state: "scanning".to_string(),
                path: path_text(&path),
                files: 0,
                folders: 0,
                links: 0,
                bytes: 0,
                cancellable: true,
                error: None,
            }),
            cancel: AtomicBool::new(false),
        });
        self.scans
            .lock()
            .map_err(|_| "property scan store lock poisoned".to_string())?
            .insert(id, Arc::clone(&control));

        thread::spawn(move || {
            let outcome = scan_properties_tree(&path, &control);
            if let Ok(mut snapshot) = control.snapshot.lock() {
                snapshot.cancellable = false;
                match outcome {
                    Ok(()) => snapshot.state = "completed".to_string(),
                    Err(error) if error == "cancelled" => {
                        snapshot.state = "cancelled".to_string();
                        snapshot.error = None;
                    }
                    Err(error) => {
                        snapshot.state = "failed".to_string();
                        snapshot.error = Some(error);
                    }
                }
            }
        });

        Ok(id)
    }

    fn get(&self, id: u64) -> Result<PropertyScanSnapshot, String> {
        let control = self
            .scans
            .lock()
            .map_err(|_| "property scan store lock poisoned".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("property scan {id} was not found"))?;
        control
            .snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| "property scan state lock poisoned".to_string())
    }

    fn cancel(&self, id: u64) -> Result<PropertyScanSnapshot, String> {
        let control = self
            .scans
            .lock()
            .map_err(|_| "property scan store lock poisoned".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("property scan {id} was not found"))?;
        control.cancel.store(true, Ordering::Relaxed);
        self.get(id)
    }
}

impl JobManager {
    fn create(&self, kind: &str, sources: &[PathBuf], destination: &Path) -> Result<Arc<JobControl>, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let snapshot = JobSnapshot {
            id,
            kind: kind.to_string(),
            state: "queued".to_string(),
            sources: sources.iter().map(|source| path_text(source)).collect(),
            destination: path_text(destination),
            bytes_total: 0,
            bytes_done: 0,
            items_total: 0,
            items_done: 0,
            cancellable: true,
            error: None,
        };
        let control = Arc::new(JobControl {
            snapshot: Mutex::new(snapshot),
            cancel: AtomicBool::new(false),
        });
        self.jobs
            .lock()
            .map_err(|_| "job store lock poisoned".to_string())?
            .insert(id, Arc::clone(&control));
        Ok(control)
    }

    fn create_delete(&self, sources: &[PathBuf]) -> Result<Arc<JobControl>, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let snapshot = JobSnapshot {
            id,
            kind: "delete".to_string(),
            state: "queued".to_string(),
            sources: sources.iter().map(|source| path_text(source)).collect(),
            destination: String::new(),
            bytes_total: 0,
            bytes_done: 0,
            items_total: 0,
            items_done: 0,
            cancellable: true,
            error: None,
        };
        let control = Arc::new(JobControl {
            snapshot: Mutex::new(snapshot),
            cancel: AtomicBool::new(false),
        });
        self.jobs
            .lock()
            .map_err(|_| "job store lock poisoned".to_string())?
            .insert(id, Arc::clone(&control));
        Ok(control)
    }

    fn get(&self, id: u64) -> Result<JobSnapshot, String> {
        let control = self
            .jobs
            .lock()
            .map_err(|_| "job store lock poisoned".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("file operation job {id} was not found"))?;
        control
            .snapshot
            .lock()
            .map(|value| value.clone())
            .map_err(|_| "job state lock poisoned".to_string())
    }

    fn cancel(&self, id: u64) -> Result<JobSnapshot, String> {
        let control = self
            .jobs
            .lock()
            .map_err(|_| "job store lock poisoned".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("file operation job {id} was not found"))?;
        control.cancel.store(true, Ordering::Relaxed);
        self.get(id)
    }

    fn active(&self) -> Result<Option<JobSnapshot>, String> {
        let controls: Vec<Arc<JobControl>> = self
            .jobs
            .lock()
            .map_err(|_| "job store lock poisoned".to_string())?
            .iter()
            .rev()
            .map(|(_, control)| Arc::clone(control))
            .collect();

        for control in controls {
            let snapshot = control
                .snapshot
                .lock()
                .map_err(|_| "job state lock poisoned".to_string())?
                .clone();
            if matches!(snapshot.state.as_str(), "queued" | "scanning" | "running" | "deleting") {
                return Ok(Some(snapshot));
            }
        }
        Ok(None)
    }
}

fn validate_leaf_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("name is required".to_string());
    }
    if trimmed == "." || trimmed == ".." || trimmed.len() > 255 {
        return Err("invalid file or folder name".to_string());
    }
    let path = Path::new(trimmed);
    if path.components().count() != 1 || path.is_absolute() {
        return Err("name must not contain a path".to_string());
    }
    Ok(trimmed)
}

fn ensure_regular_parent(path: &Path) -> Result<PathBuf, String> {
    let canonical = dunce::canonicalize(path)
        .map_err(|error| format!("cannot open {}: {error}", path_text(path)))?;
    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", path_text(&canonical)));
    }
    Ok(canonical)
}

fn resolve_entry_without_following(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "operation is not supported on a filesystem root".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "cannot resolve entry parent".to_string())?;
    let parent = dunce::canonicalize(parent)
        .map_err(|error| format!("cannot open {}: {error}", path_text(parent)))?;
    let resolved = parent.join(name);
    fs::symlink_metadata(&resolved)
        .map_err(|error| format!("cannot inspect {}: {error}", path_text(&resolved)))?;
    Ok(resolved)
}

fn create_folder(parent: &Path, name: &str) -> Result<Value, String> {
    let parent = ensure_regular_parent(parent)?;
    let name = validate_leaf_name(name)?;
    let target = parent.join(name);
    if target.exists() {
        return Err(format!("already exists: {}", path_text(&target)));
    }
    fs::create_dir(&target)
        .map_err(|error| format!("cannot create {}: {error}", path_text(&target)))?;
    Ok(json!({ "path": path_text(&target) }))
}

fn rename_entry(path: &Path, name: &str) -> Result<Value, String> {
    let source = resolve_entry_without_following(path)?;
    let name = validate_leaf_name(name)?;
    let parent = source
        .parent()
        .ok_or_else(|| "cannot rename a filesystem root".to_string())?;
    let target = parent.join(name);
    if target == source {
        return Ok(json!({ "path": path_text(&source) }));
    }
    if target.exists() {
        return Err(format!("already exists: {}", path_text(&target)));
    }
    fs::rename(&source, &target)
        .map_err(|error| format!("cannot rename {}: {error}", path_text(&source)))?;
    Ok(json!({ "path": path_text(&target) }))
}

fn entry_properties(path: &Path) -> Result<Value, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path_text(path)))?;
    let kind = entry_kind(&metadata);
    let link_target = if matches!(&kind, EntryKind::Symlink) {
        fs::read_link(path).ok().map(|value| path_text(&value))
    } else {
        None
    };
    let target_kind = if matches!(&kind, EntryKind::Symlink) {
        fs::metadata(path).ok().map(|value| entry_kind(&value))
    } else {
        None
    };
    Ok(json!({
        "name": path.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_else(|| path_text(path)),
        "path": path_text(path),
        "kind": kind,
        "size": matches!(&kind, EntryKind::File).then_some(metadata.len()),
        "modifiedMs": modified_ms(&metadata),
        "hidden": is_hidden(path, &metadata),
        "linkTarget": link_target,
        "linkTargetKind": target_kind
    }))
}

fn publish_property_scan(
    control: &PropertyScanControl,
    files: u64,
    folders: u64,
    links: u64,
    bytes: u64,
) -> Result<(), String> {
    let mut snapshot = control
        .snapshot
        .lock()
        .map_err(|_| "property scan state lock poisoned".to_string())?;
    snapshot.files = files;
    snapshot.folders = folders;
    snapshot.links = links;
    snapshot.bytes = bytes;
    Ok(())
}

fn scan_properties_tree(root: &Path, control: &PropertyScanControl) -> Result<(), String> {
    fn visit(
        path: &Path,
        control: &PropertyScanControl,
        totals: &mut (u64, u64, u64, u64),
    ) -> Result<(), String> {
        if control.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        for item in fs::read_dir(path)
            .map_err(|error| format!("cannot read {}: {error}", path_text(path)))?
        {
            if control.cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            let item = item.map_err(|error| format!("failed to read directory entry: {error}"))?;
            let item_path = item.path();
            let metadata = fs::symlink_metadata(&item_path)
                .map_err(|error| format!("cannot inspect {}: {error}", path_text(&item_path)))?;

            if metadata.file_type().is_symlink() {
                totals.2 = totals.2.saturating_add(1);
            } else if metadata.is_dir() {
                totals.1 = totals.1.saturating_add(1);
                publish_property_scan(control, totals.0, totals.1, totals.2, totals.3)?;
                visit(&item_path, control, totals)?;
                continue;
            } else if metadata.is_file() {
                totals.0 = totals.0.saturating_add(1);
                totals.3 = totals.3.saturating_add(metadata.len());
            }

            publish_property_scan(control, totals.0, totals.1, totals.2, totals.3)?;
        }
        Ok(())
    }

    let mut totals = (0u64, 0u64, 0u64, 0u64);
    visit(root, control, &mut totals)?;
    publish_property_scan(control, totals.0, totals.1, totals.2, totals.3)
}

fn recycle_entries(paths: &[String]) -> Result<Value, String> {
    if paths.is_empty() {
        return Err("at least one path is required".to_string());
    }
    if paths.len() > 10_000 {
        return Err("too many items selected".to_string());
    }
    let resolved: Vec<PathBuf> = paths
        .iter()
        .map(|path| resolve_entry_without_following(Path::new(path)))
        .collect::<Result<_, _>>()?;

    trash::delete_all(&resolved)
        .map_err(|error| format!("cannot move selected item(s) to Recycle Bin/Trash: {error}"))?;

    Ok(json!({ "deleted": resolved.len() }))
}

fn publish_scan_totals(control: &JobControl, totals: (u64, u64)) -> Result<(), String> {
    let mut snapshot = control
        .snapshot
        .lock()
        .map_err(|_| "job state lock poisoned".to_string())?;
    snapshot.bytes_total = totals.0;
    snapshot.items_total = totals.1;
    Ok(())
}

fn scan_tree(
    path: &Path,
    totals: &mut (u64, u64),
    cancel: &AtomicBool,
    progress: Option<&JobControl>,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path_text(path)))?;
    totals.1 = totals.1.saturating_add(1);
    if metadata.file_type().is_symlink() {
        if let Some(control) = progress {
            publish_scan_totals(control, *totals)?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        totals.0 = totals.0.saturating_add(metadata.len());
        if let Some(control) = progress {
            publish_scan_totals(control, *totals)?;
        }
        return Ok(());
    }
    if metadata.is_dir() {
        if let Some(control) = progress {
            publish_scan_totals(control, *totals)?;
        }
        for item in fs::read_dir(path)
            .map_err(|error| format!("cannot read {}: {error}", path_text(path)))?
        {
            let item = item.map_err(|error| format!("failed to read directory entry: {error}"))?;
            scan_tree(&item.path(), totals, cancel, progress)?;
        }
        return Ok(());
    }
    Err(format!("unsupported filesystem entry: {}", path_text(path)))
}

fn note_progress(control: &JobControl, bytes: u64, items: u64) -> Result<(), String> {
    let mut snapshot = control
        .snapshot
        .lock()
        .map_err(|_| "job state lock poisoned".to_string())?;
    snapshot.bytes_done = snapshot.bytes_done.saturating_add(bytes);
    snapshot.items_done = snapshot.items_done.saturating_add(items);
    Ok(())
}

fn copy_symlink(source: &Path, destination: &Path) -> Result<(), String> {
    let target = fs::read_link(source)
        .map_err(|error| format!("cannot read link {}: {error}", path_text(source)))?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target, destination)
            .map_err(|error| format!("cannot copy link {}: {error}", path_text(source)))
    }
    #[cfg(windows)]
    {
        let points_to_dir = fs::metadata(source).map(|metadata| metadata.is_dir()).unwrap_or(false);
        let result = if points_to_dir {
            std::os::windows::fs::symlink_dir(&target, destination)
        } else {
            std::os::windows::fs::symlink_file(&target, destination)
        };
        result.map_err(|error| format!("cannot copy link {}: {error}", path_text(source)))
    }
}

fn copy_file_with_progress(source: &Path, destination: &Path, control: &JobControl) -> Result<(), String> {
    let mut input = fs::File::open(source)
        .map_err(|error| format!("cannot open {}: {error}", path_text(source)))?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| format!("cannot create {}: {error}", path_text(destination)))?;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        if control.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {}: {error}", path_text(source)))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|error| format!("cannot write {}: {error}", path_text(destination)))?;
        note_progress(control, count as u64, 0)?;
    }
    let _ = output.flush();
    if let Ok(metadata) = fs::metadata(source) {
        let _ = fs::set_permissions(destination, metadata.permissions());
    }
    note_progress(control, 0, 1)
}

fn copy_tree(source: &Path, destination: &Path, control: &JobControl) -> Result<(), String> {
    if control.cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("cannot inspect {}: {error}", path_text(source)))?;
    if metadata.file_type().is_symlink() {
        copy_symlink(source, destination)?;
        return note_progress(control, 0, 1);
    }
    if metadata.is_file() {
        return copy_file_with_progress(source, destination, control);
    }
    if metadata.is_dir() {
        fs::create_dir(destination)
            .map_err(|error| format!("cannot create {}: {error}", path_text(destination)))?;
        note_progress(control, 0, 1)?;
        for item in fs::read_dir(source)
            .map_err(|error| format!("cannot read {}: {error}", path_text(source)))?
        {
            let item = item.map_err(|error| format!("failed to read directory entry: {error}"))?;
            copy_tree(&item.path(), &destination.join(item.file_name()), control)?;
        }
        return Ok(());
    }
    Err(format!("unsupported filesystem entry: {}", path_text(source)))
}

fn remove_tree_no_follow(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path_text(path)))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        return fs::remove_file(path)
            .map_err(|error| format!("cannot remove {}: {error}", path_text(path)));
    }
    if metadata.is_dir() {
        for item in fs::read_dir(path)
            .map_err(|error| format!("cannot read {}: {error}", path_text(path)))?
        {
            let item = item.map_err(|error| format!("failed to read directory entry: {error}"))?;
            remove_tree_no_follow(&item.path())?;
        }
        return fs::remove_dir(path)
            .map_err(|error| format!("cannot remove {}: {error}", path_text(path)));
    }
    Err(format!("unsupported filesystem entry: {}", path_text(path)))
}

fn cleanup_partial_destination(path: &Path) {
    if fs::symlink_metadata(path).is_ok() {
        let _ = remove_tree_no_follow(path);
    }
}

fn validate_job_paths(source: &Path, destination_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let source = resolve_entry_without_following(source)?;
    let destination_dir = ensure_regular_parent(destination_dir)?;
    let name = source
        .file_name()
        .ok_or_else(|| "cannot copy or move a filesystem root".to_string())?;
    let destination = destination_dir.join(name);
    if destination.exists() {
        return Err(format!("already exists: {}", path_text(&destination)));
    }
    if source == destination {
        return Err("source and destination are the same".to_string());
    }
    let metadata = fs::symlink_metadata(&source)
        .map_err(|error| format!("cannot inspect {}: {error}", path_text(&source)))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        if let Ok(destination_parent) = dunce::canonicalize(&destination_dir) {
            if destination_parent.starts_with(&source) {
                return Err("cannot copy or move a folder inside itself".to_string());
            }
        }
    }
    Ok((source, destination))
}

fn start_delete_job(
    manager: Arc<JobManager>,
    source_paths: &[String],
) -> Result<Value, String> {
    if source_paths.is_empty() {
        return Err("at least one source is required".to_string());
    }
    if source_paths.len() > 10_000 {
        return Err("too many items selected".to_string());
    }

    let sources: Vec<PathBuf> = source_paths
        .iter()
        .map(|path| resolve_entry_without_following(Path::new(path)))
        .collect::<Result<_, _>>()?;

    let control = manager.create_delete(&sources)?;
    let id = control
        .snapshot
        .lock()
        .map_err(|_| "job state lock poisoned".to_string())?
        .id;
    let control_for_thread = Arc::clone(&control);

    thread::spawn(move || {
        let outcome = (|| -> Result<(), String> {
            {
                let mut snapshot = control_for_thread
                    .snapshot
                    .lock()
                    .map_err(|_| "job state lock poisoned".to_string())?;
                snapshot.state = "scanning".to_string();
            }

            let mut totals = (0u64, 0u64);
            for source in &sources {
                scan_tree(
                    source,
                    &mut totals,
                    &control_for_thread.cancel,
                    Some(&control_for_thread),
                )?;
            }

            if control_for_thread.cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }

            {
                let mut snapshot = control_for_thread
                    .snapshot
                    .lock()
                    .map_err(|_| "job state lock poisoned".to_string())?;
                snapshot.bytes_total = totals.0;
                snapshot.items_total = totals.1;
                snapshot.state = "deleting".to_string();
                snapshot.cancellable = false;
            }

            trash::delete_all(&sources)
                .map_err(|error| format!("cannot move selected item(s) to Recycle Bin/Trash: {error}"))?;

            let mut snapshot = control_for_thread
                .snapshot
                .lock()
                .map_err(|_| "job state lock poisoned".to_string())?;
            snapshot.bytes_done = snapshot.bytes_total;
            snapshot.items_done = snapshot.items_total;
            Ok(())
        })();

        if let Ok(mut snapshot) = control_for_thread.snapshot.lock() {
            snapshot.cancellable = false;
            match outcome {
                Ok(()) => {
                    snapshot.state = "completed".to_string();
                    snapshot.bytes_done = snapshot.bytes_total;
                    snapshot.items_done = snapshot.items_total;
                }
                Err(error) if error == "cancelled" => {
                    snapshot.state = "cancelled".to_string();
                    snapshot.error = None;
                }
                Err(error) => {
                    snapshot.state = "failed".to_string();
                    snapshot.error = Some(error);
                }
            }
        }
    });

    Ok(json!({ "jobId": id }))
}

fn start_job(
    manager: Arc<JobManager>,
    kind: &str,
    source_paths: &[String],
    destination_dir: &Path,
) -> Result<Value, String> {
    if kind != "copy" && kind != "move" {
        return Err("job kind must be copy or move".to_string());
    }
    if source_paths.is_empty() {
        return Err("at least one source is required".to_string());
    }
    if source_paths.len() > 10_000 {
        return Err("too many items selected".to_string());
    }

    let destination_dir = ensure_regular_parent(destination_dir)?;
    let mut sources = Vec::with_capacity(source_paths.len());
    let mut destinations = Vec::with_capacity(source_paths.len());
    let mut seen_destinations = std::collections::BTreeSet::new();

    for source_path in source_paths {
        let (source, destination) = validate_job_paths(Path::new(source_path), &destination_dir)?;
        let destination_key = destination.to_string_lossy().to_ascii_lowercase();
        if !seen_destinations.insert(destination_key) {
            return Err(format!(
                "multiple selected items would use the same destination name: {}",
                path_text(&destination)
            ));
        }
        sources.push(source);
        destinations.push(destination);
    }

    let control = manager.create(kind, &sources, &destination_dir)?;
    let id = control
        .snapshot
        .lock()
        .map_err(|_| "job state lock poisoned".to_string())?
        .id;
    let kind_owned = kind.to_string();
    let control_for_thread = Arc::clone(&control);

    thread::spawn(move || {
        let outcome = (|| -> Result<(), String> {
            {
                let mut snapshot = control_for_thread
                    .snapshot
                    .lock()
                    .map_err(|_| "job state lock poisoned".to_string())?;
                snapshot.state = "scanning".to_string();
            }

            let mut totals = (0u64, 0u64);
            for source in &sources {
                scan_tree(source, &mut totals, &control_for_thread.cancel, Some(&control_for_thread))?;
            }
            {
                let mut snapshot = control_for_thread
                    .snapshot
                    .lock()
                    .map_err(|_| "job state lock poisoned".to_string())?;
                snapshot.bytes_total = totals.0;
                snapshot.items_total = totals.1;
                snapshot.state = "running".to_string();
            }

            for (source, destination) in sources.iter().zip(destinations.iter()) {
                if control_for_thread.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".to_string());
                }

                if kind_owned == "move" {
                    if fs::rename(source, destination).is_ok() {
                        let item_totals = {
                            let mut totals = (0u64, 0u64);
                            scan_tree(destination, &mut totals, &AtomicBool::new(false), None)?;
                            totals
                        };
                        note_progress(&control_for_thread, item_totals.0, item_totals.1)?;
                        continue;
                    }
                }

                if let Err(error) = copy_tree(source, destination, &control_for_thread) {
                    cleanup_partial_destination(destination);
                    return Err(error);
                }

                if kind_owned == "move" {
                    if control_for_thread.cancel.load(Ordering::Relaxed) {
                        cleanup_partial_destination(destination);
                        return Err("cancelled".to_string());
                    }
                    remove_tree_no_follow(source)?;
                }
            }
            Ok(())
        })();

        if let Ok(mut snapshot) = control_for_thread.snapshot.lock() {
            snapshot.cancellable = false;
            match outcome {
                Ok(()) => {
                    snapshot.state = "completed".to_string();
                    snapshot.bytes_done = snapshot.bytes_total;
                    snapshot.items_done = snapshot.items_total;
                }
                Err(error) if error == "cancelled" => {
                    snapshot.state = "cancelled".to_string();
                    snapshot.error = None;
                }
                Err(error) => {
                    snapshot.state = "failed".to_string();
                    snapshot.error = Some(error);
                }
            }
        }
    });

    Ok(json!({ "jobId": id }))
}

fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), String> {
    let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err("frame exceeds File Manager Plugin API limit".to_string());
    }
    writer
        .write_all(&(payload.len() as u32).to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&payload)
        .map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}

fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, String> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.to_string()),
    }

    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(format!("invalid frame length: {length}"));
    }

    let mut payload = vec![0u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|error| error.to_string())?;
    Ok(Some(payload))
}

fn call_host<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    next_request_id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    *next_request_id = next_request_id.wrapping_add(1).max(1);
    let request_id = *next_request_id;
    write_frame(
        writer,
        &PluginHostRequest {
            message_type: "plugin.request",
            request_id,
            method,
            params,
        },
    )?;

    let payload = read_frame(reader)?
        .ok_or_else(|| "host closed stream while awaiting host.response".to_string())?;
    let response: HostResponse = serde_json::from_slice(&payload)
        .map_err(|error| format!("invalid host.response: {error}"))?;
    if response.message_type != "host.response" {
        return Err(format!(
            "unexpected Plugin API v1 message type while awaiting host.response: {}",
            response.message_type
        ));
    }
    if response.request_id != request_id {
        return Err(format!(
            "host.response requestId mismatch: expected {request_id}, got {}",
            response.request_id
        ));
    }
    if response.ok {
        Ok(response.result)
    } else if response.error.trim().is_empty() {
        Err(format!("host API method {method} failed"))
    } else {
        Err(response.error)
    }
}


fn initial_path() -> PathBuf {
    #[cfg(windows)]
    if let Some(path) = env::var_os("USERPROFILE") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return path;
        }
    }

    #[cfg(not(windows))]
    if let Some(path) = env::var_os("HOME") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return path;
        }
    }

    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(windows)]
fn is_hidden(path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
        || path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with('.'))
}

#[cfg(not(windows))]
fn is_hidden(path: &Path, _metadata: &fs::Metadata) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn entry_kind(metadata: &fs::Metadata) -> EntryKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

fn kind_rank(kind: &EntryKind) -> u8 {
    match kind {
        EntryKind::Directory => 0,
        EntryKind::Symlink => 1,
        EntryKind::File => 2,
        EntryKind::Other => 3,
    }
}

fn is_text_like_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return true;
    };
    let extension = extension.to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "txt" | "md" | "markdown" | "log" | "json" | "jsonc" | "yaml" | "yml" |
        "toml" | "ini" | "cfg" | "conf" | "xml" | "html" | "htm" | "css" | "scss" |
        "sass" | "less" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "py" |
        "rs" | "go" | "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "cs" | "java" |
        "kt" | "kts" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" |
        "sql" | "csv" | "tsv" | "env" | "gitignore" | "gitattributes" | "editorconfig"
    ) || matches!(
        name.to_ascii_lowercase().as_str(),
        "dockerfile" | "makefile" | "license" | "readme"
    )
}

fn open_with_system_app(path: &Path) -> Result<Value, String> {
    let canonical = dunce::canonicalize(path)
        .map_err(|error| format!("cannot open {}: {error}", path_text(path)))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("cannot inspect {}: {error}", path_text(&canonical)))?;
    if !metadata.is_file() {
        return Err("Open with System App requires a regular file".to_string());
    }

    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("explorer.exe");
        command.arg(&canonical);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(&canonical);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(&canonical);
        command
    };
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    return Err("Open with System App is not supported on this platform".to_string());

    command
        .spawn()
        .map_err(|error| format!("failed to open system application: {error}"))?;

    Ok(json!({ "opened": true, "path": path_text(&canonical) }))
}

fn list_directory(path: &Path) -> Result<Value, String> {
    let canonical = dunce::canonicalize(path)
        .map_err(|error| format!("cannot open {}: {error}", path_text(path)))?;
    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", path_text(&canonical)));
    }

    let mut entries = Vec::new();
    let reader = fs::read_dir(&canonical)
        .map_err(|error| format!("cannot read {}: {error}", path_text(&canonical)))?;

    for item in reader {
        let item = item.map_err(|error| format!("failed to read directory entry: {error}"))?;
        let item_path = item.path();
        let metadata = fs::symlink_metadata(&item_path)
            .map_err(|error| format!("cannot inspect {}: {error}", path_text(&item_path)))?;
        let kind = entry_kind(&metadata);
        let (link_target_kind, broken_link) = if matches!(&kind, EntryKind::Symlink) {
            match fs::metadata(&item_path) {
                Ok(target_metadata) => (Some(entry_kind(&target_metadata)), false),
                Err(_) => (None, true),
            }
        } else {
            (None, false)
        };
        entries.push(DirectoryEntry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: path_text(&item_path),
            size: matches!(&kind, EntryKind::File).then_some(metadata.len()),
            modified_ms: modified_ms(&metadata),
            hidden: is_hidden(&item_path, &metadata),
            link_target_kind,
            broken_link,
            kind,
        });
    }

    entries.sort_by(|left, right| {
        kind_rank(&left.kind)
            .cmp(&kind_rank(&right.kind))
            .then_with(|| {
                left.name
                    .to_lowercase()
                    .cmp(&right.name.to_lowercase())
                    .then_with(|| left.name.cmp(&right.name))
            })
    });

    let parent = canonical.parent().map(path_text);
    Ok(json!({
        "path": path_text(&canonical),
        "parent": parent,
        "entries": entries
    }))
}

fn dispatch(
    request: &HostRequest,
    host_call: &mut dyn FnMut(&str, Value) -> Result<Value, String>,
    jobs: &Arc<JobManager>,
    property_scans: &Arc<PropertyScanManager>,
) -> Result<Value, String> {
    match request.method.as_str() {
        "runtime.ping" => Ok(json!({
            "pluginId": PLUGIN_ID,
            "pluginVersion": PLUGIN_VERSION,
            "apiVersion": API_VERSION,
            "status": "ok"
        })),
        "ui.state.get" => {
            let result = host_call("storage.get", json!({ "key": UI_STATE_KEY }))?;
            let state = match result.get("value") {
                None | Some(Value::Null) => UiState::default(),
                Some(value) => {
                    let parsed: UiState = serde_json::from_value(value.clone())
                        .map_err(|error| format!("invalid stored File Manager UI state: {error}"))?;
                    normalize_ui_state(parsed)
                }
            };
            Ok(serde_json::to_value(state).unwrap_or_else(|_| json!({})))
        }
        "ui.state.set" => {
            let parsed: UiState = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid File Manager UI state: {error}"))?;
            let state = normalize_ui_state(parsed);
            host_call(
                "storage.set",
                json!({ "key": UI_STATE_KEY, "value": state }),
            )?;
            Ok(json!({ "stored": true }))
        }
        "integration.status" => {
            let result = host_call(
                "plugin.available",
                json!({ "pluginId": "remotehand.editor" }),
            )?;
            Ok(json!({
                "editorAvailable": result
                    .get("available")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            }))
        }
        "integration.open-editor" => {
            #[derive(Deserialize)]
            struct Params { path: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid Open in Editor request: {error}"))?;
            let canonical = dunce::canonicalize(Path::new(&params.path))
                .map_err(|error| format!("cannot open {}: {error}", params.path))?;
            let metadata = fs::metadata(&canonical)
                .map_err(|error| format!("cannot inspect {}: {error}", path_text(&canonical)))?;
            if !metadata.is_file() {
                return Err("Open in Editor requires a regular file".to_string());
            }
            if !is_text_like_path(&canonical) {
                return Err("selected file is not a supported text/code type".to_string());
            }
            host_call(
                "plugin.open_panel",
                json!({
                    "pluginId": "remotehand.editor",
                    "panelId": "editor",
                    "context": { "path": path_text(&canonical) }
                }),
            )?;
            Ok(json!({ "opened": true }))
        }
        "fs.open-system" => {
            #[derive(Deserialize)]
            struct Params { path: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid system-open request: {error}"))?;
            open_with_system_app(Path::new(&params.path))
        }
        "fs.initial" => list_directory(&initial_path()),
        "fs.list" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                path: String,
            }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid directory request: {error}"))?;
            if params.path.trim().is_empty() {
                return Err("path is required".to_string());
            }
            list_directory(Path::new(&params.path))
        }

        "fs.mkdir" => {
            #[derive(Deserialize)]
            struct Params { parent: String, name: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid create-folder request: {error}"))?;
            create_folder(Path::new(&params.parent), &params.name)
        }
        "fs.rename" => {
            #[derive(Deserialize)]
            struct Params { path: String, name: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid rename request: {error}"))?;
            rename_entry(Path::new(&params.path), &params.name)
        }
        "fs.delete" => {
            #[derive(Deserialize)]
            struct Params { path: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid delete request: {error}"))?;
            recycle_entries(&[params.path])
        }
        "fs.delete-many" => {
            #[derive(Deserialize)]
            struct Params { paths: Vec<String> }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid multi-delete request: {error}"))?;
            recycle_entries(&params.paths)
        }
        "fs.properties" => {
            #[derive(Deserialize)]
            struct Params { path: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid properties request: {error}"))?;
            entry_properties(Path::new(&params.path))
        }
        "fs.properties.scan.start" => {
            #[derive(Deserialize)]
            struct Params { path: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid properties scan request: {error}"))?;
            Ok(json!({ "scanId": property_scans.start(Path::new(&params.path))? }))
        }
        "fs.properties.scan.get" => {
            #[derive(Deserialize)]
            struct Params { id: u64 }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid properties scan status request: {error}"))?;
            serde_json::to_value(property_scans.get(params.id)?).map_err(|error| error.to_string())
        }
        "fs.properties.scan.cancel" => {
            #[derive(Deserialize)]
            struct Params { id: u64 }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid properties scan cancel request: {error}"))?;
            serde_json::to_value(property_scans.cancel(params.id)?).map_err(|error| error.to_string())
        }
        "fs.job.delete.start" => {
            #[derive(Deserialize)]
            struct Params { sources: Vec<String> }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid delete-job request: {error}"))?;
            start_delete_job(Arc::clone(jobs), &params.sources)
        }
        "fs.job.start" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params { kind: String, sources: Vec<String>, destination_dir: String }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid file-operation request: {error}"))?;
            start_job(
                Arc::clone(jobs),
                &params.kind,
                &params.sources,
                Path::new(&params.destination_dir),
            )
        }
        "fs.job.get" => {
            #[derive(Deserialize)]
            struct Params { id: u64 }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid job-status request: {error}"))?;
            serde_json::to_value(jobs.get(params.id)?).map_err(|error| error.to_string())
        }
        "fs.job.active" => {
            serde_json::to_value(jobs.active()?).map_err(|error| error.to_string())
        }
        "fs.job.cancel" => {
            #[derive(Deserialize)]
            struct Params { id: u64 }
            let params: Params = serde_json::from_value(request.params.clone())
                .map_err(|error| format!("invalid job-cancel request: {error}"))?;
            serde_json::to_value(jobs.cancel(params.id)?).map_err(|error| error.to_string())
        }
        method => Err(format!("unsupported plugin method: {method}")),
    }
}

fn run() -> Result<(), String> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();

    write_frame(
        &mut stdout,
        &PluginHello {
            message_type: "plugin.hello",
            plugin_id: PLUGIN_ID,
            api_version: API_VERSION,
            plugin_version: PLUGIN_VERSION,
        },
    )?;

    let payload = read_frame(&mut stdin)?
        .ok_or_else(|| "host closed stream before host.ready".to_string())?;
    let ready: HostReady =
        serde_json::from_slice(&payload).map_err(|error| format!("invalid host.ready: {error}"))?;

    if ready.message_type != "host.ready"
        || ready.plugin_id != PLUGIN_ID
        || ready.api_version != API_VERSION
    {
        return Err("RemoteHand rejected Plugin API v1 startup contract".to_string());
    }

    let mut next_host_request_id = 0u64;
    let jobs = Arc::new(JobManager::default());
    let property_scans = Arc::new(PropertyScanManager::default());

    while let Some(payload) = read_frame(&mut stdin)? {
        let request: HostRequest = serde_json::from_slice(&payload)
            .map_err(|error| format!("invalid host request: {error}"))?;

        if request.message_type != "host.request" {
            return Err(format!(
                "unexpected Plugin API v1 message type: {}",
                request.message_type
            ));
        }

        let mut host_call = |method: &str, params: Value| {
            call_host(
                &mut stdin,
                &mut stdout,
                &mut next_host_request_id,
                method,
                params,
            )
        };

        let (ok, result, error) = match dispatch(&request, &mut host_call, &jobs, &property_scans) {
            Ok(result) => (true, result, String::new()),
            Err(error) => (false, Value::Null, error),
        };

        write_frame(
            &mut stdout,
            &PluginResponse {
                message_type: "plugin.response",
                request_id: request.request_id,
                ok,
                result,
                error,
            },
        )?;
    }

    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("RemoteHand File Manager plugin failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    fn temp_dir(name: &str) -> PathBuf {
        let mut path = env::temp_dir();
        path.push(format!(
            "remotehand-file-manager-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    #[test]
    fn text_like_detection_covers_editor_targets() {
        assert!(is_text_like_path(Path::new("notes.txt")));
        assert!(is_text_like_path(Path::new("script.PY")));
        assert!(is_text_like_path(Path::new("Dockerfile")));
        assert!(is_text_like_path(Path::new("README")));
        assert!(!is_text_like_path(Path::new("photo.png")));
        assert!(!is_text_like_path(Path::new("document.pdf")));
    }

    #[test]
    fn integration_status_reflects_editor_availability() {
        let jobs = Arc::new(JobManager::default());
        let property_scans = Arc::new(PropertyScanManager::default());
        let request = HostRequest {
            message_type: "host.request".to_string(),
            request_id: 1,
            method: "integration.status".to_string(),
            params: Value::Null,
        };

        let mut available_call = |method: &str, params: Value| {
            assert_eq!(method, "plugin.available");
            assert_eq!(params["pluginId"], "remotehand.editor");
            Ok(json!({ "available": true }))
        };
        let result = dispatch(&request, &mut available_call, &jobs, &property_scans).unwrap();
        assert_eq!(result["editorAvailable"], true);

        let mut unavailable_call = |method: &str, params: Value| {
            assert_eq!(method, "plugin.available");
            assert_eq!(params["pluginId"], "remotehand.editor");
            Ok(json!({ "available": false }))
        };
        let result = dispatch(&request, &mut unavailable_call, &jobs, &property_scans).unwrap();
        assert_eq!(result["editorAvailable"], false);
    }

    #[test]
    fn open_editor_revalidates_file_and_routes_context() {
        let root = temp_dir("open-editor");
        let file = root.join("hello.txt");
        fs::write(&file, b"hello").unwrap();

        let jobs = Arc::new(JobManager::default());
        let property_scans = Arc::new(PropertyScanManager::default());
        let request = HostRequest {
            message_type: "host.request".to_string(),
            request_id: 2,
            method: "integration.open-editor".to_string(),
            params: json!({ "path": path_text(&file) }),
        };

        let expected = path_text(&dunce::canonicalize(&file).unwrap());
        let mut host_call = |method: &str, params: Value| {
            assert_eq!(method, "plugin.open_panel");
            assert_eq!(params["pluginId"], "remotehand.editor");
            assert_eq!(params["panelId"], "editor");
            assert_eq!(params["context"]["path"], expected);
            Ok(json!({ "opened": true }))
        };

        let result = dispatch(&request, &mut host_call, &jobs, &property_scans).unwrap();
        assert_eq!(result["opened"], true);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_directory_sorts_directories_before_files() {
        let root = temp_dir("sort");
        fs::create_dir(root.join("z-folder")).unwrap();
        File::create(root.join("a-file.txt")).unwrap();

        let value = list_directory(&root).unwrap();
        let entries = value["entries"].as_array().unwrap();
        assert_eq!(entries[0]["name"], "z-folder");
        assert_eq!(entries[0]["kind"], "directory");
        assert_eq!(entries[1]["name"], "a-file.txt");
        assert_eq!(entries[1]["kind"], "file");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_directory_reports_parent_and_file_size() {
        let root = temp_dir("metadata");
        fs::write(root.join("hello.txt"), b"hello").unwrap();

        let value = list_directory(&root).unwrap();
        assert!(value["parent"].is_string());
        assert_eq!(value["entries"][0]["size"], 5);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn list_directory_rejects_files() {
        let root = temp_dir("file");
        let file = root.join("hello.txt");
        fs::write(&file, b"hello").unwrap();

        let error = list_directory(&file).unwrap_err();
        assert!(error.contains("not a directory"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ui_state_normalizes_active_pane() {
        let state = normalize_ui_state(UiState {
            dual_pane: true,
            left_path: Some("left".into()),
            right_path: Some("right".into()),
            active_pane: "unexpected".into(),
        });
        assert_eq!(state.active_pane, "left");
        assert!(state.dual_pane);
    }


    #[cfg(unix)]
    #[test]
    fn resolve_entry_without_following_preserves_symlink() {
        use std::os::unix::fs::symlink;
        let root = temp_dir("resolve-link");
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();

        let resolved = resolve_entry_without_following(&link).unwrap();
        assert!(fs::symlink_metadata(&resolved).unwrap().file_type().is_symlink());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validate_leaf_name_rejects_paths() {
        assert!(validate_leaf_name("../bad").is_err());
        assert!(validate_leaf_name("folder/name").is_err());
        assert!(validate_leaf_name("").is_err());
        assert_eq!(validate_leaf_name("good name").unwrap(), "good name");
    }

    #[test]
    fn multi_job_rejects_destination_conflict_before_starting() {
        let root = temp_dir("multi-conflict");
        let source_a = root.join("a");
        let source_b = root.join("b");
        let destination = root.join("dest");
        fs::create_dir(&source_a).unwrap();
        fs::create_dir(&source_b).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source_a.join("same.txt"), b"a").unwrap();
        fs::write(source_b.join("same.txt"), b"b").unwrap();

        let manager = Arc::new(JobManager::default());
        let sources = vec![
            path_text(&source_a.join("same.txt")),
            path_text(&source_b.join("same.txt")),
        ];
        let error = start_job(manager, "copy", &sources, &destination).unwrap_err();
        assert!(error.contains("same destination name"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn property_scan_counts_nested_files_and_folders() {
        let root = temp_dir("property-scan");
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(root.join("one.bin"), vec![1u8; 10]).unwrap();
        fs::write(nested.join("two.bin"), vec![2u8; 20]).unwrap();

        let control = PropertyScanControl {
            snapshot: Mutex::new(PropertyScanSnapshot {
                id: 1,
                state: "scanning".into(),
                path: path_text(&root),
                files: 0,
                folders: 0,
                links: 0,
                bytes: 0,
                cancellable: true,
                error: None,
            }),
            cancel: AtomicBool::new(false),
        };

        scan_properties_tree(&root, &control).unwrap();
        let snapshot = control.snapshot.lock().unwrap();
        assert_eq!(snapshot.files, 2);
        assert_eq!(snapshot.folders, 1);
        assert_eq!(snapshot.bytes, 30);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_job_is_reported_as_active_while_deleting() {
        let manager = JobManager::default();
        let source = PathBuf::from("source");
        let control = manager.create_delete(&[source]).unwrap();
        {
            let mut snapshot = control.snapshot.lock().unwrap();
            snapshot.state = "deleting".to_string();
            snapshot.cancellable = false;
        }
        let active = manager.active().unwrap().unwrap();
        assert_eq!(active.kind, "delete");
        assert_eq!(active.state, "deleting");
        assert!(!active.cancellable);
    }

    #[test]
    fn job_manager_reports_only_active_jobs() {
        let manager = JobManager::default();
        assert!(manager.active().unwrap().is_none());

        let source = PathBuf::from("source");
        let destination = PathBuf::from("destination");
        let control = manager.create("copy", &[source], &destination).unwrap();
        assert_eq!(manager.active().unwrap().unwrap().state, "queued");

        control.snapshot.lock().unwrap().state = "completed".to_string();
        assert!(manager.active().unwrap().is_none());
    }

    #[test]
    fn scan_tree_publishes_live_totals() {
        let root = temp_dir("scan-progress");
        fs::write(root.join("one.bin"), vec![1u8; 10]).unwrap();
        fs::write(root.join("two.bin"), vec![2u8; 20]).unwrap();

        let control = JobControl {
            snapshot: Mutex::new(JobSnapshot {
                id: 1,
                kind: "copy".into(),
                state: "scanning".into(),
                sources: vec![path_text(&root)],
                destination: String::new(),
                bytes_total: 0,
                bytes_done: 0,
                items_total: 0,
                items_done: 0,
                cancellable: true,
                error: None,
            }),
            cancel: AtomicBool::new(false),
        };

        let mut totals = (0u64, 0u64);
        scan_tree(&root, &mut totals, &control.cancel, Some(&control)).unwrap();
        let snapshot = control.snapshot.lock().unwrap();
        assert_eq!(snapshot.bytes_total, 30);
        assert_eq!(snapshot.items_total, 3);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_tree_copies_files_without_following_directory_layout_errors() {
        let root = temp_dir("copy-tree");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("hello.txt"), b"hello").unwrap();

        let control = JobControl {
            snapshot: Mutex::new(JobSnapshot {
                id: 1,
                kind: "copy".into(),
                state: "running".into(),
                sources: vec![path_text(&source)],
                destination: path_text(&destination),
                bytes_total: 5,
                bytes_done: 0,
                items_total: 2,
                items_done: 0,
                cancellable: true,
                error: None,
            }),
            cancel: AtomicBool::new(false),
        };

        copy_tree(&source, &destination, &control).unwrap();
        assert_eq!(fs::read(destination.join("hello.txt")).unwrap(), b"hello");
        let snapshot = control.snapshot.lock().unwrap();
        assert_eq!(snapshot.bytes_done, 5);
        assert_eq!(snapshot.items_done, 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn list_directory_reports_symlink_target_kind() {
        use std::os::unix::fs::symlink;
        let root = temp_dir("symlink");
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        symlink(&target, root.join("link")).unwrap();

        let value = list_directory(&root).unwrap();
        let entries = value["entries"].as_array().unwrap();
        let link = entries.iter().find(|entry| entry["name"] == "link").unwrap();
        assert_eq!(link["kind"], "symlink");
        assert_eq!(link["linkTargetKind"], "directory");
        assert_eq!(link["brokenLink"], false);

        fs::remove_dir_all(root).unwrap();
    }
}
