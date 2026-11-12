use super::*;
use crate::accounts::{Account, Book, DEFAULT};

impl Store {
    pub fn provider_accounts(&self) -> Result<Book, String> {
        let mut book: Book = self
            .setting("provider-accounts")?
            .map(|raw| parse(&raw))
            .transpose()?
            .unwrap_or_default();
        if let Some(directory) = self.data_dir() {
            for provider in ["codex", "claude"] {
                let root = directory.join("provider-accounts").join(provider);
                if !root.is_dir() {
                    continue;
                }
                let mut ids = Vec::new();
                for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                        ids.push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
                ids.sort();
                for id in ids {
                    if crate::accounts::validate_id(&id).is_ok()
                        && id != DEFAULT
                        && !book
                            .accounts
                            .iter()
                            .any(|a| a.provider == provider && a.id == id)
                    {
                        let ordinal = book
                            .accounts
                            .iter()
                            .filter(|a| a.provider == provider && a.id != DEFAULT)
                            .count()
                            + 2;
                        book.accounts.push(Account {
                            id,
                            provider: provider.into(),
                            label: format!("Account {ordinal}"),
                        });
                    }
                }
            }
        }
        Ok(book)
    }

    pub fn save_provider_accounts(&self, book: &Book) -> Result<(), String> {
        let value = encode(book)?;
        self.update_settings(|_| Ok(((), vec![("provider-accounts".into(), value)])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::Kind;

    #[test]
    fn isolated_account_names_and_selections_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vorkspace.db");
        let store = Store::open_vorkspace(&path).unwrap();
        let mut book = Book::default();
        let account = book.add(Kind::Codex, "My development account").unwrap();
        book.select(Kind::Codex, "/project", &account.id).unwrap();
        store.save_provider_accounts(&book).unwrap();
        drop(store);
        let store = Store::open_vorkspace(&path).unwrap();
        let book = store.provider_accounts().unwrap();
        assert_eq!(book.list(Kind::Codex)[1].label, "My development account");
        assert_eq!(book.selected(Kind::Codex, "/project"), account.id);
    }
}
