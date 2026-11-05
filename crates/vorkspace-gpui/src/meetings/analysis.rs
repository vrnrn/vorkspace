//! Explicit summary generation through the selected restricted provider.
use super::{
    Page,
    document::{Field, Section, Snapshot, decode},
};
use gpui::Context;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use vorkspace_core::{
    accounts::Profile,
    provider::{self, Kind},
};

pub(super) struct Analysis {
    pub id: String,
    cancel: Arc<AtomicBool>,
}

impl Page {
    pub(super) fn cancel_analysis(&mut self) {
        if let Some(job) = &self.analysis {
            job.cancel.store(true, Ordering::Release);
        }
    }

    pub(super) fn summarize(&mut self, cx: &mut Context<Self>) {
        if self.analysis.is_some() || self.deleting {
            return;
        }
        let Some(doc) = self.document.as_ref().filter(|doc| {
            doc.saved.is_some()
                && !doc.state.phase.active()
                && !doc.text(Field::Transcript).trim().is_empty()
        }) else {
            return;
        };
        let id = doc.id().to_owned();
        let Some(meetings) = self.storage(cx) else {
            return;
        };
        let intent = self.intent;
        let enable_epoch = self.enable_epoch;
        let cancel = Arc::new(AtomicBool::new(false));
        self.analysis = Some(Analysis {
            id: id.clone(),
            cancel: cancel.clone(),
        });
        self.error = None;
        self.notice = None;
        let timeout = cancel.clone();
        let deadline = cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(900))
                .await;
            timeout.store(true, Ordering::Release);
        });
        let profiles: Vec<(Kind, Profile)> = Kind::ALL
            .into_iter()
            .filter(|kind| {
                provider::supports_text_only(*kind) && !self.hidden.iter().any(|id| id == kind.id())
            })
            .map(|kind| {
                let profile = crate::accounts::get(cx)
                    .map(|center| center.read(cx).selected(kind, &self.cwd))
                    .unwrap_or_default();
                (kind, profile)
            })
            .collect();
        let summarize_id = id.clone();
        let work = cx.background_executor().spawn(async move {
            let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
            let result = meetings.summarize(&summarize_id, &cancel, |settings, prompt| {
                if cancel.load(Ordering::Acquire) {
                    return Err("Meeting summary canceled.".into());
                }
                let (kind, account) = profiles
                    .iter()
                    .find(|(kind, _)| kind.id() == settings.provider)
                    .ok_or("This provider is unavailable in Vorkspace.")?;
                let binary = provider::resolve(*kind)
                    .ok_or("Connect the selected AI provider in Settings first.")?;
                provider::run_restricted_text(
                    provider::Request {
                        kind: *kind,
                        binary,
                        cwd: scratch.path().into(),
                        prompt,
                        provider_id: None,
                        model: Some(settings.model.clone()),
                        settings: vorkspace_core::agent::Settings {
                            account: account.clone(),
                            ..Default::default()
                        },
                    },
                    cancel.clone(),
                    Duration::from_secs(180),
                )
                .and_then(|text| {
                    if cancel.load(Ordering::Acquire) {
                        Err("Meeting summary canceled.".into())
                    } else {
                        Ok(text)
                    }
                })
            })?;
            decode(result)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            drop(deadline);
            let _ = this.update(cx, |this, cx| {
                this.analysis = None;
                if this.enable_epoch == enable_epoch {
                    match result {
                        Ok(result) => this.complete_analysis(intent, result, cx),
                        Err(error) => this.fail(error, cx),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn complete_analysis(
        &mut self,
        intent: u64,
        mut result: Snapshot,
        cx: &mut Context<Self>,
    ) {
        cx.emit(super::Event::Changed);
        let Some(doc) = self
            .document
            .as_mut()
            .filter(|doc| doc.id() == result.meeting.id)
        else {
            return;
        };
        let Some(saved) = &doc.saved else { return };
        if saved.meeting.revision >= result.meeting.revision {
            return;
        }
        result.draft_version = saved.draft_version.clone();
        doc.saved = Some(result);
        if doc.dirty() {
            self.notice = Some("Summary ready. Your unsaved edits are still here.".into());
        } else if self.intent == intent {
            self.section = Section::Summary;
            cx.emit(super::Event::SummaryReady(doc.id().to_owned()));
        }
        self.update_inputs(false, cx);
        self.update_preview(cx);
    }
}
