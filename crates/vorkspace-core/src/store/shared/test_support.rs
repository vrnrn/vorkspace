//! Fixtures use the desktop schema and keep temporary files alive with the store.
use super::*;

impl Store {
    /// Create or reopen a fixture at a caller-owned temporary path. The memory
    /// marker creates an owned temporary directory, including its real sidecar.
    pub fn test(path: &Path) -> Result<Self, String> {
        let directory = (path == Path::new(":memory:"))
            .then(|| tempfile::tempdir().map(std::sync::Arc::new))
            .transpose()
            .map_err(|error| error.to_string())?;
        let path = directory
            .as_ref()
            .map(|directory| directory.path().join("vorkspace.db"))
            .unwrap_or_else(|| path.to_owned());
        let mut store = Self::open_shared(&path, false)?;
        store.test_directory = directory;
        Ok(store)
    }

    /// Fail changed sidecar writes at their persistence boundary without
    /// making unrelated reads or rollback cleanup fail.
    pub fn fail_setting_for_test(&self, failure: Option<(&str, &str)>) {
        *self.shared.setting_write_failure.borrow_mut() =
            failure.map(|(prefix, error)| (prefix.into(), error.into()));
    }

    pub(super) fn check_setting_for_test(
        &self,
        key: &str,
        value: &str,
        previous: Option<&str>,
    ) -> Result<(), String> {
        if let Some((prefix, error)) = self.shared.setting_write_failure.borrow().as_ref() {
            if key.starts_with(prefix) && previous != Some(value) {
                return Err(error.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_retains_its_temporary_database_and_sidecar() {
        let store = Store::test(Path::new(":memory:")).unwrap();
        let path = store.database_path().to_owned();
        let id = store.create("codex", "/project").unwrap().id;
        store.save_setting("fixture", "saved").unwrap();
        let reader = store.search_reader().unwrap().unwrap();
        drop(store);

        assert!(path.exists());
        assert_eq!(reader.sessions().unwrap()[0].id, id);
        assert_eq!(reader.setting("fixture").unwrap().as_deref(), Some("saved"));
        assert!(reader.save_setting("fixture", "changed").is_err());
        let reopened = Store::test(&path).unwrap();
        assert_eq!(reopened.sessions().unwrap()[0].id, id);
        assert_eq!(
            reopened.setting("fixture").unwrap().as_deref(),
            Some("saved")
        );
        drop(reopened);
        drop(reader);
        assert!(!path.parent().unwrap().exists());
    }
}
