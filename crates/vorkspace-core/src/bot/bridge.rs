//! Session-scoped local app access. Provider children inherit only their own
//! capability; all requests are checked again against the Bot's project list.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

// One action per line lets the CLI and a Bot's scoped reference share the
// same signatures. Keep explanatory help separate from the action catalog.
const ACTIONS: &str = r#"models.list {}
bots.list {}
bots.read {bot,before?,limit?,maxChars?}
bots.historyStatus {bot}
bots.recoverHistory {bot,expectedHistory,confirmStopped}
bots.send {to,message,expectReply?}
bots.configure {compactionMode}
sessions.list {project?}
sessions.read {sessionId,project?,before?,limit?,maxChars?}
sessions.start {prompt,project?,harness?,model?,modelSettings?,runtimeMode?,title?,draft?,reveal?,workspaceMode?,worktreeBase?,worktreeCwd?,notifyOnComplete?,placement?,besideSessionId?}
sessions.send {sessionId,prompt,project?,notifyOnComplete?}
sessions.draft {sessionId,prompt,project?}
sessions.stop {sessionId,project?}
sessions.archive {sessionId,project?}
sessions.delete {sessionId,project?}
workspace.list {project?}
workspace.open {kind,sessionId?,noteId?,provider?,itemKey?,bot?,document?,path?,placement?,targetPaneId?,project?}
workspace.focus {paneId,project?}
workspace.close {paneId,confirmRunning?,expectedProcess?,project?}
workspace.move {paneId?,tabId?,targetPaneId,edge,project?}
workspace.detach {paneId,index?,project?}
workspace.reorder {tabId,index,project?}
workspace.resize {tabId,sashPath,index,ratio,project?}
worktrees.list {project?}
worktrees.create {branch,base?,existing?,project?}
worktrees.remove {path,project?}
folders.list {project?}
folders.move {sessionId,folderId?,newFolderName?,project?}
codebase.configure {enabled,project?,worktreeCwd?}
codebase.status {project?,worktreeCwd?}
codebase.index {project?,worktreeCwd?}
codebase.tools {project?}
codebase.query {tool,arguments?,project?,worktreeCwd?}
notes.list {project?,limit?,offset?}
notes.read {id?,noteId?,project?}
notes.write {id?,noteId?,title,body,tags?,project?}
todos.list {project?}
todos.add {title,description?,attachments?,dependencies?,status?,github?,project?}
todos.update {id,title?,description?,attachments?,dependencies?,status?,before?,project?}
todos.remove {id,project?}
todos.publish {id,project?}
todos.reconcile {id,project?}
todos.retry {id,confirmNoIssue,project?}
todos.reconcileEdit {id,project?}
todos.retryEdit {id,confirmNoEffect,project?}
todos.discardEdit {id,confirmChecked,project?}
todos.cancelPublish {id,confirmNoIssue,project?}
minions.hire {name,role,specialties?,project?}
minions.fire {minion}
minions.list {}
minions.retry {minion,messageId}
minions.discard {minion,messageId}
minions.spawn {prompt,name?,role?,specialties?,project?,harness?,model?}
artifacts.list {kind?,limit?,offset?}
artifacts.read {id}
artifacts.write {id?,kind,title,body,summary?}
soul.read {}
soul.update {text,expectedHash}
memory.read {topic?}
memory.search {query,since?}
memory.add {fact,topic?,until?}
memory.replace {find,fact,topic?,until?}
memory.remove {find,topic?}
habits.list {}
habits.add {name,instructions,schedule:{kind,time?,minute?,dayOfWeek?}}
habits.update {id,name?,instructions?,schedule?,enabled?}
habits.run {id}
habits.remove {id}
chat.card {type:"pr",repo?,number,note?} / {type:"session",sessionId,note?} / {type:"choices",options:[text]} / {type:"habit",name,instructions,schedule}
"#;

const COMMAND_NOTES: &str = "The CLI returns {ok,result,requestId}. Retry uncertain mutations with the same request ID. Assigned projects alone may be addressed. Credentials are provided in the environment; never print them.";

fn help() -> String {
    format!(
        "vorkspace app access\nUsage: vorkspace-gpui app ACTION --json '{{...}}' [--request-id ID]\nPass --input FILE or --input - to read a JSON object instead.\nActions:\n{ACTIONS}\n\
         Peer replies arrive in the sender's conversation after the recipient finishes. End your turn to allow queued replies to run.\n\
         bots.configure changes only the calling Bot's context policy for subsequent turns. compactionMode is sessionRollover (the default fresh session at 80%) or native (provider compaction). Minions cannot change this setting.\n\
         bots.historyStatus checks an accessible peer Bot's native transcript. After the user confirms its external CLI has stopped, bots.recoverHistory accepts the returned expectedHistory and confirmStopped:true to release that unfinished turn. It preserves the session and never resends input. New native writes invalidate the token. Minions cannot recover another Bot.\n\
         worktrees.list reports dirty, merged, unpublishedCommits, sessions and removable for each worktree. worktrees.remove refuses uncommitted or unique work, ignored files tools can't regenerate, open terminals and unarchived sessions. A merged branch goes with it.\n\
         Workspace pane IDs from workspace.list are valid only in the caller's project. Open kinds: session, notes, note, connector, item, bot, botDocument, terminal, search, todos, automations, file. Split with placement right/down and targetPaneId; move one pane with paneId or an entire tab layout with tabId, plus targetPaneId and edge left/right/top/bottom. Reorder a standalone tab with tabId and index. Resize uses a listed sashPath/index and normalized ratio. Closing a running terminal first returns a confirmation-required error; repeat with a new request ID, confirmRunning:true and the listed expectedProcess. Notes list returns both legacy numeric id and native string noteId; use either for notes.read/write, but use noteId to target an exact native note.\n\
         To-do statuses are todo, inProgress, inReview and done. Bots use an assigned project board; include project when several are assigned. Local to-dos always work. A GitHub project also shows its issues; add with github:true or publish a local item explicitly. An uncertain GitHub create is never retried automatically: reconcile first, then retry only after checking GitHub and setting confirmNoIssue:true.\n\
         Minions are persistent team members. Reuse bots.send/bots.read for work; users cannot message Minions. minions.spawn is compatibility hire plus an initial peer task.\n\
         {COMMAND_NOTES}\n"
    )
}

pub(super) fn command_reference(minion: bool, minions_enabled: bool) -> String {
    let mut reference = String::from(
        "Use this reference directly. A question mark marks an optional JSON field. Pass --input FILE or --input - for larger JSON.\nAvailable app actions:\n",
    );
    for line in ACTIONS.lines() {
        let action = line.split_once(' ').expect("action followed by fields").0;
        if minion && !super::api::minion_allowed(action)
            || !minions_enabled && action.starts_with("minions.")
        {
            continue;
        }
        reference.push_str(line);
        reference.push('\n');
    }
    reference.push_str(COMMAND_NOTES);
    reference.push_str("\nFor a requested copy of your saved files, soul.read {} returns SOUL.md in result.text. memory.read {} returns MEMORY.md in result.text. Use memory.read with each listed topic name for memory/<topic>.md, and with topic archive for memory/archive.md. A null text means the file is absent.");
    reference
}

#[derive(Serialize, Deserialize)]
struct Wire {
    session: i64,
    token: String,
    request_id: String,
    action: String,
    input: Value,
}
pub struct Call {
    pub session: i64,
    pub request_id: String,
    pub action: String,
    pub input: Value,
    pub reply: mpsc::Sender<Result<Value, String>>,
}
pub struct Server {
    endpoint: String,
    secret: Arc<String>,
    // Closing this end wakes the listener even when no client connects.
    _shutdown: UnixStream,
}
impl Server {
    pub fn start() -> Result<(Self, async_channel::Receiver<Call>), String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let endpoint = listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .to_string();
        let secret = Arc::new(format!("{:032x}", rand::random::<u128>()) + &super::new_id());
        let (shutdown, wake) = UnixStream::pair().map_err(|e| e.to_string())?;
        let connections = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = async_channel::bounded(32);
        let key = secret.clone();
        std::thread::spawn(move || {
            while !tx.is_closed() && wait_for_client(&listener, &wake) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if connections.fetch_add(1, Ordering::Relaxed) >= 16 {
                            connections.fetch_sub(1, Ordering::Relaxed);
                            continue;
                        }
                        let tx = tx.clone();
                        let key = key.clone();
                        let connections = connections.clone();
                        std::thread::spawn(move || {
                            let _ = serve(stream, &key, &tx);
                            connections.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        continue
                    }
                    Err(_) => break,
                }
            }
        });
        Ok((
            Self {
                endpoint,
                secret,
                _shutdown: shutdown,
            },
            rx,
        ))
    }
    pub fn environment(&self, session: i64) -> HashMap<String, String> {
        HashMap::from([
            ("VORKSPACE_APP_ENDPOINT".into(), self.endpoint.clone()),
            ("VORKSPACE_APP_TOKEN".into(), token(&self.secret, session)),
            ("VORKSPACE_APP_SESSION".into(), session.to_string()),
        ])
    }
}
fn wait_for_client(listener: &TcpListener, wake: &UnixStream) -> bool {
    let mut descriptors = [
        libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: wake.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        // Both descriptors remain open for this thread. A closed shutdown
        // peer takes precedence over a simultaneously queued connection.
        let result = unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, -1) };
        if result >= 0 {
            return descriptors[1].revents == 0 && descriptors[0].revents & libc::POLLIN != 0;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

fn token(secret: &str, session: i64) -> String {
    Sha256::digest(format!("{secret}:{session}").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn serve(
    mut stream: TcpStream,
    secret: &str,
    tx: &async_channel::Sender<Call>,
) -> Result<(), String> {
    // On macOS an accepted socket inherits the listener's non-blocking mode,
    // which would turn a client's first write arriving after accept into a
    // WouldBlock read and drop the connection.
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new((&stream).take(1_048_577));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.len() > 1_048_576 || !line.ends_with('\n') {
        return Err("Invalid app request".into());
    }
    let wire: Wire = serde_json::from_str(&line).map_err(|_| "Invalid app request")?;
    let expected = token(secret, wire.session);
    let equal = wire.token.len() == expected.len()
        && wire
            .token
            .bytes()
            .zip(expected.bytes())
            .fold(0, |diff, (a, b)| diff | a ^ b)
            == 0;
    if !equal
        || !wire.input.is_object()
        || wire.request_id.len() > 128
        || wire.request_id.is_empty()
    {
        return Err("Invalid app capability".into());
    }
    let (reply, result) = mpsc::channel();
    let request_id = wire.request_id;
    tx.send_blocking(Call {
        session: wire.session,
        request_id: request_id.clone(),
        action: wire.action,
        input: wire.input,
        reply,
    })
    .map_err(|_| "App closed")?;
    let response = match result.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(value)) => json!({"ok":true,"result":value,"requestId":request_id}),
        Ok(Err(error)) => json!({"ok":false,"error":error,"requestId":request_id}),
        Err(_) => {
            json!({"ok":false,"error":"App request timed out; retry using the same request ID","requestId":request_id})
        }
    };
    writeln!(stream, "{response}").map_err(|e| e.to_string())
}
/// Runs before any GUI initialization, so a provider tool never opens a window.
pub fn client(args: &[String]) -> Result<Value, String> {
    if args
        .first()
        .is_none_or(|s| matches!(s.as_str(), "help" | "--help" | "-h"))
    {
        return Ok(json!({"help":help()}));
    }
    let action = args[0].clone();
    let mut input = json!({});
    let mut request_id = super::new_id();
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => {
                input = serde_json::from_str(args.next().ok_or("--json needs JSON")?)
                    .map_err(|e| e.to_string())?
            }
            "--input" => {
                let path = args.next().ok_or("--input needs a path")?;
                input = if path == "-" {
                    read_input(std::io::stdin())?
                } else {
                    read_input(std::fs::File::open(path).map_err(|e| e.to_string())?)?
                };
            }
            "--request-id" => request_id = args.next().ok_or("--request-id needs an ID")?.clone(),
            json if json.starts_with('{') => {
                input = serde_json::from_str(json).map_err(|e| e.to_string())?
            }
            _ => return Err(format!("Unknown app argument: {arg}")),
        }
    }
    let endpoint: SocketAddr = std::env::var("VORKSPACE_APP_ENDPOINT")
        .map_err(|_| "App access is unavailable outside a Bot's provider process")?
        .parse()
        .map_err(|_| "Invalid app endpoint")?;
    if !endpoint.ip().is_loopback() {
        return Err("App access requires a local endpoint".into());
    }
    let token = std::env::var("VORKSPACE_APP_TOKEN").map_err(|_| "App capability missing")?;
    let session = std::env::var("VORKSPACE_APP_SESSION")
        .map_err(|_| "App session missing")?
        .parse()
        .map_err(|_| "Invalid app session")?;
    let wire = Wire {
        session,
        token,
        request_id,
        action,
        input,
    };
    let mut stream =
        TcpStream::connect_timeout(&endpoint, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(65)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&wire).map_err(|e| e.to_string())?
    )
    .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream.take(4_194_305))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if line.len() > 4_194_304 || line.is_empty() {
        return Err("App response unavailable".into());
    }
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

fn read_input(input: impl Read) -> Result<Value, String> {
    let mut bytes = Vec::new();
    input
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1_048_576 {
        return Err("Input too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn bot_reference_and_offline_help_share_valid_actions() {
        let result = client(&["help".into()]).unwrap();
        let help = result["help"].as_str().unwrap();
        let bot = command_reference(false, true);
        let minion = command_reference(true, true);
        let disabled = command_reference(false, false);
        let mut seen = std::collections::HashSet::new();
        for line in ACTIONS.lines() {
            let (action, _) = line.split_once(' ').unwrap();
            assert!(seen.insert(action), "{action} is duplicated");
            let schema = super::super::api::validate(action, &json!({}));
            assert!(
                schema
                    .as_ref()
                    .err()
                    .is_none_or(|error| !error.starts_with("Unknown app action")),
                "{action}: {schema:?}"
            );
            assert!(help.lines().any(|candidate| candidate == line), "{action}");
            assert!(bot.lines().any(|candidate| candidate == line), "{action}");
            assert_eq!(
                minion.lines().any(|candidate| candidate == line),
                super::super::api::minion_allowed(action),
                "{action}"
            );
            assert_eq!(
                disabled.lines().any(|candidate| candidate == line),
                !action.starts_with("minions."),
                "{action}"
            );
        }
    }

    fn send(
        server: &Server,
        session: i64,
        credential: String,
    ) -> std::thread::JoinHandle<Option<Value>> {
        send_after(server, session, credential, Duration::ZERO)
    }
    fn send_after(
        server: &Server,
        session: i64,
        credential: String,
        delay: Duration,
    ) -> std::thread::JoinHandle<Option<Value>> {
        let endpoint = server.endpoint.clone();
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(endpoint).unwrap();
            std::thread::sleep(delay);
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let wire = Wire {
                session,
                token: credential,
                request_id: "read-1".into(),
                action: "models.list".into(),
                input: json!({}),
            };
            writeln!(stream, "{}", serde_json::to_string(&wire).unwrap()).unwrap();
            let mut result = String::new();
            let _ = BufReader::new(stream).read_line(&mut result);
            serde_json::from_str(&result).ok()
        })
    }
    fn dispatched(rx: &async_channel::Receiver<Call>) -> Call {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(call) = rx.try_recv() {
                break call;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "authenticated request was not dispatched"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn capability_is_bound_to_its_session_and_only_authenticated_calls_reach_the_app() {
        let (server, rx) = Server::start().unwrap();
        let credential = server.environment(7)["VORKSPACE_APP_TOKEN"].clone();
        assert_ne!(credential, server.environment(8)["VORKSPACE_APP_TOKEN"]);
        assert!(send(&server, 8, credential.clone())
            .join()
            .unwrap()
            .is_none());
        assert!(rx.try_recv().is_err());
        let request = send(&server, 7, credential);
        let call = dispatched(&rx);
        assert_eq!(call.session, 7);
        call.reply.send(Ok(json!({"models":[]}))).unwrap();
        assert_eq!(request.join().unwrap().unwrap()["ok"], true);
    }
    #[test]
    fn a_client_that_writes_after_it_is_accepted_is_still_served() {
        let (server, rx) = Server::start().unwrap();
        let credential = server.environment(7)["VORKSPACE_APP_TOKEN"].clone();
        // Leave the accepted connection idle before writing its request.
        let request = send_after(&server, 7, credential, Duration::from_millis(100));
        let call = dispatched(&rx);
        call.reply.send(Ok(json!({"models":[]}))).unwrap();
        assert_eq!(request.join().unwrap().unwrap()["ok"], true);
    }
}
