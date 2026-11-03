use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use vorkspace_plugin::{Context, call};
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
#[test]
fn real_connector_binaries_preserve_status_shapes_and_reject_wrong_identity() {
    let directory = tempfile::tempdir().unwrap();
    let context = Context {
        data_dir: directory.path().join("data"),
        cwd: directory.path().into(),
    };
    // No user credentials or network: GitHub's CLI auth check uses this local fake.
    let gh = directory.path().join("gh");
    std::fs::write(&gh, "#!/bin/sh\n[ \"$1 $2\" = \"auth status\" ]\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let wrapper = directory.path().join("github");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport PATH={}:$PATH\nexec {}\n",
            quote(directory.path()),
            quote(Path::new(env!("CARGO_BIN_EXE_vorkspace-github-plugin")))
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (id, path) in [
        ("github", wrapper.as_path()),
        (
            "gitlab",
            Path::new(env!("CARGO_BIN_EXE_vorkspace-gitlab-plugin")),
        ),
        (
            "jira",
            Path::new(env!("CARGO_BIN_EXE_vorkspace-jira-plugin")),
        ),
    ] {
        let (manifest, status) = call(
            path,
            Some(id),
            &context,
            "connector.status",
            json!({}),
            Duration::from_secs(10),
            |_, _| panic!("Connector must not request native capabilities"),
        )
        .unwrap();
        assert_eq!(manifest.id, id);
        assert!(manifest.connector && !manifest.ui && manifest.capabilities.is_empty());
        assert_eq!(status["connected"], Value::Bool(id == "github"));
        assert!(
            call(
                path,
                Some("wrong.connector"),
                &context,
                "connector.status",
                json!({}),
                Duration::from_secs(10),
                |_, _| unreachable!()
            )
            .unwrap_err()
            .contains("identity")
        );
    }
}
