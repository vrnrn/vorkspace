//! One file per Settings page, each a port of its SettingsView.tsx function.
mod appearance;
mod archive;
mod bots;
mod chat;
mod connections;
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

use super::{Section, SettingsView};
use gpui::{AnyElement, Context, Window};

impl SettingsView {
    pub(super) fn page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match self.section {
            Section::General => self.general_page(cx),
            Section::Vox => self.vox_page(cx),
            Section::Connections => self.connections_page(cx),
            Section::Appearance => self.appearance_page(window, cx),
            Section::Keybindings => self.keybindings_page(cx),
            Section::Chat => self.chat_page(cx),
            Section::Providers => self.providers_page(cx),
            Section::Mcp => self.mcp_page(cx),
            Section::Skills => self.skills_page(cx),
            Section::Bots => self.bots_page(cx),
            Section::Inbox => self.inbox_page(cx),
            Section::Archive => self.archive_page(cx),
            Section::Worktrees => self.worktrees_page(cx),
        }
    }
}
