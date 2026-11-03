//! Thin executable boundaries around the existing, shared connector implementations.
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use vorkspace_core::inbox::{self, BuiltinService, Item, ViewState};
use vorkspace_plugin::{Manifest, VERSION};
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn encode(value: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}
pub fn manifest(id: &str) -> Manifest {
    Manifest {
        protocol_version: VERSION,
        id: id.into(),
        name: inbox::provider_label(id).into(),
        version: env!("CARGO_PKG_VERSION").into(),
        capabilities: vec![],
        ui: false,
        connector: true,
    }
}
pub fn dispatch(
    service: &BuiltinService,
    id: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    if !inbox::PROVIDERS.contains(&id) {
        return Err("Unknown connector".into());
    }
    let string = |key: &str| params[key].as_str().unwrap_or_default().to_owned();
    if method == "connector.status" {
        return encode(service.status(id)?);
    }
    if method == "connector.connect" {
        return encode(service.connect(id, string("url"), string("email"), string("token"))?);
    }
    if method == "connector.validate_jql" && id == "jira" {
        inbox::jira::jira_list_issues(
            service.data_dir.clone(),
            false,
            "all".into(),
            vec![],
            Some(1),
            Some(string("jql")),
        )?;
        return Ok(json!(null));
    }
    if method == "connector.list" {
        let mut state: ViewState = decode(params["state"].clone())?;
        state.jira_filter = decode(params["jira_filter"].clone())?;
        for provider in inbox::PROVIDERS {
            state.connectors.set(provider, provider == id)?;
        }
        let projects: Vec<std::path::PathBuf> = decode(params["projects"].clone())?;
        return encode(service.list(&projects, &state));
    }
    let item: Item = decode(params["item"].clone())?;
    if item.provider != id {
        return Err("Item belongs to another connector".into());
    }
    match method {
        "connector.details" => encode(service.details(&item)?),
        "connector.thread" => encode(service.thread(&item)?),
        "connector.comment" => {
            encode(service.comment(&item, string("body"), string("reply_to"))?)
        }
        "connector.diff" => encode(service.diff(&item)?),
        "connector.checks" => encode(service.checks(&item)?),
        "connector.action" => encode(service.action(&item, &string("action"))?),
        _ => Err("Unknown connector method".into()),
    }
}
pub fn run(id: &str) {
    if let Err(error) = vorkspace_plugin::serve(manifest(id), |context, method, params, _| {
        dispatch(
            &BuiltinService {
                data_dir: context.data_dir.clone(),
                cwd: context.cwd.clone(),
            },
            id,
            method,
            params,
        )
    }) {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
