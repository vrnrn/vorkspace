use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use vorkspace_plugin::{Context, call};
fn quote(value: impl AsRef<std::ffi::OsStr>) -> String {
    format!(
        "'{}'",
        value.as_ref().to_string_lossy().replace('\'', "'\\''")
    )
}
#[test]
fn real_connector_binaries_preserve_status_shapes_and_reject_wrong_identity() {
    let directory = tempfile::tempdir().unwrap();
    let context = Context {
        data_dir: directory.path().join("data"),
        cwd: directory.path().into(),
    };
    // GUI discovery prefers the login-shell PATH over the inherited PATH.
    // Control that shell as well, so a real authenticated gh can never make the
    // fixture pass while bypassing its fake executable.
    let calls = directory.path().join("gh.calls");
    let gh = directory.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n[ \"$1 $2\" = \"auth status\" ]\n",
            quote(&calls)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let shell = directory.path().join("login-shell");
    let path =
        std::env::join_paths([directory.path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
    std::fs::write(
        &shell,
        format!("#!/bin/sh\nprintf 'PATH=%s\\n' {}\n", quote(path)),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let wrapper = directory.path().join("github");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport SHELL={}\nexec {}\n",
            quote(&shell),
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
    assert_eq!(
        std::fs::read_to_string(calls).unwrap(),
        "auth status --active --hostname github.com\n"
    );
}
