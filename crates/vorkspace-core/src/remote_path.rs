//! `remote://` paths: how a project on another machine is named everywhere.
use serde::{Deserialize, Serialize};

pub const REMOTE_PATH_PREFIX: &str = "remote://";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePath {
    pub environment_id: String,
    pub host_path: String,
}

/// A saved binding from a desktop project to the Host's project identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteProject {
    pub key: String,
    pub environment_id: String,
    pub project_id: String,
    pub cwd: String,
}

/// Qualifies a host path without losing Windows drive letters or UNC roots.
pub fn remote_path(environment_id: &str, host_path: &str) -> String {
    let path = host_path.replace('\\', "/");
    let path = if path.starts_with("//") {
        &path[1..]
    } else {
        path.trim_start_matches('/')
    };
    format!("{REMOTE_PATH_PREFIX}{environment_id}/{path}")
}

pub fn parse_remote_path(path: &str) -> Option<RemotePath> {
    let path = path.strip_prefix(REMOTE_PATH_PREFIX)?.replace('\\', "/");
    let (environment_id, host_path) = path.split_once('/')?;
    if environment_id.is_empty() {
        return None;
    }
    let bytes = host_path.as_bytes();
    let drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/');
    Some(RemotePath {
        environment_id: environment_id.into(),
        host_path: if drive {
            host_path.into()
        } else {
            format!("/{host_path}")
        },
    })
}
