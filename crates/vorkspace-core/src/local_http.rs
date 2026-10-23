//! Bounded, process-owned loopback HTTP. Every application endpoint authenticates
//! its own capability; this transport owns admission, limits and socket shutdown.
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    reader: BufReader<TcpStream>,
    length: usize,
    pub stream: TcpStream,
    pub stopped: Arc<AtomicBool>,
    started: bool,
}
impl Request {
    pub fn has_started(&self) -> bool {
        self.started
    }
    pub fn read_body(&mut self) -> Result<Vec<u8>, String> {
        if self.stopped.load(Ordering::Acquire) {
            return Err("Stopped".into());
        }
        let mut body = vec![0; self.length];
        self.reader
            .read_exact(&mut body)
            .map_err(|e| e.to_string())?;
        self.length = 0;
        if self.stopped.load(Ordering::Acquire) {
            return Err("Stopped".into());
        }
        Ok(body)
    }
    pub fn start(&mut self, status: u16, content_type: &str) -> Result<(), String> {
        self.headers(status, content_type, None)
    }
    fn headers(
        &mut self,
        status: u16,
        content_type: &str,
        length: Option<usize>,
    ) -> Result<(), String> {
        if self.started {
            return Err("Response already started".into());
        }
        self.started = true;
        let length = length
            .map(|n| format!("Content-Length: {n}\r\n"))
            .unwrap_or_default();
        write!(self.stream, "HTTP/1.1 {status} Response\r\nContent-Type: {content_type}\r\n{length}Connection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n").map_err(|e| e.to_string())
    }
    pub fn respond(&mut self, status: u16, content_type: &str, body: &[u8]) -> Result<(), String> {
        self.headers(status, content_type, Some(body.len()))?;
        self.write(body)
    }
    pub fn json(&mut self, status: u16, body: &serde_json::Value) -> Result<(), String> {
        self.respond(status, "application/json", body.to_string().as_bytes())
    }
    fn finish(&mut self) {
        // An early rejection can race with the client's body write. Closing a
        // socket with unread inbound bytes sends RST and can erase the response.
        // Publish EOF first, then drain bounded chunks for a bounded interval;
        // authentication still precedes body allocation and application work.
        let _ = self.stream.shutdown(Shutdown::Write);
        if self.length == 0 {
            return;
        }
        let _ = self
            .stream
            .set_read_timeout(Some(Duration::from_millis(50)));
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut buffer = [0; 8192];
        while self.length > 0 && Instant::now() < deadline && !self.stopped.load(Ordering::Acquire)
        {
            let count = self.length.min(buffer.len());
            match self.reader.read(&mut buffer[..count]) {
                Ok(0) | Err(_) => break,
                Ok(n) => self.length -= n,
            }
        }
    }
    pub fn write(&mut self, body: &[u8]) -> Result<(), String> {
        if self.stopped.load(Ordering::Acquire) {
            return Err("Stopped".into());
        }
        self.stream
            .write_all(body)
            .and_then(|_| self.stream.flush())
            .map_err(|e| e.to_string())
    }
}

pub(crate) struct Server {
    pub address: SocketAddr,
    stopped: Arc<AtomicBool>,
    sockets: Arc<Mutex<HashMap<u64, TcpStream>>>,
}
impl Server {
    pub fn start(
        max_body: usize,
        handler: impl Fn(&mut Request) -> Result<(), String> + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let stopped = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(HashMap::<u64, TcpStream>::new()));
        let stop = stopped.clone();
        let active = sockets.clone();
        let handler = Arc::new(handler);
        std::thread::Builder::new()
            .name("local-http".into())
            .spawn(move || {
                let serial = AtomicU64::new(1);
                for stream in listener.incoming() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { break };
                    let id = serial.fetch_add(1, Ordering::Relaxed);
                    let Ok(socket) = stream.try_clone() else {
                        continue;
                    };
                    {
                        let Ok(mut slots) = active.lock() else { break };
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        if slots.len() >= 8 {
                            continue;
                        }
                        slots.insert(id, socket);
                    }
                    let active = active.clone();
                    let stop = stop.clone();
                    let handler = handler.clone();
                    std::thread::spawn(move || {
                        if let Ok(mut request) = read(stream, max_body, stop) {
                            let result = if request.stopped.load(Ordering::Acquire) {
                                Err("Stopped".into())
                            } else {
                                handler(&mut request)
                            };
                            if let Err(error) = result {
                                if !request.started {
                                    let _ = request.json(400, &serde_json::json!({"error":error}));
                                }
                            }
                            request.finish();
                        }
                        if let Ok(mut slots) = active.lock() {
                            slots.remove(&id);
                        }
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            address,
            stopped,
            sockets,
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Ok(sockets) = self.sockets.lock() {
            for socket in sockets.values() {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        // Wake the blocking accept; no idle polling or retained listener.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
    }
}
fn read(stream: TcpStream, max_body: usize, stopped: Arc<AtomicBool>) -> Result<Request, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut bytes = Vec::new();
    loop {
        let before = bytes.len();
        reader
            .by_ref()
            .take((16_385 - before) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 16_384 || bytes.len() == before || !bytes.ends_with(b"\n") {
            return Err("Invalid HTTP headers".into());
        }
        if &bytes[before..] == b"\r\n" {
            break;
        }
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "Invalid HTTP headers")?;
    let mut lines = text.split("\r\n");
    let parts: Vec<_> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if parts.len() != 3 || !matches!(parts[2], "HTTP/1.0" | "HTTP/1.1") {
        return Err("Invalid HTTP request".into());
    }
    let mut headers = HashMap::new();
    for line in lines.filter(|s| !s.is_empty()) {
        let (key, value) = line.split_once(':').ok_or("Invalid HTTP header")?;
        if headers
            .insert(key.to_ascii_lowercase(), value.trim().to_string())
            .is_some()
        {
            return Err("Duplicate HTTP header".into());
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err("Content-Length is required".into());
    }
    let length = headers
        .get("content-length")
        .map(|n| n.parse::<usize>())
        .transpose()
        .map_err(|_| "Invalid content length")?
        .unwrap_or(0);
    if length > max_body {
        return Err("Request is too large".into());
    }

    Ok(Request {
        method: parts[0].into(),
        path: parts[1].into(),
        headers,
        reader,
        length,
        stream,
        stopped,
        started: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn early_rejection_survives_a_body_arriving_before_connection_close() {
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let server = Server::start(8192, move |request| {
            request.respond(403, "text/plain", b"denied")?;
            // Hold the handler until the client has read our rejection and
            // written its body. The reader cannot prefetch that body in headers.
            released.lock().unwrap().recv().unwrap();
            Ok(())
        })
        .unwrap();
        let mut client = TcpStream::connect(server.address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8192\r\n\r\n")
            .unwrap();
        let mut reply = BufReader::new(client.try_clone().unwrap());
        let mut status = String::new();
        reply.read_line(&mut status).unwrap();
        assert!(status.starts_with("HTTP/1.1 403"));
        loop {
            let mut line = String::new();
            reply.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            assert!(!line.is_empty());
        }
        let mut body = [0; 6];
        reply.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"denied");
        client.write_all(&[b'x'; 8192]).unwrap();
        release.send(()).unwrap();
        let mut tail = Vec::new();
        reply
            .read_to_end(&mut tail)
            .expect("early rejection must end with EOF, not a connection reset");
        assert!(tail.is_empty());
    }
}
