use super::*;

#[test]
fn status_ink_follows_appearance_and_has_contrast_on_solid_surfaces() {
    for preset in ["default", "harbor", "moss"] {
        for (appearance, system_light, light) in [
            ("dark", true, false),
            ("light", false, true),
            ("system", true, true),
            ("system", false, false),
        ] {
            for glass in [false, true] {
                for accent in [None, Some("#ffff00".into())] {
                    let mut preferences = crate::preferences::Preferences {
                        theme: appearance.into(),
                        inset_glass: glass,
                        body_glass: glass,
                        accent_color: accent,
                        ..Default::default()
                    };
                    preferences.apply_workspace_theme(preset.into());
                    let _restore = test_appearance(&preferences, system_light, false);
                    assert_eq!(is_light(), light);
                    for role in [
                        StatusColor::Danger,
                        StatusColor::Warning,
                        StatusColor::Success,
                    ] {
                        let ink = status(role, 1.);
                        assert_eq!(status(role, 0.1), ink.opacity(0.1));
                        // Solid panes/cards, floating menus and existing confirmation washes.
                        let mut pane = pane_fill();
                        pane.a = 1.;
                        let card = pane.blend(content(0.03));
                        let wash = if matches!(role, StatusColor::Danger) {
                            0.2
                        } else {
                            0.1
                        };
                        for background in [
                            pane,
                            card,
                            floating_fill(),
                            pane.blend(ink.opacity(wash)),
                            card.blend(ink.opacity(wash)),
                        ] {
                            let contrast = test_contrast(ink, background);
                            assert!(
                                contrast >= 4.5,
                                "{role:?}: {contrast}, {preset}, {appearance}, light={light}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn status_dark_palette_is_preserved_and_light_palette_is_distinct() {
    let preferences = crate::preferences::Preferences {
        theme: "dark".into(),
        ..Default::default()
    };
    let _restore = test_appearance(&preferences, false, true);
    for (role, expected) in [
        (StatusColor::Danger, 0xf87171),
        (StatusColor::Warning, 0xfbbf24),
        (StatusColor::Success, 0x10b981),
    ] {
        let dark = status(role, 1.);
        assert_eq!(dark, rgb(expected).into());
        let light_preferences = crate::preferences::Preferences {
            theme: "light".into(),
            ..Default::default()
        };
        {
            let _light = test_appearance(&light_preferences, false, true);
            assert_ne!(dark, status(role, 1.));
        }
        assert_eq!(dark, status(role, 1.));
    }
}

#[test]
fn cached_diff_tokens_follow_live_light_dark_and_custom_palettes() {
    let saved = TOKENS.with(|tokens| *tokens.borrow());
    let mut preferences = crate::preferences::Preferences::default();
    configure(&preferences, false, true);
    let dark = diff_syntax_color(0xd4d4d4);
    preferences.theme = "light".into();
    configure(&preferences, false, true);
    let light = diff_syntax_color(0xd4d4d4);
    assert!(dark.l > 0.6 && light.l < 0.3);
    assert_eq!(
        diff_syntax_color(0x569cd6),
        syntax_color("var(--syntax-keyword)")
    );
    let keyword = diff_syntax_color(0x569cd6);
    preferences.workspace_theme = "harbor".into();
    configure(&preferences, false, true);
    assert_ne!(keyword, diff_syntax_color(0x569cd6));
    assert_eq!(
        diff_syntax_color(0x6a9955),
        syntax_color("var(--syntax-comment)")
    );
    TOKENS.with(|tokens| *tokens.borrow_mut() = saved);
}

#[test]
fn accent_reset_and_theme_switch_preserve_readable_controls() {
    struct Restore(Option<Tokens>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TOKENS.with(|tokens| *tokens.borrow_mut() = self.0);
        }
    }
    let _restore = Restore(TOKENS.with(|tokens| *tokens.borrow()));
    let mut preferences = crate::preferences::Preferences::default();
    for theme in ["dark", "light", "dark"] {
        preferences.theme = theme.into();
        configure(&preferences, false, true);
        assert_eq!(user_bubble_fill(), content(0.1));
        assert_eq!(primary_action(true), None);
        assert_eq!(floating_fill().a, 1.);
        assert_eq!(
            text_selection().a,
            if theme == "light" { 0.2 } else { 0.35 }
        );
        for (color, ink) in [("#ffff00", black(1.)), ("#000080", white(1.))] {
            preferences.accent_color = Some(color.into());
            configure(&preferences, false, true);
            let accent = accent_color();
            assert_eq!(primary_action(true), Some((accent, ink)));
            assert_eq!(
                primary_action(false),
                Some((accent.opacity(0.28), content(0.35)))
            );
            assert_eq!(user_bubble_fill(), accent.opacity(0.24));
        }
        preferences.accent_color = None;
        configure(&preferences, false, true);
        assert_eq!(primary_action(true), None);
        assert_eq!(user_bubble_fill(), content(0.1));
    }
}

#[test]
fn both_layouts_share_opacity_and_keep_dark_glass_preferences_when_disabled() {
    let before = TOKENS.with(|tokens| *tokens.borrow());
    let mut preferences = crate::preferences::Preferences {
        theme: "dark".into(),
        sidebar_opacity: 43,
        sidebar_blur: 57,
        ..Default::default()
    };
    for classic in [true, false] {
        configure(&preferences, false, classic);
        let alpha = TOKENS.with(|tokens| tokens.borrow().unwrap().shell.a);
        assert_eq!(alpha, 0.43);
    }
    preferences.inset_glass = false;
    assert!(!window_glass(&preferences, false, false));
    assert!(window_glass(&preferences, false, true));
    configure(&preferences, false, false);
    assert_eq!(TOKENS.with(|tokens| tokens.borrow().unwrap().shell.a), 1.);
    preferences.theme = "light".into();
    for classic in [true, false] {
        assert!(!window_glass(&preferences, false, classic));
    }
    assert_eq!(preferences.sidebar_opacity, 43);
    assert_eq!(preferences.sidebar_blur, 57);
    TOKENS.with(|tokens| *tokens.borrow_mut() = before);
}

#[test]
fn composer_surface_matches_react_dark_ink_and_solid_light_background() {
    struct Restore(Option<Tokens>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TOKENS.with(|tokens| *tokens.borrow_mut() = self.0);
        }
    }
    let _restore = Restore(TOKENS.with(|tokens| *tokens.borrow()));
    let mut preferences = crate::preferences::Preferences::default();
    configure(&preferences, false, true);
    assert_eq!(composer_fill(), content(0.03));
    assert_eq!(composer_blur(), 24.);
    assert_eq!(composer_border(false), content(0.1));
    assert_eq!(composer_border(true), content(0.2));

    preferences.theme = "light".into();
    preferences.theme_hue = 180;
    preferences.theme_saturation = 20;
    configure(&preferences, false, true);
    assert_eq!(composer_fill(), gpui::hsla(0.5, 0.2, 0.97, 1.));
    assert_eq!(composer_blur(), 0.);
    assert_eq!(composer_border(false), content(0.1));
    assert_eq!(composer_border(true), content(0.2));
}
