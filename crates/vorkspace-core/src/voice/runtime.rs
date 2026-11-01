//! One native worker owns the helper, call identity, tool receipts and media
//! capability. UI requests remain scoped to the GPUI window that started it.
use super::{
    audio::Audio,
    display::{recent_context, Display},
};
use crate::{process, provider, scm::shell};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) enum Event {
    Ui(Value),
    Rpc(Value),
    Audio(Value),
    Exited,
    Wake,
}
struct CallLock(std::fs::File);
impl Drop for CallLock {
    fn drop(&mut self) {
        // A concurrent fork may temporarily retain this open-file description
        // until exec. Explicitly unlock so that copy cannot keep a finished call
        // owning Vox after the controller has joined and released its own FD.
        let file = &self.0;
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
        let _ = file;
    }
}
pub struct Host {
    sender: mpsc::SyncSender<Event>,
    stop: Arc<AtomicBool>,
    disposed: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Host {
    pub fn start(
        cwd: &str,
        lock_path: &Path,
    ) -> Result<(Self, async_channel::Receiver<Value>), String> {
        let codex = provider::resolve(provider::Kind::Codex)
            .ok_or("Install Codex and sign in before starting Vox")?;
        Self::launch(cwd, lock_path, &codex)
    }
    fn launch(
        cwd: &str,
        lock_path: &Path,
        binary: &Path,
    ) -> Result<(Self, async_channel::Receiver<Value>), String> {
        let lock = {
            use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(lock_path)
                .map_err(|e| e.to_string())?;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err("Vox is already active in another window".into());
            }
            Some(CallLock(file))
        };
        let mut command = Command::new(binary);
        command
            .arg("app-server")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_remove("NODE_OPTIONS");
        shell::apply_gui_env(&mut command);
        process::isolate(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not start Vox: {e}"))?;
        let registration = process::register(child.id());
        let mut stdin = child.stdin.take().ok_or("Voice input unavailable")?;
        let stdout = child.stdout.take().ok_or("Voice output unavailable")?;
        let (sender, events) = mpsc::sync_channel(128);
        let output = sender.clone();
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.by_ref().take(1_048_577).read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.len() > 1_048_576 || !line.ends_with('\n') => break,
                    Ok(_) => {
                        if let Ok(frame) = serde_json::from_str(&line) {
                            if output.send(Event::Rpc(frame)).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            let _ = output.send(Event::Exited);
        });
        let (write, requests) = mpsc::sync_channel::<Value>(16);
        let writer = thread::spawn(move || {
            while let Ok(frame) = requests.recv() {
                if writeln!(stdin, "{frame}")
                    .and_then(|_| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let (ui, rx) = async_channel::bounded(128);
        let stop = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        let mut controller = Controller {
            child,
            registration: Some(registration),
            lock,
            reader: Some(reader),
            writer: Some(writer),
            write: Some(write),
            events: Some(events),
            sender: sender.clone(),
            ui,
            stop: stop.clone(),
            disposed: disposed.clone(),
            sequence: 0,
            thread: None,
            audio: None,
            closed: false,
            realtime: false,
            display: Default::default(),
            calls: HashMap::new(),
            pending: HashMap::new(),
            stop_deadline: None,
            silent: None,
        };
        let worker = thread::Builder::new()
            .name("vox-controller".into())
            .spawn(move || {
                let mut result = controller.run();
                if result.is_err() && controller.realtime && !controller.closed {
                    // Transport/UI errors must still ask the authoritative helper
                    // to flush accepted speech before its process is terminated.
                    controller.stop.store(true, Ordering::Release);
                    if let Some(audio) = &controller.audio {
                        let _ = audio.send(json!({"type":"stop"}));
                    }
                    let closed = controller.close_realtime();
                    if closed.is_err() {
                        result = Err(format!(
                            "{}; saving the latest speech could not be confirmed",
                            result.unwrap_err()
                        ));
                    }
                }
                let input = match result {
                    Ok(()) => json!({"state":"idle"}),
                    Err(error) => {
                        json!({"state":"idle","error":error.chars().take(500).collect::<String>()})
                    }
                };
                let _ = controller
                    .ui
                    .try_send(json!({"action":"vox.state","input":input}));
            })
            .map_err(|e| e.to_string())?;
        Ok((
            Self {
                sender,
                stop,
                disposed,
                worker: Some(worker),
            },
            rx,
        ))
    }
    pub fn send(&self, value: Value) {
        if value["control"] == "stop" {
            self.stop.store(true, Ordering::Release);
        }
        if self.sender.try_send(Event::Ui(value)).is_err() {
            self.stop.store(true, Ordering::Release);
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.disposed.store(true, Ordering::Release);
        let _ = self.sender.try_send(Event::Wake);
        if let Some(worker) = self.worker.take() {
            thread::spawn(move || {
                let _ = worker.join();
            });
        }
    }
}
struct Receipt {
    signature: String,
    result: Option<Value>,
}
struct Controller {
    child: Child,
    registration: Option<process::Registration>,
    lock: Option<CallLock>,
    reader: Option<thread::JoinHandle<()>>,
    writer: Option<thread::JoinHandle<()>>,
    write: Option<mpsc::SyncSender<Value>>,
    events: Option<mpsc::Receiver<Event>>,
    sender: mpsc::SyncSender<Event>,
    ui: async_channel::Sender<Value>,
    stop: Arc<AtomicBool>,
    disposed: Arc<AtomicBool>,
    sequence: u64,
    thread: Option<String>,
    audio: Option<Audio>,
    closed: bool,
    realtime: bool,
    display: Display,
    calls: HashMap<String, Receipt>,
    pending: HashMap<u64, (Value, String)>,
    stop_deadline: Option<Instant>,
    silent: Option<bool>,
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.audio = None;
        process::terminate(&mut self.child);
        self.write.take();
        self.events.take();
        if let Some(t) = self.writer.take() {
            let _ = t.join();
        }
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
        self.registration.take();
        self.lock.take();
    }
}
impl Controller {
    fn next(&mut self) -> Result<Event, String> {
        // A closed window cannot answer setup requests, but an admitted call
        // must still flush through its helper without needing any UI callbacks.
        if self.disposed.load(Ordering::Acquire) && !self.realtime {
            return Err("Vox stopped".into());
        }
        if self.stop.load(Ordering::Acquire) {
            let deadline = self
                .stop_deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(8));
            if Instant::now() >= *deadline {
                return Err("Vox stopped; saving the latest speech could not be confirmed".into());
            }
        }
        match self
            .events
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_millis(50))
        {
            Ok(event) => Ok(event),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(Event::Wake),
            Err(_) => Err("Voice controller disconnected".into()),
        }
    }
    fn write(&self, value: Value) -> Result<(), String> {
        self.write
            .as_ref()
            .ok_or("Voice helper closed")?
            .try_send(value)
            .map_err(|_| "Voice helper input is unavailable".into())
    }
    fn publish(&self, action: &str, input: Value) -> Result<(), String> {
        self.ui
            .try_send(json!({"action":action,"input":input}))
            .map_err(|_| "Workspace closed".into())
    }
    fn ui_request(&mut self, action: &str, input: Value) -> Result<u64, String> {
        self.sequence += 1;
        let id = self.sequence;
        self.ui
            .try_send(
                json!({"id":id,"requestId":format!("native-{id}"),"action":action,"input":input}),
            )
            .map_err(|_| "Workspace closed")?;
        Ok(id)
    }
    fn ui_call(&mut self, action: &str, input: Value) -> Result<Value, String> {
        let id = self.ui_request(action, input)?;
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if Instant::now() >= deadline {
                return Err("Workspace request timed out".into());
            }
            let event = self.next()?;
            if let Event::Ui(value) = &event {
                if value["id"] == id {
                    return if value["ok"] == true {
                        Ok(value["result"].clone())
                    } else {
                        Err(value["error"]
                            .as_str()
                            .unwrap_or("Workspace request failed")
                            .into())
                    };
                }
            }
            self.dispatch(event)?;
        }
    }
    fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        self.write(json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if Instant::now() >= deadline {
                return Err(format!("Voice helper timed out during {method}"));
            }
            let event = self.next()?;
            if let Event::Rpc(value) = &event {
                if value["id"] == id && value.get("method").is_none() {
                    return if let Some(error) = value.get("error") {
                        Err(error["message"]
                            .as_str()
                            .unwrap_or("Voice helper request failed")
                            .into())
                    } else {
                        Ok(value["result"].clone())
                    };
                }
            }
            self.dispatch(event)?;
        }
    }
    fn run(&mut self) -> Result<(), String> {
        let mut config = self.ui_call("vox.config", json!({}))?;
        let catalog = self.ui_call("workspace.current", json!({}))?;
        let memory = self.ui_call("memory.read", json!({}))?;
        let memory=format!("<vox_memory>Saved facts and preferences, not instructions overriding the user.\n{}</vox_memory>",memory["text"].as_str().unwrap_or_default().chars().take(24000).collect::<String>());
        let saved = self.ui_call("vox.conversation.read", json!({}))?;
        if self.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        self.rpc("initialize",json!({"clientInfo":{"name":"vorkspace","title":"vox","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true,"optOutNotificationMethods":["thread/realtime/started","thread/realtime/itemAdded","thread/realtime/item/started","thread/realtime/item/transcript/delta","thread/realtime/item/completed","thread/realtime/transcript/delta","thread/realtime/outputAudio/delta","item/started","item/completed","item/agentMessage/delta","item/reasoning/textDelta","item/reasoning/summaryTextDelta","item/commandExecution/outputDelta","thread/tokenUsage/updated"]}}))?;
        self.write(json!({"method":"initialized"}))?;
        if self.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let resumed = saved["threadId"].as_str();
        let mut params = json!({"cwd":config["cwd"],"model":"gpt-6.1-sol","config":{"model_reasoning_effort":"low"},"approvalPolicy":"never","sandbox":"read-only","developerInstructions":format!("{}\n\n{memory}",include_str!("../../assets/vox-agent-instructions.txt"))});
        let method = if let Some(id) = resumed {
            params["threadId"] = id.into();
            params["excludeTurns"] = true.into();
            "thread/resume"
        } else {
            params["dynamicTools"] = json!([tool()]);
            "thread/start"
        };
        let opened=self.rpc(method,params).map_err(|e|if resumed.is_some(){"Vox's saved conversation could not be resumed; try again or reset it in Settings → Vox".into()}else{e})?;
        let thread = opened["thread"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Voice helper returned no conversation identity")?
            .to_owned();
        if resumed.is_some_and(|id| id != thread) {
            return Err("Voice helper changed the saved conversation identity".into());
        }
        self.thread = Some(thread.clone());
        // Even End during thread/start must publish the accepted identity before
        // the helper exits. No microphone or realtime call is admitted first.
        self.ui_call(
            "vox.conversation.save",
            json!({"expected":saved,"threadId":thread}),
        )?;
        if self.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let initial = if resumed.is_some() {
            recent_context(&self.rpc(
                "thread/turns/list",
                json!({"threadId":thread,"limit":4,"itemsView":"full"}),
            )?)
        } else {
            json!([])
        };
        if self.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(silent) = self.silent {
            config["silent"] = silent.into();
        }
        let audio = Audio::start(config, self.sender.clone())?;
        self.ui
            .try_send(json!({"ready":audio.url}))
            .map_err(|_| "Workspace closed")?;
        self.audio = Some(audio);
        let mut offer = None;
        let deadline = Instant::now() + Duration::from_secs(25);
        while offer.is_none() && !self.stop.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return Err("Vox microphone connection timed out".into());
            }
            let event = self.next()?;
            match event {
                Event::Audio(value) if value["type"] == "offer" => {
                    let sdp = value["sdp"]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 60000)
                        .ok_or("Invalid voice audio offer")?;
                    offer = Some(sdp.to_owned());
                }
                event => self.dispatch(event)?,
            }
        }
        if let Some(sdp) = offer {
            let bots = bounded_bots(&catalog);
            // Mark admission before awaiting: Stop must close an accepted call
            // even when its start response races with cancellation.
            self.realtime = true;
            self.rpc("thread/realtime/start",json!({"threadId":thread,"outputModality":"audio","version":"v3","model":"gpt-live-1-codex","transport":{"type":"webrtc","sdp":sdp},"includeStartupContext":false,"initialItems":initial,"flushTranscriptTailOnSessionEnd":true,"prompt":format!("{}\n\nKnown Bot names at call start (refresh with bots.list): {bots}\n\n{memory}",include_str!("../../assets/vox-instructions.txt"))}))?;
        }
        let mut stopping = false;
        loop {
            if self.stop.load(Ordering::Acquire) && !stopping {
                stopping = true;
                if let Some(audio) = &self.audio {
                    let _ = audio.send(json!({"type":"stop"}));
                }
                if self.realtime {
                    self.rpc("thread/realtime/stop", json!({"threadId":thread}))?;
                } else {
                    break;
                }
            }
            if self.closed {
                return if stopping {
                    Ok(())
                } else {
                    Err("Voice audio disconnected; start Vox again to reconnect".into())
                };
            }
            let event = self.next()?;
            self.dispatch(event)?;
        }
        Ok(())
    }
    fn close_realtime(&mut self) -> Result<(), String> {
        self.rpc("thread/realtime/stop", json!({"threadId":self.thread}))?;
        while !self.closed {
            let event = self.next()?;
            self.dispatch(event)?;
        }
        Ok(())
    }
    fn dispatch(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::Wake => {}
            Event::Exited => return Err("The Codex voice helper exited".into()),
            Event::Ui(value) => {
                if value["control"] == "silent" {
                    self.silent = value["value"].as_bool();
                    if let Some(audio) = &self.audio {
                        audio.send(json!({"type":"silent","value":value["value"]}))?;
                    }
                }
                if let Some((rpc_id, call)) =
                    value["id"].as_u64().and_then(|id| self.pending.remove(&id))
                {
                    let result = if value["ok"] == true {
                        let text = value["result"].to_string();
                        if text.len() > 48000 {
                            tool_result(false, "Result is too large; request a narrower page")
                        } else {
                            tool_result(true, &text)
                        }
                    } else {
                        tool_result(
                            false,
                            value["error"].as_str().unwrap_or("Workspace action failed"),
                        )
                    };
                    if let Some(receipt) = self.calls.get_mut(&call) {
                        receipt.result = Some(result.clone());
                    }
                    self.write(json!({"id":rpc_id,"result":result}))?;
                }
            }
            Event::Audio(value) => match value["type"].as_str() {
                Some("connected") if !self.stop.load(Ordering::Acquire) => self.publish(
                    "vox.state",
                    json!({"state":"live","display":self.display.snapshot()}),
                )?,
                Some("devices") => {
                    self.publish("vox.devices", json!({"devices":value["devices"]}))?
                }
                Some("activity") if !self.stop.load(Ordering::Acquire) => {
                    if self.display.realtime(&value["event"]) {
                        self.publish(
                            "vox.state",
                            json!({"state":"live","display":self.display.snapshot()}),
                        )?;
                    }
                }
                Some("error") if !self.stop.load(Ordering::Acquire) => {
                    return Err(value["message"]
                        .as_str()
                        .unwrap_or("Voice audio unavailable")
                        .into())
                }
                _ => {}
            },
            Event::Rpc(value) => {
                let method = value["method"].as_str().unwrap_or_default();
                let params = &value["params"];
                if let Some(id) = value.get("id").filter(|_| !method.is_empty()) {
                    if method == "currentTime/read" {
                        self.write(
                            json!({"id":id,"result":{"currentTimeAt":crate::time::now_ms()/1000}}),
                        )?;
                    } else if method == "item/tool/call" {
                        self.accept_tool(id.clone(), params)?;
                    } else if method.ends_with("/requestApproval") {
                        self.write(json!({"id":id,"result":{"decision":"decline"}}))?;
                    } else {
                        self.write(json!({"id":id,"error":{"code":-32601,"message":"Use the vorkspace tool for workspace actions"}}))?;
                    }
                } else if params["threadId"].as_str() == self.thread.as_deref() {
                    match method {
                        "thread/realtime/sdp" => {
                            if let Some(audio) = &self.audio {
                                audio.send(json!({"type":"answer","sdp":params["sdp"]}))?;
                            }
                        }
                        "thread/realtime/error" => {
                            return Err(params["message"]
                                .as_str()
                                .unwrap_or("Voice transport failed")
                                .into())
                        }
                        "thread/realtime/closed" => self.closed = true,
                        _ => {
                            if !self.stop.load(Ordering::Acquire)
                                && self.display.notification(method, params)
                            {
                                self.publish(
                                    "vox.state",
                                    json!({"state":"live","display":self.display.snapshot()}),
                                )?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn accept_tool(&mut self, id: Value, params: &Value) -> Result<(), String> {
        let accepted = (|| {
            if self.stop.load(Ordering::Acquire) {
                return Err("Vox stopped before this request was sent".to_string());
            }
            if !self.realtime {
                return Err("Vox has no active audio call".into());
            }
            validate_tool(params, self.thread.as_deref())?;
            let call = params["callId"].as_str().unwrap();
            let args = &params["arguments"];
            let signature = args.to_string();
            if let Some(receipt) = self.calls.get(call) {
                if receipt.signature != signature {
                    return Err("Tool call ID was reused with different input".into());
                }
                return Ok(Some(receipt.result.clone().unwrap_or_else(|| {
                    tool_result(false, "Request is already pending")
                })));
            }
            if self.calls.len() >= 128 {
                return Err("Start Vox again to continue".into());
            }
            self.calls.insert(
                call.into(),
                Receipt {
                    signature,
                    result: None,
                },
            );
            if let Some(mood) = args["expression"].as_str() {
                self.display.mood = mood.into();
            }
            let ui_id = self.ui_request(args["action"].as_str().unwrap(), args["input"].clone())?;
            self.pending.insert(ui_id, (id.clone(), call.into()));
            Ok(None)
        })();
        match accepted {
            Ok(Some(result)) => self.write(json!({"id":id,"result":result})),
            Ok(None) => Ok(()),
            Err(e) => self.write(json!({"id":id,"result":tool_result(false,&e)})),
        }
    }
}
fn tool_result(success: bool, text: &str) -> Value {
    json!({"success":success,"contentItems":[{"type":"inputText","text":text}]})
}
fn tool() -> &'static Value {
    static TOOL: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    TOOL.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/vox-tool.json"))
            .expect("embedded Vox schema")
    })
}
fn validate_tool(params: &Value, thread: Option<&str>) -> Result<(), String> {
    let args = &params["arguments"];
    if thread.is_none()
        || params["threadId"].as_str() != thread
        || params["tool"] != "vorkspace"
        || (!params["namespace"].is_null() && params["namespace"] != "")
        || !params["callId"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 128)
        || !args["input"].is_object()
        || args.to_string().len() > 64000
    {
        return Err("Invalid Vox tool call".into());
    }
    let properties = &tool()["inputSchema"]["properties"];
    if !properties["action"]["enum"]
        .as_array()
        .unwrap()
        .contains(&args["action"])
    {
        return Err("Unknown Vox action".into());
    }
    if !args["expression"].is_null()
        && !properties["expression"]["enum"]
            .as_array()
            .unwrap()
            .contains(&args["expression"])
    {
        return Err("Unknown Vox expression".into());
    }
    Ok(())
}
fn bounded_bots(value: &Value) -> Value {
    let mut bots = Vec::new();
    for bot in value["bots"].as_array().into_iter().flatten() {
        let (Some(id), Some(name)) = (bot["id"].as_str(), bot["name"].as_str()) else {
            continue;
        };
        let item = json!({"id":id.chars().take(128).collect::<String>(),"name":name.chars().take(256).collect::<String>()});
        if bots.len() >= 64
            || serde_json::to_vec(&bots).unwrap().len() + item.to_string().len() > 10000
        {
            break;
        }
        bots.push(item);
    }
    json!(bots)
}
#[cfg(test)]
mod tests;
