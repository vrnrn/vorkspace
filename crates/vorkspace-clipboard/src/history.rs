use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
    sync::Arc,
};

pub(super) const MAX_ITEMS: usize = 50;
pub(super) const MAX_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub(super) const PREVIEW_SIDE: usize = 256;
// Reserve the thumbnail plus its native presentation copy while the menu is open.
const PREVIEW_BYTES: usize = PREVIEW_SIDE * PREVIEW_SIDE * 4 * 2;

pub(super) struct Preview {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

#[derive(PartialEq, Eq)]
pub(super) enum Content {
    Text { value: String, search: String },
    Image { kind: &'static str, bytes: Vec<u8> },
}
impl Content {
    pub fn fingerprint(&self) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        match self {
            Self::Text { value, .. } => value.hash(&mut hash),
            Self::Image { kind, bytes } => {
                kind.hash(&mut hash);
                bytes.hash(&mut hash);
            }
        }
        hash.finish()
    }
    pub(super) fn text(value: String) -> Option<Self> {
        (!value.is_empty() && value.len() <= MAX_TEXT_BYTES).then(|| Self::Text {
            search: value.to_lowercase(),
            value,
        })
    }
    fn cost(&self) -> Option<usize> {
        match self {
            Self::Text { value, search } if value.len() <= MAX_TEXT_BYTES => {
                Some(value.capacity() + search.capacity())
            }
            Self::Image { bytes, .. } if !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES => {
                Some(bytes.capacity() + PREVIEW_BYTES)
            }
            _ => None,
        }
    }
    pub(super) fn label(&self) -> String {
        match self {
            Self::Text { value, .. } => value
                .chars()
                .take(120)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect(),
            Self::Image { .. } => "Image".into(),
        }
    }
    fn matches(&self, query: &str) -> bool {
        query.is_empty() || matches!(self, Self::Text { search, .. } if search.contains(query))
    }
}
pub(super) struct Item {
    pub id: u64,
    pub content: Arc<Content>,
    pub preview: Option<Arc<Preview>>,
    hash: u64,
    cost: usize,
}
#[derive(Default)]
pub(super) struct History {
    pub items: VecDeque<Item>,
    pub bytes: usize,
    next: u64,
}
impl History {
    // Originals, normalized text and reserved previews share one byte budget.
    #[cfg(test)]
    pub fn insert(&mut self, content: Content) -> Option<u64> {
        let hash = content.fingerprint();
        self.insert_hashed(content, hash)
    }
    pub fn insert_hashed(&mut self, content: Content, hash: u64) -> Option<u64> {
        let cost = content.cost().filter(|n| *n <= MAX_BYTES)?;
        if let Some(index) = self
            .items
            .iter()
            .position(|i| i.hash == hash && *i.content == content)
        {
            let item = self.items.remove(index).unwrap();
            let id = item.id;
            self.items.push_front(item);
            return Some(id);
        }
        while self.items.len() >= MAX_ITEMS || self.bytes + cost > MAX_BYTES {
            let old = self.items.pop_back().unwrap();
            self.bytes -= old.cost;
        }
        self.next += 1;
        let id = self.next;
        self.items.push_front(Item {
            id,
            content: Arc::new(content),
            preview: None,
            hash,
            cost,
        });
        self.bytes += cost;
        Some(id)
    }
    pub fn get(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }
    pub fn matching(&self, query: &str) -> Vec<u64> {
        self.items
            .iter()
            .filter(|i| i.content.matches(query))
            .map(|i| i.id)
            .collect()
    }
    pub fn clear(&mut self) {
        self.bytes = 0;
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fiftieth_boundary_duplicates_search_and_clear_release_originals() {
        let mut h = History::default();
        let first = h
            .insert(Content::text("First Straße".into()).unwrap())
            .unwrap();
        let weak = Arc::downgrade(&h.get(first).unwrap().content);
        for i in 0..49 {
            h.insert(Content::text(format!("other {i}")).unwrap());
        }
        assert_eq!(h.get(first).unwrap().content.label(), "First Straße");
        assert_eq!(h.items.len(), 50);
        assert_eq!(h.matching("straße"), vec![first]);
        assert_eq!(
            h.insert(Content::text("First Straße".into()).unwrap()),
            Some(first)
        );
        assert_eq!(h.items.len(), 50);
        assert_eq!(h.items[0].id, first);
        for i in 50..100 {
            h.insert(Content::text(format!("other {i}")).unwrap());
        }
        assert!(weak.upgrade().is_none());
        let weak = Arc::downgrade(&h.items[0].content);
        h.clear();
        assert!(weak.upgrade().is_none());
        assert_eq!(h.bytes, 0);
    }
    #[test]
    fn byte_budget_evicts_before_count_and_oversized_items_leave_history_intact() {
        let mut h = History::default();
        for i in 0..8 {
            h.insert(Content::Image {
                kind: "public.png",
                bytes: vec![i; MAX_IMAGE_BYTES],
            });
            assert!(h.bytes <= MAX_BYTES);
        }
        assert_eq!(h.items.len(), 3);
        let before = h.items.iter().map(|i| i.id).collect::<Vec<_>>();
        assert_eq!(
            h.insert(Content::Image {
                kind: "public.png",
                bytes: vec![0; MAX_IMAGE_BYTES + 1]
            }),
            None
        );
        assert_eq!(h.items.iter().map(|i| i.id).collect::<Vec<_>>(), before);
    }
}
