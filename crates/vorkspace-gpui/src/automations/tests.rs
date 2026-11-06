use super::*;
use gpui::{Modifiers, TestAppContext, VisualTestContext, point, size};

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let position = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_move(position, None, Modifiers::none());
    visual.simulate_click(position, Modifiers::none());
    visual.run_until_parked();
    visual.refresh().unwrap();
}

#[gpui::test]
fn narrow_navigation_retains_filter_and_editor(cx: &mut TestAppContext) {
    let mut service = Service::open(std::path::Path::new(":memory:")).unwrap();
    let mut draft = AutomationUpsert::new("/tmp".into(), "codex".into(), String::new());
    draft.name = "Weekly review".into();
    draft.prompt = "Summarize recent changes.".into();
    draft.enabled = false;
    draft.triggers = Some(vec![AutomationTrigger::new("time", "weekly")]);
    service.upsert(draft).unwrap();

    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let view = Automations::new(
                    Arc::new(Mutex::new(service)),
                    "/tmp".into(),
                    vec!["/tmp".into()],
                    cx,
                );
                view.filter
                    .update(cx, |input, cx| input.set_text("review".into(), cx));
                view
            })
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let filter = window
        .update(visual, |view, _, _| {
            assert_eq!(view.automations.len(), 1);
            view.filter.clone()
        })
        .unwrap();
    for light in [false, true] {
        visual.update(|_, _| {
            configure(
                &vorkspace_ui::preferences::Preferences {
                    theme: if light { "light" } else { "dark" }.into(),
                    ..Default::default()
                },
                light,
                true,
            );
        });
        for (width, narrow) in [(1100., false), (480., true), (1100., false)] {
            visual.simulate_resize(size(px(width), px(720.)));
            visual.run_until_parked();
            visual.refresh().unwrap();
            let sidebar = visual.debug_bounds("automation-sidebar").unwrap();
            if narrow {
                assert_eq!(sidebar.size.width, px(width));
                assert!(visual.debug_bounds("automation-scratch").is_none());
                click(visual, "automation-new");
                assert!(visual.debug_bounds("automation-sidebar").is_none());
            }
            let first = visual.debug_bounds("automation-scratch").unwrap();
            let second = visual
                .debug_bounds("automation-template-find-critical-bugs")
                .unwrap();
            if narrow {
                assert!(first.top() > visual.debug_bounds("automation-back").unwrap().bottom());
                assert_eq!(first.left(), second.left());
                assert!(second.top() > first.bottom());
                click(visual, "automation-back");
                assert_eq!(
                    visual
                        .debug_bounds("automation-sidebar")
                        .unwrap()
                        .size
                        .width,
                    px(width),
                );
            } else {
                assert_eq!(sidebar.size.width, px(page::LIST_WIDTH));
                assert!(first.left() > sidebar.right());
                assert_eq!(first.top(), second.top());
                assert!(second.left() > first.right());
            }
            window
                .update(visual, |view, _, cx| {
                    assert_eq!(view.filter.entity_id(), filter.entity_id());
                    assert_eq!(view.filter.read(cx).content, "review");
                })
                .unwrap();
        }
    }
    let name = window
        .update(visual, |view, _, cx| {
            let draft = AutomationUpsert::from_automation(&view.automations[0]);
            view.selected = Some(draft.id.clone());
            view.narrow_detail = true;
            view.install_editor(draft, true, cx);
            let editor = view.editor.as_mut().unwrap();
            editor.dirty = true;
            editor.name.update(cx, |input, cx| {
                input.set_text("Unsaved title".into(), cx);
            });
            cx.notify();
            editor.name.clone()
        })
        .unwrap();
    visual.simulate_resize(size(px(480.), px(440.)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("automation-sidebar").is_none());
    let settings_offset = point(px(0.), px(-32.));
    window
        .update(visual, |view, _, cx| {
            assert!(view.settings_scroll.max_offset().y >= px(32.));
            view.settings_scroll.set_offset(settings_offset);
            view.history = true;
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    click(visual, "automation-back");
    click(visual, "automation-selected-row");
    assert!(visual.debug_bounds("automation-sidebar").is_none());
    window
        .update(visual, |view, _, cx| {
            let editor = view.editor.as_ref().unwrap();
            assert_eq!(editor.name.entity_id(), name.entity_id());
            assert_eq!(editor.name.read(cx).content, "Unsaved title");
            assert!(editor.dirty);
            assert!(view.history);
            assert_eq!(view.filter.read(cx).content, "review");
            assert_eq!(view.settings_scroll.offset(), settings_offset);
            view.history = false;
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    window
        .update(visual, |view, _, cx| {
            assert_eq!(view.settings_scroll.offset(), settings_offset);
            view.selected = None;
            view.editor.as_mut().unwrap().saved = false;
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    click(visual, "automation-back");
    click(visual, "automation-resume-draft");
    window
        .update(visual, |view, _, cx| {
            let editor = view.editor.as_ref().unwrap();
            assert_eq!(editor.name.entity_id(), name.entity_id());
            assert_eq!(editor.name.read(cx).content, "Unsaved title");
            assert_eq!(view.settings_scroll.offset(), settings_offset);
        })
        .unwrap();
    visual.update(|_, _| configure(&Default::default(), false, true));
}

#[gpui::test]
fn automation_gallery_uses_narrow_tile_width_in_wide_window(cx: &mut TestAppContext) {
    let service = Service::open(std::path::Path::new(":memory:")).unwrap();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Automations::new(Arc::new(Mutex::new(service)), "/tmp".into(), vec![], cx))
        })
        .unwrap()
    });
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.simulate_resize(size(px(1100.), px(700.)));
    window
        .update(visual, |view, _, cx| view.set_pane_width(420., cx))
        .unwrap();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("automation-sidebar").is_some());
    click(visual, "automation-new");
    assert!(visual.debug_bounds("automation-sidebar").is_none());
    assert!(visual.debug_bounds("automation-back").is_some());

    window
        .update(visual, |view, _, cx| view.set_pane_width(1000., cx))
        .unwrap();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("automation-sidebar").is_some());
    assert!(visual.debug_bounds("automation-back").is_none());
}
