use super::*;
use gpui::TestAppContext;

#[gpui::test]
fn close_others_confirmation_preserves_terminals_admitted_after_the_prompt(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = super::super::tests::window(cx);
    let (kept, old) = window
        .update(cx, |shell, w, cx| {
            shell.new_terminal(w, cx);
            shell.new_terminal(w, cx);
            let live = shell.live_terminals();
            let kept = live[0].entity_id();
            let old = live[1].downgrade();
            live[1].update(cx, |terminal, _| terminal.foreground = Some("job".into()));
            shell.run_terminal_tab_menu(kept, "close-others", w, cx);
            (kept, old)
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());

    let successor = window
        .update(cx, |shell, w, cx| {
            shell.new_terminal(w, cx);
            shell.live_terminals().last().unwrap().entity_id()
        })
        .unwrap();
    cx.simulate_prompt_answer("Close");
    cx.run_until_parked();

    assert!(old.upgrade().is_none());
    window
        .update(cx, |shell, w, cx| {
            assert_eq!(
                shell
                    .live_terminals()
                    .iter()
                    .map(Entity::entity_id)
                    .collect::<Vec<_>>(),
                [kept, successor]
            );
            let dock = &shell.workspace.docks[&shell.workspace.project];
            assert_eq!((dock.terminals.len(), dock.active), (2, 0));
            shell.run_terminal_tab_menu(kept, "close", w, cx);
            shell.run_terminal_tab_menu(kept, "close", w, cx);
            assert_eq!(shell.live_terminals()[0].entity_id(), successor);
        })
        .unwrap();
}

#[gpui::test]
fn dock_drop_discards_positions_from_a_changed_tab_order(cx: &mut TestAppContext) {
    let (window, _, _) = super::super::tests::window(cx);
    window
        .update(cx, |shell, w, cx| {
            shell.new_terminal(w, cx);
            shell.new_terminal(w, cx);
            let original = shell.live_terminals();
            let order = original.iter().map(Entity::entity_id).collect();
            shell.new_terminal(w, cx);
            let live = shell.live_terminals();
            shell.dock_drag = Some(DockTabDrag {
                from: 0,
                to: 1,
                origin: px(0.),
                offset: px(20.),
                active: true,
                bounds: vec![Bounds::default(); 2],
                order,
            });
            shell.finish_dock_drag(cx);
            assert_eq!(shell.live_terminals(), live);
            assert!(shell.dock_drag.is_none());
        })
        .unwrap();
}
