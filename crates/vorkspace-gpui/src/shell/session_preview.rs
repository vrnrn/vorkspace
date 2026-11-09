//! Hover cards for unopened sessions do not construct conversation entities.
use crate::pickers::cached_model_name;
use gpui::{AnyElement, FontWeight, div, prelude::*, px};
use std::path::Path;
use vorkspace_core::{
    provider::Kind,
    store::{Session, SessionPreviewData},
};
use vorkspace_ui::theme::*;

pub(super) fn render(
    summary: &Session,
    data: Option<&SessionPreviewData>,
    done: bool,
) -> AnyElement {
    let model = data
        .and_then(|data| data.model.as_deref())
        .and_then(|model| vorkspace_core::store::native_model(model, &summary.provider));
    let name = cached_model_name(Kind::from_id(&summary.provider), model.as_deref());
    let worktree = data.and_then(|data| data.worktree_cwd.as_deref());
    let cwd = worktree.unwrap_or(&summary.cwd);
    let directory = std::env::var_os("HOME")
        .and_then(|home| {
            Path::new(cwd).strip_prefix(home).ok().map(|path| {
                if path.as_os_str().is_empty() {
                    "~".to_owned()
                } else {
                    format!("~/{}", path.display())
                }
            })
        })
        .unwrap_or_else(|| cwd.to_owned());
    let directory = elide_middle(&directory, 34);
    div()
        .px_3()
        .py_2p5()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(11.))
                        .text_color(content(0.55))
                        .child(provider_icon(&summary.provider, 14.))
                        .child(div().min_w_0().truncate().child(name)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(if done {
                            vorkspace_ui::theme::success()
                        } else {
                            content(0.45)
                        })
                        .child(if done {
                            "Done".to_owned()
                        } else {
                            vorkspace_core::time::relative(
                                summary.updated_at,
                                vorkspace_core::time::now_ms(),
                            )
                        }),
                ),
        )
        .child(
            div()
                .mt_1p5()
                .text_size(px(13.))
                .line_height(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(1.))
                .child(
                    vorkspace_core::session_title::display_title(&summary.title, &summary.provider)
                        .replace('\n', " "),
                ),
        )
        .child(
            div()
                .mt_2()
                .pt_2()
                .border_t_1()
                .border_color(content(0.08))
                .text_size(px(11.))
                .line_height(px(16.))
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(72.))
                        .flex_shrink_0()
                        .text_color(content(0.45))
                        .child(if worktree.is_some() {
                            "Worktree"
                        } else {
                            "Directory"
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        // Clip from the left so the project folder stays.
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_start()
                        .text_color(content(0.8))
                        .child(directory),
                ),
        )
        .into_any_element()
}

/// A path short enough for one line: its root and its last folders, with
/// the middle as "…". A path wrapped at its slashes reads as two paths.
fn elide_middle(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').collect();
    let head = if path.starts_with('/') { "" } else { parts[0] };
    let mut tail: Vec<&str> = Vec::new();
    for part in parts.iter().skip(1).rev() {
        let next = tail.iter().map(|p| p.len() + 1).sum::<usize>() + part.len() + 1;
        if !tail.is_empty() && head.len() + 2 + next > max {
            break;
        }
        tail.insert(0, part);
    }
    format!("{head}/…/{}", tail.join("/"))
}

#[cfg(test)]
mod elide_tests {
    use super::elide_middle;

    #[test]
    fn long_paths_keep_their_root_and_last_folders() {
        assert_eq!(elide_middle("~/code/app", 34), "~/code/app");
        assert_eq!(
            elide_middle("~/Development/clients/acme/services/billing-api", 34),
            "~/…/acme/services/billing-api"
        );
        assert_eq!(
            elide_middle("/private/tmp/a-very-long-folder-name-that-overflows", 34),
            "/…/a-very-long-folder-name-that-overflows"
        );
    }
}
