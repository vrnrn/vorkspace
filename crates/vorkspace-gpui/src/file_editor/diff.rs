use super::*;
impl FileEditor {
    pub(super) fn apply_change(&mut self, stage: bool, cx: &mut Context<Self>) {
        if stage && self.git_staged {
            return;
        }
        let input = self.input.read(cx);
        if input.is_composing() {
            return;
        }
        let changes = &input.editor_analysis().changes;
        let Some(change) = changes.get(self.change_index.min(changes.len().saturating_sub(1)))
        else {
            return;
        };
        let Some(original) = self.git_original.clone() else {
            return;
        };
        let source = input.content.clone();
        let selection = input.selection_range();
        let at = change.from;
        let cursor = at;
        let line_ending = if original.is_empty() {
            self.document.as_ref().map_or("\n", |d| {
                if d.crlf {
                    "\r\n"
                } else if d.cr {
                    "\r"
                } else {
                    "\n"
                }
            })
        } else {
            self.git_line_ending
        };
        let cwd = PathBuf::from(&self.cwd);
        let Ok(relative) = PathBuf::from(&self.path)
            .strip_prefix(&cwd)
            .map(|p| p.to_string_lossy().into_owned())
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let text = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    vorkspace_editor::editor_analysis::chunk_action(
                        &original, &text, at, selection, stage,
                    )
                })
                .await;
            let Ok(Some(mut text)) = result else {
                return;
            };
            if stage {
                if line_ending != "\n" {
                    text = text.replace('\n', line_ending);
                }
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        vorkspace_core::scm::stage_contents(&cwd, &relative, text.as_bytes())
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Err(error) = result {
                        this.error = Some(error);
                    } else {
                        this.git.update(cx, |git, cx| git.changed(cx));
                        this.refresh_git(cx);
                    }
                    cx.notify();
                });
            } else {
                let _ = this.update(cx, |this, cx| {
                    if this.input.read(cx).content == source && !this.input.read(cx).is_composing()
                    {
                        let mut cursor = cursor.min(text.len());
                        while !text.is_char_boundary(cursor) {
                            cursor -= 1;
                        }
                        this.input.update(cx, |i, cx| {
                            i.replace_bytes(0..source.len(), &text, cursor, cx)
                        });
                    }
                });
            }
        })
        .detach();
    }
}
