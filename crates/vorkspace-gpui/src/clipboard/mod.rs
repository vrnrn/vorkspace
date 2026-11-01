//! A bounded, process-local clipboard owner. No database, files, or rich formats.
mod history;
#[cfg(not(test))]
mod menu;
mod native;
#[cfg(not(test))]
use gpui::Task;
use gpui::{App, AppContext, Context, Entity, Global};
use history::{Content, History, Preview};
use std::sync::Arc;
gpui::actions!(clipboard, [Toggle, Settings, Composer]);

struct Clipboard(Entity<Service>);
impl Global for Clipboard {}
#[derive(Default)]
pub(crate) struct Service {
    history: History,
    #[cfg(not(test))]
    query: String,
    #[cfg(not(test))]
    menu: Option<(menu::Menu, Task<()>)>,
    enabled: bool,
    epoch: u64,
    pub error: Option<String>,
    last: isize,
    #[cfg(not(test))]
    poll: Option<Task<()>>,
    #[cfg(not(test))]
    hotkey: Option<(vorkspace_macos::global_hotkey::Registration, Task<()>)>,
}
pub(crate) fn get(cx: &App) -> Option<Entity<Service>> {
    cx.try_global::<Clipboard>().map(|c| c.0.clone())
}
pub(crate) fn configure(enabled: bool, cx: &mut App) {
    let service = get(cx).unwrap_or_else(|| {
        let service = cx.new(|_| Service::default());
        cx.set_global(Clipboard(service.clone()));
        service
    });
    service.update(cx, |s, cx| {
        if s.enabled == enabled {
            return;
        }
        s.enabled = enabled;
        s.clear(cx);
        s.error = None;
        #[cfg(not(test))]
        {
            if !enabled && let Some((menu, _)) = &s.menu {
                menu.hide();
            }
            s.poll = None;
            s.hotkey = None;
            s.last = native::revision();
            if enabled {
                match vorkspace_macos::global_hotkey::Registration::new("cmd-shift-c") {
                    Ok((registration, rx)) => {
                        let task = cx.spawn(async move |_, cx| {
                            while rx.recv().await.is_ok() {
                                cx.update(|cx| cx.dispatch_action(&Toggle));
                            }
                        });
                        s.hotkey = Some((registration, task));
                    }
                    Err(error) => s.error = Some(format!("⌘⇧C: {error}")),
                }
                s.poll = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(500))
                            .await;
                        let Ok(Some((last, epoch))) = this.update(cx, |s, _| {
                            (native::revision() != s.last).then_some((s.last, s.epoch))
                        }) else {
                            continue;
                        };
                        // Pasteboard providers can be slow. Payload reads,
                        // normalization and hashing also stay off the UI thread.
                        let (revision, prepared) = cx
                            .background_executor()
                            .spawn(async move {
                                let mut revision = last;
                                let prepared = native::read(&mut revision).map(|content| {
                                    let hash = content.fingerprint();
                                    (content, hash)
                                });
                                (revision, prepared)
                            })
                            .await;
                        let work =
                            this.update(cx, |s, cx| s.admit(last, epoch, revision, prepared, cx));
                        let Ok(Some((id, content, epoch))) = work else {
                            continue;
                        };
                        // One in-flight native decode, with no queued payloads.
                        // Clearing/disabling invalidates publication, even if
                        // ImageIO finishes after the worker has been cancelled.
                        let preview = cx
                            .background_executor()
                            .spawn(async move {
                                if let Content::Image { bytes, .. } = &*content {
                                    native::thumbnail(bytes)
                                } else {
                                    None
                                }
                            })
                            .await;
                        let _ = this.update(cx, |s, cx| s.publish(id, epoch, preview, cx));
                    }
                }));
            }
        }
        cx.notify();
    });
}
pub(crate) fn clear(cx: &mut App) {
    if let Some(s) = get(cx) {
        s.update(cx, |s, cx| s.clear(cx));
    }
}
impl Service {
    fn sync_menu(&self) {
        #[cfg(not(test))]
        if let Some((menu, _)) = &self.menu
            && menu.visible()
        {
            menu.sync(self);
        }
    }
    #[cfg(not(test))]
    pub(crate) fn toggle_menu(
        &mut self,
        anchor: objc2_foundation::NSRect,
        status_click: bool,
        cx: &mut Context<Self>,
    ) {
        if self.menu.is_none() {
            let (menu, rx) = menu::Menu::new();
            let events = cx.spawn(async move |this, cx| {
                while let Ok(event) = rx.recv().await {
                    let _ = this.update(cx, |s, cx| match event {
                        menu::Event::Copy(id) => {
                            s.copy(id, cx);
                        }
                        menu::Event::Clear => s.clear(cx),
                        menu::Event::Search(query) => {
                            s.query = query.to_lowercase();
                            s.sync_menu();
                        }
                        menu::Event::Settings => cx.dispatch_action(&Settings),
                        menu::Event::Composer => cx.dispatch_action(&Composer),
                        menu::Event::Quit => crate::app_menu::clipboard_route(
                            vorkspace_macos::status_menu::Action::Quit,
                            cx,
                        ),
                        menu::Event::Closed => s.query.clear(),
                    });
                }
            });
            self.menu = Some((menu, events));
        }
        let menu = &self.menu.as_ref().unwrap().0;
        // Reject a completed dismissal before rebuilding native presentation copies.
        if !menu.accept_toggle(status_click) {
            return;
        }
        if !menu.visible() {
            self.query.clear();
            menu.sync(self);
        }
        menu.toggle(anchor, status_click);
    }

    fn admit(
        &mut self,
        last: isize,
        epoch: u64,
        revision: isize,
        prepared: Option<(Content, u64)>,
        cx: &mut Context<Self>,
    ) -> Option<(u64, Arc<Content>, u64)> {
        if !self.enabled || self.epoch != epoch || self.last != last {
            return None;
        }
        self.last = revision;
        let (content, hash) = prepared?;
        let id = self.history.insert_hashed(content, hash);
        self.sync_menu();
        cx.notify();
        let item = self.history.get(id?)?;
        (item.preview.is_none() && matches!(&*item.content, Content::Image { .. }))
            .then(|| (item.id, item.content.clone(), self.epoch))
    }
    fn clear(&mut self, cx: &mut Context<Self>) {
        self.epoch += 1;
        #[cfg(not(test))]
        {
            self.last = native::revision();
        }
        self.history.clear();
        self.sync_menu();
        cx.notify();
    }
    fn publish(
        &mut self,
        id: u64,
        epoch: u64,
        preview: Option<Arc<Preview>>,
        cx: &mut Context<Self>,
    ) {
        if self.enabled
            && self.epoch == epoch
            && let Some(item) = self.history.items.iter_mut().find(|i| i.id == id)
        {
            item.preview = preview;
            self.sync_menu();
            cx.notify();
        }
    }
    fn copy(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(item) = self.history.get(id) else {
            return false;
        };
        #[cfg(not(test))]
        let success = native::write(&item.content)
            .map(|revision| self.last = revision)
            .is_some();
        #[cfg(test)]
        let success = match &*item.content {
            Content::Text { value, .. } => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(value.clone()));
                true
            }
            Content::Image { .. } => false,
        };
        if !success {
            self.error = Some("Could not restore this clipboard item.".into());
            cx.notify();
        }
        success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    #[gpui::test]
    fn search_and_copy_restore_the_original_without_modifying_history(cx: &mut TestAppContext) {
        cx.update(|cx| configure(true, cx));
        let service = cx.update(|cx| get(cx).unwrap());
        service.update(cx, |s, cx| {
            let id = s
                .history
                .insert(Content::text("Alpha NEEDLE\noriginal".into()).unwrap())
                .unwrap();
            s.history
                .insert(Content::text("Beta needle".into()).unwrap());
            assert_eq!(s.history.matching(&"NeEdLe".to_lowercase()).len(), 2);
            assert!(s.copy(id, cx));
            assert_eq!(s.history.items.len(), 2);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "Alpha NEEDLE\noriginal"
            );
        });
    }
    #[gpui::test]
    fn clear_and_newer_copy_supersede_inflight_reads_without_rewinding_the_counter(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| configure(true, cx));
        let service = cx.update(|cx| get(cx).unwrap());
        service.update(cx, |s, cx| {
            let epoch = s.epoch;
            s.clear(cx);
            let value = Content::text("discarded".into()).unwrap();
            let hash = value.fingerprint();
            s.admit(0, epoch, 1, Some((value, hash)), cx);
            assert!(s.history.items.is_empty());
            s.last = 5;
            let value = Content::text("superseded copy".into()).unwrap();
            let hash = value.fingerprint();
            s.admit(0, s.epoch, 2, Some((value, hash)), cx);
            assert_eq!(s.last, 5);
            assert!(s.history.items.is_empty());
            let value = Content::text("current".into()).unwrap();
            let hash = value.fingerprint();
            s.admit(5, s.epoch, 6, Some((value, hash)), cx);
            assert_eq!(s.last, 6);
            assert_eq!(s.history.items.len(), 1);
        });
    }
    #[gpui::test]
    fn clearing_disabling_and_eviction_cannot_publish_a_late_preview(cx: &mut TestAppContext) {
        cx.update(|cx| configure(true, cx));
        let service = cx.update(|cx| get(cx).unwrap());
        let image = || {
            Arc::new(Preview {
                pixels: vec![0; 64],
                width: 4,
                height: 4,
            })
        };
        for transition in 0..3 {
            cx.update(|cx| configure(true, cx));
            let (id, epoch) = service.update(cx, |s, _| {
                let id = s
                    .history
                    .insert(Content::Image {
                        kind: "public.png",
                        bytes: vec![1, 2, 3],
                    })
                    .unwrap();
                (id, s.epoch)
            });
            match transition {
                0 => cx.update(clear),
                1 => cx.update(|cx| configure(false, cx)),
                _ => service.update(cx, |s, _| {
                    for i in 0..50 {
                        s.history.insert(Content::text(format!("{i}")).unwrap());
                    }
                }),
            }
            let preview = image();
            assert_eq!(preview.pixels.len(), preview.width * preview.height * 4);
            let weak = Arc::downgrade(&preview);
            service.update(cx, |s, cx| s.publish(id, epoch, Some(preview), cx));
            assert!(weak.upgrade().is_none());
            service.read_with(cx, |s, _| assert!(s.history.get(id).is_none()));
        }
    }
}
