use super::*;
impl FileEditor {
    pub(super) fn revert_change(&mut self, cx: &mut Context<Self>) {
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
        if self.checkout.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let text = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    vorkspace_editor::editor_analysis::chunk_action(&original, &text, at, selection)
                })
                .await;
            let Ok(Some(text)) = result else {
                return;
            };

            let _ = this.update(cx, |this, cx| {
                if this.input.read(cx).content == source && !this.input.read(cx).is_composing() {
                    let mut cursor = cursor.min(text.len());
                    while !text.is_char_boundary(cursor) {
                        cursor -= 1;
                    }
                    this.input.update(cx, |i, cx| {
                        i.replace_bytes(0..source.len(), &text, cursor, cx)
                    });
                }
            });
        })
        .detach();
    }
}
