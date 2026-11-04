//! Recovery versions are visible in this chat but outside the active transcript.
use super::*;

#[derive(Default)]
pub(super) struct TurnRecoveries {
    versions: Vec<vorkspace_core::store::RecoveredTurn>,
    open: Option<(String, Arc<str>)>,
    selection: u64,
    refresh: u64,
    retrying: bool,
}

impl Conversation {
    pub(super) fn refresh_turn_recoveries(&mut self, cx: &mut Context<Self>) {
        self.turn_recoveries.refresh += 1;
        let refresh = self.turn_recoveries.refresh;
        let store = self.store.clone();
        let id = self.summary.id;
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.recovered_turns(id) });
        cx.spawn(async move |this, cx| {
            if let Ok(versions) = work.await {
                let _ = this.update(cx, |this, cx| {
                    if this.turn_recoveries.refresh == refresh {
                        this.turn_recoveries.versions = versions;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn open_recovered_turn(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(version) = self.turn_recoveries.versions.get(index) else {
            return;
        };
        let store = self.store.clone();
        let session = self.summary.id;
        let id = version.id.clone();
        self.turn_recoveries.selection += 1;
        let selection = self.turn_recoveries.selection;
        let work = cx.background_executor().spawn(async move {
            let rows = locked(&store)?
                .read_recovered_turn(session, &id)?
                .ok_or("This recovery is unavailable")?;
            Ok::<_, String>((
                id,
                Arc::<str>::from(vorkspace_core::store::recovery_text(&rows)),
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.turn_recoveries.selection != selection {
                    return;
                }
                match result {
                    Ok(open) => this.turn_recoveries.open = Some(open),
                    Err(error) => {
                        this.status = format!("Could not open recovered version: {error}")
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retry_recovered_turn(&mut self, id: String, cx: &mut Context<Self>) {
        if self.turn_recoveries.retrying {
            return;
        }
        self.turn_recoveries.retrying = true;
        let store = self.store.clone();
        let session = self.summary.id;
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store)?.retry_turn_overlap(session, &id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.turn_recoveries.retrying = false;
                match result {
                    Ok(()) => this.recover_history_in_place(cx),
                    Err(error) => {
                        this.status = format!("Could not save the recovered version: {error}")
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn recovered_turn_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let versions = &self.turn_recoveries.versions;
        if versions.is_empty() {
            return None;
        }
        let mut banner = div()
            .px_3()
            .pt_2()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(12.))
            .text_color(content(0.8));
        if let Some((id, text)) = &self.turn_recoveries.open {
            let index = versions
                .iter()
                .position(|version| version.id == *id)
                .unwrap_or(0);
            let copy = text.clone();
            let retry = id.clone();
            let pending = versions.get(index).is_some_and(|version| version.pending);
            banner = banner
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(format!(
                            "Recovered version {} of {}",
                            index + 1,
                            versions.len()
                        ))
                        .child(
                            crate::controls::secondary_button(
                                "recovery-newer",
                                "Newer version",
                                false,
                                index == 0,
                            )
                            .child("Newer")
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.open_recovered_turn(index.saturating_sub(1), cx)
                                },
                            )),
                        )
                        .child(
                            crate::controls::secondary_button(
                                "recovery-older",
                                "Older version",
                                false,
                                index + 1 >= versions.len(),
                            )
                            .child("Older")
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.open_recovered_turn(index + 1, cx),
                            )),
                        )
                        .child(
                            crate::controls::secondary_button(
                                "recovery-copy",
                                "Copy full version",
                                false,
                                false,
                            )
                            .child("Copy full version")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    copy.to_string(),
                                ));
                            }),
                        )
                        .child(
                            crate::controls::secondary_button(
                                "recovery-close",
                                "Close recovered version",
                                false,
                                false,
                            )
                            .child("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.turn_recoveries.selection += 1;
                                this.turn_recoveries.open = None;
                                cx.notify();
                            })),
                        ),
                )
                .when(pending, |banner| {
                    banner.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child("Kept in memory. Copy or retry the save before quitting.")
                            .child(
                                crate::controls::secondary_button(
                                    "recovery-retry",
                                    "Retry saving this version",
                                    true,
                                    self.turn_recoveries.retrying,
                                )
                                .child("Retry save")
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.retry_recovered_turn(retry.clone(), cx)
                                    },
                                )),
                            ),
                    )
                })
                .child(
                    div()
                        .id("recovered-turn-preview")
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .child(vorkspace_core::response::limit(text, 8192)),
                );
        } else {
            banner = banner.child(
                crate::controls::secondary_button(
                    "recovered-turns",
                    "Review recovered versions",
                    false,
                    false,
                )
                .child(if versions.iter().any(|version| version.pending) {
                    "Recovered versions · Save pending".into()
                } else {
                    format!("Recovered versions ({})", versions.len())
                })
                .on_click(cx.listener(|this, _, _, cx| this.open_recovered_turn(0, cx))),
            );
        }
        Some(banner.into_any_element())
    }
}
