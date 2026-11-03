//! Versioned executable plugins. No native UI, provider or connector dependency.
mod process;
pub mod recording;
pub use process::call;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{self, BufRead, Read, Write},
    path::{Path, PathBuf},
};

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub protocol_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub ui: bool,
    #[serde(default)]
    pub connector: bool,
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-._".contains(&c))
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
}
impl Manifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != VERSION {
            return Err("Unsupported plugin protocol version".into());
        }
        if !valid_id(&self.id)
            || self.name.trim().is_empty()
            || self.name.len() > 120
            || self.version.len() > 80
        {
            return Err("Invalid plugin identity".into());
        }
        if self
            .capabilities
            .iter()
            .any(|c| !["ai.text", "audio.transcribe"].contains(&c.as_str()))
        {
            return Err("Unknown plugin capability".into());
        }
        Ok(())
    }
    pub fn permits(&self, method: &str) -> bool {
        let capability = match method {
            "ai.models" | "ai.generate" => "ai.text",
            "audio.start" | "audio.stop" | "audio.status" => "audio.transcribe",
            _ => return false,
        };
        self.capabilities.iter().any(|c| c == capability)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Context {
    pub data_dir: PathBuf,
    pub cwd: PathBuf,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}
impl Request {
    pub fn new(id: u64, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        }
    }
}
impl Response {
    pub fn new(id: u64, result: Result<Value, String>) -> Self {
        let (result, error) = match result {
            Ok(v) => (Some(v), None),
            Err(message) => (
                None,
                Some(RpcError {
                    code: -32000,
                    message,
                }),
            ),
        };
        Self {
            jsonrpc: "2.0".into(),
            id,
            result,
            error,
        }
    }
    pub fn value(self, id: u64) -> Result<Value, String> {
        if self.jsonrpc != "2.0" || self.id != id || (self.result.is_some() && self.error.is_some())
        {
            return Err("Plugin response identity mismatch".into());
        }
        if let Some(error) = self.error {
            Err(error.message)
        } else {
            Ok(self.result.unwrap_or(Value::Null))
        }
    }
}
pub fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("Plugin message exceeded 8 MiB".into());
    }
    writer
        .write_all(&bytes)
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
        .map_err(|e| e.to_string())
}
pub fn read_frame<T: serde::de::DeserializeOwned>(reader: &mut impl BufRead) -> Result<T, String> {
    let mut bytes = Vec::new();
    let count = reader
        .take((MAX_FRAME + 2) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    if count == 0 {
        return Err("Plugin disconnected".into());
    }
    if count > MAX_FRAME + 1 || bytes.last() != Some(&b'\n') {
        return Err("Plugin message exceeded its framing limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid plugin message: {e}"))
}
/// The SDK deliberately scopes mutable state to the plugin's private directory.
/// Executables may be written in any language implementing these JSON messages.
pub struct Host<'a, R: BufRead, W: Write> {
    reader: &'a mut R,
    writer: &'a mut W,
    next: u64,
}
impl<R: BufRead, W: Write> Host<'_, R, W> {
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        write_frame(
            self.writer,
            &Request::new(self.next, &format!("host.{method}"), params),
        )?;
        read_frame::<Response>(self.reader)?.value(self.next)
    }
}
pub fn serve(
    manifest: Manifest,
    mut handle: impl FnMut(
        &Context,
        &str,
        Value,
        &mut Host<'_, io::StdinLock<'_>, io::StdoutLock<'_>>,
    ) -> Result<Value, String>,
) -> Result<(), String> {
    manifest.validate()?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let init = read_frame::<Request>(&mut reader)?;
    if init.jsonrpc != "2.0" || init.method != "initialize" {
        return Err("Initialize must be the first plugin request".into());
    }
    let context: Context = serde_json::from_value(init.params).map_err(|e| e.to_string())?;
    write_frame(
        &mut writer,
        &Response::new(init.id, Ok(serde_json::to_value(&manifest).unwrap())),
    )?;
    loop {
        let request = match read_frame::<Request>(&mut reader) {
            Ok(r) => r,
            Err(e) if e == "Plugin disconnected" => break,
            Err(e) => return Err(e),
        };
        if request.jsonrpc != "2.0" {
            return Err("Unsupported JSON-RPC version".into());
        }
        let mut host = Host {
            reader: &mut reader,
            writer: &mut writer,
            next: 1_000,
        };
        let result = if request.method == "plugin.describe" {
            Ok(serde_json::to_value(&manifest).unwrap())
        } else {
            handle(&context, &request.method, request.params, &mut host)
        };
        write_frame(&mut writer, &Response::new(request.id, result))?;
    }
    Ok(())
}
pub fn builtin_executable(id: &str) -> Result<PathBuf, String> {
    if !["github", "gitlab", "jira", "meetings"].contains(&id) {
        return Err("Unknown bundled plugin".into());
    }
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = current.parent().ok_or("Executable has no directory")?;
    let parent = if parent.file_name().is_some_and(|s| s == "deps") {
        parent.parent().unwrap_or(parent)
    } else {
        parent
    };
    let name = format!("vorkspace-{id}-plugin");
    let bundled = parent.join("../PlugIns").join(&name);
    let path = if bundled.is_file() {
        bundled
    } else {
        parent.join(name)
    };
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "The {id} plugin is missing. Rebuild or reinstall Vorkspace."
        ))
    }
}
pub fn private_directory(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Invalid plugin id".into());
    }
    let plugins = root.join("plugins");
    let plugin = plugins.join(id);
    let data = plugin.join("data");
    for path in [&plugins, &plugin, &data] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err("Plugin data paths must be real directories".into());
            }
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.to_string()),
            _ => {}
        }
    }
    Ok(data)
}
