//! Inline SVG and Mermaid diagrams, drawn natively. Mermaid is rendered to SVG
//! by Pele in QuickJS; Pele's CSS variables are resolved to the app's palette,
//! then the SVG is rasterized off the main thread through GPUI's image assets.
use crate::theme::*;
use gpui::{
    AnyElement, App, Asset, Hsla, ImageCacheError, ImageSource, IntoElement, ParentElement,
    RenderImage, SharedString, Styled, Window, div, img, prelude::*,
};
use rquickjs::{Context, Function, Object, Runtime};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use vorkspace_core::visual::{MAX_MERMAID_BYTES, validate_svg};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DiagramKind {
    Svg,
    Mermaid,
}

impl DiagramKind {
    /// The fenced code languages drawn as diagrams.
    pub(crate) fn for_language(language: &str) -> Option<Self> {
        match language
            .split_whitespace()
            .next()?
            .to_ascii_lowercase()
            .as_str()
        {
            "svg" => Some(Self::Svg),
            "mermaid" | "mmd" => Some(Self::Mermaid),
            _ => None,
        }
    }
}

/// Concrete colours for Pele's theme variables, as CSS colour strings.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Palette {
    bg: String,
    fg: String,
    muted: String,
    line: String,
    surface: String,
    surface_alt: String,
    border: String,
    accent: String,
}

fn css(color: Hsla) -> String {
    let rgba = color.to_rgb();
    format!(
        "rgba({},{},{},{:.3})",
        (rgba.r * 255.).round(),
        (rgba.g * 255.).round(),
        (rgba.b * 255.).round(),
        rgba.a
    )
}

impl Palette {
    /// The live appearance, read on the main thread.
    pub(crate) fn current() -> Self {
        let light = is_light();
        Self {
            // Label masks need an opaque ground close to the pane.
            bg: if light { "#ffffff" } else { "#1c1c1c" }.into(),
            fg: css(content(0.9)),
            muted: css(content(0.55)),
            line: css(content(0.6)),
            surface: if light { "#f3f3f3" } else { "#262626" }.into(),
            surface_alt: if light { "#e8e8e8" } else { "#303030" }.into(),
            border: css(content(0.3)),
            accent: css(accent_color()),
        }
    }
    fn get(&self, name: &str) -> Option<&str> {
        Some(match name {
            "bg" => &self.bg,
            "fg" | "fg-on-light" => &self.fg,
            "muted" => &self.muted,
            "line" => &self.line,
            "surface" => &self.surface,
            "surface-alt" => &self.surface_alt,
            "border" => &self.border,
            "accent" | "series-1" => &self.accent,
            _ => return None,
        })
    }
}

/// A loaded Pele context and the deadline its interrupt handler checks.
type PeleContext = (Runtime, Context, Rc<Cell<Instant>>);

thread_local! {
    /// A warm Pele context per worker thread, with the current call's deadline.
    static PELE: RefCell<Option<PeleContext>> = const { RefCell::new(None) };
}

fn pele<R>(work: impl FnOnce(&Context) -> Result<R, String>) -> Result<R, String> {
    PELE.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            let runtime = Runtime::new().map_err(|e| e.to_string())?;
            runtime.set_memory_limit(256 * 1024 * 1024);
            runtime.set_max_stack_size(4 * 1024 * 1024);
            let deadline = Rc::new(Cell::new(Instant::now() + Duration::from_secs(10)));
            let check = deadline.clone();
            runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > check.get())));
            let context = Context::full(&runtime).map_err(|e| e.to_string())?;
            context
                .with(|ctx| ctx.eval::<(), _>(include_str!("../assets/pele.global.js")))
                .map_err(|e| format!("Pele failed to load: {e}"))?;
            *cell = Some((runtime, context, deadline));
        }
        let (_, context, deadline) = cell.as_ref().unwrap();
        deadline.set(Instant::now() + Duration::from_secs(3));
        work(context)
    })
}

/// Renders Mermaid source to Pele's themeable SVG.
pub(crate) fn mermaid_svg(source: &str) -> Result<String, String> {
    if source.trim().is_empty() || source.len() > MAX_MERMAID_BYTES {
        return Err(format!(
            "Mermaid source must contain 1–{MAX_MERMAID_BYTES} bytes"
        ));
    }
    pele(|context| {
        context.with(|ctx| {
            let pele: Object = ctx.globals().get("pele").map_err(|e| e.to_string())?;
            let render: Function = pele.get("render").map_err(|e| e.to_string())?;
            let options = Object::new(ctx.clone()).map_err(|e| e.to_string())?;
            // Static output: no links or remote images in a native image.
            options.set("links", false).map_err(|e| e.to_string())?;
            options.set("images", false).map_err(|e| e.to_string())?;
            let result: Object = render.call((source, options)).map_err(|e| {
                let message = ctx
                    .catch()
                    .as_exception()
                    .and_then(|e| e.message())
                    .unwrap_or_else(|| e.to_string());
                format!("Mermaid: {message}")
            })?;
            result.get::<_, String>("svg").map_err(|e| e.to_string())
        })
    })
}

/// Replaces Pele's `var(--…)` references and `color-mix()` with concrete
/// colours; the native SVG renderer resolves neither.
pub(crate) fn themed(svg: &str, palette: &Palette) -> String {
    // The root style declares `--_x:var(--pele-name,fallback)` aliases.
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut rest = svg;
    while let Some(at) = rest.find("--_") {
        let tail = &rest[at..];
        let Some(colon) = tail.find(':') else { break };
        let name = &tail[..colon];
        let value_end = tail[colon + 1..]
            .find(';')
            .map(|i| colon + 1 + i)
            .unwrap_or(tail.len());
        let value = &tail[colon + 1..value_end];
        if !name.contains(|c: char| c.is_whitespace() || c == '(' || c == ')')
            && value.starts_with("var(")
        {
            aliases.insert(name.into(), resolve_vars(value, palette, &HashMap::new()));
        }
        rest = &tail[3..];
    }
    let resolved = resolve_vars(svg, palette, &aliases);
    mix_colors(&resolved)
}

/// Resolves every `var(--name[,fallback])` from the aliases, then the palette,
/// then the fallback.
fn resolve_vars(text: &str, palette: &Palette, aliases: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("var(--") {
        out.push_str(&rest[..at]);
        let inner_start = at + 4;
        let Some(close) = matching_paren(&rest[at + 3..]).map(|i| at + 3 + i) else {
            out.push_str(&rest[at..]);
            return out;
        };
        let inner = &rest[inner_start..close];
        let (name, fallback) = match inner.find(',') {
            Some(comma) => (inner[..comma].trim(), Some(inner[comma + 1..].trim())),
            None => (inner.trim(), None),
        };
        let value = aliases
            .get(name)
            .cloned()
            .or_else(|| {
                name.strip_prefix("--pele-")
                    .and_then(|key| palette.get(key))
                    .map(str::to_owned)
            })
            .or_else(|| fallback.map(|f| resolve_vars(f, palette, aliases)))
            .unwrap_or_else(|| palette.fg.clone());
        out.push_str(&value);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The index of the `)` closing the `(` at the start of `text`.
fn matching_paren(text: &str) -> Option<usize> {
    let mut depth = 0;
    for (ix, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(ix);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_color(text: &str) -> Option<[f32; 4]> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        let hex: String = if hex.len() == 3 {
            hex.chars().flat_map(|c| [c, c]).collect()
        } else {
            hex.into()
        };
        let v = u32::from_str_radix(&hex, 16).ok()?;
        return match hex.len() {
            6 => Some([
                (v >> 16 & 255) as f32,
                (v >> 8 & 255) as f32,
                (v & 255) as f32,
                1.,
            ]),
            _ => None,
        };
    }
    let inner = text
        .strip_prefix("rgba(")
        .or_else(|| text.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<f32> = inner
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [r, g, b] => Some([*r, *g, *b, 1.]),
        [r, g, b, a] => Some([*r, *g, *b, *a]),
        _ => None,
    }
}

/// Evaluates `color-mix(in srgb, A p%, B)` for resolved colours.
fn mix_colors(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("color-mix(") {
        out.push_str(&rest[..at]);
        let Some(close) = matching_paren(&rest[at + 9..]).map(|i| at + 9 + i) else {
            out.push_str(&rest[at..]);
            return out;
        };
        let inner = &rest[at + 10..close];
        out.push_str(&mix(inner).unwrap_or_else(|| rest[at..=close].to_owned()));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

fn mix(inner: &str) -> Option<String> {
    // "in srgb,A p%,B": split at top-level commas.
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (ix, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&inner[start..ix]);
                start = ix + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    let [_, first, second] = parts.as_slice() else {
        return None;
    };
    let (first, weight) = match first.trim().rsplit_once(' ') {
        Some((color, pct)) if pct.ends_with('%') => {
            (color, pct.trim_end_matches('%').parse::<f32>().ok()? / 100.)
        }
        _ => (first.trim(), 0.5),
    };
    let (a, b) = (parse_color(first)?, parse_color(second)?);
    let c: Vec<f32> = (0..4)
        .map(|i| a[i] * weight + b[i] * (1. - weight))
        .collect();
    Some(format!(
        "rgba({},{},{},{:.3})",
        c[0].round(),
        c[1].round(),
        c[2].round(),
        c[3]
    ))
}

/// Drops `<image>` elements that point anywhere but an inline `data:` URI,
/// so a drawn SVG never reads files or the network.
pub(crate) fn without_external_images(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(at) = lower.find("<image") else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        let end = lower[at..]
            .find('>')
            .map(|i| at + i + 1)
            .unwrap_or(rest.len());
        let tag = &lower[at..end];
        let inline = ["href=\"data:", "href='data:"]
            .iter()
            .any(|prefix| tag.contains(prefix))
            && !tag.contains("href=\"http")
            && !tag.contains("href='http");
        if inline {
            out.push_str(&rest[at..end]);
        }
        // A paired `</image>` closes nothing once its opening tag is gone.
        rest = &rest[end..];
        if !inline && rest.to_ascii_lowercase().starts_with("</image>") {
            rest = &rest["</image>".len()..];
        }
    }
}

/// The SVG to rasterize for a diagram.
pub(crate) fn prepare(
    kind: DiagramKind,
    source: &str,
    palette: &Palette,
) -> Result<String, String> {
    match kind {
        DiagramKind::Svg => {
            validate_svg(source)?;
            Ok(without_external_images(source))
        }
        DiagramKind::Mermaid => Ok(themed(&mermaid_svg(source)?, palette)),
    }
}

/// Rasterized at twice the logical size, for sharp edges on Retina displays.
const SCALE: f32 = 2.;

pub(crate) enum DiagramAsset {}

impl Asset for DiagramAsset {
    type Source = (DiagramKind, SharedString, Palette);
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        (kind, source, palette): Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        async move {
            let svg =
                prepare(kind, &source, &palette).map_err(|e| ImageCacheError::Asset(e.into()))?;
            renderer
                .render_single_frame(svg.as_bytes(), SCALE)
                .map_err(|e| ImageCacheError::Asset(e.to_string().into()))
        }
    }
}

/// A diagram, rendered off the main thread; until then, and if it fails,
/// `fallback` shows instead.
pub(crate) fn diagram(
    kind: DiagramKind,
    source: SharedString,
    title: Option<SharedString>,
    fallback: impl Fn() -> AnyElement + 'static,
) -> AnyElement {
    let key = (kind, source, Palette::current());
    let label: SharedString = title.unwrap_or_else(|| {
        match kind {
            DiagramKind::Svg => "Diagram",
            DiagramKind::Mermaid => "Mermaid diagram",
        }
        .into()
    });
    div()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .child(
            img(ImageSource::Custom(Arc::new(
                move |window: &mut Window, cx: &mut App| window.use_asset::<DiagramAsset>(&key, cx),
            )))
            .max_w_full()
            .with_fallback(fallback)
            .id(label.clone())
            .role(gpui::Role::Image)
            .aria_label(label),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        Palette {
            bg: "#101010".into(),
            fg: "#eeeeee".into(),
            muted: "#999999".into(),
            line: "#cccccc".into(),
            surface: "#202020".into(),
            surface_alt: "#303030".into(),
            border: "#444444".into(),
            accent: "#ff6600".into(),
        }
    }

    #[test]
    fn mermaid_renders_through_pele_and_resolves_its_theme() {
        let svg =
            mermaid_svg("flowchart LR\n  A[Mermaid text] --> B{Ok?}\n  B -- yes --> C([SVG])")
                .unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Mermaid text"));
        assert!(svg.contains("var(--_s)"));
        let themed = themed(&svg, &palette());
        assert!(!themed.contains("var("), "{themed}");
        assert!(themed.contains("fill=\"#202020\""));
        assert!(themed.contains("stroke=\"#cccccc\""));
        // A warm context renders the next diagram too.
        assert!(
            mermaid_svg("sequenceDiagram\n  A->>B: hi")
                .unwrap()
                .contains("<svg")
        );
        let error = mermaid_svg("notADiagram at all").unwrap_err();
        assert!(error.starts_with("Mermaid:"), "{error}");
    }

    #[test]
    fn variables_fall_back_and_colours_mix() {
        let p = palette();
        assert_eq!(
            resolve_vars("var(--pele-unknown,#123456)", &p, &HashMap::new()),
            "#123456"
        );
        assert_eq!(
            resolve_vars("var(--pele-accent,#000)", &p, &HashMap::new()),
            "#ff6600"
        );
        assert_eq!(
            mix_colors("fill=\"color-mix(in srgb,#ffffff 25%,#000000)\""),
            "fill=\"rgba(64,64,64,1.000)\""
        );
        assert_eq!(mix_colors("color-mix(oops"), "color-mix(oops");
    }

    #[test]
    fn svg_blocks_lose_external_images_and_reject_scripts() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="file:///etc/passwd"/><image href="data:image/png;base64,AAAA"/><rect width="4" height="4"/></svg>"#;
        let kept = prepare(DiagramKind::Svg, svg, &palette()).unwrap();
        assert!(!kept.contains("passwd"));
        assert!(kept.contains("data:image/png"));
        assert!(kept.contains("<rect"));
        assert!(
            prepare(
                DiagramKind::Svg,
                "<svg><script>x()</script></svg>",
                &palette()
            )
            .is_err()
        );
        assert!(prepare(DiagramKind::Svg, "<div>no</div>", &palette()).is_err());
    }

    #[test]
    fn fence_languages_choose_diagrams() {
        assert_eq!(
            DiagramKind::for_language("mermaid"),
            Some(DiagramKind::Mermaid)
        );
        assert_eq!(
            DiagramKind::for_language("SVG title=x"),
            Some(DiagramKind::Svg)
        );
        assert_eq!(DiagramKind::for_language("rust"), None);
    }

    #[gpui::test]
    fn the_asset_rasterizes_a_themed_diagram(cx: &mut gpui::TestAppContext) {
        let key = (
            DiagramKind::Mermaid,
            SharedString::from("flowchart TD\n  A --> B"),
            palette(),
        );
        let renderer = cx.update(|cx| cx.svg_renderer());
        let svg = prepare(key.0, &key.1, &key.2).unwrap();
        let image = renderer.render_single_frame(svg.as_bytes(), SCALE).unwrap();
        assert!(image.size(0).width.0 > 0);
    }
}
