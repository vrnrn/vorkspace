use gpui::{CursorStyle, Div, Hsla, Role, Stateful, Svg, div, img, prelude::*, px, rgb, rgba, svg};
// Shared appearance tokens. Interaction fills use translucent content ink
// rather than opaque replacement greys.
pub const CHAT: u32 = 0x141414;
pub const TEXT: u32 = 0xebebeb;
pub const MUTED: u32 = 0x969696;
pub const ACCENT: u32 = 0x4da3f5;
pub const COLUMN: f32 = 832.;
pub const TITLE_BAR_HEIGHT: f32 = 40.;
// AppKit points, independent of GPUI UI zoom and display DPI. Keep the
// renderer's safe inset and the native constraints on the same geometry.
pub const TRAFFIC_LIGHT_SIZE: f32 = 14.;
pub const TRAFFIC_LIGHT_MARGIN: f32 = 12.;
pub const TRAFFIC_LIGHT_SPACING: f32 = 6.;
pub const TRAFFIC_LIGHT_SAFE_INSET: f32 =
    2. * TRAFFIC_LIGHT_MARGIN + 3. * TRAFFIC_LIGHT_SIZE + 2. * TRAFFIC_LIGHT_SPACING;
pub const NO_TRAFFIC_LIGHTS: f32 = 10.;

pub fn traffic_light_inset(ui_scale: f32, fullscreen: bool) -> f32 {
    if !fullscreen {
        TRAFFIC_LIGHT_SAFE_INSET / ui_scale
    } else {
        NO_TRAFFIC_LIGHTS
    }
}
pub const TITLE_TAB_TEXT_SIZE: f32 = 13.;
pub const TITLE_TAB_MAX_WIDTH: f32 = 224.;
pub const PANE_HEADER_HEIGHT: f32 = 36.;
/// Every tab's plate: title, file and terminal tabs, and the new-tab button.
pub const TAB_PLATE_HEIGHT: f32 = 27.;
/// How far a tab's label and close button sit below the bar's centre, so
/// they centre in the pill that runs from the plate down to the bar's line.
pub const TAB_LABEL_DROP: f32 = 3.;

/// Attachment previews keep a fixed crop, independent of image dimensions.
pub fn attachment_thumbnail(source: impl Into<gpui::ImageSource>) -> gpui::AnyElement {
    div()
        .size(px(36.))
        .min_w(px(36.))
        .min_h(px(36.))
        .max_w(px(36.))
        .max_h(px(36.))
        .flex_shrink_0()
        .rounded_lg()
        .overflow_hidden()
        .child(
            img(source)
                .size(px(36.))
                .aspect_square()
                .flex_shrink_0()
                .rounded_lg()
                .object_fit(gpui::ObjectFit::Cover),
        )
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq)]
struct Tokens {
    light: bool,
    ink: Hsla,
    accent: Hsla,
    user_accent: Option<Hsla>,
    shell: Hsla,
    pane: Hsla,
    syntax: [u32; 11],
    diff: [u32; 4],
    diff_strong: bool,
}
thread_local! { static TOKENS: std::cell::RefCell<Option<Tokens>> = const { std::cell::RefCell::new(None) }; }
pub fn configure(p: &crate::preferences::Preferences, system_light: bool, classic: bool) -> bool {
    let light = p.theme == "light" || (p.theme == "system" && system_light);
    let palette = crate::palettes::palette(&p.workspace_theme, light);
    let glass = window_glass(p, system_light, classic);
    // Both layouts derive their surface roles from the same saved settings.
    let shell = gpui::hsla(
        p.theme_hue.min(360) as f32 / 360.,
        p.theme_saturation.min(100) as f32 / 100.,
        if light {
            0.97
        } else {
            p.theme_dark_lightness.min(30) as f32 / 100.
        },
        if glass {
            p.sidebar_opacity.clamp(15, 100) as f32 / 100.
        } else {
            1.
        },
    );
    let mut pane = shell;
    pane.a = if glass && p.body_glass { 0.28 } else { 1. };
    let custom = p
        .accent_color
        .as_deref()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok())
        .filter(|_| p.accent_color.as_ref().is_some_and(|s| s.len() == 7));
    let tokens = Tokens {
        light,
        ink: rgb(palette.content).into(),
        accent: rgb(custom.unwrap_or(palette.accent)).into(),
        user_accent: custom.map(|color| rgb(color).into()),
        shell,
        pane,
        diff: diff_palette(&p.diff_palette, light),
        diff_strong: p.diff_palette == "high-contrast",
        syntax: syntax_palette(&p.workspace_theme, light),
    };
    TOKENS.with(|current| {
        let changed = *current.borrow() != Some(tokens);
        *current.borrow_mut() = Some(tokens);
        changed
    })
}

pub fn window_glass(
    p: &crate::preferences::Preferences,
    system_light: bool,
    classic: bool,
) -> bool {
    (classic || p.inset_glass) && p.theme != "light" && !(p.theme == "system" && system_light)
}
pub fn accent_color() -> Hsla {
    TOKENS.with(|t| {
        t.borrow()
            .as_ref()
            .map(|t| t.accent)
            .unwrap_or_else(|| rgb(ACCENT).into())
    })
}
pub fn content(alpha: f32) -> Hsla {
    let mut color: Hsla = TOKENS.with(|t| {
        t.borrow()
            .as_ref()
            .map(|t| t.ink)
            .unwrap_or_else(|| rgb(TEXT).into())
    });
    color.a = alpha;
    color
}

pub fn is_light() -> bool {
    TOKENS.with(|tokens| tokens.borrow().as_ref().is_some_and(|tokens| tokens.light))
}

/// Semantic state ink and translucent washes. Dark ink preserves the native UI
/// palette. Light ink starts from the artifact palette, with deeper danger and
/// success shades to keep contrast over tinted surfaces and status washes.
#[derive(Clone, Copy, Debug)]
pub enum StatusColor {
    Danger,
    Warning,
    Success,
}

pub fn status(role: StatusColor, alpha: f32) -> Hsla {
    let (dark, light) = match role {
        StatusColor::Danger => (0xf87171, 0xa52632),
        StatusColor::Warning => (0xfbbf24, 0x845c0e),
        StatusColor::Success => (0x10b981, 0x166c49),
    };
    let color: Hsla = rgb(if is_light() { light } else { dark }).into();
    color.opacity(alpha)
}

pub fn floating_fill() -> Hsla {
    let mut fill = pane_fill();
    if is_light() {
        fill.l = 1.;
    }
    fill.a = 1.;
    fill
}

/// Text selection follows the current accent without obscuring light-theme ink.
pub fn text_selection() -> Hsla {
    accent_color().opacity(if is_light() { 0.2 } else { 0.35 })
}

fn user_accent() -> Option<Hsla> {
    TOKENS.with(|tokens| {
        tokens
            .borrow()
            .as_ref()
            .and_then(|tokens| tokens.user_accent)
    })
}

pub fn user_bubble_fill() -> Hsla {
    user_accent().map_or(content(0.1), |accent| accent.opacity(0.24))
}

/// Preserve neutral controls until the user explicitly chooses an accent.
pub fn primary_action(enabled: bool) -> Option<(Hsla, Hsla)> {
    let accent = user_accent()?;
    if !enabled {
        return Some((accent.opacity(0.28), content(0.35)));
    }
    let rgb = accent.to_rgb();
    let linear = |value: f32| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b);
    Some((
        accent,
        if luminance > 0.179 {
            black(1.)
        } else {
            white(1.)
        },
    ))
}

/// `[data-composer-box]::before` uses content ink at 3% over its backdrop.
/// Light mode instead gives the box the solid `background-base` surface.
pub fn composer_fill() -> Hsla {
    let light = TOKENS.with(|tokens| tokens.borrow().as_ref().is_some_and(|t| t.light));
    if light {
        let mut background = shell_fill();
        background.a = 1.;
        background
    } else {
        content(0.03)
    }
}

/// The inset composer's frost; the solid light surface has no backdrop blur.
pub fn composer_blur() -> f32 {
    let light = TOKENS.with(|tokens| tokens.borrow().as_ref().is_some_and(|t| t.light));
    if light { 0. } else { 24. }
}

pub fn composer_border(focused: bool) -> Hsla {
    content(if focused { 0.2 } else { 0.1 })
}

pub fn title_tab_fill(selected: bool, hovered: bool) -> Hsla {
    content(if selected {
        0.1
    } else if hovered {
        0.07
    } else {
        0.04
    })
}

/// The plate for session tabs in the title bar. Its fill is a pill rising
/// from the line beneath the tabs (`crate::liquid`), so the plate has none.
/// Headline ink owns inactive dimming; the plate never fades its children.
pub fn title_tab_plate(_selected: bool, _hovered: bool) -> Div {
    div()
        .h(px(TAB_PLATE_HEIGHT))
        .rounded_md()
        .px_2()
        // The pill runs on to the line beneath; centre the label within it.
        .pt(px(TAB_LABEL_DROP * 2.))
        .flex()
        .items_center()
}
pub fn shell_fill() -> Hsla {
    if let Some(fill) = TOKENS.with(|t| t.borrow().as_ref().map(|t| t.shell)) {
        return fill;
    }
    // #0d0d0d at 80%, with the stylesheet's #ebebeb 5% wash above it.
    rgba(0x1b1b1bcf).into()
}
pub fn pane_fill() -> Hsla {
    if let Some(fill) = TOKENS.with(|t| t.borrow().as_ref().map(|t| t.pane)) {
        return fill;
    }
    rgba(0x06060666).into()
}
pub fn section_label(label: &'static str) -> Div {
    div()
        .h(px(28.))
        .px_2()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(content(0.5))
        .child(div().flex_1().child(label))
}
pub fn icon(path: &'static str, side: f32) -> Svg {
    svg()
        .path(path)
        .size(px(side))
        .text_color(content(0.5))
        .flex_shrink_0()
}
pub fn provider_icon(provider: &str, side: f32) -> Div {
    let (path, monochrome, scale) = match provider {
        "claude" => ("claude.svg", false, 1.),
        "antigravity" => ("antigravity.svg", false, 1.),
        "pi" => ("pi.svg", true, 1.),
        "omp" => ("omp.svg", false, 1.),
        _ => ("codex.svg", false, 1.),
    };
    // Like React's currentColor / CSS masks, GPUI's SVG mask follows the theme.
    // Image rendering preserves brand colors but has no inherited currentColor.
    let mark = if monochrome {
        svg()
            .path(path)
            .size(px(side * scale))
            .text_color(content(1.))
            .flex_shrink_0()
            .into_any_element()
    } else {
        img(path).size(px(side)).flex_shrink_0().into_any_element()
    };
    div()
        .size(px(side))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
}
pub fn icon_button(id: &'static str, label: &'static str, path: &'static str) -> Stateful<Div> {
    icon_button_hover(id, label, path, 0.1)
}
pub fn icon_button_hover(
    id: &'static str,
    label: &'static str,
    path: &'static str,
    hover_alpha: f32,
) -> Stateful<Div> {
    chrome_button(id, label, path, true, hover_alpha)
}
pub fn icon_button_enabled(
    id: &'static str,
    label: &'static str,
    path: &'static str,
    enabled: bool,
) -> Stateful<Div> {
    chrome_button(id, label, path, enabled, 0.1)
}
fn chrome_button(
    id: &'static str,
    label: &'static str,
    path: &'static str,
    enabled: bool,
    hover_alpha: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .group("chrome-button")
        .cursor(CursorStyle::Arrow)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_md()
        .text_color(content(if enabled { 0.5 } else { 0.25 }))
        .when(enabled, |d| {
            d.hover(|s| s.bg(content(hover_alpha)).text_color(content(1.)))
        })
        .child(
            icon(path, 14.)
                .text_color(content(if enabled { 0.5 } else { 0.25 }))
                .when(enabled, |i| {
                    i.group_hover("chrome-button", |s| s.text_color(content(1.)))
                }),
        )
}
// `.primary-action` paints with literal white and black, not theme ink.
pub fn white(alpha: f32) -> Hsla {
    gpui::hsla(0., 0., 1., alpha)
}
pub fn black(alpha: f32) -> Hsla {
    gpui::hsla(0., 0., 0., alpha)
}

const SYNTAX_KEYS: [&str; 11] = [
    "keyword", "callable", "string", "type", "tag", "number", "comment", "property", "meta",
    "invalid", "heading",
];
fn syntax_palette(theme: &str, light: bool) -> [u32; 11] {
    static PALETTES: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let palettes = PALETTES.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/editor-palettes.json"))
            .expect("bundled editor palettes")
    });
    let palette = palettes.get(theme).unwrap_or(&palettes["default"]);
    let palette = &palette[if light { "light" } else { "dark" }];
    SYNTAX_KEYS.map(|key| {
        u32::from_str_radix(
            palette[key]
                .as_str()
                .unwrap_or("#ebebeb")
                .trim_start_matches('#'),
            16,
        )
        .unwrap_or(0xebebeb)
    })
}
pub fn syntax_color(kind: &str) -> Hsla {
    let name = kind
        .strip_prefix("var(--syntax-")
        .and_then(|s| s.strip_suffix(')'));
    let index = if kind == "var(--color-markdown-heading)" {
        Some(10)
    } else {
        name.and_then(|name| SYNTAX_KEYS.iter().position(|key| *key == name))
    };
    index.map_or_else(
        || content(1.),
        |index| {
            let color = TOKENS
                .with(|t| t.borrow().as_ref().map(|t| t.syntax[index]))
                .unwrap_or_else(|| syntax_palette("default", false)[index]);
            rgb(color).into()
        },
    )
}

/// Resolve cached Dark+ grammar colors through the live appearance palette.
pub fn diff_syntax_color(color: u32) -> Hsla {
    let kind = match color {
        0x569cd6 | 0xc586c0 => "var(--syntax-keyword)",
        0xdcdcaa => "var(--syntax-callable)",
        0xce9178 | 0xd7ba7d => "var(--syntax-string)",
        0x4ec9b0 | 0xb8d7a3 => "var(--syntax-type)",
        0xb5cea8 => "var(--syntax-number)",
        0x6a9955 | 0x608b4e => "var(--syntax-comment)",
        0x9cdcfe => "var(--syntax-property)",
        0xd16969 | 0x646695 => "var(--syntax-meta)",
        0xf44747 => "var(--syntax-invalid)",
        _ => return content(0.92),
    };
    syntax_color(kind)
}

fn diff_palette(name: &str, light: bool) -> [u32; 4] {
    match (name, light) {
        ("colorblind", false) => [0x388bfd, 0xdb6d28, 0x79c0ff, 0xffa657],
        ("colorblind", true) => [0x0969da, 0xbc4c00, 0x0550ae, 0x953800],
        ("high-contrast", false) => [0x58a6ff, 0xf0883e, 0xcae8ff, 0xffdfb6],
        ("high-contrast", true) => [0x0550ae, 0x953800, 0x032563, 0x471700],
        (_, true) => [0x10b981, 0xf43f5e, 0x047857, 0xbe123c],
        _ => [0x10b981, 0xf43f5e, 0x6ee7b7, 0xfda4af],
    }
}
pub fn diff_foreground(add: bool) -> Hsla {
    let index = if add { 2 } else { 3 };
    let color = TOKENS
        .with(|t| t.borrow().as_ref().map(|t| t.diff[index]))
        .unwrap_or_else(|| diff_palette("default", false)[index]);
    rgb(color).into()
}
pub fn diff_background(add: bool, gutter: bool) -> Hsla {
    let index = if add { 0 } else { 1 };
    let (color, strong) = TOKENS
        .with(|t| t.borrow().as_ref().map(|t| (t.diff[index], t.diff_strong)))
        .unwrap_or((diff_palette("default", false)[index], false));
    let mut color: Hsla = rgb(color).into();
    color.a = match (gutter, strong) {
        (true, true) => 0.45,
        (true, false) => 0.25,
        (false, true) => 0.28,
        _ => 0.15,
    };
    color
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_appearance(
    preferences: &crate::preferences::Preferences,
    system_light: bool,
    classic: bool,
) -> impl Drop {
    struct Restore(Option<Tokens>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TOKENS.with(|tokens| *tokens.borrow_mut() = self.0);
        }
    }
    let restore = Restore(TOKENS.with(|tokens| *tokens.borrow()));
    configure(preferences, system_light, classic);
    restore
}

#[cfg(any(test, feature = "test-support"))]
pub fn test_contrast(ink: Hsla, background: Hsla) -> f32 {
    let background = background.to_rgb();
    let foreground = background.blend(ink.to_rgb());
    let luminance = |color: gpui::Rgba| {
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    };
    let (a, b) = (luminance(foreground), luminance(background));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod composer_tests {
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
}
