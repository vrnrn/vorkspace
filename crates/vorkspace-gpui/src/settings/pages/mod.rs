//! One file per Settings page, each a port of its SettingsView.tsx function.
mod appearance;
mod archive;
mod bots;
mod chat;
mod clipboard;
mod connections;
pub(super) mod connectors;
mod general;
mod inbox;
mod keybindings;
mod mcp;
mod providers;
mod skills;
mod vox;
mod worktrees;

pub(super) use appearance::parse_hex;
pub(super) use connections::ConnectionsState;
pub(super) use inbox::InboxState;
pub(super) use keybindings::command_shortcut;

use super::{Section, SettingsView};
use gpui::{AnyElement, Context, Window};

impl SettingsView {
    pub(super) fn page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match self.section {
            Section::General => self.general_page(cx),
            Section::Vox => self.vox_page(cx),
            Section::Connections => self.connections_page(cx),
            Section::Appearance => self.appearance_page(window, cx),
            Section::Keybindings => self.keybindings_page(window, cx),
            Section::Chat => self.chat_page(cx),
            Section::Providers => self.providers_page(cx),
            Section::Mcp => self.mcp_page(cx),
            Section::Skills => self.skills_page(cx),
            Section::Bots => self.bots_page(cx),
            Section::Connectors => self.connectors_page(cx),
            Section::Github => self.connector_page(crate::workspace::Connector::Github, cx),
            Section::Gitlab => self.connector_page(crate::workspace::Connector::Gitlab, cx),
            Section::Jira => self.connector_page(crate::workspace::Connector::Jira, cx),
            Section::Notes => self.notes_page(cx),
            Section::Todos => self.local_feature_page(Section::Todos, cx),
            Section::Archive => self.archive_page(cx),
            Section::Worktrees => self.worktrees_page(cx),
        }
    }
}
