use super::*;
use bot::memory;

/// A line is meaningful only within its Bot and source file. The core's unique
/// original-line check still decides whether an edit can safely be saved.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct MemoryTarget {
    pub bot: String,
    pub topic: Option<String>,
    pub original: Option<String>,
}
impl MemoryTarget {
    pub(super) fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.bot,
            self.topic
                .as_ref()
                .map(|t| format!("topic:{t}"))
                .unwrap_or_else(|| "root".into()),
            self.original
                .as_deref()
                .map(bot::files::content_hash)
                .unwrap_or_else(|| "new".into())
        )
    }
}
#[derive(Clone)]
pub(super) struct MemoryEditor {
    pub target: MemoryTarget,
    pub key: String,
}
#[derive(Clone)]
pub(super) struct MemoryTopic {
    pub name: String,
    pub text: Option<String>,
    pub error: Option<String>,
}
fn visible_facts(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
}
impl Bots {
    pub(super) fn publish_memory_topics(&mut self, result: Result<Vec<MemoryTopic>, String>) {
        match result {
            Ok(mut topics) => {
                for topic in &mut topics {
                    if topic.error.is_some() {
                        topic.text = self
                            .memory_topics
                            .iter()
                            .find(|old| old.name == topic.name)
                            .and_then(|old| old.text.clone());
                    }
                }
                self.memory_topics = topics;
                self.memory_load_error = None;
            }
            // Retain the last successful view when a source is unavailable.
            Err(error) => self.memory_load_error = Some(error),
        }
    }
    pub(super) fn memory_page(
        &mut self,
        bot: &Bot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let files = self.files.clone();
        let topics = self.memory_topics.clone();
        let mut rows = Vec::new();
        let mut editor_shown = false;
        if let Some(files) = &files {
            let root_rows =
                self.memory_facts(bot, None, &files.memory, &mut editor_shown, window, cx);
            if !root_rows.is_empty()
                || self
                    .memory_editor
                    .as_ref()
                    .is_some_and(|e| e.target.bot == bot.id && e.target.topic.is_none())
            {
                rows.push(Self::memory_heading("Root memory"));
                if let Some(editor) = self
                    .memory_editor
                    .clone()
                    .filter(|e| e.target.bot == bot.id && e.target.original.is_none())
                {
                    rows.push(self.render_memory_editor(&editor, window, cx));
                    editor_shown = true;
                }
                rows.extend(root_rows);
            }
            for topic in topics {
                let facts = self.memory_facts(
                    bot,
                    Some(&topic.name),
                    topic.text.as_deref().unwrap_or_default(),
                    &mut editor_shown,
                    window,
                    cx,
                );
                if !facts.is_empty() || topic.error.is_some() {
                    rows.push(Self::memory_heading(&topic.name));
                    if let Some(error) = topic.error {
                        rows.push(
                            div()
                                .id(SharedString::from(format!(
                                    "memory-source-error:{}:{}",
                                    bot.id, topic.name
                                )))
                                .child(Self::memory_status(
                                    &format!("Could not load {}: {error}", topic.name),
                                    "bot-memory-source-error",
                                ))
                                .into_any_element(),
                        );
                    }
                    rows.extend(facts);
                }
            }
        }
        // A concurrent CLI change may remove the original line or entire topic.
        // Keep the user's draft visible; edit_line will reject a stale save.
        if !editor_shown
            && let Some(editor) = self
                .memory_editor
                .clone()
                .filter(|e| e.target.bot == bot.id)
        {
            rows.push(Self::memory_heading(
                editor.target.topic.as_deref().unwrap_or("Root memory"),
            ));
            rows.push(self.render_memory_editor(&editor, window, cx));
        }
        let empty = rows.is_empty();
        let failed = self.memory_load_error.is_some();
        let gauge = files.as_ref().map(|files| {
            let loaded = memory::within_budget(&files.memory);
            let lines = loaded.lines().count();
            let dropped = files.memory.lines().count().saturating_sub(lines);
            let share = (lines as f32 / memory::MAX_LINES as f32)
                .max(loaded.len() as f32 / memory::MAX_BYTES as f32)
                .min(1.);
            let tone = if dropped > 0 {
                gpui::Hsla::from(rgb(0xfb2c36)).opacity(0.7)
            } else if share >= 0.8 {
                gpui::Hsla::from(rgb(0xffb900)).opacity(0.7)
            } else {
                content(0.3)
            };
            let label = if dropped > 0 {
                format!("Root memory · {lines} / 200 lines · {dropped} lines not loading")
            } else {
                format!("Root memory · {lines} / 200 lines")
            };
            div()
                .flex_shrink_0()
                .border_t_1()
                .border_color(content(0.04))
                .px_4()
                .pb_3()
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(content(0.45))
                        .child(
                            div()
                                .flex_1()
                                .h(px(4.))
                                .rounded_full()
                                .overflow_hidden()
                                .bg(content(0.08))
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(share))
                                        .rounded_full()
                                        .bg(tone),
                                ),
                        )
                        .child(
                            div()
                                .id("bot-memory-budget")
                                .debug_selector({
                                    let label = label.clone();
                                    move || label.clone()
                                })
                                .aria_label(label.clone())
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(content(0.45))
                        .child(
                            "Root memory loads every turn (up to 24 KB). Topics load on demand.",
                        ),
                )
        });
        div()
            .id("bot-memory-page")
            .debug_selector(|| "bot-memory-page".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                Self::settings_page_header("Memory").when(files.is_some(), |d| {
                    d.child(
                        icon_button_enabled(
                            "bot-add-memory",
                            "Add memory",
                            "plus",
                            !self
                                .memory_editor
                                .as_ref()
                                .is_some_and(|e| e.target.original.is_none()),
                        )
                        .debug_selector(|| "bot-add-memory".into())
                        .size(px(28.))
                        .on_click(cx.listener({
                            let id = bot.id.clone();
                            move |this, _, window, cx| {
                                if this
                                    .memory_editor
                                    .as_ref()
                                    .is_some_and(|e| e.target.original.is_none())
                                {
                                    return;
                                }
                                this.begin_memory_editor(
                                    MemoryTarget {
                                        bot: id.clone(),
                                        topic: None,
                                        original: None,
                                    },
                                    window,
                                    cx,
                                );
                            }
                        })),
                    )
                }),
            )
            .child(
                div()
                    .id("bot-memory-facts")
                    .debug_selector(|| "bot-memory-facts".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_px()
                    .when(self.memory_loading || (files.is_none() && !failed), |d| {
                        d.child(Self::memory_status(
                            if empty { "Loading…" } else { "Refreshing…" },
                            "bot-memory-loading",
                        ))
                    })
                    .when_some(self.memory_load_error.clone(), |d, error| {
                        d.child(Self::memory_status(
                            &format!("Could not load memory: {error}"),
                            "bot-memory-load-error",
                        ))
                    })
                    .when(
                        failed || self.memory_topics.iter().any(|t| t.error.is_some()),
                        |d| {
                            d.child(
                                panels::button("bot-memory-retry", "Retry loading memory")
                                    .debug_selector(|| "bot-memory-retry".into())
                                    .on_click(cx.listener({
                                        let id = bot.id.clone();
                                        move |this, _, _, cx| this.reload_files(&id, cx)
                                    })),
                            )
                        },
                    )
                    .when(
                        files.is_some() && empty && !self.memory_loading && !failed,
                        |d| {
                            d.child(Self::memory_status(
                                "Nothing remembered yet",
                                "bot-memory-empty",
                            ))
                        },
                    )
                    .children(rows),
            )
            .children(gauge)
            .into_any_element()
    }
    fn memory_heading(name: &str) -> AnyElement {
        div()
            .pt_3()
            .pb_1()
            .px_2()
            .text_size(px(11.))
            .line_height(px(16.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(content(0.6))
            .child(name.to_string())
            .into_any_element()
    }
    fn memory_status(text: &str, selector: &'static str) -> AnyElement {
        div()
            .id(selector)
            .debug_selector(move || selector.into())
            .px_2()
            .py_3()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(content(0.55))
            .child(text.to_string())
            .into_any_element()
    }
    fn memory_icon(dim: bool) -> gpui::Div {
        div()
            .size(px(28.))
            .flex_shrink_0()
            .rounded_md()
            .bg(content(0.06))
            .flex()
            .items_center()
            .justify_center()
            .child(icon("sticky-note", 14.).text_color(content(if dim { 0.3 } else { 0.6 })))
    }
    pub(super) fn begin_memory_editor(
        &mut self,
        target: MemoryTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active.as_deref() != Some(&target.bot) {
            return;
        }
        self.memory_editor_revision += 1;
        let key = format!(
            "memory-edit:{}:{}",
            target.key(),
            self.memory_editor_revision
        );
        let field = self.field(
            &key,
            if target.original.is_some() {
                "Edit memory"
            } else {
                "New memory"
            },
            target
                .original
                .as_deref()
                .map(memory::fact)
                .unwrap_or_default(),
            false,
            cx,
        );
        self.memory_editor = Some(MemoryEditor { target, key });
        self.error = None;
        window.focus(&field.focus_handle(cx), cx);
        cx.notify();
    }
    fn render_memory_editor(
        &mut self,
        state: &MemoryEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = &state.key;
        let editor = self.fields[key].clone();
        editor.update(cx, |f, cx| {
            f.set_placeholder("Something it should remember", cx)
        });
        if self.field_blurs.insert(editor.entity_id()) {
            let key = key.clone();
            self.observations.push(
                cx.subscribe(&editor, move |this, _, _: &input::Submitted, cx| {
                    this.commit_memory_input(&key, false, cx)
                }),
            );
            let key = state.key.clone();
            self.observations.push(cx.on_blur(
                &editor.focus_handle(cx),
                window,
                move |this, _, cx| this.commit_memory_input(&key, true, cx),
            ));
        }
        div()
            .id(SharedString::from(format!("memory-editor-{key}")))
            .debug_selector(|| "bot-memory-editor".into())
            .flex()
            .items_start()
            .gap_2p5()
            .rounded_md()
            .bg(content(0.05))
            .px_2()
            .py_2()
            .child(Self::memory_icon(false))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(px(5.))
                    .text_color(content(0.9))
                    .child(editor),
            )
            .on_key_down(cx.listener({
                let key = key.clone();
                move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape"
                        && this.memory_editor.as_ref().is_some_and(|e| e.key == key)
                    {
                        this.memory_editor = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }
            }))
            .into_any_element()
    }
    fn memory_facts(
        &mut self,
        bot: &Bot,
        topic: Option<&str>,
        text: &str,
        editor_shown: &mut bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut result = Vec::new();
        for (index, line) in visible_facts(text).enumerate() {
            let target = MemoryTarget {
                bot: bot.id.clone(),
                topic: topic.map(str::to_owned),
                original: Some(line.into()),
            };
            let row_key = format!("{}:{index}", target.key());
            if let Some(editor) = self.memory_editor.clone().filter(|e| e.target == target) {
                if !*editor_shown {
                    result.push(self.render_memory_editor(&editor, window, cx));
                    *editor_shown = true;
                }
                continue;
            }
            let struck = line.contains("~~");
            let fact = memory::fact(line);
            let date = line
                .trim_start_matches("- ")
                .trim_start_matches("~~")
                .get(..10)
                .and_then(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
                .map(|day| day.format("%b %-d").to_string());
            let edit = target.clone();
            let remove = target.clone();
            let edit_button = edit.clone();
            result.push(
                div()
                    .id(SharedString::from(format!("bot-memory-fact-{row_key}")))
                    .debug_selector({
                        let key = row_key.clone();
                        move || format!("bot-memory-fact-{key}")
                    })
                    .group("memory-fact")
                    .rounded_md()
                    .flex()
                    .items_center()
                    .hover(|s| s.bg(content(0.05)))
                    .child(
                        div()
                            .id(SharedString::from(format!("memory-fact-body-{row_key}")))
                            .debug_selector({
                                let key = row_key.clone();
                                move || format!("memory-fact-body-{key}")
                            })
                            .role(Role::Button)
                            .aria_label(format!("Edit memory: {fact}"))
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .px_2()
                            .py_2()
                            .child(Self::memory_icon(struck))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .line_height(px(20.))
                                    .text_color(content(if struck { 0.35 } else { 0.85 }))
                                    .child(
                                        div()
                                            .truncate()
                                            .when(struck, |d| d.line_through())
                                            .child(fact),
                                    )
                                    .when_some(date, |d, date| {
                                        d.child(
                                            div()
                                                .text_size(px(11.))
                                                .line_height(px(16.))
                                                .text_color(content(0.4))
                                                .child(date),
                                        )
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if struck {
                                    return;
                                }
                                this.begin_memory_editor(edit.clone(), window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .pr_2()
                            .opacity(0.)
                            .group_hover("memory-fact", |s| s.opacity(1.))
                            .when(!struck, |d| {
                                d.child(
                                    icon_button(
                                        "bot-memory-edit",
                                        "Edit memory fact",
                                        "pencil-edit",
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.begin_memory_editor(
                                                edit_button.clone(),
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                                )
                            })
                            .child(
                                icon_button("bot-memory-forget", "Forget memory fact", "trash")
                                    .debug_selector({
                                        let key = row_key.clone();
                                        move || format!("memory-forget-{key}")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.forget_memory_fact(remove.clone(), cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }
        result
    }
    pub(super) fn commit_memory_input(&mut self, key: &str, close: bool, cx: &mut Context<Self>) {
        let Some(editor) = self
            .memory_editor
            .clone()
            .filter(|e| e.key == key && self.active.as_deref() == Some(&e.target.bot))
        else {
            return;
        };
        let text = self
            .value(key, cx)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            self.memory_editor = None;
            cx.notify();
            return;
        }
        let target = editor.target.clone();
        if !self.memory_pending.insert(target.clone()) {
            return;
        }
        let (root, submitted, edited) = (self.root.clone(), text.clone(), target.clone());
        let work = cx.background_executor().spawn(async move {
            let entry = memory::entry(&submitted, &memory::date(), None)?;
            memory::edit(&root, &edited.bot, edited.topic.as_deref(), |old| {
                if let Some(original) = &edited.original {
                    memory::edit_line(old, original, Some(&submitted))
                } else {
                    Ok(memory::add(old, &entry))
                }
            })?;
            Ok::<_, String>(entry)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.memory_pending.remove(&target);
                let current = this.active.as_deref() == Some(&target.bot)
                    && this
                        .memory_editor
                        .as_ref()
                        .is_some_and(|e| e.key == editor.key && e.target == target);
                match result {
                    Ok(entry) => {
                        if current {
                            let field = this.fields[&editor.key].clone();
                            let unchanged = field
                                .read(cx)
                                .content
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                                == text;
                            if unchanged {
                                if target.original.is_some() || close {
                                    this.memory_editor = None;
                                } else {
                                    field.update(cx, |f, cx| f.set_text(String::new(), cx));
                                }
                            } else if target.original.is_some()
                                && let Some(editor) = &mut this.memory_editor
                            {
                                // A newer draft still edits this source, now against the
                                // line just saved instead of its superseded original.
                                editor.target.original = Some(entry);
                            }
                            this.error = None;
                        }
                        this.reload_files(&target.bot, cx);
                    }
                    Err(error) => {
                        if this.active.as_deref() == Some(&target.bot) {
                            this.error = Some(error);
                        }
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
    }
    fn forget_memory_fact(&mut self, target: MemoryTarget, cx: &mut Context<Self>) {
        if self.active.as_deref() != Some(&target.bot)
            || !self.memory_pending.insert(target.clone())
        {
            return;
        }
        let (root, edited) = (self.root.clone(), target.clone());
        let work = cx.background_executor().spawn(async move {
            memory::edit(&root, &edited.bot, edited.topic.as_deref(), |text| {
                memory::edit_line(
                    text,
                    edited
                        .original
                        .as_deref()
                        .ok_or("Missing original memory")?,
                    None,
                )
            })
            .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.memory_pending.remove(&target);
                match result {
                    Ok(()) => this.reload_files(&target.bot, cx),
                    Err(error) => {
                        if this.active.as_deref() == Some(&target.bot) {
                            this.error = Some(error);
                        }
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
    }
}
