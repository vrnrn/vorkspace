use super::*;
use serde_json::json;
use std::fs;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(value: serde_json::Value) -> Self {
        let dir =
            std::env::temp_dir().join(format!("vorkspace-pi-usage-{}", crate::session::new_id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("auth.json"), value.to_string()).unwrap();
        Self(dir)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn auth() -> serde_json::Value {
    json!({"anthropic":{"type":"oauth","access":"pi-anthropic","expires":9999999999999_u64},
            "openai-codex":{"type":"oauth","access":"pi-codex","accountId":"pi-account","expires":9999999999999_u64}})
}
fn ok() -> PiUsageResult {
    PiUsageResult::Ok {
        windows: UsageWindows::default(),
    }
}

#[test]
fn reads_only_the_selected_pi_credential_without_writing() {
    let f = Fixture::new(auth());
    let before = fs::read(f.0.join("auth.json")).unwrap();
    let result = read_usage(&f.0, PiUsageProvider::OpenaiCodex, |credential| {
        assert_eq!(credential.access, "pi-codex");
        assert_eq!(credential.account_id.as_deref(), Some("pi-account"));
        ok()
    });
    assert!(matches!(result, PiUsageResult::Ok { .. }));
    assert_eq!(fs::read(f.0.join("auth.json")).unwrap(), before);
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}

#[test]
fn ineligible_credentials_never_make_a_request() {
    for entry in [
        json!(null),
        json!({"type":"api_key","key":"!do-not-run"}),
        json!({"type":"oauth","access":"expired","expires":1}),
        json!({"type":"oauth","access":"no-expiry"}),
        json!({"type":"oauth","access":"bad\r\nheader","expires":9999999999999_u64}),
    ] {
        let f = Fixture::new(json!({"anthropic":entry}));
        let result = read_usage(&f.0, PiUsageProvider::Anthropic, |_| {
            panic!("must not fetch")
        });
        assert!(!matches!(result, PiUsageResult::Ok { .. }));
    }
}

#[test]
fn custom_directory_missing_auth_does_not_fall_back() {
    let f = Fixture::new(auth());
    fs::remove_file(f.0.join("auth.json")).unwrap();
    let result = read_usage(&f.0, PiUsageProvider::Anthropic, |_| {
        panic!("must not fetch")
    });
    assert!(matches!(result, PiUsageResult::Unavailable { .. }));
}

#[test]
fn changed_credentials_discard_the_response() {
    let f = Fixture::new(auth());
    let result = read_usage(&f.0, PiUsageProvider::Anthropic, |_| {
        let mut replacement = auth();
        replacement["anthropic"]["access"] = json!("different-account");
        fs::write(f.0.join("auth.json"), replacement.to_string()).unwrap();
        ok()
    });
    assert!(matches!(result, PiUsageResult::Error { .. }));
}

#[test]
fn known_provider_auth_overrides_are_not_mistaken_for_oauth() {
    let f = Fixture::new(auth());
    fs::write(
        f.0.join("models.json"),
        r#"{"providers":{"anthropic":{"apiKey":"other-account"}}}"#,
    )
    .unwrap();
    let result = read_usage(&f.0, PiUsageProvider::Anthropic, |_| {
        panic!("must not fetch")
    });
    assert!(matches!(result, PiUsageResult::Unavailable { .. }));
}

#[test]
fn per_model_auth_overrides_are_not_mistaken_for_saved_oauth() {
    let f = Fixture::new(auth());
    fs::write(f.0.join("models.json"), r#"{"providers":{"anthropic":{"modelOverrides":{"claude-sonnet-5":{"headers":{"Authorization":"other-account"}}}}}}"#).unwrap();
    assert!(matches!(
        read_usage(&f.0, PiUsageProvider::Anthropic, |_| panic!(
            "must not fetch"
        )),
        PiUsageResult::Unavailable { .. }
    ));
}

#[test]
fn relative_agent_directories_do_not_read_the_apps_working_directory() {
    let home = std::env::temp_dir().to_string_lossy().into_owned();
    assert_eq!(agent_dir(Some("relative-pi".into()), Some(home)), None);
}

#[test]
fn codex_requires_its_own_account_id() {
    let mut value = auth();
    value["openai-codex"]
        .as_object_mut()
        .unwrap()
        .remove("accountId");
    let f = Fixture::new(value);
    let result = read_usage(&f.0, PiUsageProvider::OpenaiCodex, |_| {
        panic!("must not fetch")
    });
    assert!(!matches!(result, PiUsageResult::Ok { .. }));
}

#[test]
fn parses_anthropic_zero_usage_and_never_returns_raw_identity() {
    let result = parse_usage(
        PiUsageProvider::Anthropic,
        r#"{"five_hour":{"utilization":0,"resets_at":"2026-09-25T13:00:00Z"},"seven_day":{"utilization":10,"resets_at":null},"email":"private@example.com"}"#,
    );
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["status"], "ok");
    assert_eq!(value["windows"]["session"]["usedPercent"], 0.0);
    assert_eq!(value["windows"]["weekly"]["windowMinutes"], 10080);
    assert!(!value.to_string().contains("private@example.com"));
}

#[test]
fn codex_weekly_primary_is_not_labeled_five_hour() {
    let result = parse_usage(
        PiUsageProvider::OpenaiCodex,
        r#"{"rate_limit":{"primary_window":{"used_percent":32,"limit_window_seconds":604800,"reset_at":1790700198},"secondary_window":null}}"#,
    );
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["status"], "ok");
    assert!(value["windows"]["session"].is_null());
    assert_eq!(value["windows"]["weekly"]["usedPercent"], 32.0);
    assert_eq!(value["windows"]["weekly"]["resetsAt"], 1790700198000_u64);
}

#[test]
fn agent_directory_matches_pi_override_semantics() {
    let home = std::env::temp_dir().to_string_lossy().into_owned();
    let home_path = PathBuf::from(&home);
    let custom_dir = home_path.join("custom").join("pi");
    assert!(custom_dir.is_absolute());

    assert_eq!(
        agent_dir(None, Some(home.clone())),
        Some(home_path.join(".pi").join("agent"))
    );
    assert_eq!(
        agent_dir(Some("~/custom/pi".into()), Some(home)),
        Some(custom_dir.clone())
    );
    assert_eq!(
        agent_dir(Some(custom_dir.clone().into_os_string()), None),
        Some(custom_dir)
    );
}

fn serve(status: &str, headers: &str, body: String) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/usage", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            request.push_str(&line);
        }
        let _ = stream.write_all(response.as_bytes());
        request
    });
    (url, worker)
}

#[test]
fn http_request_uses_pi_token_and_codex_account_header() {
    let (url, worker) = serve("200 OK", "", r#"{"rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000,"reset_at":1790700198}}}"#.into());
    let creds = PiOAuth {
        access: "synthetic-pi-token".into(),
        account_id: Some("synthetic-pi-account".into()),
        expires: u64::MAX,
    };
    let result = request_usage(PiUsageProvider::OpenaiCodex, &creds, &url);
    assert!(matches!(result, PiUsageResult::Ok { .. }));
    let request = worker.join().unwrap().to_lowercase();
    assert!(request.contains("authorization: bearer synthetic-pi-token\r\n"));
    assert!(request.contains("chatgpt-account-id: synthetic-pi-account\r\n"));
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("synthetic"));
}

#[test]
fn redirects_never_forward_credentials_and_errors_do_not_forward_bodies() {
    let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let location = format!(
        "Location: http://{}/steal\r\n",
        target.local_addr().unwrap()
    );
    let (url, worker) = serve("302 Found", &location, "private upstream detail".into());
    let creds = PiOAuth {
        access: "synthetic-pi-token".into(),
        account_id: None,
        expires: u64::MAX,
    };
    let result = request_usage(PiUsageProvider::Anthropic, &creds, &url);
    assert!(matches!(result, PiUsageResult::Error { .. }));
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    worker.join().unwrap();
    let (url, worker) = serve("401 Unauthorized", "", "private upstream detail".into());
    let result = request_usage(PiUsageProvider::Anthropic, &creds, &url);
    assert!(matches!(result, PiUsageResult::Unavailable { .. }));
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("private upstream detail"));
    worker.join().unwrap();
}

#[test]
fn oversized_auth_and_responses_are_rejected() {
    let f = Fixture::new(auth());
    fs::write(f.0.join("auth.json"), " ".repeat(MAX_BYTES as usize + 1)).unwrap();
    assert!(matches!(
        read_usage(&f.0, PiUsageProvider::Anthropic, |_| panic!(
            "must not fetch"
        )),
        PiUsageResult::Error { .. }
    ));
    let (url, worker) = serve("200 OK", "", " ".repeat(MAX_BYTES as usize + 1));
    let creds = PiOAuth {
        access: "synthetic".into(),
        account_id: None,
        expires: u64::MAX,
    };
    assert!(matches!(
        request_usage(PiUsageProvider::Anthropic, &creds, &url),
        PiUsageResult::Error { .. }
    ));
    worker.join().unwrap();
}

#[test]
fn malformed_responses_are_not_zero_usage() {
    for body in [
        "{}",
        "not JSON",
        r#"{"five_hour":{"utilization":"30"}}"#,
        r#"{"five_hour":{"utilization":-1}}"#,
        r#"{"five_hour":{"utilization":25,"resets_at":"bad"}}"#,
    ] {
        assert!(matches!(
            parse_usage(PiUsageProvider::Anthropic, body),
            PiUsageResult::Error { .. }
        ));
    }
}
