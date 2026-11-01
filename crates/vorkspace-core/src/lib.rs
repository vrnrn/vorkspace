//! UI-independent services for the native desktop app.
pub mod accounts;
pub mod agent;
pub mod automations;
pub mod bot;
pub mod bot_model;
pub mod catalog;
pub mod checkpoint;
pub mod context;
pub mod desktop_preferences;
pub mod diff;
pub mod external_editor;
pub mod files;
pub mod follow_up;
pub mod git;
pub mod harness_updates;
pub mod inbox;
pub(crate) mod js_text;
pub mod kind;
pub mod limit;
pub(crate) mod model_cache;
pub mod notes;
pub mod process;
pub mod project_appearance;
pub mod provider;
pub mod remote;
pub mod remote_path;
pub mod remote_workspace;
pub mod resources;
pub mod response;
pub mod scm;
pub mod session;
pub mod session_filters;
pub mod session_groups;
pub mod session_title;
pub(crate) mod shell_env;
pub mod store;
pub(crate) mod ticket_admission;
pub mod ticket_context;
pub mod time;
pub mod todos;
pub mod tool;
pub mod transcript;
pub mod usage;
pub mod visual;
pub mod visual_bridge;

pub mod composer;
pub mod orchestration;
pub mod skills;

pub mod search;

pub mod cli;
pub mod mcp;

pub mod worktree_management;

pub mod voice;

mod compatibility;

mod local_http;

pub mod release_notes;
