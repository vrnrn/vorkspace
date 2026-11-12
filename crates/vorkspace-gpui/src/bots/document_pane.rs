//! A Bot document owns its reader independently of the Bot island.
use super::*;
use crate::controls::page;
use gpui::{ClipboardItem, FocusHandle};

pub(crate) struct BotDocumentPane {
    store: Database,
    bot: String,
    id: String,
    document: Option<bot::Document>,
    pending_metadata: Option<bot::Document>,
    transcript: Option<Entity<crate::transcript::TranscriptView>>,
    html: Option<Entity<html::Html>>,
    focus: FocusHandle,
    authorized: bool,
    desktop: bool,
    active: bool,
    epoch: u64,
    loading: bool,
    error: Option<String>,
}

impl Focusable for BotDocumentPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BotDocumentPane {
    pub(crate) fn new(
        store: Database,
        bot: String,
        id: String,
        initial: Option<BotDocumentSource>,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = initial.filter(|source| source.bot == bot && source.document.id == id);
        let mut pane = Self {
            store,
            bot,
            id,
            document: None,
            pending_metadata: None,
            transcript: None,
            html: None,
            focus: cx.focus_handle(),
            authorized: initial.is_some(),
            desktop: initial.as_ref().is_some_and(|source| source.desktop),
            active: false,
            epoch: 0,
            loading: false,
            error: None,
        };
        if let Some(source) = initial {
            if source.desktop && source.document.body.is_empty() {
                pane.pending_metadata = Some(source.document);
                pane.refresh(cx);
            } else {
                pane.install(source.document, cx);
            }
        } else {
            pane.error = Some("This document is not listed for the Bot.".into());
        }
        pane
    }

    pub(crate) fn reference(&self) -> (&str, &str) {
        (&self.bot, &self.id)
    }

    pub(crate) fn title(&self) -> String {
        self.pending_metadata
            .as_ref()
            .or(self.document.as_ref())
            .map(|document| document.title.clone())
            .unwrap_or_else(|| "Document".into())
    }

    pub(crate) fn set_document(&mut self, source: BotDocumentSource, cx: &mut Context<Self>) {
        if source.bot != self.bot || source.document.id != self.id {
            return;
        }
        self.authorized = true;
        self.desktop = source.desktop;
        if source.desktop && source.document.body.is_empty() {
            if self.document.as_ref().is_some_and(|old| {
                old.updated_at == source.document.updated_at
                    && old.title == source.document.title
                    && old.kind == source.document.kind
            }) {
                return;
            }
            if self.pending_metadata.as_ref().is_some_and(|old| {
                old.updated_at == source.document.updated_at
                    && old.title == source.document.title
                    && old.kind == source.document.kind
            }) && self.error.is_none()
            {
                return;
            }
            self.pending_metadata = Some(source.document);
            self.refresh(cx);
            return;
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.install(source.document, cx);
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.authorized || !self.desktop {
            return;
        }
        self.epoch = self.epoch.wrapping_add(1);
        let epoch = self.epoch;
        let store = self.store.clone();
        let id = self.id.clone();
        let expected = self
            .pending_metadata
            .clone()
            .or_else(|| self.document.clone());
        self.loading = true;
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.bot_document(&id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.epoch != epoch {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(Some(document))
                        if expected.as_ref().is_some_and(|source| {
                            source.id == this.id
                                && document.id == source.id
                                && document.kind == source.kind
                                && document.title == source.title
                                && document.created_at == source.created_at
                                && document.updated_at == source.updated_at
                        }) =>
                    {
                        this.install(document, cx)
                    }
                    Ok(Some(_)) => this.error = Some("Document identity changed.".into()),
                    Ok(None) => this.error = Some("Document unavailable.".into()),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if !active && let Some(html) = &self.html {
            html.read(cx).hide();
        }
        cx.notify();
    }

    fn install(&mut self, document: bot::Document, cx: &mut Context<Self>) {
        let unchanged = self.document.as_ref().is_some_and(|old| {
            old.id == document.id
                && old.kind == document.kind
                && old.title == document.title
                && old.summary == document.summary
                && old.body == document.body
                && old.created_at == document.created_at
                && old.updated_at == document.updated_at
        });
        let was_loading = self.loading || self.pending_metadata.is_some();
        self.loading = false;
        self.pending_metadata = None;
        self.error = None;
        if unchanged {
            if was_loading {
                cx.notify();
            }
            return;
        }
        let changed_body = self
            .document
            .as_ref()
            .is_none_or(|old| old.kind != document.kind || old.body != document.body);
        if changed_body {
            if let Some(html) = &self.html {
                html.read(cx).hide();
            }
            if document.kind == "html" {
                self.html = Some(cx.new(|_| html::Html::new(document.body.clone())));
                self.transcript = None;
            } else {
                self.transcript = Some(cx.new(|cx| {
                    crate::transcript::TranscriptView::document(document.body.clone(), cx)
                }));
                self.html = None;
            }
        }
        self.document = Some(document);
        cx.notify();
    }
}

impl gpui::Render for BotDocumentPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title();
        let body: AnyElement = if self.loading && self.pending_metadata.is_some() {
            page::empty(
                icon("file", 24.),
                "Loading document",
                "Opening saved content…",
            )
            .into_any_element()
        } else if let Some(html) = &self.html {
            html.clone().into_any_element()
        } else if let Some(transcript) = &self.transcript {
            transcript.clone().into_any_element()
        } else {
            page::empty(
                icon("file", 24.),
                if self.loading {
                    "Loading document"
                } else {
                    "Document unavailable"
                },
                self.error
                    .as_deref()
                    .unwrap_or("The document could not be opened."),
            )
            .into_any_element()
        };
        div()
            .id("bot-document-pane")
            .track_focus(&self.focus)
            .role(Role::Region)
            .aria_label(format!("Bot document: {title}"))
            .size_full()
            .flex()
            .flex_col()
            .child(
                page::toolbar()
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        page::icon_action(
                            "copy-bot-document",
                            "Copy document",
                            self.document.is_some(),
                        )
                        .child(icon("response-copy", page::ICON_SIZE))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(document) = &this.document {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    document.body.clone(),
                                ));
                            }
                        })),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn store() -> Database {
        std::sync::Arc::new(std::sync::Mutex::new(
            vorkspace_core::store::Store::test(std::path::Path::new(":memory:")).unwrap(),
        ))
    }

    fn document(id: &str, body: &str) -> bot::Document {
        bot::Document {
            id: id.into(),
            kind: "document".into(),
            title: id.into(),
            summary: String::new(),
            body: body.into(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn source(id: &str, body: &str) -> BotDocumentSource {
        BotDocumentSource {
            bot: "bot".into(),
            document: document(id, body),
            desktop: false,
        }
    }

    #[gpui::test]
    fn separate_bot_documents_keep_independent_readers_and_unchanged_selection_owners(
        cx: &mut TestAppContext,
    ) {
        let store = store();
        let first = cx.new(|cx| {
            BotDocumentPane::new(
                store.clone(),
                "bot".into(),
                "first".into(),
                Some(source("first", "First body")),
                cx,
            )
        });
        let second = cx.new(|cx| {
            BotDocumentPane::new(
                store,
                "bot".into(),
                "second".into(),
                Some(source("second", "Second body")),
                cx,
            )
        });
        let first_reader = first.read_with(cx, |pane, _| pane.transcript.clone().unwrap());
        let second_reader = second.read_with(cx, |pane, _| pane.transcript.clone().unwrap());
        assert_ne!(first_reader, second_reader);
        first.update(cx, |pane, cx| {
            pane.set_active(true, cx);
            pane.set_active(true, cx);
            pane.set_document(source("first", "First body"), cx);
            assert_eq!(pane.transcript.as_ref(), Some(&first_reader));
            pane.set_document(source("first", "Updated body"), cx);
            assert_ne!(pane.transcript.as_ref(), Some(&first_reader));
        });
        second.read_with(cx, |pane, _| {
            assert_eq!(pane.reference(), ("bot", "second"));
            assert_eq!(pane.document.as_ref().unwrap().body, "Second body");
            assert_eq!(pane.transcript.as_ref(), Some(&second_reader));
        });
    }

    #[gpui::test]
    fn a_document_snapshot_must_match_the_bot_and_document_identity(cx: &mut TestAppContext) {
        let store = store();
        let pane = cx.new(|cx| {
            BotDocumentPane::new(
                store,
                "bot".into(),
                "first".into(),
                Some(BotDocumentSource {
                    bot: "another-bot".into(),
                    document: document("first", "Wrong Bot"),
                    desktop: true,
                }),
                cx,
            )
        });
        pane.read_with(cx, |pane, _| {
            assert!(!pane.authorized);
            assert!(pane.document.is_none());
            assert!(!pane.loading);
        });
        pane.update(cx, |pane, cx| {
            pane.set_document(
                BotDocumentSource {
                    bot: "bot".into(),
                    document: document("second", "Wrong document"),
                    desktop: false,
                },
                cx,
            );
            assert!(pane.document.is_none());
            pane.set_document(source("first", "Owned document"), cx);
            assert_eq!(pane.document.as_ref().unwrap().body, "Owned document");
        });
    }
}
