
use super::*;

#[test]
fn stale_requests_cannot_invalidate_a_newer_tunnel() {
    let tunnels = Tunnels::default();
    let slot = |generation| {
        Arc::new(Mutex::new(Slot {
            tunnel: None,
            failure: Some((Instant::now(), "keep".into())),
            generation,
        }))
    };
    let old = slot(1);
    tunnels.slots().insert("host".into(), old.clone());
    let old_lease = TunnelLease {
        endpoint: String::new(),
        slot: old,
        generation: 1,
    };
    let newer = slot(2);
    tunnels.slots().insert("host".into(), newer.clone());
    tunnels.invalidate("host", &old_lease);
    assert!(newer.lock().unwrap().failure.is_some());
    let stale_lease = TunnelLease {
        endpoint: String::new(),
        slot: newer.clone(),
        generation: 1,
    };
    tunnels.invalidate("host", &stale_lease);
    assert!(newer.lock().unwrap().failure.is_some());
    tunnels.invalidate(
        "host",
        &TunnelLease {
            generation: 2,
            ..stale_lease
        },
    );
    assert!(newer.lock().unwrap().failure.is_none());
}
// scripts/checks/test-remote-ssh.py creates an isolated sshd, host and keypair.
// This test uses the production tunnel lifecycle and shell transport.
#[test]
#[ignore = "requires the isolated loopback SSH fixture"]
fn loopback_transport_preserves_host_and_reconnects() {
    let required = |key| std::env::var(key).expect("Run scripts/checks/test-remote-ssh.py");
    let target = SshTarget {
        target: required("VORKSPACE_TEST_SSH_TARGET"),
        port: Some(required("VORKSPACE_TEST_SSH_PORT").parse().unwrap()),
        remote_port: required("VORKSPACE_TEST_HOST_PORT").parse().unwrap(),
    };
    let make_command = || {
        let mut command = command(&target, false);
        command.args([
            "-F",
            "/dev/null",
            "-i",
            &required("VORKSPACE_TEST_SSH_KEY"),
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            &format!(
                "UserKnownHostsFile={}",
                required("VORKSPACE_TEST_KNOWN_HOSTS")
            ),
        ]);
        command
    };
    let job = Job::new();
    let askpass = job.askpass().unwrap();
    let detected = run_remote_command(
        &target,
        String::new(),
        &job,
        &askpass,
        make_command(),
        PLATFORM_PROBE,
    )
    .unwrap();
    parse_platform(&detected).unwrap();
    let output = run_script_with_command(
        &target,
        "printf 'remote-script-ok\\n'\n".into(),
        &job,
        &askpass,
        make_command(),
    )
    .unwrap();
    assert_eq!(output.trim(), "remote-script-ok");
    let environment = required("VORKSPACE_TEST_ENVIRONMENT");
    for _ in 0..2 {
        let tunnel = Tunnel::start_with_command(&target, None, None, make_command()).unwrap();
        let response = ureq::post(&format!("http://127.0.0.1:{}/rpc", tunnel.port))
            .set(
                "Authorization",
                &format!("Bearer {}", required("VORKSPACE_TEST_TOKEN")),
            )
            .send_string(r#"{"version":1,"method":"environment.describe"}"#)
            .unwrap();
        let value: serde_json::Value = serde_json::from_reader(response.into_reader()).unwrap();
        assert_eq!(value["result"]["environmentId"], environment);
        drop(tunnel);
        // A client disappearing must not kill the independently owned host.
        assert!(TcpStream::connect(("127.0.0.1", target.remote_port)).is_ok());
    }
}
#[test]
fn ssh_targets_cannot_inject_options_or_shell_commands() {
    for target in [
        "home",
        "me@mac-mini.local",
        "user@192.168.1.4",
        "user@[::1]",
    ] {
        assert!(validate_target(target, None).is_ok());
    }
    for target in [
        "",
        "-oProxyCommand=bad",
        "host;touch /tmp/x",
        "host\nname",
        "$(whoami)",
        "user@host command",
        "host/../../x",
        "ssh://user@host",
        "a@b@c",
    ] {
        assert!(validate_target(target, None).is_err(), "{target}");
    }
    assert!(validate_target("host", Some(0)).is_err());
    assert_eq!(shell_quote("a'b"), "'a'\\''b'");
}
#[test]
fn unix_bootstrap_accepts_windows_checkout_line_endings() {
    let lf_template = include_str!("../remote_bootstrap.sh").replace("\r\n", "\n");
    let crlf_template = lf_template.replace('\n', "\r\n");
    let script = bootstrap_script_from_template(&crlf_template);
    assert!(script.starts_with("set -eu\n"));
    assert!(!script.contains('\r'));
    assert_eq!(script, bootstrap_script_from_template(&lf_template));
}
#[test]
fn bootstrap_is_versioned_and_only_explicit_upgrade_restarts_the_host() {
    let script = bootstrap_script();
    assert!(!script.contains('\r'));
    assert!(!script.contains("@@"));
    assert!(script.contains("--proto '=https'"));
    assert!(script.contains("checksum mismatch"));
    assert!(script.contains("\"$FORCE_UPGRADE\" = 1"));
    assert!(script.contains("service uninstall"));
    assert!(upgrade_script(3774).starts_with("VORKSPACE_HOST_FORCE_UPGRADE=1"));
    assert!(!upgrade_script(3774).contains('\r'));
}
#[test]
fn remote_platform_probe_refuses_windows_and_accepts_unix() {
    for output in [
        "VORKSPACE_PLATFORM $env:OS Windows_NT $OS\r\n",
        "VORKSPACE_PLATFORM\r\nWindows_NT\r\n%OS%\r\n",
    ] {
        assert!(parse_platform(output)
            .unwrap_err()
            .contains("not supported"));
    }
    parse_platform("VORKSPACE_PLATFORM :OS %OS%\n").unwrap();
    assert!(parse_platform("unrecognized shell").is_err());
}
#[test]
fn device_names_are_bounded_single_lines() {
    let name = device_name();
    assert!(name.starts_with("vorkspace"));
    assert!(name.chars().count() <= 100);
    assert!(!name.chars().any(char::is_control));
}
#[test]
fn answers_must_match_the_current_prompt() {
    let job = Job::new();
    assert!(job.answer("old", "yes".into()).is_err());
    let waiter = job.clone();
    let thread = std::thread::spawn(move || waiter.prompt("Trust this host?".into(), true));
    while job.view().prompt.is_none() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let prompt = job.view().prompt.unwrap();
    assert!(job.answer(&prompt.id, "arbitrary".into()).is_err());
    job.answer(&prompt.id, "yes".into()).unwrap();
    assert_eq!(thread.join().unwrap(), Some("yes".into()));
    assert!(job.answer(&prompt.id, "yes".into()).is_err());
}
