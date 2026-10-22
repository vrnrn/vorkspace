use super::*;
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
};

fn model() -> Value {
    json!({"id":"gpt-6.1-sol","name":"Sol (1M)","contextWindow":900000,"supportedEffortLevels":["none","low","medium","high"]})
}
#[test]
fn preparation_preserves_settings_and_none_effort_without_a_node_wrapper() {
    let mut args = vec![
        "--model".into(),
        "copilot/gpt-6.1-sol".into(),
        "--effort".into(),
        "none".into(),
        "--settings".into(),
        r#"{"disableAllHooks":true,"env":{"LOCAL":"keep"}}"#.into(),
    ];
    let (settings, _, _, effort) = prepare(&mut args, &[model()]).unwrap();
    assert_eq!(effort.as_deref(), Some("none"));
    assert_eq!(settings["alwaysThinkingEnabled"], false);
    assert_eq!(settings["env"]["LOCAL"], "keep");
    assert_eq!(settings["disableAllHooks"], true);
    assert!(!args.iter().any(|v| v == "--effort" || v == "--settings"));
    args.extend(["--effort".into(), "unsupported".into()]);
    assert!(prepare(&mut args, &[model()]).is_err());
}
#[test]
fn ordinary_providers_do_not_start_or_authenticate_a_gateway() {
    let request = Request {
        kind: Kind::Claude,
        binary: "/a path/claude".into(),
        cwd: "/tmp".into(),
        prompt: "test".into(),
        provider_id: None,
        model: Some("sonnet".into()),
        settings: Default::default(),
    };
    let mut command = Command::new(&request.binary);
    assert!(configure(&mut command, &request).unwrap().is_none());
    assert_eq!(command.get_program(), "/a path/claude");
}
#[test]
fn request_translation_preserves_order_tools_images_errors_and_structured_output() {
    let body = json!({"model":"copilot/gpt-6.1-sol","system":[{"type":"text","text":"system"}],"max_tokens":2000,"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"before"},{"type":"tool_use","id":"call","name":"read","input":{"path":"a"}},{"type":"text","text":"after"}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"YWJj"}}],"is_error":true}]}],"tools":[{"name":"read","description":"Read","input_schema":{"type":"object"}}],"tool_choice":{"type":"tool","name":"read","disable_parallel_tool_use":true},"output_config":{"effort":"high","format":{"type":"json_schema","schema":{"type":"object"}}}});
    let v = protocol::to_responses(&body, None).unwrap();
    assert_eq!(v["input"].as_array().unwrap().len(), 4);
    assert_eq!(v["input"][0]["content"][0]["text"], "before");
    assert_eq!(v["input"][1]["arguments"], r#"{"path":"a"}"#);
    assert_eq!(v["input"][2]["content"][0]["text"], "after");
    assert!(v["input"][3]["output"]
        .as_str()
        .unwrap()
        .contains("data:image/png;base64,YWJj"));
    assert_eq!(v["reasoning"]["effort"], "high");
    assert_eq!(v["tools"][0]["strict"], false);
    assert_eq!(v["parallel_tool_calls"], false);
    assert_eq!(v["text"]["format"]["schema"], json!({"type":"object"}));
    assert_eq!(v["store"], false);
}
#[test]
fn stream_tracks_tool_indexes_across_rotated_ids_and_finishes_with_usage() {
    let mut events = Events::default();
    let start=events.push(&json!({"type":"response.output_item.added","output_index":2,"item":{"id":"first","type":"function_call","call_id":"call","name":"read"}})).unwrap();
    assert_eq!(start[0]["content_block"]["id"], "call");
    let delta=events.push(&json!({"type":"response.function_call_arguments.delta","output_index":2,"item_id":"second","delta":"{\"a\":1}"})).unwrap();
    assert_eq!(delta[0]["index"], 0);
    let stop=events.push(&json!({"type":"response.output_item.done","output_index":2,"item":{"id":"third","type":"function_call","arguments":"{\"a\":1}"}})).unwrap();
    assert_eq!(stop.len(), 1);
    let final_events=events.push(&json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":100,"output_tokens":4,"input_tokens_details":{"cached_tokens":30}}}})).unwrap();
    assert_eq!(final_events[0]["delta"]["stop_reason"], "tool_use");
    assert_eq!(final_events[0]["usage"]["input_tokens"], 70);
    assert_eq!(final_events[1]["type"], "message_stop");
    assert!(events.done);
}
#[test]
fn missing_tool_deltas_fall_back_to_final_arguments_and_incomplete_remains_incomplete() {
    let mut events = Events::default();
    events.push(&json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"c","name":"run"}})).unwrap();
    let done=events.push(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","arguments":"{}"}})).unwrap();
    assert_eq!(done.len(), 2);
    assert_eq!(done[0]["delta"]["partial_json"], "{}");
    let result = events
        .push(&json!({"type":"response.incomplete","response":{"status":"incomplete"}}))
        .unwrap();
    assert_eq!(result[0]["delta"]["stop_reason"], "max_tokens");
}
struct Bytes<'a>(&'a [u8]);
impl Read for Bytes<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0.is_empty() {
            return Ok(0);
        }
        buf[0] = self.0[0];
        self.0 = &self.0[1..];
        Ok(1)
    }
}
#[test]
fn event_reader_handles_split_utf8_crlf_and_multiline_but_rejects_truncated_frames() {
    let input = "data: {\"text\":\r\ndata: \"你好\"}\r\n\r\ndata: [DONE]\r\n\r\n";
    let mut values = Vec::new();
    for_each_event(Bytes(input.as_bytes()), |v| {
        values.push(v);
        Ok(true)
    })
    .unwrap();
    assert_eq!(values, json!([{"text":"你好"}]).as_array().unwrap().clone());
    assert!(for_each_event(b"data: {}".as_slice(), |_| Ok(true)).is_err());
    let oversized = format!("data: {}\n\n", " ".repeat(1_048_576));
    assert!(for_each_event(oversized.as_bytes(), |_| Ok(true)).is_err());
}
#[test]
fn catalog_filters_policy_endpoint_and_actual_context_capacity() {
    let good = json!({"id":"gpt-6.1-sol","name":"Sol","supported_endpoints":["/responses"],"capabilities":{"limits":{"max_context_window_tokens":1_000_000,"max_prompt_tokens":900000},"supports":{"reasoning_effort":["medium"]}}});
    let mut disabled = good.clone();
    disabled["policy"] = json!({"state":"disabled"});
    let mut small = good.clone();
    small["capabilities"]["limits"]["max_context_window_tokens"] = 128000.into();
    let rows = catalog_rows(&json!({"data":[good,disabled,small]})).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["contextWindow"], 900000);
}
#[test]
fn gateway_authenticates_before_reading_body_and_rejects_browser_origins() {
    let (gateway, url, key) =
        Gateway::start("secret".into(), None, "http://127.0.0.1:1".into()).unwrap();
    let mut socket = TcpStream::connect(gateway._server.address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(socket,"POST /v1/messages HTTP/1.1\r\nHost: localhost\r\nContent-Length: 67108864\r\nAuthorization: Bearer wrong\r\n\r\n").unwrap();
    let mut result = String::new();
    socket.read_to_string(&mut result).unwrap();
    assert!(result.starts_with("HTTP/1.1 401"));
    let error = ureq::post(&format!("{url}/v1/messages"))
        .set("x-api-key", &key)
        .set("Origin", "https://untrusted.example")
        .send_string("{}")
        .unwrap_err();
    assert!(matches!(error, ureq::Error::Status(401, _)));
}
#[test]
fn gateway_streams_protocol_and_redacts_upstream_credentials() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let capture = captured.clone();
    let upstream=local_http::Server::start(1_048_576,move|request|{capture.lock().unwrap().push(serde_json::from_slice::<Value>(&request.read_body()?).unwrap());request.respond(200,"text/event-stream",b"data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hello\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3}}}\n\n")}).unwrap();
    let (_gateway, url, key) = Gateway::start(
        "secret".into(),
        Some("none".into()),
        format!("http://{}", upstream.address),
    )
    .unwrap();
    let response=ureq::post(&format!("{url}/v1/messages")).set("x-api-key",&key).send_string(&json!({"model":"copilot/gpt-6.1-sol","messages":[{"role":"user","content":"hello"}],"stream":true}).to_string()).unwrap().into_string().unwrap();
    assert!(response.contains("event: message_start"));
    assert!(response.contains("event: message_stop"));
    assert!(response.contains("hello"));
    assert_eq!(captured.lock().unwrap()[0]["reasoning"]["effort"], "none");
    let failing = local_http::Server::start(1024, |req| {
        req.json(403, &json!({"error":{"message":"secret was rejected"}}))
    })
    .unwrap();
    let (_gateway, url, key) =
        Gateway::start("secret".into(), None, format!("http://{}", failing.address)).unwrap();
    let err = ureq::post(&format!("{url}/v1/messages"))
        .set("x-api-key", &key)
        .send_string(r#"{"model":"copilot/gpt-6.1-sol","messages":[]}"#)
        .unwrap_err();
    let ureq::Error::Status(403, response) = err else {
        panic!("unexpected response: {err:?}")
    };
    let body = response.into_string().unwrap();
    assert!(!body.contains("secret"));
    assert!(body.contains("[redacted]"));
}
#[test]
fn dropping_gateway_closes_admitted_sockets_and_its_listener() {
    let (gateway, _, _) =
        Gateway::start("secret".into(), None, "http://127.0.0.1:1".into()).unwrap();
    let address = gateway._server.address;
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    drop(gateway);
    let mut byte = [0];
    match stream.read(&mut byte) {
        Ok(0) => {}
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
            ) => {}
        result => panic!("socket remained open after gateway shutdown: {result:?}"),
    }
}
