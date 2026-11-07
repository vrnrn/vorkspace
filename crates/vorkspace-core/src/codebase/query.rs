use serde_json::{json, Value};
use std::path::{Component, Path};

/// Restricted schemas from the pinned engine. Project selection belongs to the
/// app capability, never to an untrusted tool argument.
pub fn tools() -> Value {
    serde_json::from_str(include_str!("tools.json")).expect("bundled Codebase tool schemas")
}

fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    let valid = match schema["type"].as_str() {
        Some("string") => value.is_string(),
        Some("integer") => value.as_i64().is_some(),
        Some("boolean") => value.is_boolean(),
        Some("array") => value.is_array(),
        Some("object") => value.is_object(),
        _ => false,
    };
    if !valid {
        return Err("Incorrect Codebase argument type".into());
    }
    if schema["enum"]
        .as_array()
        .is_some_and(|choices| !choices.contains(value))
    {
        return Err("Unsupported Codebase argument value".into());
    }
    if value.as_i64().is_some_and(|n| {
        schema["minimum"].as_i64().is_some_and(|min| n < min)
            || schema["maximum"].as_i64().is_some_and(|max| n > max)
            || n < 0
    }) {
        return Err("Codebase argument exceeds its supported range".into());
    }
    if let Some(items) = value.as_array() {
        if items.len() > schema["maxItems"].as_u64().unwrap_or(128) as usize {
            return Err("Too many Codebase arguments".into());
        }
        for item in items {
            validate(&schema["items"], item)?;
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err("Codebase paths must stay inside this working copy".into());
    }
    // Missing paths can legitimately describe deleted files. Existing prefixes
    // still have to resolve inside the root, including symlinked directories.
    let mut current = root.to_path_buf();
    for component in path.components() {
        current.push(component);
        if std::fs::symlink_metadata(&current).is_ok() {
            let resolved = current.canonicalize().map_err(|e| e.to_string())?;
            if !resolved.starts_with(root) {
                return Err("Codebase path leaves this working copy".into());
            }
        }
    }
    Ok(())
}

pub(super) fn arguments(
    tool: &str,
    value: Value,
    project: &str,
    root: &Path,
) -> Result<Value, String> {
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 24 * 1024 {
        return Err("Codebase arguments exceed 24 KiB".into());
    }
    let catalog = tools();
    let schema = catalog
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == tool)
        .ok_or("This Codebase tool is not available")?
        .get("inputSchema")
        .unwrap();
    let mut values = value
        .as_object()
        .cloned()
        .ok_or("Codebase arguments must be an object")?;
    for (name, value) in &values {
        let property = schema["properties"]
            .get(name)
            .ok_or_else(|| format!("Unsupported Codebase argument: {name}"))?;
        validate(property, value)?;
        if matches!(name.as_str(), "path" | "file_path" | "path_filter") {
            relative_path(root, value.as_str().unwrap())?;
        } else if name == "paths" {
            for path in value.as_array().unwrap() {
                relative_path(root, path.as_str().unwrap())?;
            }
        }
    }
    for name in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !values.contains_key(name) {
            return Err(format!("Missing Codebase argument: {name}"));
        }
    }
    values.insert("project".into(), json!(project));
    values.entry("format").or_insert(json!("json"));
    if schema["properties"].get("max_output_tokens").is_some() {
        values.entry("max_output_tokens").or_insert(json!(4000));
    }
    if schema["properties"].get("max_rows").is_some() {
        values.entry("max_rows").or_insert(json!(200));
    }
    if schema["properties"].get("depth").is_some() {
        values.entry("depth").or_insert(json!(2));
    }
    Ok(Value::Object(values))
}
