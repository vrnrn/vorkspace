//! Provider I/O for isolated, bounded, read-only side replies.
use super::{Arc, AtomicBool, Database, Kind, Settings, locked};
use vorkspace_core::provider::{self, Event};

pub(super) enum Stream {
    Text(String),
    Finished(Result<(), String>),
}

pub(super) struct Request {
    pub(super) provider: Kind,
    pub(super) model: Option<String>,
    pub(super) cwd: String,
    pub(super) prompt: String,
    pub(super) settings: Settings,
    pub(super) remaining: usize,
}

pub(super) fn run(
    request: Request,
    store: Database,
    cancel: Arc<AtomicBool>,
    tx: async_channel::Sender<Stream>,
) {
    let Request {
        provider,
        model,
        cwd,
        prompt,
        mut settings,
        mut remaining,
    } = request;
    let result = (|| {
        if settings.account.id != vorkspace_core::accounts::DEFAULT {
            let store = locked(&store)?;
            if !store
                .provider_accounts()?
                .exists(provider, &settings.account.id)
            {
                return Err("This provider account was removed. Choose another account before retrying the side question.".into());
            }
            settings.account.root = store.data_dir().map(std::path::Path::to_path_buf);
        }
        let binary = provider::resolve(provider)
            .ok_or_else(|| format!("{} CLI not found.", provider.label()))?;
        provider::run(
            provider::Request {
                kind: provider,
                binary,
                cwd: cwd.into(),
                prompt,
                provider_id: None,
                model,
                settings,
            },
            cancel,
            |events| {
                let mut batch = String::new();
                let mut error = None;
                for event in events {
                    match event {
                        Event::Text(text) => {
                            let end = text.floor_char_boundary(remaining.min(text.len()));
                            batch.push_str(&text[..end]);
                            remaining -= end;
                            if end < text.len() {
                                error = Some(
                                    "This side question reached its transcript limit. Start a new side question to continue.",
                                );
                                break;
                            }
                        }
                        Event::Tool(_) | Event::Approval(_) | Event::Question(_) => {
                            error = Some(
                                "The side-question provider attempted to use tools or request input.",
                            );
                            break;
                        }
                        _ => {}
                    }
                }
                if !batch.is_empty() {
                    tx.send_blocking(Stream::Text(batch))
                        .map_err(|_| "Side question closed".to_owned())?;
                }
                error.map_or(Ok(()), |error| Err(error.into()))
            },
        )
    })();
    let _ = tx.send_blocking(Stream::Finished(result));
}
