//! File-type icons: the Material Icon Theme lookup, from the
//! react-material-icon-theme 1.2.0 tables (scripts/assets/generate-material-icons.mjs).
use gpui::{AnyElement, img, prelude::*, px};
use serde::Deserialize;
use std::{borrow::Cow, collections::HashMap, sync::OnceLock};

#[derive(Deserialize)]
struct Table {
    files: HashMap<String, String>,
    exts: HashMap<String, String>,
    folders: HashMap<String, String>,
    svgs: HashMap<String, String>,
}
fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: Table = serde_json::from_str(include_str!("../assets/material-icons.json"))
            .expect("bundled icon table");
        table.files.retain(|_, icon| table.svgs.contains_key(icon));
        table.exts.retain(|_, icon| table.svgs.contains_key(icon));
        table
            .folders
            .retain(|_, icon| table.svgs.contains_key(icon));
        table
    })
}
/// The package matches the whole lowercased name, then each compound suffix
/// (`d.ts`, then `ts`).
pub(crate) fn file_icon_name(name: &str) -> &'static str {
    let table = table();
    let key = if name
        .bytes()
        .all(|b| b.is_ascii() && !b.is_ascii_uppercase())
    {
        Cow::Borrowed(name)
    } else {
        Cow::Owned(name.to_lowercase())
    };
    if let Some(icon) = table.files.get(key.as_ref()) {
        return icon;
    }
    let name = key.strip_prefix('.').unwrap_or(&key);
    for (dot, _) in name.match_indices('.') {
        if let Some(icon) = table.exts.get(&name[dot + 1..]) {
            return icon;
        }
    }
    "file"
}
/// `getFolderIcon` with the specific theme, never as a root.
fn folder_icon_name(name: &str, open: bool) -> Cow<'static, str> {
    let table = table();
    let base = table.folders.get(name).map_or("folder", String::as_str);
    if open {
        if base != "folder" {
            let icon = format!("{base}-open");
            if table.svgs.contains_key(&icon) {
                return Cow::Owned(icon);
            }
        }
        Cow::Borrowed("folder-open")
    } else {
        Cow::Borrowed(base)
    }
}
/// `mi/<icon>.svg` asset bytes.
pub fn svg(asset: &str) -> Option<&'static [u8]> {
    let name = asset.strip_prefix("mi/")?.strip_suffix(".svg")?;
    table().svgs.get(name).map(|svg| svg.as_bytes())
}
/// Colored, so drawn as an image rather than a monochrome SVG mask.
pub fn file_type_icon(name: &str, is_dir: bool, open: bool, side: f32) -> AnyElement {
    let icon = if is_dir {
        folder_icon_name(name, open)
    } else {
        Cow::Borrowed(file_icon_name(name))
    };
    img(format!("mi/{icon}.svg"))
        .size(px(side))
        .flex_shrink_0()
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_then_compound_extensions_then_folders() {
        assert_eq!(
            file_icon_name("Package.JSON"),
            table().files["package.json"]
        );
        assert_eq!(file_icon_name("Cargo.toml"), "toml");
        assert_eq!(file_icon_name("main.rs"), "rust");
        assert_eq!(file_icon_name("index.d.ts"), table().exts["d.ts"]);
        assert_eq!(file_icon_name(".index.d.ts"), table().exts["d.ts"]);
        assert_eq!(file_icon_name(".rs"), "file");
        assert_eq!(
            file_icon_name("Instructions.md"),
            file_icon_name("unknown.md")
        );
        assert_eq!(
            file_icon_name("copilot.instructions.md"),
            file_icon_name("unknown.md")
        );
        assert_eq!(file_icon_name("no-extension"), "file");
        assert_eq!(folder_icon_name("src", false), "folder-src");
        assert_eq!(folder_icon_name("src", true), "folder-src-open");
        assert_eq!(folder_icon_name("migrations", false), "folder-migrations");
        assert_eq!(folder_icon_name("migrations", true), "folder-open");
        assert_eq!(folder_icon_name("deprecated", false), "folder");
        assert_eq!(folder_icon_name("zzz-unknown", true), "folder-open");
        for icon in ["rust", "file", "folder", "folder-open", "folder-src-open"] {
            assert!(svg(&format!("mi/{icon}.svg")).is_some(), "{icon}");
        }
    }
}
