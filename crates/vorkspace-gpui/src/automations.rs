//! Native counterpart of AutomationsView: persisted list, example gallery,
//! trigger editor and run history. All scheduling lives in vorkspace-core.
use crate::{
    controls::page,
    input::{self, Input},
    pickers::{ModelList, ModelPicked, ProjectPicked, ProjectPicker, cached_model_name},
    popover::{Anchor, AnchorSlot, popover},
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Role, ScrollHandle,
    SharedString, Subscription, Window, div, prelude::*, px, rgb,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use vorkspace_core::{
    automations::{
        self, Automation, AutomationRun, AutomationTrigger, AutomationUpsert, DefinitionRevision,
        DueAutomationRun, Service,
    },
    provider::Kind,
};
use vorkspace_runtime::runtime::Database;
use vorkspace_ui::radius;
use vorkspace_ui::theme::*;

mod editor;
mod gallery;
#[cfg(test)]
mod screenshots;
mod storage;
#[cfg(test)]
mod tests;
mod trigger_menu;

/// Event providers and schedules are separate from agent harness branding.
fn trigger_mark(kind: &str, side: f32) -> AnyElement {
    if kind == "gitlab" {
        return gpui::img("inbox-gitlab-color.svg")
            .size(px(side))
            .flex_shrink_0()
            .object_fit(gpui::ObjectFit::Contain)
            .into_any_element();
    }
    let (path, color) = match kind {
        "time" => ("clock", page::secondary_ink()),
        "github" => ("inbox-github", page::secondary_ink()),
        "jira" => ("inbox-jira", rgb(0x2684ff).into()),
        _ => ("zap", page::secondary_ink()),
    };
    icon(path, side).text_color(color).into_any_element()
}

pub(crate) enum Event {
    Close,
    Launch(Box<DueAutomationRun>),
    OpenSession(String),
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Template {
    id: String,
    category: String,
    #[serde(default)]
    popular: bool,
    icon: String,
    name: String,
    description: String,
    prompt: String,
    trigger: serde_json::Value,
    trigger_label: String,
}
struct TriggerFields {
    id: String,
    time: Entity<Input>,
    repos: Entity<Input>,
    actor: Entity<Input>,
    _subscriptions: [Subscription; 3],
}
struct Editor {
    draft: AutomationUpsert,
    name: Entity<Input>,
    prompt: Entity<Input>,
    completion: Entity<crate::composer::composer_completion::Completion>,
    fields: Vec<TriggerFields>,
    saved_revision: DefinitionRevision,
    edits: u64,
    dirty: bool,
    _subscriptions: Vec<Subscription>,
}
impl Editor {
    fn saved(&self) -> bool {
        matches!(self.saved_revision, DefinitionRevision::Saved(_))
    }

    fn mark_dirty(&mut self) {
        self.edits += 1;
        self.dirty = true;
    }
}
#[derive(Clone)]
enum Choice {
    Access,
    Workspace,
    Conversation,
    Grace,
    AddTrigger,
    TriggerEvent(String),
    Schedule(String),
    Time(String),
    Minute(String),
    Weekday(String),
    ModelSetting(String),
}
struct ChoiceMenu {
    field: Choice,
    options: Vec<(String, String)>,
    anchor: AnchorSlot,
}

pub(crate) struct Automations {
    service: Arc<Mutex<Service>>,
    database: Option<Database>,
    cwd: String,
    projects: Vec<String>,
    automations: Vec<Automation>,
    runs: Vec<AutomationRun>,
    selected: Option<String>,
    editor: Option<Editor>,
    loading: bool,
    error: Option<String>,
    saving: bool,
    running: bool,
    gallery: bool,
    narrow_detail: bool,
    pane_width: Option<f32>,
    category: String,
    templates: Vec<Template>,
    filter: Entity<Input>,
    list_scroll: ScrollHandle,
    gallery_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    history_scroll: ScrollHandle,
    focus: FocusHandle,
    _filter_subscription: Subscription,
    history: bool,
    advanced: bool,
    confirm_delete: bool,
    choice: Option<ChoiceMenu>,
    models: Option<Entity<ModelList>>,
    model_controls: Vec<vorkspace_core::catalog::ModelControl>,
    project_picker: Option<Entity<ProjectPicker>>,
    picker_subscription: Option<Subscription>,
    project_anchor: AnchorSlot,
    model_anchor: AnchorSlot,
    trigger_anchor: AnchorSlot,
    trigger_menu: Option<Entity<trigger_menu::TriggerMenu>>,
    trigger_subscription: Option<Subscription>,
    trigger_connections: HashMap<String, bool>,
    request_generation: u64,
    refresh_generation: u64,
}
impl EventEmitter<Event> for Automations {}
impl Focusable for Automations {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Automations {
    pub(crate) fn new(
        service: Arc<Mutex<Service>>,
        cwd: String,
        projects: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            Input::field(
                "Filter automations",
                "Filter automations",
                page::secondary_ink(),
                cx,
            )
        });
        let subscription = cx.subscribe(&filter, |_, _, _: &input::Changed, cx| cx.notify());
        let mut this = Self {
            service,
            database: None,
            cwd,
            projects,
            automations: Vec::new(),
            runs: Vec::new(),
            selected: None,
            editor: None,
            loading: true,
            error: None,
            saving: false,
            running: false,
            gallery: true,
            narrow_detail: false,
            pane_width: None,
            category: "popular".into(),
            templates: serde_json::from_str(include_str!("automations/templates.json"))
                .unwrap_or_default(),
            filter,
            list_scroll: ScrollHandle::new(),
            gallery_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            history_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _filter_subscription: subscription,
            history: false,
            advanced: false,
            confirm_delete: false,
            choice: None,
            models: None,
            model_controls: Vec::new(),
            project_picker: None,
            picker_subscription: None,
            project_anchor: AnchorSlot::default(),
            model_anchor: AnchorSlot::default(),
            trigger_anchor: AnchorSlot::default(),
            trigger_menu: None,
            trigger_subscription: None,
            trigger_connections: HashMap::new(),
            request_generation: 0,
            refresh_generation: 0,
        };
        this.refresh(cx);
        this
    }
    pub(crate) fn set_database(&mut self, database: Database) {
        self.database = Some(database);
    }
    pub(crate) fn set_context(
        &mut self,
        cwd: String,
        projects: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.cwd = cwd;
        self.projects = projects;
        cx.notify();
    }
    pub(crate) fn set_pane_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if width.is_finite() && width > 0. && self.pane_width != Some(width) {
            self.pane_width = Some(width);
            cx.notify();
        }
    }
    fn is_narrow(&self, window: &Window) -> bool {
        self.pane_width
            .map_or_else(|| page::narrow(window), |width| width < 900.)
    }
    fn show_gallery(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.selected = None;
        self.runs.clear();
        self.gallery = true;
        self.narrow_detail = true;
        self.history = false;
        self.confirm_delete = false;
        self.choice = None;
        self.trigger_menu = None;
        self.models = None;
        self.project_picker = None;
        self.error = None;
        self.request_generation += 1;
        cx.notify();
    }
    fn back_to_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.narrow_detail = false;
        self.confirm_delete = false;
        self.choice = None;
        self.trigger_menu = None;
        self.models = None;
        self.project_picker = None;
        window.focus(&self.filter.focus_handle(cx), cx);
        cx.notify();
    }
    /// Opens the editor on the first template, for captures.
    #[cfg(test)]
    pub(crate) fn begin_first_template(&mut self, cx: &mut Context<Self>) {
        let template = self.templates.first().cloned();
        self.begin(template, cx);
    }
    fn begin(&mut self, template: Option<Template>, cx: &mut Context<Self>) {
        let preferences = cx.try_global::<vorkspace_ui::preferences::Preferences>();
        let kind = preferences
            .and_then(|p| p.default_provider.as_deref())
            .map(Kind::from_id)
            .unwrap_or(Kind::Codex);
        let model = preferences
            .and_then(|p| p.default_models.get(kind.id()))
            .cloned()
            .unwrap_or_default();
        let mut draft = AutomationUpsert::new(self.cwd.clone(), kind.id().into(), model);
        if let Some(template) = template {
            draft.name = template.name;
            draft.prompt = template.prompt;
            let kind = template.trigger["kind"].as_str().unwrap_or("time");
            let event = template.trigger["event"].as_str().unwrap_or("weekdays");
            let mut trigger = AutomationTrigger::new(kind, event);
            if let Some(value) = template.trigger["time"].as_str() {
                trigger.time = value.into();
            }
            if let Some(value) = template.trigger["dayOfWeek"].as_i64() {
                trigger.day_of_week = value;
            }
            if let Some(value) = template.trigger["minute"].as_i64() {
                trigger.minute = value;
            }
            draft.triggers = Some(vec![trigger]);
        }
        self.selected = None;
        self.runs.clear();
        self.narrow_detail = true;
        self.settings_scroll = ScrollHandle::new();
        self.history_scroll = ScrollHandle::new();
        self.set_editor(draft, DefinitionRevision::New, cx);
    }
    fn open_selection(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(&id) && self.editor.is_some() {
            self.narrow_detail = true;
            cx.notify();
        } else {
            self.select(id, cx);
        }
        if self.narrow_detail && self.is_narrow(window) {
            window.focus(&self.focus, cx);
        }
    }
    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(automation) = self.automations.iter().find(|a| a.id == id) else {
            return;
        };
        let draft = AutomationUpsert::from_automation(automation);
        let revision = DefinitionRevision::Saved(automation.updated_at);
        if self.selected.as_deref() != Some(&id) {
            self.settings_scroll = ScrollHandle::new();
            self.history_scroll = ScrollHandle::new();
        }
        self.selected = Some(id);
        self.narrow_detail = true;
        self.runs.clear();
        self.set_editor(draft, revision, cx);
        self.refresh(cx);
    }
    fn set_editor(
        &mut self,
        draft: AutomationUpsert,
        revision: DefinitionRevision,
        cx: &mut Context<Self>,
    ) {
        self.install_editor(draft, revision, cx);
        self.load_controls(cx);
        cx.notify();
    }
    fn install_editor(
        &mut self,
        draft: AutomationUpsert,
        revision: DefinitionRevision,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| {
            let mut input = Input::automation_name(cx);
            input.set_text(draft.name.clone(), cx);
            input
        });
        let prompt = cx.new(|cx| {
            let mut input = Input::automation_instructions(cx);
            input.set_text(draft.prompt.clone(), cx);
            input
        });
        let completion = cx.new(|cx| {
            let completion = crate::composer::composer_completion::Completion::new(
                prompt.clone(),
                draft.cwd.clone(),
                cx,
            )
            .inline_field();
            if let Some(store) = &self.database {
                completion.with_store(store.clone())
            } else {
                completion
            }
        });
        completion.update(cx, |completion, cx| {
            completion.set_provider(&draft.harness, cx)
        });
        let mut subscriptions = Vec::new();
        for field in [&name, &prompt] {
            subscriptions.push(cx.subscribe(field, |this, _, _: &input::Changed, cx| {
                if let Some(editor) = &mut this.editor {
                    editor.mark_dirty();
                }
                cx.notify();
            }));
        }
        self.editor = Some(Editor {
            draft,
            name,
            prompt,
            completion,
            fields: Vec::new(),
            saved_revision: revision,
            edits: 0,
            dirty: revision == DefinitionRevision::New,
            _subscriptions: subscriptions,
        });
        self.rebuild_trigger_fields(cx);
        self.gallery = false;
        self.history = false;
        self.confirm_delete = false;
        self.choice = None;
        self.trigger_menu = None;
        self.models = None;
        self.project_picker = None;
        self.error = None;
    }
    fn rebuild_trigger_fields(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let triggers = editor.draft.triggers.clone().unwrap_or_default();
        editor
            .fields
            .retain(|fields| triggers.iter().any(|t| t.id == fields.id));
        for trigger in triggers {
            if editor.fields.iter().any(|f| f.id == trigger.id) {
                continue;
            }
            let field = |label: &str, placeholder: &str, text: String, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let mut input = Input::field(
                        label.to_string(),
                        placeholder.to_string(),
                        page::secondary_ink(),
                        cx,
                    );
                    input.set_text(text, cx);
                    input
                })
            };
            let time = field("Trigger time", "09:00", trigger.time, cx);
            let repos = field(
                "Trigger repositories",
                "Any repository",
                trigger
                    .repos
                    .iter()
                    .chain(std::iter::once(&trigger.repo))
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                cx,
            );
            let actor = field("Trigger actor", "Anyone", trigger.actor, cx);
            let subscriptions = [&time, &repos, &actor].map(|input| {
                cx.subscribe(input, |this, _, _: &input::Changed, cx| {
                    if let Some(e) = &mut this.editor {
                        e.mark_dirty();
                    }
                    cx.notify();
                })
            });
            editor.fields.push(TriggerFields {
                id: trigger.id,
                time,
                repos,
                actor,
                _subscriptions: subscriptions,
            });
        }
    }
    fn collected(&self, cx: &App) -> Option<AutomationUpsert> {
        let editor = self.editor.as_ref()?;
        let mut draft = editor.draft.clone();
        draft.name = editor.name.read(cx).content.trim().to_string();
        draft.prompt = editor.prompt.read(cx).content.to_string();
        for trigger in draft.triggers.get_or_insert_with(Vec::new) {
            if let Some(fields) = editor.fields.iter().find(|f| f.id == trigger.id) {
                trigger.time = fields.time.read(cx).content.trim().to_string();
                trigger.repo.clear();
                trigger.repos = fields
                    .repos
                    .read(cx)
                    .content
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                trigger.actor = fields.actor.read(cx).content.trim().to_string();
            }
        }
        draft.sync_triggers(vorkspace_core::time::now_ms() as i64);
        Some(draft)
    }
    fn load_controls(&mut self, cx: &mut Context<Self>) {
        self.model_controls.clear();
        self.request_generation += 1;
        let generation = self.request_generation;
        let Some(editor) = &self.editor else {
            return;
        };
        let kind = Kind::from_id(&editor.draft.harness);
        let model = vorkspace_core::store::native_model(&editor.draft.model, &editor.draft.harness)
            .unwrap_or_default();
        let profile = crate::accounts::get(cx)
            .map(|a| a.read(cx).selected(kind, &editor.draft.cwd))
            .unwrap_or_default();
        let host = self
            .database
            .as_ref()
            .and_then(|db| crate::pickers::HostModels::for_project(db, &editor.draft.cwd));
        if editor.draft.cwd.starts_with("remote://") && host.is_none() {
            return;
        }
        let work = cx.background_executor().spawn(async move {
            if let Some(host) = host {
                host.read()?.models_for(kind)
            } else {
                vorkspace_core::catalog::models_for(kind, &profile)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.request_generation != generation {
                    return;
                }
                if let Ok(models) = result
                    && let Some(model) = models
                        .iter()
                        .find(|m| {
                            m.id == model
                                || m.id
                                    .strip_prefix(kind.id())
                                    .and_then(|id| id.strip_prefix(':'))
                                    == Some(model.as_str())
                        })
                        .or_else(|| models.iter().find(|m| m.default))
                {
                    this.model_controls = model.controls.clone();
                    if !model.efforts.is_empty()
                        && !this.model_controls.iter().any(|c| {
                            matches!(
                                c.id.as_str(),
                                "reasoningEffort" | "reasoning_effort" | "effort" | "reasoning"
                            )
                        })
                    {
                        this.model_controls
                            .push(vorkspace_core::catalog::ModelControl {
                                id: "reasoningEffort".into(),
                                name: "Reasoning".into(),
                                current: model
                                    .default_effort
                                    .clone()
                                    .unwrap_or_else(|| "medium".into()),
                                choices: model
                                    .efforts
                                    .iter()
                                    .map(|e| (e.clone(), e.clone()))
                                    .collect(),
                            });
                    }
                    if !model.tiers.is_empty()
                        && !this
                            .model_controls
                            .iter()
                            .any(|c| c.id == "serviceTier" || c.id == "service_tier")
                    {
                        let mut choices = vec![("default".into(), "Standard".into())];
                        choices.extend(model.tiers.clone());
                        this.model_controls
                            .push(vorkspace_core::catalog::ModelControl {
                                id: "serviceTier".into(),
                                name: "Speed".into(),
                                current: "default".into(),
                                choices,
                            });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn choose_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let kind = Kind::from_id(&editor.draft.harness);
        let profile = crate::accounts::get(cx)
            .map(|a| a.read(cx).selected(kind, &editor.draft.cwd))
            .unwrap_or_default();
        let picker = cx.new(|cx| {
            ModelList::new_for(
                kind,
                vorkspace_core::store::native_model(&editor.draft.model, &editor.draft.harness),
                &editor.draft.cwd,
                profile,
                self.database
                    .as_ref()
                    .and_then(|db| crate::pickers::HostModels::for_project(db, &editor.draft.cwd)),
                cx,
            )
        });
        self.picker_subscription = Some(cx.subscribe_in(
            &picker,
            window,
            |this, _, picked: &ModelPicked, window, cx| {
                if let Some(editor) = &mut this.editor {
                    let current = vorkspace_core::store::native_model(
                        &editor.draft.model,
                        &editor.draft.harness,
                    );
                    let picked_model = picked.1.as_deref().and_then(|model| {
                        vorkspace_core::store::native_model(model, picked.0.id())
                    });
                    if Kind::from_id(&editor.draft.harness) != picked.0 || current != picked_model {
                        editor.draft.harness = picked.0.id().into();
                        editor.draft.model = picked.1.clone().unwrap_or_default();
                        editor.completion.update(cx, |completion, cx| {
                            completion.set_provider(picked.0.id(), cx)
                        });
                        editor.mark_dirty();
                    }
                }
                this.models = None;
                this.load_controls(cx);
                window.focus(&this.focus, cx);
                cx.notify();
            },
        ));
        window.focus(&picker.read(cx).search_focus(cx), cx);
        self.models = Some(picker);
        self.choice = None;
        self.trigger_menu = None;
        cx.notify();
    }
    fn choose_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let picker = cx.new(|cx| ProjectPicker::new(&self.projects, &editor.draft.cwd, cx));
        self.picker_subscription = Some(cx.subscribe_in(
            &picker,
            window,
            |this, _, picked: &ProjectPicked, window, cx| {
                this.project_picker = None;
                match picked {
                    ProjectPicked::Project(path) => {
                        if let Some(editor) = &mut this.editor {
                            editor.draft.cwd = path.clone();
                            editor.draft.session_folder_id.clear();
                            editor
                                .completion
                                .update(cx, |completion, cx| completion.set_cwd(path, cx));
                            editor.mark_dirty();
                        }
                        this.load_controls(cx);
                    }
                    ProjectPicked::New => this.browse_project(cx),
                    ProjectPicked::Remote => {}
                }
                window.focus(&this.focus, cx);
                cx.notify();
            },
        ));
        window.focus(&picker.read(cx).search_focus(cx), cx);
        self.project_picker = Some(picker);
        self.choice = None;
        self.trigger_menu = None;
        cx.notify();
    }
    fn browse_project(&mut self, cx: &mut Context<Self>) {
        let Some(identity) = self.editor.as_ref().map(|editor| editor.name.entity_id()) else {
            return;
        };
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = prompt.await;
            let _ = this.update(cx, |this, cx| {
                let Some(editor) = this
                    .editor
                    .as_mut()
                    .filter(|editor| editor.name.entity_id() == identity)
                else {
                    return;
                };
                if let Ok(Ok(Some(paths))) = result
                    && let Some(path) = paths.first()
                {
                    let path = path.to_string_lossy().to_string();
                    if !this.projects.contains(&path) {
                        this.projects.push(path.clone());
                    }
                    editor.draft.cwd = path;
                    editor.completion.update(cx, |completion, cx| {
                        completion.set_cwd(&editor.draft.cwd, cx)
                    });
                    editor.draft.session_folder_id.clear();
                    editor.mark_dirty();
                    this.load_controls(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.into())
}
fn detail_frame(narrow: bool) -> gpui::Div {
    div()
        .w_full()
        .max_w(px(page::DETAIL_MAX))
        .mx_auto()
        .p(page::detail_padding(narrow))
}
impl Automations {
    fn render_list(&self, narrow: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.filter.read(cx).content.to_lowercase();
        let query = query.trim();
        let mut visible = self
            .automations
            .iter()
            .filter(|a| {
                query.is_empty()
                    || format!("{} {} {}", a.name, a.prompt, a.cwd)
                        .to_lowercase()
                        .contains(query)
            })
            .peekable();
        let mut list = div()
            .id("automations-list")
            .role(Role::ListBox)
            .aria_label("Automations")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(4.));
        if visible.peek().is_none() {
            list = list.child(if self.loading {
                page::meta()
                    .px(px(8.))
                    .py(px(4.))
                    .child("Loading automations…")
            } else if query.is_empty() {
                page::empty(
                    icon("zap", 20.),
                    "No automations yet",
                    // The gallery beside it already invites a first pick.
                    if self.gallery {
                        "Automations you create are listed here."
                    } else {
                        "The one you are drafting appears here once it is created."
                    },
                )
                .h(px(240.))
            } else {
                page::empty(icon("search", 20.), "No matching automations", "").h(px(200.))
            });
        }
        for automation in visible {
            let id = automation.id.clone();
            let toggle_id = id.clone();
            let enabled = automation.enabled;
            let revision = automation.updated_at;
            let active = !self.gallery && self.selected.as_deref() == Some(&id);
            let kind = Kind::from_id(&automation.harness);
            let model = vorkspace_core::store::native_model(&automation.model, &automation.harness);
            list = list.child(
                div()
                    .id(SharedString::from(format!("automation-{id}")))
                    .relative()
                    .child(
                        page::row(
                            SharedString::from(format!("open-{id}")),
                            format!("Open {}", automation.name),
                            active,
                        )
                        .role(Role::ListBoxOption)
                        .aria_selected(active)
                        .when(active, |d| {
                            d.debug_selector(|| "automation-selected-row".into())
                        })
                        .flex()
                        .flex_col()
                        .child(
                            page::meta()
                                .pr(px(36.))
                                .truncate()
                                .child(automation.trigger_label()),
                        )
                        .child(
                            page::label()
                                .truncate()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(automation.name.clone()),
                        )
                        .child(
                            page::meta()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(crate::project_mark::project_icon(
                                    &automation.cwd,
                                    page::ICON_SIZE,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .child(project_name(&automation.cwd)),
                                )
                                .child(provider_icon(kind.id(), page::ICON_SIZE))
                                .child(
                                    div()
                                        .max_w(px(100.))
                                        .truncate()
                                        .child(cached_model_name(kind, model.as_deref())),
                                ),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.open_selection(id.clone(), window, cx),
                        )),
                    )
                    .child(
                        div().absolute().right(px(10.)).top(px(8.)).child(
                            editor::toggle(
                                SharedString::from(format!("toggle-{toggle_id}")),
                                enabled,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.toggle(toggle_id.clone(), revision, !enabled, cx)
                                },
                            )),
                        ),
                    ),
            );
        }
        div()
            .id("automation-sidebar")
            .debug_selector(|| "automation-sidebar".into())
            .flex_shrink_0()
            .when(narrow, |d| d.flex_1().min_w_0())
            .when(!narrow, |d| {
                d.w(px(page::LIST_WIDTH))
                    .min_w(px(page::LIST_MIN))
                    .max_w(px(page::LIST_MAX))
            })
            .flex()
            .flex_col()
            .child(
                page::toolbar()
                    .child(
                        page::search_frame(&self.filter.focus_handle(cx))
                            .flex_1()
                            .child(icon("search", page::ICON_SIZE))
                            .child(div().flex_1().min_w_0().child(self.filter.clone())),
                    )
                    .child(
                        page::icon_action("new-automation", "New automation", true)
                            .debug_selector(|| "automation-new".into())
                            .child(icon("plus", page::ICON_SIZE))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_gallery(cx);
                                if this.is_narrow(window) {
                                    window.focus(&this.focus, cx);
                                }
                            })),
                    )
                    .when(
                        narrow && self.selected.is_none() && self.editor.is_some(),
                        |d| {
                            d.child(
                                page::action("resume-automation-draft", "Resume draft", true)
                                    .debug_selector(|| "automation-resume-draft".into())
                                    .child("Resume draft")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.narrow_detail = true;
                                        window.focus(&this.focus, cx);
                                        cx.notify();
                                    })),
                            )
                        },
                    ),
            )
            .child(list)
    }
}

impl Render for Automations {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = self.is_narrow(window);
        let mut root = div()
            .id("automations-view")
            .role(Role::Region)
            .aria_label("Automations")
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_action(
                cx.listener(|this, _: &vorkspace_ui::actions::DismissMenu, window, cx| {
                    if this.confirm_delete {
                        this.confirm_delete = false;
                    } else if this.choice.is_some()
                        || this.trigger_menu.is_some()
                        || this.models.is_some()
                        || this.project_picker.is_some()
                    {
                        this.choice = None;
                        this.trigger_menu = None;
                        this.models = None;
                        this.project_picker = None;
                        window.focus(&this.focus, cx);
                    } else if this.is_narrow(window) && this.narrow_detail {
                        this.back_to_list(window, cx);
                    } else if !this.is_narrow(window) && this.editor.is_some() && !this.gallery {
                        this.show_gallery(cx);
                    } else {
                        cx.emit(Event::Close);
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!narrow || !self.narrow_detail, |d| {
                        d.child(self.render_list(narrow, cx))
                    })
                    .when(!narrow || self.narrow_detail, |d| {
                        d.child(
                            page::detail_pane(!narrow)
                                .when(narrow, |d| {
                                    d.child(
                                        page::toolbar().child(
                                            page::action(
                                                "automation-back",
                                                "Back to automations",
                                                true,
                                            )
                                            .debug_selector(|| "automation-back".into())
                                            .child(icon("chevron-left", page::ICON_SIZE))
                                            .child("Automations")
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.back_to_list(window, cx)
                                                }),
                                            ),
                                        ),
                                    )
                                })
                                .child(if self.gallery || self.editor.is_none() {
                                    self.render_gallery(narrow, cx)
                                } else {
                                    self.render_editor(narrow, cx)
                                }),
                        )
                    }),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    page::meta()
                        .id("automation-error")
                        .role(Role::Alert)
                        .m(px(16.))
                        .p(px(12.))
                        .rounded(px(radius::SURFACE))
                        .bg(status(StatusColor::Danger, 0.08))
                        .text_color(status(StatusColor::Danger, 1.))
                        .child(error),
                )
            });
        if let Some(menu) = &self.trigger_menu {
            root = root.child(
                menu.read(cx)
                    .popover(Anchor::Slot(self.trigger_anchor.clone()), cx),
            );
        }
        if let Some(menu) = &self.choice {
            let weak = cx.weak_entity();
            let field = menu.field.clone();
            root = root.child(
                popover("automation-choice", Anchor::Slot(menu.anchor.clone()))
                    .width(250.)
                    .on_dismiss(move |_, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.choice = None;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id("automation-choices")
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .p(px(4.))
                            .children(menu.options.iter().enumerate().map(
                                |(index, (value, label))| {
                                    let field = field.clone();
                                    let value = value.clone();
                                    page::row(("automation-choice", index), label.clone(), false)
                                        .role(Role::MenuItem)
                                        .child(page::label().child(label.clone()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.apply_choice(field.clone(), value.clone(), cx)
                                        }))
                                },
                            )),
                    ),
            );
        }
        if let Some(picker) = &self.models {
            let weak = cx.weak_entity();
            root = root.child(
                popover(
                    "automation-model-picker",
                    Anchor::Slot(self.model_anchor.clone()),
                )
                .width(340.)
                .on_dismiss(move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.models = None;
                        cx.notify();
                    });
                })
                .child(self.render_model_settings(cx))
                .child(div().h(px(440.)).child(picker.clone())),
            );
        }
        if let Some(picker) = &self.project_picker {
            let weak = cx.weak_entity();
            root = root.child(
                popover(
                    "automation-project-picker",
                    Anchor::Slot(self.project_anchor.clone()),
                )
                .width(320.)
                .on_dismiss(move |_, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.project_picker = None;
                        cx.notify();
                    });
                })
                .child(picker.clone()),
            );
        }
        root
    }
}
