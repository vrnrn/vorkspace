//! Corner scale for app chrome, in logical points (before UI zoom).
//!
//! Pick by size and role, not by screen. Use the same value for a surface's
//! fill, border, clipping and hover state. Inset shapes subtract their distance
//! from the outer edge, including the border, to keep the corners concentric.
//! AppKit owns the window silhouette; circles, capsules and artwork retain
//! their own geometry. See DESIGN.md for the complete surface conventions.

/// Small chips, keyboard hints and compact icon actions.
pub const SMALL: f32 = 4.;
/// Buttons, fields, tabs and navigation highlights.
pub const CONTROL: f32 = 6.;
/// Composers, code blocks, image previews, menu rows and other inset content.
pub const CONTAINER: f32 = 8.;
/// Panes, cards, settings groups, menus and notifications.
pub const SURFACE: f32 = 12.;
/// Dialogs and large floating panels, including the Bot island.
pub const MODAL: f32 = 16.;
