//! A Rust-owned loopback adapter for Claude's Copilot models. The provider
//! process owns the gateway, including while parked between turns.
mod protocol;
#[cfg(test)]
mod tests;
use super::{Kind, Request};
use crate::{catalog::Model, local_http, process, scm::shell};
use protocol::{model_id, Events};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
    time::Duration,
};
use subtle::ConstantTimeEq;
const API: &str = "https://api.githubcopilot.com";
const MAX_JSON: u64 = 64 * 1024 * 1024;

pub(super) struct Gateway {
    _server: local_http::Server,
}
/// Copilot access is opt-in: only an explicit `COPILOT_GITHUB_TOKEN` enables it.
/// A `gh` login alone must not surface Copilot models in the Claude picker.
fn credential() -> Result<String, String> {
    // Only carries the login-shell value of the variable; never spawned.
    let mut cmd = Command::new("true");
    shell::apply_copilot_env(&mut cmd);
    std::env::var("COPILOT_GITHUB_TOKEN")
        .ok()
        .or_else(|| {
            cmd.get_envs()
                .find(|(key, _)| *key == "COPILOT_GITHUB_TOKEN")
                .and_then(|(_, v)| v.and_then(|v| v.to_str()).map(str::to_owned))
        })
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Export COPILOT_GITHUB_TOKEN to use Copilot models.".into())
}
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build()
}
fn request(agent: &ureq::Agent, method: &str, url: &str, credential: &str) -> ureq::Request {
    agent
        .request(method, url)
        .set("Authorization", &format!("Bearer {credential}"))
        .set("Content-Type", "application/json")
        .set("Copilot-Integration-Id", "vscode-chat")
        .set("Editor-Version", "vscode/1.104.0")
        .set("Editor-Plugin-Version", "copilot-chat/0.35.0")
        .set("X-GitHub-Api-Version", "2026-06-01")
}
fn read_json(response: ureq::Response, limit: u64) -> Result<Value, String> {
    let mut data = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|_| "Copilot response could not be read")?;
    if data.len() as u64 > limit {
        return Err("Copilot response is too large".into());
    }
    serde_json::from_slice(&data).map_err(|_| "Copilot returned invalid JSON".into())
}
fn redact(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(text.to_string(), |text, key| {
            text.replace(key, "[redacted]")
        })
}
fn response_error(response: ureq::Response) -> String {
    let fallback = format!("Copilot returned HTTP {}", response.status());
    read_json(response, 1_048_576)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["message"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or(fallback)
}
fn catalog(credential: &str) -> Result<Vec<Value>, String> {
    let response = request(&agent(), "GET", &format!("{API}/models"), credential)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(_, r) => redact(&response_error(r), &[credential]),
            _ => "Copilot catalog could not be reached".into(),
        })?;
    catalog_rows(&read_json(response, 1_048_576)?)
}
fn catalog_rows(value: &Value) -> Result<Vec<Value>, String> {
    Ok(value["data"].as_array().ok_or("Invalid Copilot catalog")?.iter().filter(|m|{
        m["id"].as_str().is_some_and(|id|protocol::supported(id))&&m["supported_endpoints"].as_array().is_some_and(|v|v.iter().any(|s|s=="/responses"))&&m["capabilities"]["limits"]["max_context_window_tokens"].as_u64().is_some_and(|v|v>=1_000_000)&&m["capabilities"]["limits"]["max_prompt_tokens"].as_u64().is_some_and(|v|v>0&&v<=9_007_199_254_740_991)&&(m["policy"].is_null()||m["policy"]["state"]=="enabled")
    }).map(|m|json!({"id":m["id"],"name":format!("{} (1M)",m["name"].as_str().unwrap_or_default()),"contextWindow":m["capabilities"]["limits"]["max_prompt_tokens"],"supportedEffortLevels":m["capabilities"]["supports"]["reasoning_effort"]})).collect())
}
pub(crate) fn models() -> Result<Vec<Model>, String> {
    Ok(catalog(&credential()?)?
        .iter()
        .filter_map(|row| {
            let mut model = crate::catalog::model(
                &format!("copilot/{}", row["id"].as_str()?),
                row["name"].as_str()?,
            );
            model.efforts = row["supportedEffortLevels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            model.default_effort = model
                .efforts
                .iter()
                .find(|e| *e == "medium")
                .or(model.efforts.first())
                .cloned();
            Some(model)
        })
        .collect())
}
impl Gateway {
    fn start(
        credential: String,
        effort: Option<String>,
        endpoint: String,
    ) -> Result<(Self, String, String), String> {
        let key = rand::random::<[u8; 32]>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let token = key.clone();
        let server = local_http::Server::start(MAX_JSON as usize, move |local| {
            let supplied = local
                .headers
                .get("authorization")
                .and_then(|s| s.strip_prefix("Bearer "))
                .or_else(|| local.headers.get("x-api-key").map(String::as_str))
                .unwrap_or_default();
            if local.headers.contains_key("origin")
                || !bool::from(supplied.as_bytes().ct_eq(token.as_bytes()))
            {
                return local.json(401,&json!({"type":"error","error":{"type":"authentication_error","message":"Invalid gateway credential"}}));
            }
            if local.method != "POST" || local.path.split('?').next() != Some("/v1/messages") {
                return local.json(404, &json!({"error":"Unknown endpoint"}));
            }
            let result = (|| {
                let body: Value = serde_json::from_slice(&local.read_body()?)
                    .map_err(|_| "Invalid request JSON")?;
                let mut payload = protocol::to_responses(&body, effort.as_deref())?;
                let streaming = body["stream"] == true;
                payload["stream"] = streaming.into();
                let upstream = request(
                    &agent(),
                    "POST",
                    &format!("{endpoint}/responses"),
                    &credential,
                )
                .send_string(&payload.to_string());
                let response=match upstream {Ok(r)=>r,Err(ureq::Error::Status(status,r))=>return local.json(status,&json!({"type":"error","error":{"type":"api_error","message":redact(&response_error(r),&[&credential,&token])}})),Err(_)=>return Err("Copilot request failed or timed out".into())};
                if !streaming {
                    return local.json(
                        200,
                        &protocol::to_message(
                            &read_json(response, MAX_JSON)?,
                            body["model"].as_str().unwrap_or_default(),
                        )?,
                    );
                }
                local.start(200, "text/event-stream")?;
                emit(
                    local,
                    &Events::start(body["model"].as_str().unwrap_or_default()),
                )?;
                let mut converted = Events::default();
                for_each_event(response.into_reader(), |event| {
                    for frame in converted.push(&event)? {
                        emit(local, &frame)?;
                    }
                    Ok(!converted.done)
                })?;
                if !converted.done {
                    return Err("Copilot ended without a completed response".into());
                }
                Ok(())
            })();
            if let Err(error) = result {
                let value = json!({"type":"error","error":{"type":"api_error","message":redact(&error,&[&credential,&token])}});
                if local.has_started() {
                    emit(local, &value)
                } else {
                    local.json(400, &value)
                }
            } else {
                Ok(())
            }
        })?;
        let url = format!("http://{}", server.address);
        Ok((Self { _server: server }, url, key))
    }
}
fn emit(local: &mut local_http::Request, event: &Value) -> Result<(), String> {
    local.write(
        format!(
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap_or("error")
        )
        .as_bytes(),
    )
}
fn for_each_event(
    reader: impl Read,
    mut emit: impl FnMut(Value) -> Result<bool, String>,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    let mut frame = String::new();
    let mut count = 0usize;
    loop {
        let mut line = String::new();
        let n = reader
            .by_ref()
            .take(1_048_577)
            .read_line(&mut line)
            .map_err(|_| "Copilot event stream disconnected")?;
        if n == 0 {
            if count > 0 {
                return Err("Copilot ended an incomplete event frame".into());
            }
            return Ok(());
        }
        count += n;
        if count > 1_048_576 {
            return Err("Copilot event is too large".into());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if !frame.is_empty()
                && frame != "[DONE]"
                && !emit(serde_json::from_str(&frame).map_err(|_| "Invalid Copilot event")?)?
            {
                return Ok(());
            }
            frame.clear();
            count = 0;
        } else if let Some(data) = line.strip_prefix("data:") {
            if !frame.is_empty() {
                frame.push('\n');
            }
            frame.push_str(data.trim_start());
        }
    }
}
/// Called only for a new child, after all provider/account arguments are ready.
pub(super) fn configure(
    command: &mut Command,
    session: &Request,
) -> Result<Option<Gateway>, String> {
    if session.kind != Kind::Claude
        || !session
            .model
            .as_deref()
            .is_some_and(|m| m.starts_with("copilot/"))
    {
        return Ok(None);
    }
    let credential = credential()?;
    let models = catalog(&credential)?;
    let mut args = command
        .get_args()
        .map(|v| v.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let (mut settings, model, selected, effort) = prepare(&mut args, &models)?;
    let (gateway, url, key) = Gateway::start(credential, effort, API.into())?;
    let environment = json!({"ANTHROPIC_BASE_URL":url,"ANTHROPIC_AUTH_TOKEN":key,"ANTHROPIC_API_KEY":"","ENABLE_TOOL_SEARCH":"false","CLAUDE_CODE_USE_BEDROCK":"0","CLAUDE_CODE_USE_VERTEX":"0","CLAUDE_CODE_USE_FOUNDRY":"0","CLAUDE_CODE_MAX_CONTEXT_TOKENS":model["contextWindow"].to_string(),"ANTHROPIC_DEFAULT_OPUS_MODEL":selected,"ANTHROPIC_DEFAULT_SONNET_MODEL":selected,"ANTHROPIC_DEFAULT_HAIKU_MODEL":selected,"ANTHROPIC_DEFAULT_FABLE_MODEL":selected});
    if !settings["env"].is_object() {
        settings["env"] = json!({});
    }
    for (k, v) in environment.as_object().unwrap() {
        settings["env"][k] = v.clone();
    }
    settings["modelPicker"] = json!({"replaceBuiltInOptions":true,"options":models.iter().map(|m|json!({"model":format!("copilot/{}",m["id"].as_str().unwrap()),"label":m["name"]})).collect::<Vec<_>>()});
    args.extend(["--settings".into(), settings.to_string()]);
    let mut next = Command::new(command.get_program());
    next.args(args);
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            next.env(key, value);
        } else {
            next.env_remove(key);
        }
    }
    if let Some(cwd) = command.get_current_dir() {
        next.current_dir(cwd);
    }
    next.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process::isolate(&mut next);
    *command = next;
    Ok(Some(gateway))
}
fn prepare(
    args: &mut Vec<String>,
    models: &[Value],
) -> Result<(Value, Value, String, Option<String>), String> {
    let selected = args
        .windows(2)
        .find(|w| w[0] == "--model")
        .map(|w| w[1].clone())
        .ok_or("Missing Copilot model")?;
    let model = models
        .iter()
        .find(|m| m["id"] == model_id(&selected).unwrap_or_default())
        .ok_or("Copilot does not expose the selected 1M context model")?
        .clone();
    let mut settings = if let Some(i) = args.iter().position(|a| a == "--settings") {
        let raw = args.get(i + 1).ok_or("Missing session settings")?;
        let value: Value = serde_json::from_str(raw).map_err(|_| "Invalid session settings")?;
        args.drain(i..i + 2);
        value
    } else {
        json!({})
    };
    if !settings.is_object() {
        return Err("Session settings must be an object".into());
    }
    let levels = model["supportedEffortLevels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let effort = if let Some(i) = args.iter().position(|a| a == "--effort") {
        let v = args.get(i + 1).ok_or("Missing effort")?.clone();
        args.drain(i..i + 2);
        Some(v)
    } else {
        levels
            .iter()
            .find(|e| **e == "medium")
            .or(levels.first())
            .map(|v| v.to_string())
    };
    if let Some(effort) = &effort {
        if !levels.contains(&effort.as_str()) {
            return Err(format!("Copilot does not support effort {effort}"));
        }
        if effort == "none" {
            settings["alwaysThinkingEnabled"] = false.into();
        } else {
            args.extend(["--effort".into(), effort.clone()]);
        }
    }
    Ok((settings, model, selected, effort))
}
