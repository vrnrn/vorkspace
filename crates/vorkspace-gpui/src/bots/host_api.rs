//! Everything `Bots` needs from the workspace that hosts it. The shell implements
//! this, so Bots never depends on the shell itself.
use crate::conversation::Conversation;
use gpui::{App, Entity, SharedString, Window};
use serde_json::Value;
use std::collections::HashMap;
use vorkspace_core::{
    agent::Settings, project_appearance::ProjectAppearance, session_groups::Groups, store::Session,
};
use vorkspace_model::pane_layout::Edge;

pub(crate) struct BotSession {
    pub id: i64,
    pub title: String,
    pub cwd: String,
    pub busy: bool,
    pub has_messages: bool,
    pub archived: bool,
    pub updated_at: u64,
    pub needs_input: bool,
    pub branch: Option<String>,
    pub model: SharedString,
}

pub(crate) struct AppStart {
    pub summary: Session,
    pub settings: Settings,
    pub prompt: String,
    pub draft: bool,
    pub reveal: bool,
    pub placement: Option<(i64, Edge)>,
    pub cleanup: Option<vorkspace_core::bot::launch::CreatedWorktree>,
}

pub(crate) trait BotsHost {
    fn terminals_using(&self, path: &str, cx: &App) -> bool;
    fn session_views(&self, cx: &App) -> Vec<BotSession>;
    fn session_entity(&self, id: i64, cx: &App) -> Option<Entity<Conversation>>;
    fn ensure_session(&self, id: i64, cx: &mut App) -> Option<Entity<Conversation>>;
    fn groups(&self, project: &str, cx: &App) -> Groups;
    fn apply_groups(&self, project: String, groups: Groups, cx: &mut App);
    fn placement(
        &self,
        caller: i64,
        project: &str,
        input: &Value,
        cx: &App,
    ) -> Result<Option<(i64, Edge)>, String>;
    fn install_start(
        &self,
        caller: i64,
        start: AppStart,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Result<Result<i64, String>>;
    fn session_action(
        &self,
        caller: i64,
        id: i64,
        project: &str,
        action: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Result<Result<Value, String>>;
    fn open_session(&self, id: i64, window: &mut Window, cx: &mut App);
    fn open_channels(&self, window: &mut Window, cx: &mut App);
    fn project_appearance(&self, project: &str, cx: &App) -> Option<ProjectAppearance>;
    fn projects(&self, cx: &App) -> (Vec<String>, HashMap<String, ProjectAppearance>);
}
