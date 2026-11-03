//! The app executes no connector adapter. All operations cross the plugin IO boundary.
use super::*;
use serde_json::json;
use std::time::Duration;
impl Service {
    fn invoke<T: DeserializeOwned>(
        &self,
        id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<T, String> {
        if !PROVIDERS.contains(&id) {
            return Err("Unsupported connector".into());
        }
        let descriptor = crate::plugins::find(&self.data_dir, id)?;
        let context = vorkspace_plugin::Context {
            data_dir: self.data_dir.clone(),
            cwd: self.cwd.clone(),
        };
        let (_, value) = vorkspace_plugin::call(
            &descriptor.executable,
            Some(id),
            &context,
            method,
            params,
            Duration::from_secs(120),
            |_, _| Err("Connectors do not have host capabilities".into()),
        )?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
    pub fn status(&self, id: &str) -> Result<Status, String> {
        self.invoke(id, "connector.status", json!({}))
    }
    pub fn connect(
        &self,
        id: &str,
        url: String,
        email: String,
        token: String,
    ) -> Result<Status, String> {
        self.invoke(
            id,
            "connector.connect",
            json!({"url":url,"email":email,"token":token}),
        )
    }
    pub fn validate_jql(&self, jql: &str) -> Result<(), String> {
        self.invoke("jira", "connector.validate_jql", json!({"jql":jql}))
    }
    pub fn list(&self, projects: &[PathBuf], state: &ViewState) -> Snapshot {
        let mut output = Snapshot::default();
        for id in state.connectors.enabled_providers() {
            match self.invoke::<Snapshot>(
                id,
                "connector.list",
                json!({"projects":projects,"state":state,"jira_filter":state.jira_filter}),
            ) {
                Ok(part) => {
                    output.items.extend(part.items);
                    output.errors.extend(part.errors);
                    output.connections.extend(part.connections);
                    output.jira_projects.extend(part.jira_projects)
                }
                Err(error) => {
                    output.errors.insert(id.into(), error);
                }
            }
        }
        output.items.sort_by(|a, b| {
            b.updated_ms()
                .cmp(&a.updated_ms())
                .then_with(|| a.title.cmp(&b.title))
        });
        let mut seen = HashSet::new();
        output.items.retain(|item| seen.insert(item.key()));
        output
    }
    pub fn details(&self, item: &Item) -> Result<Details, String> {
        self.invoke(&item.provider, "connector.details", json!({"item":item}))
    }
    pub fn thread(&self, item: &Item) -> Result<Thread, String> {
        self.invoke(&item.provider, "connector.thread", json!({"item":item}))
    }
    pub fn comment(
        &self,
        item: &Item,
        body: String,
        reply_to: String,
    ) -> Result<CommentReceipt, String> {
        self.invoke(
            &item.provider,
            "connector.comment",
            json!({"item":item,"body":body,"reply_to":reply_to}),
        )
    }
    pub fn diff(&self, item: &Item) -> Result<Diff, String> {
        self.invoke(&item.provider, "connector.diff", json!({"item":item}))
    }
    pub fn checks(&self, item: &Item) -> Result<Checks, String> {
        self.invoke(&item.provider, "connector.checks", json!({"item":item}))
    }
    pub fn action(&self, item: &Item, action: &str) -> Result<Item, String> {
        self.invoke(
            &item.provider,
            "connector.action",
            json!({"item":item,"action":action}),
        )
    }
}
