//! Session-scoped local app access. Provider children inherit only their own
//! capability; all requests are checked again against the Bot's project list.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

pub const HELP: &str = r#"vorkspace app access
Usage: vorkspace-gpui app ACTION --json '{...}' [--request-id ID]
Pass --input FILE or --input - to read a JSON object instead.
Actions:
models.list {}
bots.list {}; bots.read {bot,before?,limit?,maxChars?}; bots.send {to,message,expectReply?}
Peer replies arrive in the sender's conversation after the recipient finishes. End your turn to allow queued replies to run.
sessions.list {project?}
sessions.read {sessionId,project?,before?,limit?,maxChars?}
sessions.start {prompt,project?,harness?,model?,modelSettings?,runtimeMode?,title?,draft?,reveal?,workspaceMode?,worktreeBase?,worktreeCwd?,notifyOnComplete?,placement?,besideSessionId?}
sessions.send {sessionId,prompt,project?,notifyOnComplete?}
sessions.draft {sessionId,prompt,project?}
sessions.stop / sessions.archive / sessions.delete {sessionId,project?}
worktrees.list {project?} reports dirty, merged, unpublishedCommits, sessions and removable for each worktree.
worktrees.create {branch,base?,existing?,project?}
worktrees.remove {path,project?} removes a worktree only if nothing would be lost: it refuses uncommitted or unique work, open terminals and unarchived sessions. A merged branch goes with it.
folders.list {project?}
folders.move {sessionId,folderId?,newFolderName?,project?}
notes.list {limit?,offset?}; notes.read {id}; notes.write {id?,title,body,tags?}
todos.list {project?}; todos.add {title,status?,project?}; todos.update {id,title?,status?,before?,project?}; todos.remove {id,project?}
To-do statuses are todo, inProgress and done. Bots use their own board unless project names an assigned project's board.
minions.hire {name,role,specialties?,project?}; minions.fire {minion}; minions.list {}
minions.retry / minions.discard {minion,messageId}
Minions are persistent team members. Reuse bots.send/bots.read and team-only channels for work; users cannot message Minions.
minions.spawn {prompt,name?,role?,specialties?,project?,harness?,model?} is compatibility hire plus an initial peer task.
artifacts.list {kind?,limit?,offset?}; artifacts.read {id}; artifacts.write {id?,kind,title,body,summary?}
channels.list {}; channels.create {name,topic?,members:[Bot names]}; channels.read {channel,limit?}; channels.post {channel,message,to?:[Bot names]}
soul.read {}; soul.update {text,expectedHash}
memory.read {topic?}; memory.search {query,since?}; memory.add {fact,topic?,until?}; memory.replace {find,fact,topic?,until?}; memory.remove {find,topic?}
habits.list {}; habits.add {name,instructions,schedule:{kind,time?,minute?,dayOfWeek?}}; habits.update {id,name?,instructions?,schedule?,enabled?}; habits.run {id}; habits.remove {id}
chat.card {type:"pr",repo?,number,note?} / {type:"session",sessionId,note?} / {type:"choices",options:[text]} / {type:"habit",name,instructions,schedule}
The CLI returns {ok,result,requestId}. Retry uncertain mutations with the same request ID.
Assigned projects alone may be addressed. Credentials are provided in the environment; never print them.
"#;
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
    stop: Arc<AtomicBool>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
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
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = async_channel::bounded(32);
        let owner = stop.clone();
        let key = secret.clone();
        std::thread::spawn(move || {
            while !owner.load(Ordering::Relaxed) && !tx.is_closed() {
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
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok((
            Self {
                endpoint,
                secret,
                stop,
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
        return Ok(json!({"help":HELP}));
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
                let text = if path == "-" {
                    let mut text = String::new();
                    std::io::stdin()
                        .take(1_048_577)
                        .read_to_string(&mut text)
                        .map_err(|e| e.to_string())?;
                    text
                } else {
                    std::fs::read_to_string(path).map_err(|e| e.to_string())?
                };
                if text.len() > 1_048_576 {
                    return Err("Input too large".into());
                }
                input = serde_json::from_str(&text).map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod capability_tests {
    use super::*;
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
        // Longer than the accept poll, so the server reads before the request.
        let request = send_after(&server, 7, credential, Duration::from_millis(100));
        let call = dispatched(&rx);
        call.reply.send(Ok(json!({"models":[]}))).unwrap();
        assert_eq!(request.join().unwrap().unwrap()["ok"], true);
    }
}
