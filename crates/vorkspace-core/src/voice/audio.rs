//! Minimal WebRTC media surface. It cannot issue workspace actions or spawn a
//! process. The native controller alone exchanges SDP with the Codex helper.
use super::runtime::Event;
use crate::local_http;
use serde_json::{json, Value};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
pub(super) struct Audio {
    _server: local_http::Server,
    pub url: String,
    pub commands: mpsc::SyncSender<Value>,
}
impl Audio {
    pub fn start(config: Value, events: mpsc::SyncSender<Event>) -> Result<Self, String> {
        let token = rand::random::<[u8; 32]>()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let capability = token.clone();
        let (commands, rx) = mpsc::sync_channel::<Value>(16);
        let rx = Arc::new(Mutex::new(rx));
        let server = local_http::Server::start(65536, move |request| {
            let path = request.path.clone();
            let mut path = path.trim_start_matches('/').splitn(2, '/');
            if !bool::from(
                path.next()
                    .unwrap_or_default()
                    .as_bytes()
                    .ct_eq(capability.as_bytes()),
            ) {
                return request.json(401, &json!({"error":"Invalid audio capability"}));
            }
            let address = request.stream.local_addr().map_err(|e| e.to_string())?;
            if request
                .headers
                .get("origin")
                .is_some_and(|origin| origin != &format!("http://{address}"))
            {
                return request.json(403, &json!({"error":"Invalid origin"}));
            }
            match (request.method.as_str(),path.next().unwrap_or_default()){
                ("GET","")=>request.respond(200,"text/html; charset=utf-8",b"<!doctype html><meta charset=utf-8><meta http-equiv=Content-Security-Policy content=\"default-src 'none'; script-src 'self'; connect-src 'self'; media-src blob:;\"><title>Vox audio</title><script src=audio.js></script>"),
                ("GET","audio.js")=>request.respond(200,"text/javascript; charset=utf-8",include_bytes!("../../assets/vox-audio.js")),
                ("GET","config")=>request.json(200,&config),
                ("GET","events")=>{
                    let message=rx.lock().map_err(|_|"Audio channel closed")?.recv_timeout(Duration::from_secs(10)).unwrap_or(Value::Null);request.json(200,&message)
                }
                ("POST","event")=>{
                    let event:Value=serde_json::from_slice(&request.read_body()?).map_err(|_|"Invalid audio event")?;
                    events.try_send(Event::Audio(event)).map_err(|_|"Audio controller is busy")?;request.json(200,&json!({"ok":true}))
                }
                _=>request.json(404,&json!({"error":"Unknown audio route"}))
            }
        })?;
        Ok(Self {
            url: format!("http://{}/{token}/", server.address),
            _server: server,
            commands,
        })
    }
    pub fn send(&self, event: Value) -> Result<(), String> {
        self.commands
            .try_send(event)
            .map_err(|_| "Audio control queue is full".into())
    }
}
impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.commands.try_send(json!({"type":"stop"}));
    }
}
