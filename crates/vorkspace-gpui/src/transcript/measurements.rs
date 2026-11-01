//! Retain unchanged text-row heights and share probes within each frame.
use super::*;

#[derive(PartialEq)]
struct LayoutStyle {
    text: gpui::TextStyle,
    rem: Pixels,
    scale: f32,
}

#[derive(Default)]
pub(super) struct Measurements {
    style: Option<LayoutStyle>,
    viewport: gpui::Size<Pixels>,
    row_count: usize,
    rows: HashMap<usize, Pixels>,
    retained: HashMap<(usize, usize), Pixels>,
    #[cfg(test)]
    pub(super) fresh: usize,
}

impl Measurements {
    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.retained.clear();
    }
}

impl TranscriptView {
    pub(super) fn begin_measurements(&mut self, window: &Window) {
        let style = LayoutStyle {
            text: window.text_style(),
            rem: window.rem_size(),
            scale: window.scale_factor(),
        };
        if self.measurements.style.as_ref() != Some(&style) {
            // GPUI retains measured item heights independently of text styling.
            self.list.remeasure_items(0..self.list.item_count());
            self.phase_layouts.clear();
            self.measurements.style = Some(style);
            self.measurements.clear();
        }
        if self.measurements.viewport != self.viewport.size
            || self.measurements.row_count != self.rows.len()
        {
            // Removing closing rows can also change their neighbours' spacing.
            self.measurements.clear();
            self.measurements.viewport = self.viewport.size;
            self.measurements.row_count = self.rows.len();
        }
        self.measurements.rows.clear();
        let motions = &self.motions;
        self.measurements
            .retained
            .retain(|key, _| !motions.contains_key(key));
    }

    pub(super) fn measure_row_height(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Pixels {
        // The viewport probe can discover a resize after render's follow pass.
        if self.measurements.viewport != self.viewport.size {
            self.measurements.clear();
            self.measurements.viewport = self.viewport.size;
        }
        if let Some(height) = self.measurements.rows.get(&ix) {
            return *height;
        }
        let key = self.rows[ix].key;
        let stable = stable_height(&self.rows[ix], &self.messages, &self.motions);
        if stable && let Some(&height) = self.measurements.retained.get(&key) {
            self.measurements.rows.insert(ix, height);
            return height;
        }
        let mut row = self.row(ix, window, cx);
        let height = row
            .layout_as_root(
                size(
                    AvailableSpace::Definite(self.viewport.size.width),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            )
            .height;
        #[cfg(test)]
        {
            self.measurements.fresh += 1;
        }
        self.measurements.rows.insert(ix, height);
        if stable {
            self.measurements.retained.insert(key, height);
        }
        height
    }
}

fn stable_height(
    row: &UiRow,
    messages: &[Message],
    motions: &HashMap<(usize, usize), Motion>,
) -> bool {
    // Independent child views, attachments and disclosures can resize without
    // a source patch. Retain only text geometry that rebuild invalidates.
    matches!(row.body, Body::Plain(_) | Body::Markdown(_))
        && !row.in_phase()
        && !motions.contains_key(&row.key)
        && messages[row.message].attachments.is_empty()
}

#[cfg(test)]
mod tests;
