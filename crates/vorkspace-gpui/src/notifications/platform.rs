//! OS bridge. Call initialization and Dock updates on the UI thread. All other
//! results are channels so authorization/dispatch never block the UI thread.

use std::sync::mpsc::{self, Receiver};

use super::delivery::{Permission, Target};

pub struct Notification {
    pub tag: String,
    pub subtitle: String,
    pub body: String,
    pub sound: bool,
}

#[path = "platform_macos.rs"]
mod implementation;

pub use implementation::*;

fn resolved<T>(value: T) -> Receiver<T> {
    let (sender, receiver) = mpsc::channel();
    let _ = sender.send(value);
    receiver
}
