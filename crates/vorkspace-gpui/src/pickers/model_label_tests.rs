use super::*;

#[gpui::test]
fn local_inventory_hides_missing_tabs_but_preserves_current_and_successful_catalogs(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let list =
            cx.new(|cx| ModelList::new_for(Kind::Codex, None, "/project", Default::default(), cx));
        list.update(cx, |list, cx| {
            list.tab = ModelTab::Harness(Kind::Claude);
            list.apply_installation_probe(vec![Kind::Claude, Kind::Codex], cx);
            assert!(!list.provider_visible(Kind::Claude));
            assert!(list.provider_visible(Kind::Codex));
            assert!(!list.enabled(Kind::Codex));
            assert!(matches!(list.tab, ModelTab::Harness(Kind::Codex)));
            // A newer successful discovery wins over a slow missing-CLI probe.
            list.catalogs.push((Kind::Pi, Vec::new()));
            list.tab = ModelTab::Harness(Kind::Pi);
            list.apply_installation_probe(vec![Kind::Pi], cx);
            assert!(list.provider_visible(Kind::Pi));
            assert!(matches!(list.tab, ModelTab::Harness(Kind::Pi)));
        });
    });
}

#[gpui::test]
fn hidden_models_leave_the_picker_but_the_current_model_stays(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::settings::Preferences {
            hidden_models: vec![
                crate::settings::model_key("codex", "a"),
                crate::settings::model_key("codex", "b"),
                // Another provider's model with the same id is unaffected.
                crate::settings::model_key("claude", "c"),
            ],
            favorite_models: ["a", "b", "c"]
                .into_iter()
                .map(|model| crate::settings::ModelChoice {
                    provider: "codex".into(),
                    model: Some(model.into()),
                })
                .collect(),
            ..Default::default()
        });
        let list = cx.new(|cx| {
            ModelList::new_for(
                Kind::Codex,
                Some("a".into()),
                "/project",
                Default::default(),
                cx,
            )
        });
        list.update(cx, |list, cx| {
            list.catalogs.push((
                Kind::Codex,
                ["a", "b", "c"]
                    .map(|id| vorkspace_core::catalog::Model {
                        id: id.into(),
                        name: id.into(),
                        alias_target: None,
                        efforts: Vec::new(),
                        default_effort: None,
                        fast: false,
                        thinking: false,
                        default: false,
                        tiers: Vec::new(),
                        controls: Vec::new(),
                    })
                    .into(),
            ));
            let ids: Vec<_> = list
                .entries(cx)
                .into_iter()
                .filter_map(|entry| entry.id)
                .collect();
            assert!(
                ids.contains(&"a".to_string()),
                "current model stays: {ids:?}"
            );
            assert!(!ids.contains(&"b".to_string()), "{ids:?}");
            assert!(ids.contains(&"c".to_string()), "{ids:?}");
            list.tab = ModelTab::Favorites;
            let favorites: Vec<_> = list
                .entries(cx)
                .into_iter()
                .filter_map(|entry| entry.id)
                .collect();
            assert_eq!(favorites, ["a", "c"]);
        });
    });
}

#[gpui::test]
fn remote_model_availability_comes_from_the_host(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let list = cx.new(|cx| {
            ModelList::new_for(
                Kind::Codex,
                None,
                "remote://env/home/code",
                Default::default(),
                cx,
            )
            .hide_providers(vec!["claude".into()])
        });
        list.update(cx, |list, cx| {
            list.remote_providers = Some(vec![Kind::Claude]);
            list.missing = vec![Kind::Codex];
            list.tab = ModelTab::Harness(Kind::Claude);
            list.apply_installation_probe(Kind::ALL.to_vec(), cx);
            assert!(list.hidden.is_empty());
            assert!(list.provider_visible(Kind::Claude));
            assert!(list.provider_visible(Kind::Codex));
            assert!(!list.provider_visible(Kind::Pi));
            assert!(list.enabled(Kind::Claude));
            assert!(!list.enabled(Kind::Codex));
            assert!(!list.enabled(Kind::Antigravity));
            // Until its host catalog arrives, no local Claude aliases
            // are advertised as available remote models.
            let entries = list.entries(cx);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].id, None);
        });
    });
}

#[test]
fn account_metadata_never_becomes_a_model_label() {
    for id in [
        "account-de37caea9106642f26b59835a2e9aa4f",
        "fixture-account-de37caea-9106-642f-26b5-9835a2e9aa4f",
    ] {
        let label = format!("{id} model");
        assert_eq!(model_name(Kind::Codex, Some(id)).as_ref(), "Codex");
        assert_eq!(
            model_display_name(Kind::Codex, id, &label).as_ref(),
            "Codex"
        );
        assert_eq!(
            model_display_name(Kind::Codex, "gpt-6.1-sol", &label).as_ref(),
            "GPT-6.1-Sol"
        );
    }
    assert_eq!(
        model_display_name(Kind::Codex, "custom-model", "My custom model").as_ref(),
        "My custom model"
    );
    assert_eq!(
        model_name(Kind::Codex, Some("custom-model")).as_ref(),
        "custom-model"
    );
}

#[gpui::test]
fn remembered_favorites_and_recents_keep_provider_identity_and_busy_rules(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let prefs = crate::settings::Preferences {
            model_picker_tab: Some("favorites".into()),
            favorite_models: vec![
                crate::settings::ModelChoice {
                    provider: "codex".into(),
                    model: Some("gpt-saved".into()),
                },
                crate::settings::ModelChoice {
                    provider: "claude".into(),
                    model: Some("opus".into()),
                },
            ],
            recent_models: vec![crate::settings::ModelChoice {
                provider: "codex".into(),
                model: Some("gpt-recent".into()),
            }],
            ..Default::default()
        };
        cx.set_global(prefs);
        let list = cx.new(|cx| {
            ModelList::new_for(
                Kind::Codex,
                Some("gpt-saved".into()),
                "/work",
                Default::default(),
                cx,
            )
            .allow_provider_switch(false)
        });
        let favorites = list.read(cx).entries(cx);
        assert_eq!(favorites.len(), 2);
        assert_eq!(favorites[0].kind, Kind::Codex);
        assert!(favorites[0].enabled);
        assert_eq!(favorites[1].kind, Kind::Claude);
        assert!(!favorites[1].enabled);
        list.update(cx, |list, _| list.tab = ModelTab::Harness(Kind::Codex));
        let rows = list.read(cx).entries(cx);
        assert_eq!(rows[1].id.as_deref(), Some("gpt-recent"));
        assert_eq!(rows[2].id.as_deref(), Some("gpt-saved"));
        assert_eq!(list.read(cx).current_index(cx), 2);
    });
}
