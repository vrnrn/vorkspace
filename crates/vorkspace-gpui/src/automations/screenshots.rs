use super::*;
use gpui::{HeadlessAppContext, size};
use std::path::Path;
use vorkspace_ui::{clock, fonts, preferences::Preferences};

struct Surface(Entity<Automations>);

impl Render for Surface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(pane_fill())
            .font_family(fonts::UI)
            .child(self.0.clone())
    }
}

#[test]
#[ignore = "Requires Metal and VORKSPACE_PAGES_SCREEN_DIR"]
fn native_automations_capture() {
    let output = std::path::PathBuf::from(std::env::var_os("VORKSPACE_PAGES_SCREEN_DIR").unwrap());
    assert!(output.is_absolute() && output.is_dir());
    let mut service = Service::open(Path::new(":memory:")).unwrap();
    let samples = [
        (
            "Review release changes",
            "/tmp/platform",
            "time",
            "weekdays",
        ),
        (
            "Triage new issues",
            "/tmp/customer-portal",
            "github",
            "issue_opened",
        ),
    ];
    for (name, project, kind, event) in samples {
        let mut draft = AutomationUpsert::new(project.into(), "codex".into(), String::new());
        draft.name = name.into();
        draft.prompt = "Summarize the changes and call out anything that needs attention.".into();
        draft.enabled = false;
        draft.workspace_mode = "current".into();
        let mut triggers = vec![AutomationTrigger::new(kind, event)];
        if kind == "time" {
            triggers.push(AutomationTrigger::new("github", "pull_request_opened"));
        }
        draft.triggers = Some(triggers);
        service.upsert(draft).unwrap();
    }
    let automations = service.list().unwrap();
    let service = Arc::new(Mutex::new(service));
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(gpui_apple::AppleTextSystem::new()),
        Arc::new(crate::assets::Assets),
        || {
            Ok(Some(Box::new(
                gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
            )))
        },
    );
    clock::use_test_clock(&cx);
    for theme in ["dark", "light"] {
        cx.update(|_| {
            configure(
                &Preferences {
                    theme: theme.into(),
                    ..Default::default()
                },
                theme == "light",
                true,
            );
        });
        for (width, suffix, detail) in [
            (1200., "", false),
            (540., "", true),
            (540., "-list", false),
            (1200., "-editor", true),
            (540., "-editor", true),
        ] {
            let window = cx
                .open_window(size(px(width), px(720.)), |_, cx| {
                    let page = cx.new(|cx| {
                        let mut page = Automations::new(
                            service.clone(),
                            "/tmp/platform".into(),
                            vec!["/tmp/platform".into(), "/tmp/customer-portal".into()],
                            cx,
                        );
                        page.automations = automations.clone();
                        page.loading = false;
                        page.narrow_detail = detail;
                        if suffix == "-editor" {
                            let saved = automations
                                .iter()
                                .find(|automation| automation.name == "Review release changes")
                                .unwrap();
                            page.selected = Some(saved.id.clone());
                            page.install_editor(AutomationUpsert::from_automation(saved), true, cx);
                            page.advanced = true;
                        }
                        page
                    });
                    cx.new(|_| Surface(page))
                })
                .unwrap();
            clock::settle_headless_animations(window, &mut cx);
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(output.join(format!("{theme}-automations-{width}{suffix}.png")))
                .unwrap();
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
            cx.run_until_parked();
        }
    }
    cx.update(|_| configure(&Preferences::default(), false, true));
}
