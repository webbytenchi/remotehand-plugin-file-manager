use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
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
    #[serde(default)]
    granted_permissions: Vec<String>,
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

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
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

        let (ok, result, error) = match dispatch(&request, &mut host_call) {
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
