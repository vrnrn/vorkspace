//! Native capabilities never expose app IPC, credentials, arbitrary files or tools.
use super::*;
use vorkspace_core::{catalog, provider};
impl Page {
    pub(super) fn host_request(
        &mut self,
        owner: &str,
        method: &str,
        params: Value,
        scope: &AiScope,
        reply: Reply,
        cx: &mut Context<Self>,
    ) {
        let registry = plugins::list(&self.directory);
        let Some(manifest) = registry
            .iter()
            .find(|plugin| plugin.manifest.id == owner && plugin.enabled)
            .map(|plugin| &plugin.manifest)
        else {
            reply(Err("Plugin is disabled or unavailable".into()));
            return;
        };
        if !manifest.permits(method) {
            reply(Err(
                "Plugin has not been granted this host capability".into()
            ));
            return;
        }
        let result = match method {
            "audio.start" => {
                let Some(id) = params["id"].as_str() else {
                    reply(Err("Missing recording id".into()));
                    return;
                };
                vorkspace_plugin::private_directory(&self.directory, owner).and_then(|directory| {
                    crate::clipboard::audio::start(owner.into(), id.into(), directory, cx)
                })
            }
            "audio.stop" => {
                let Some(id) = params["id"].as_str() else {
                    reply(Err("Missing recording id".into()));
                    return;
                };
                crate::clipboard::audio::stop(owner, id, cx)
            }
            "audio.status" => {
                let Some(id) = params["id"].as_str() else {
                    reply(Err("Missing recording id".into()));
                    return;
                };
                crate::clipboard::audio::status(
                    owner,
                    id,
                    params["after"].as_u64().unwrap_or(0),
                    match vorkspace_plugin::private_directory(&self.directory, owner) {
                        Ok(path) => path,
                        Err(error) => {
                            reply(Err(error));
                            return;
                        }
                    },
                    cx,
                )
            }
            "ai.models" => {
                let providers = scope.providers.clone();
                let work = cx.background_executor().spawn(async move {
                    let mut rows = Vec::new();
                    for (kind, profile) in providers {
                        if provider::resolve(kind).is_none() {
                            continue;
                        }
                        let models = catalog::models_for(kind, &profile).unwrap_or_default();
                        rows.push(json!({"id":kind.id(),"name":kind.label(),"models":models}));
                    }
                    Ok(json!(rows))
                });
                cx.spawn(async move |_, _| {
                    reply(work.await);
                })
                .detach();
                return;
            }
            "ai.generate" => {
                let Some(kind) = params["provider"]
                    .as_str()
                    .and_then(Kind::parse)
                    .filter(|kind| scope.providers.iter().any(|(allowed, _)| allowed == kind))
                else {
                    reply(Err("This provider is unavailable in Vorkspace".into()));
                    return;
                };
                let Some(model) = params["model"]
                    .as_str()
                    .filter(|v| !v.trim().is_empty() && v.len() <= 200)
                else {
                    reply(Err("Choose a summary model first".into()));
                    return;
                };
                let Some(prompt) = params["prompt"]
                    .as_str()
                    .filter(|v| !v.trim().is_empty() && v.len() <= 768 * 1024)
                else {
                    reply(Err(
                        "The AI request is empty or exceeds its input budget".into()
                    ));
                    return;
                };
                let Some(binary) = provider::resolve(kind) else {
                    reply(Err(
                        "The selected provider is not installed. Connect it in Vorkspace settings."
                            .into(),
                    ));
                    return;
                };
                let account = scope
                    .providers
                    .iter()
                    .find(|(allowed, _)| *allowed == kind)
                    .unwrap()
                    .1
                    .clone();
                let request = provider::Request {
                    kind,
                    binary,
                    cwd: PathBuf::from(&scope.cwd),
                    prompt: prompt.into(),
                    provider_id: None,
                    model: Some(model.into()),
                    settings: vorkspace_core::agent::Settings {
                        account,
                        ..Default::default()
                    },
                };
                let cancel = Arc::new(AtomicBool::new(false));
                self.jobs.retain(|(_, job)| Arc::strong_count(job) > 1);
                if self.jobs.len() >= 4 {
                    reply(Err("AI requests are busy. Try again shortly.".into()));
                    return;
                }
                self.jobs.push((owner.into(), cancel.clone()));
                let timeout_cancel = cancel.clone();
                let timer = cx.spawn(async move |_, cx| {
                    cx.background_executor()
                        .timer(Duration::from_secs(180))
                        .await;
                    timeout_cancel.store(true, Ordering::Release);
                });
                let work = cx.background_executor().spawn(async move {
                    provider::run_text(request, cancel).map(|text| json!({"text":text}))
                });
                cx.spawn(async move |_, _| {
                    let result = work.await;
                    drop(timer);
                    reply(result);
                })
                .detach();
                return;
            }
            _ => Err("Unknown host capability method".into()),
        };
        reply(result);
    }
}
