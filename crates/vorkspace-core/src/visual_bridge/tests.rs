
use super::*;

fn input() -> Value {
    json!({"source":"return () => visual.text('Hello');","title":"Hello","requestId":"test-request","id":"hello"})
}

#[test]
fn scoped_publication_acknowledges_after_save_and_deduplicates() {
    let server = Server::start(42).unwrap();
    let environment = server.environment(std::path::Path::new("/test/vorkspace-gpui"));
    let child_environment = environment.clone();
    let worker = std::thread::spawn(move || publish_with_environment(input(), &child_environment));
    let publication = server
        .publications
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    assert_eq!(publication.document.title, "Hello");
    assert!(!worker.is_finished());
    publication.reply.send(Ok(json!({"id":"hello"}))).unwrap();
    assert_eq!(worker.join().unwrap().unwrap()["result"]["id"], "hello");
    assert!(publish_with_environment(input(), &environment).is_ok());
    assert!(server.drain().is_empty());
    let mut changed = input();
    changed["source"] = "return () => visual.text('Changed');".into();
    assert!(publish_with_environment(changed, &environment)
        .unwrap_err()
        .contains("already used"));
}

#[test]
fn wrong_session_and_expired_capabilities_cannot_publish() {
    let server = Server::start(7).unwrap();
    let mut environment = server.environment(std::path::Path::new("/test/app"));
    environment.insert(ENV_SESSION.into(), "8".into());
    assert!(publish_with_environment(input(), &environment).is_err());
    assert!(server.drain().is_empty());
    environment.insert(ENV_SESSION.into(), "7".into());
    drop(server);
    std::thread::sleep(Duration::from_millis(30));
    assert!(publish_with_environment(input(), &environment).is_err());
}

#[test]
fn publication_reports_persistence_failure() {
    let server = Server::start(3).unwrap();
    let environment = server.environment(std::path::Path::new("/test/app"));
    let worker = std::thread::spawn(move || publish_with_environment(input(), &environment));
    let publication = server
        .publications
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    publication.reply.send(Err("Saving failed".into())).unwrap();
    assert!(worker
        .join()
        .unwrap()
        .unwrap_err()
        .contains("Saving failed"));
}

#[test]
fn local_validation_and_mcp_notifications_need_no_capability() {
    assert!(check("return () => visual.text('Local');").is_ok());
    assert!(check("while (true) {}").is_err());
    assert!(check(&" ".repeat(MAX_SOURCE_BYTES + 1)).is_err());
    let environment = HashMap::new();
    assert!(mcp_response(json!({"method":"notifications/initialized"}), &environment).is_none());
    let response = mcp_response(
        json!({"id":2,"method":"tools/call","params":{"name":"visual_show","arguments":input()}}),
        &environment,
    )
    .unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(tool_definitions().len(), 3);
    assert_eq!(acp_servers(&environment), json!([]));
}

#[test]
fn visual_reference_is_loaded_on_demand_without_global_instructions() {
    let environment = HashMap::new();
    let initialization = mcp_response(
        json!({"id":1,"method":"initialize","params":{}}),
        &environment,
    )
    .unwrap();
    assert!(initialization["result"].get("instructions").is_none());
    let catalog = mcp_response(json!({"id":2,"method":"tools/list"}), &environment).unwrap();
    let metadata = catalog["result"].to_string();
    assert!(
        metadata.len() < 2_500,
        "Discovery must stay small: {} bytes",
        metadata.len()
    );
    assert!(!metadata.contains("visual.column"));
    assert!(!metadata.contains("hardcode font"));
    let response = mcp_response(
        json!({"id":3,"method":"tools/call","params":{"name":"visual_docs","arguments":{}}}),
        &environment,
    )
    .unwrap();
    assert_eq!(response["result"]["isError"], false);
    let reference: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(reference["documentation"]
        .as_str()
        .unwrap()
        .contains("visual.scene"));
    assert!(reference["documentation"]
        .as_str()
        .unwrap()
        .contains("Style is inherited from the app"));
    assert_eq!(reference, client(&["docs".into()]).unwrap());
    assert!(client(&["docs".into(), "--input".into(), "unused".into()]).is_err());
}

#[test]
fn stable_session_configuration_rejects_old_in_flight_requests() {
    let server = Server::start(83).unwrap();
    let environment = server.environment(std::path::Path::new("/test/app"));
    let child_environment = environment.clone();
    let worker = std::thread::spawn(move || publish_with_environment(input(), &child_environment));
    let old = server
        .publications
        .recv_timeout(Duration::from_secs(3))
        .unwrap();
    drop(server);
    let next = Server::start(83).unwrap();
    assert_eq!(
        environment,
        next.environment(std::path::Path::new("/test/app"))
    );
    let _ = old.reply.send(Ok(json!({"id":"old"})));
    assert!(worker.join().unwrap().unwrap_err().contains("ended"));
    assert!(next.drain().is_empty());
}

#[test]
fn provider_configuration_forwards_capability_without_token_in_arguments() {
    let server = Server::start(84).unwrap();
    let environment = server.environment(std::path::Path::new("/test/app"));
    let mut command = std::process::Command::new("provider");
    configure_codex(&mut command, &environment);
    let arguments = command
        .get_args()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(arguments.contains("visual-mcp"));
    assert!(arguments.contains(ENV_TOKEN));
    assert!(!arguments.contains(&environment[ENV_TOKEN]));
    let acp = acp_servers(&environment);
    assert_eq!(acp[0]["name"], "vorkspace_visual");
    assert_eq!(acp[0]["env"].as_array().unwrap().len(), 4);
    assert_eq!(mcp_server(&environment).unwrap()["command"], "/test/app");
}
