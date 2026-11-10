//! Resolve model discovery through a saved Host project or session binding.
use std::sync::Arc;
use vorkspace_core::{
    catalog::RemoteCatalog,
    remote::{Connections, default_connections},
    store::RemoteProject,
};
use vorkspace_runtime::runtime::{Database, locked};

#[derive(Clone)]
pub(crate) struct HostModels {
    database: Database,
    project: String,
    session: Option<i64>,
    client: Option<Arc<Connections>>,
}

impl HostModels {
    pub(crate) fn for_project(database: &Database, project: &str) -> Option<Self> {
        project.starts_with("remote://").then(|| Self {
            database: database.clone(),
            project: project.to_owned(),
            session: None,
            client: None,
        })
    }

    pub(crate) fn for_session(
        database: &Database,
        project: &str,
        session: i64,
        client: Option<Arc<Connections>>,
    ) -> Option<Self> {
        let mut models = Self::for_project(database, project)?;
        models.session = Some(session);
        models.client = client;
        Some(models)
    }

    pub(crate) fn read(self) -> Result<Arc<RemoteCatalog>, String> {
        let project = self.bound_project()?;
        let client = self.client.clone().map_or_else(default_connections, Ok)?;
        let catalog = client.model_catalog(&project)?;
        let current = self.bound_project()?;
        if current.environment_id != project.environment_id
            || current.project_id != project.project_id
        {
            return Err("Remote project changed while loading models".into());
        }
        Ok(catalog)
    }

    fn bound_project(&self) -> Result<RemoteProject, String> {
        let store = locked(&self.database)?;
        let Some(session) = self.session else {
            return store.remote_project(&self.project);
        };
        let binding = store
            .remote_session(session)?
            .ok_or("Remote session no longer exists")?;
        if binding.summary.cwd != self.project {
            return Err("Remote session changed while loading models".into());
        }
        Ok(binding.project)
    }
}
