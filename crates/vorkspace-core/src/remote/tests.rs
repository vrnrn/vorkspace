use super::*;
struct ReceivedRequest {
    headers: String,
    payload: Value,
}

fn host_fixture(
    responses: Vec<Value>,
) -> (
    String,
    std::sync::mpsc::Receiver<ReceivedRequest>,
    std::thread::JoinHandle<()>,
) {
    use std::io::{BufRead, BufReader};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    if key.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap();
                    }
                }
                headers.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let _ = sender.send(ReceivedRequest {
                headers,
                payload: serde_json::from_slice(&body).unwrap(),
            });
            let body = response.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    (endpoint, receiver, server)
}

#[test]
fn existing_store_remains_compatible_and_credentials_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remote-machines.json");
    std::fs::write(&path, r#"[{"id":"old","name":"server","endpoint":"https://host.example","environmentId":"env","token":"private"}]"#).unwrap();
    let connections = Connections::new(dir.path());
    let machine = connections.machine_for_environment("env").unwrap();
    assert_eq!(machine.id, "old");
    assert_eq!(machine.ssh, None);
    assert!(!serde_json::to_string(&machine).unwrap().contains("private"));
    assert!(!format!("{connections:?}").contains("private"));
    connections.disconnect("old").unwrap();
    assert!(connections.list().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn connecting_same_host_keeps_identity_and_scopes_authenticated_requests() {
    let descriptor =
        json!({ "result": { "protocolVersion": 1, "environmentId": "env", "name": "Server" }});
    let (url, requests, server) = host_fixture(vec![
        descriptor.clone(),
        descriptor,
        json!({ "result": { "projects": [] } }),
        json!({ "result": { "protocolVersion": 1, "environmentId": "replacement" } }),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let connections = Connections::new(dir.path());
    let token = "a".repeat(43);
    let first = connections
        .connect("".into(), url.clone(), token.clone())
        .unwrap();
    assert_eq!(first.name, "Server");
    let second = connections
        .connect(" New name ".into(), url, token.clone())
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.name, "New name");
    assert_eq!(connections.list().unwrap().len(), 1);
    assert_eq!(
        connections
            .request(&first.id, "projects.list", json!({}))
            .unwrap(),
        json!({ "projects": [] })
    );
    let error = connections
        .request(&first.id, "environment.describe", json!({}))
        .unwrap_err();
    assert!(error.starts_with("Host identity changed"));
    server.join().unwrap();
    let requests: Vec<_> = requests.iter().collect();
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert!(request.headers.starts_with("POST /rpc HTTP/1.1"));
        assert!(request
            .headers
            .contains(&format!("Authorization: Bearer {token}")));
        assert_eq!(request.payload["version"], 1);
    }
    assert_eq!(requests[0].payload["environmentId"], Value::Null);
    assert_eq!(requests[2].payload["environmentId"], "env");
    assert_eq!(requests[2].payload["method"], "projects.list");
    assert!(requests[3].payload["params"]["supportedProviders"]
        .as_array()
        .unwrap()
        .contains(&json!("antigravity")));
}

#[test]
fn incompatible_pairing_does_not_create_a_machine() {
    let (url, _, server) = host_fixture(vec![
        json!({ "result": { "protocolVersion": 2, "environmentId": "env" } }),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let connections = Connections::new(dir.path());
    assert!(connections
        .connect("".into(), url, "a".repeat(43))
        .unwrap_err()
        .contains("incompatible"));
    server.join().unwrap();
    assert!(connections.list().unwrap().is_empty());
    assert!(!dir.path().join("remote-machines.json").exists());
}

#[test]
fn invalid_store_and_unknown_remote_operations_never_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let connections = Connections::new(dir.path());
    assert_eq!(
        connections
            .request("missing", "git.arbitrary", json!({}))
            .unwrap_err(),
        "Unsupported remote operation"
    );
    assert_eq!(
        connections
            .request("missing", "files.read", json!({ "path": "/tmp/private" }))
            .unwrap_err(),
        "Machine is no longer connected"
    );
    std::fs::write(connections.store_path(), "invalid").unwrap();
    assert!(connections.list().is_err());
    assert!(connections.disconnect("missing").is_err());
    assert_eq!(
        std::fs::read_to_string(connections.store_path()).unwrap(),
        "invalid"
    );
}

#[test]
fn remote_paths_round_trip_unix_drives_and_unc_roots() {
    for (host, normalized, qualified) in [
        (
            "/home/user/repo",
            "/home/user/repo",
            "remote://env/home/user/repo",
        ),
        (
            r"C:\Users\Nick\repo",
            "C:/Users/Nick/repo",
            "remote://env/C:/Users/Nick/repo",
        ),
        (
            r"\\server\share\repo",
            "//server/share/repo",
            "remote://env//server/share/repo",
        ),
        ("/", "/", "remote://env/"),
        ("C:", "C:", "remote://env/C:"),
    ] {
        assert_eq!(remote_path("env", host), qualified);
        let parsed = parse_remote_path(qualified).unwrap();
        assert_eq!(parsed.environment_id, "env");
        assert_eq!(parsed.host_path, normalized);
    }
    assert!(parse_remote_path("/local/repo").is_none());
    assert!(parse_remote_path("remote://env").is_none());
    assert!(parse_remote_path("remote:///repo").is_none());
}

#[test]
fn a_slow_host_does_not_look_like_a_dead_tunnel() {
    use std::{net::TcpListener, thread, time::Duration};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_millis(200));
    });
    let error = ureq::get(&url)
        .timeout(Duration::from_millis(50))
        .call()
        .unwrap_err();
    let ureq::Error::Transport(error) = error else {
        panic!("Expected a timeout")
    };
    assert!(!connection_refused(&error));
    server.join().unwrap();
    // A freed ephemeral port can be claimed by another parallel fixture.
    // Exercise ureq's actual wrapped I/O source without that port race.
    let error = ureq::Error::from(std::io::Error::from(std::io::ErrorKind::ConnectionRefused));
    let ureq::Error::Transport(error) = error else {
        panic!("Expected a refused connection")
    };
    assert!(connection_refused(&error));
}

#[test]
fn desktop_forwards_supported_host_operations() {
    for method in [
        "git.branches",
        "git.switch",
        "git.createBranch",
        "git.worktrees",
        "git.worktreeCreate",
        "attachments.upload",
        "attachments.read",
    ] {
        assert!(supported_remote_method(method), "{method}");
    }
    assert!(!supported_remote_method("git.arbitrary"));
}
#[test]
fn endpoints_require_an_encrypted_route_and_no_embedded_secrets() {
    assert_eq!(
        endpoint("http://127.0.0.1:3774/").unwrap(),
        "http://127.0.0.1:3774"
    );
    assert!(endpoint("http://[::1]:3774").is_ok());
    assert!(endpoint("https://host.example").is_ok());
    for url in [
        "http://192.168.1.5:3774",
        "file:///tmp/socket",
        "https://user:secret@host.example",
        "https://host.example/rpc",
        "https://host.example?token=secret",
        "https://host.example#secret",
    ] {
        assert!(endpoint(url).is_err(), "{url}");
    }
}
#[test]
fn public_machine_metadata_never_contains_the_token() {
    let stored = StoredMachine {
        id: "id".into(),
        name: "server".into(),
        endpoint: "https://host.example".into(),
        environment_id: "env".into(),
        token: "secret".into(),
        ssh: None,
    };
    let value = serde_json::to_string(&stored.public()).unwrap();
    assert!(!value.contains("secret"));
    assert!(!value.contains("token"));
}
