//! Reconcile references at materialization, never by deleting source history.
use super::*;
use serde_json::Value;

#[derive(Clone, Hash, PartialEq, Eq)]
enum Resource {
    Session(i64),
    Document(String),
    Unidentified(usize, usize),
}

// Preserve the core routing distinction: numbers are native handles; serialized
// desktop IDs resolve through session_handle, including numeric-looking IDs.
// Invalid references must not coalesce.
pub(super) fn session(value: &Value) -> Option<i64> {
    if let Some(id) = value.as_i64() {
        return (id > 0).then_some(id);
    }
    let id = value.as_str()?;
    if id.is_empty() || id.len() > 512 {
        return None;
    }
    id.bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        .then(|| vorkspace_core::bot::work::session_handle(value))
        .flatten()
}

fn resource(value: &Value) -> Option<Resource> {
    match value["type"].as_str()? {
        "session" => session(&value["sessionId"]).map(Resource::Session),
        "document" => value["documentId"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .map(|id| Resource::Document(id.into())),
        _ => None,
    }
}

struct Card {
    resource: Resource,
    group: usize,
    anchor: usize,
    ordinal: usize,
    key: Option<(usize, usize)>,
    run: usize,
    at_end: bool,
    explicit_id: Option<Value>,
    explicit: bool,
    value: Value,
}

// Reuse surviving semantic keys from the suffix we already rebuild. This
// preserves reading anchors if an earlier duplicate is removed or becomes
// available when loading older history, without a second history cache.
pub(super) struct Previous(HashMap<(usize, Resource), (usize, usize)>);
impl Previous {
    pub(super) fn new(rows: &[UiRow], messages: &[Message]) -> Self {
        Self(
            rows.iter()
                .filter_map(|row| {
                    let resource = match &row.body {
                        Body::Launch(id) => Resource::Session(*id),
                        Body::BotCard(value) if row.key.1 >= usize::MAX / 2 => resource(value)?,
                        _ => return None,
                    };
                    Some(((messages.get(row.message)?.turn, resource), row.key))
                })
                .collect(),
        )
    }
}

#[derive(Default)]
pub(super) struct Cards {
    cards: Vec<Card>,
    identities: HashMap<(usize, Resource), usize>,
    sources: HashSet<usize>,
    emissions: HashMap<(bool, usize), Vec<usize>>,
}

impl Cards {
    fn insert(
        &mut self,
        group: usize,
        run: usize,
        at: usize,
        ordinal: usize,
        mut value: Value,
        explicit: bool,
    ) {
        let resource = match resource(&value) {
            Some(resource) => resource,
            None if !explicit && value.is_object() => Resource::Unidentified(at, ordinal),
            None => return,
        };
        if let Resource::Session(id) = resource {
            value["sessionId"] = id.into();
        }
        if explicit {
            self.sources.insert(at);
        }
        let index = *self
            .identities
            .entry((group, resource.clone()))
            .or_insert_with(|| {
                self.cards.push(Card {
                    resource,
                    group,
                    anchor: at,
                    ordinal,
                    key: None,
                    run,
                    at_end: !explicit,
                    explicit_id: None,
                    explicit: false,
                    value: serde_json::json!({}),
                });
                self.cards.len() - 1
            });
        let card = &mut self.cards[index];
        if explicit && card.explicit_id.is_none() {
            card.explicit_id = value.get("id").cloned();
        }
        card.explicit |= explicit;
        // Latest supplied fields win, while supplemental notes/actions and
        // unknown explicit metadata survive sparse automatic projections.
        for (key, value) in value.as_object().unwrap() {
            if key != "id" && (explicit || !value.is_null()) {
                card.value[key] = value.clone();
            }
        }
        if let Some(id) = &card.explicit_id {
            card.value["id"] = id.clone();
        } else if let Some(id) = value.get("id") {
            card.value["id"] = id.clone();
        }
    }

    pub(super) fn collect(view: &TranscriptView, start: usize, previous: Previous) -> Self {
        let mut result = Self::default();
        let mut at = start;
        let mut run = start;
        while at < view.source.len() {
            let next = (at + 1..view.source.len())
                .find(|&i| view.source[i].first)
                .unwrap_or(view.source.len());
            let row = &view.source[at];
            if row.is_prompt() {
                run = next;
                at = next;
                continue;
            }
            let group = if view.bot {
                view.bot_group_start(run)
            } else {
                run
            };
            match row.role.as_str() {
                "bot-card" => {
                    let text: String = view.source[at..next]
                        .iter()
                        .map(|r| r.text.as_str())
                        .collect();
                    if let Ok(value) = serde_json::from_str(&text) {
                        result.insert(group, run, at, 0, value, true);
                    }
                }
                "bot-document" if view.bot => {
                    if let Ok(value) = serde_json::from_str(&row.text) {
                        result.insert(group, run, at, 0, value, false);
                    }
                }
                "bot-launch" | "tool" if view.bot => {
                    for (ordinal, id) in view.run_launches(at, next).into_iter().enumerate() {
                        result.insert(
                            group,
                            run,
                            at,
                            ordinal,
                            serde_json::json!({"type":"session","sessionId":id}),
                            false,
                        );
                    }
                }
                _ => {}
            }
            at = next;
        }
        let mut used = HashSet::new();
        for card in &mut result.cards {
            if let Some(&key) = previous.0.get(&(card.group, card.resource.clone()))
                && used.insert(key)
            {
                card.key = Some(key);
            }
        }
        for (index, card) in result.cards.iter_mut().enumerate() {
            if card.key.is_none() {
                let mut key = (card.anchor, usize::MAX / 2 + card.ordinal);
                while !used.insert(key) {
                    key.1 += 1;
                }
                card.key = Some(key);
            }
            let location = if card.at_end { card.run } else { card.anchor };
            result
                .emissions
                .entry((card.at_end, location))
                .or_default()
                .push(index);
        }
        result
    }

    pub(super) fn owns(&self, at: usize) -> bool {
        self.sources.contains(&at)
    }

    pub(super) fn rows(
        &self,
        at: usize,
        run: usize,
        end_run: bool,
        message: usize,
    ) -> impl Iterator<Item = UiRow> + '_ {
        self.emissions
            .get(&(false, at))
            .into_iter()
            .flatten()
            .chain(
                end_run
                    .then(|| self.emissions.get(&(true, run)))
                    .flatten()
                    .into_iter()
                    .flatten(),
            )
            .map(|&index| &self.cards[index])
            .map(move |card| UiRow {
                // No capped ordinals or hash collisions. Surviving resources
                // keep their keys even when their first source changes.
                key: card.key.unwrap(),
                body: match card.resource {
                    Resource::Session(id) if !card.explicit => Body::Launch(id),
                    _ => Body::BotCard(card.value.clone()),
                },
                message,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            })
    }
}
