//! The dashboard's HTTP/1.1 server on loopback: each connection has a
//! thread, joined when the connection ends, within [`LIMITS`]. A connection
//! must deliver each whole request, and take its response, within
//! [`IDLE_TIMEOUT`] of its accept or of its previous response, on the
//! lobby's clock; keep-alive connections last that long between requests.
//! Bodies come with a `Content-Length` of at most [`MAX_BODY_BYTES`]; the
//! dashboard page, `fetch` and the client tools send nothing else.

use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

#[cfg(test)]
use lucy_wire::tcp::PeerCount;
use lucy_wire::tcp::{Admission, Limit, Now, PeerLimits, PeerStream, PeerWorkers, accept};
use serde_json::json;

/// The browser opens up to six connections per page; tools add a few.
const LIMITS: PeerLimits = PeerLimits {
    total: 32,
    per_address: 16,
};
/// How long a connection waits for its next request.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The request line and headers.
const MAX_HEAD_BYTES: usize = 16 * 1024;
pub(super) const MAX_BODY_BYTES: usize = 65_536;
const ACCEPT_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Method {
    Get,
    Post,
    Other,
}

pub(super) struct Request {
    pub method: Method,
    pub url: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first header of this name, any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub(super) struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

type Handler = dyn Fn(Request) -> Response + Send + Sync;

pub(super) struct HttpServer {
    addr: SocketAddr,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
    #[cfg(test)]
    peers: PeerCount,
}

impl HttpServer {
    /// Serve `listener` until [`stop`](Self::stop); the timeouts read `clock`.
    pub fn start(
        listener: TcpListener,
        clock: Now,
        handler: impl Fn(Request) -> Response + Send + Sync + 'static,
    ) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let peers = PeerWorkers::new(LIMITS).with_accept_deadline(clock, IDLE_TIMEOUT);
        #[cfg(test)]
        let count = peers.count();
        let handler: Arc<Handler> = Arc::new(handler);
        let (stop, shutdown) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("lucy-dashboard".into())
            .spawn(move || serve(&listener, &handler, &shutdown, peers))?;
        Ok(Self {
            addr,
            stop,
            worker: Some(worker),
            #[cfg(test)]
            peers: count,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The connections that still have a thread.
    #[cfg(test)]
    pub fn peers(&self) -> usize {
        self.peers.get()
    }

    /// Close the listener, then stop and join every connection's thread.
    pub fn stop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.stop.send(());
            let _ = worker.join();
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(
    listener: &TcpListener,
    handler: &Arc<Handler>,
    shutdown: &mpsc::Receiver<()>,
    mut peers: PeerWorkers,
) {
    loop {
        peers.reap();
        match accept(listener) {
            Ok(Some((socket, address))) => {
                let handler = Arc::clone(handler);
                let served = peers.spawn("lucy-dashboard-peer", socket, address, move |stream| {
                    serve_connection(stream, &*handler)
                });
                log_admission(served, address);
                continue;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!("[!] The dashboard stopped accepting: {error}");
                return;
            }
        }
        if !matches!(
            shutdown.recv_timeout(ACCEPT_POLL),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            return;
        }
    }
}

fn log_admission(served: io::Result<Admission>, address: SocketAddr) {
    match served {
        Ok(Admission::Served | Admission::Refused { first: false, .. }) => {}
        Ok(Admission::Refused {
            limit: Limit::Total,
            first: true,
        }) => tracing::warn!(
            "[!] The dashboard serves {} connections, its most; refusing more",
            LIMITS.total
        ),
        Ok(Admission::Refused {
            limit: Limit::Address,
            first: true,
        }) => tracing::warn!(
            "[!] {} dashboard connections from {}, the most from one address; refusing more",
            LIMITS.per_address,
            address.ip()
        ),
        Err(error) => tracing::warn!("[!] Dashboard connection from {address} not served: {error}"),
    }
}

/// Requests and responses until the client closes, a timeout passes, or a
/// request asks to close.
fn serve_connection(mut stream: PeerStream, handler: &Handler) {
    let mut buffer = Vec::new();
    while let Ok(true) = exchange(&mut stream, &mut buffer, handler) {}
}

/// One request and its response. True when the connection stays open.
fn exchange(stream: &mut PeerStream, buffer: &mut Vec<u8>, handler: &Handler) -> io::Result<bool> {
    // The deadline set at the accept or the previous response runs until
    // this response is written.
    let head_end = loop {
        if let Some(end) = buffer.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            break end + 4;
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return refuse(stream, 431, "Request headers too large");
        }
        if !receive(stream, buffer)? {
            return Ok(false);
        }
    };
    let Some((method, url, version_1_0, headers)) = parse_head(&buffer[..head_end]) else {
        return refuse(stream, 400, "Malformed request");
    };
    let mut request = Request {
        method,
        url,
        headers,
        body: Vec::new(),
    };
    if request.header("Transfer-Encoding").is_some() {
        return refuse(stream, 501, "Send the body with a Content-Length");
    }
    let length = match request.header("Content-Length").map(str::parse::<usize>) {
        None => 0,
        Some(Ok(length)) => length,
        Some(Err(_)) => return refuse(stream, 400, "Malformed request"),
    };
    if length > MAX_BODY_BYTES {
        return refuse(stream, 413, "Request too large");
    }
    if buffer.len() < head_end + length
        && request
            .header("Expect")
            .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    while buffer.len() < head_end + length {
        if !receive(stream, buffer)? {
            return Ok(false);
        }
    }
    request.body = buffer[head_end..head_end + length].to_vec();
    buffer.drain(..head_end + length);
    let connection = request.header("Connection").map(str::to_ascii_lowercase);
    let has = |token: &str| {
        connection
            .as_deref()
            .is_some_and(|value| value.split(',').any(|part| part.trim() == token))
    };
    let keep_alive = !has("close") && (!version_1_0 || has("keep-alive"));
    let response = handler(request);
    stream.set_time_limit(Some(IDLE_TIMEOUT));
    write_response(stream, &response, keep_alive)?;
    Ok(keep_alive)
}

/// Read more of a request. False when the client closed.
fn receive(stream: &mut PeerStream, buffer: &mut Vec<u8>) -> io::Result<bool> {
    let mut chunk = [0; 4096];
    let count = stream.read(&mut chunk)?;
    buffer.extend_from_slice(&chunk[..count]);
    Ok(count != 0)
}

/// Method, target, whether the version is 1.0, and the headers.
#[allow(clippy::type_complexity)]
fn parse_head(head: &[u8]) -> Option<(Method, String, bool, Vec<(String, String)>)> {
    let head = std::str::from_utf8(head).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = match request_line.next()? {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => Method::Other,
    };
    let url = request_line.next()?.to_owned();
    let version_1_0 = match request_line.next()? {
        "HTTP/1.0" => true,
        "HTTP/1.1" => false,
        _ => return None,
    };
    if request_line.next().is_some() {
        return None;
    }
    let headers = lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect::<Option<_>>()?;
    Some((method, url, version_1_0, headers))
}

/// Answer a request the server cannot take, as the API answers errors, and
/// close.
fn refuse(stream: &mut PeerStream, status: u16, message: &str) -> io::Result<bool> {
    let response = Response {
        status,
        headers: vec![("Content-Type", "application/json; charset=utf-8".to_owned())],
        body: json!({ "error": message }).to_string().into_bytes(),
    };
    write_response(stream, &response, false)?;
    Ok(false)
}

fn write_response(
    stream: &mut PeerStream,
    response: &Response,
    keep_alive: bool,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: {}\r\n\r\n",
        response.body.len(),
        if keep_alive { "keep-alive" } else { "close" }
    ));
    let mut message = head.into_bytes();
    message.extend_from_slice(&response.body);
    stream.write_all(&message)?;
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::{Ipv4Addr, TcpStream},
        sync::Mutex,
        time::Instant,
    };

    /// Waits in real time for the server's threads; fails after 10 s.
    fn settle(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn echo() -> impl Fn(Request) -> Response + Send + Sync + 'static {
        |request| Response {
            status: 200,
            headers: vec![("Content-Type", "text/plain".to_owned())],
            body: [request.url.as_bytes(), b" ", &request.body].concat(),
        }
    }

    /// Reads one response: status and body.
    fn response(stream: &mut TcpStream) -> (u16, String) {
        let mut data = Vec::new();
        let mut byte = [0];
        while !data.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut byte).unwrap(), 1, "the head ended early");
            data.push(byte[0]);
        }
        let head = String::from_utf8(data).unwrap();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse()
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        (
            head[9..12].parse().unwrap(),
            String::from_utf8(body).unwrap(),
        )
    }

    fn closed(stream: &mut TcpStream) -> bool {
        matches!(stream.read(&mut [0]), Ok(0) | Err(_))
    }

    #[test]
    fn keeps_connections_alive_until_idle_and_ends_every_connection_thread() {
        let time = Arc::new(Mutex::new(Duration::ZERO));
        let clock = Arc::clone(&time);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut server =
            HttpServer::start(listener, Arc::new(move || *clock.lock().unwrap()), echo()).unwrap();
        let advance = |by| *time.lock().unwrap() += by;
        let connect = || {
            let stream = TcpStream::connect(server.addr()).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
        };

        // Two requests on one connection; the second in pieces, after a
        // 100-continue.
        let mut browser = connect();
        browser
            .write_all(b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        assert_eq!(response(&mut browser), (200, "/a ".into()));
        browser
            .write_all(b"POST /b HTTP/1.1\r\nContent-Length: 3\r\nExpect: 100-continue\r\n\r\n")
            .unwrap();
        let mut continued = [0; 25];
        browser.read_exact(&mut continued).unwrap();
        assert_eq!(&continued, b"HTTP/1.1 100 Continue\r\n\r\n");
        browser.write_all(b"xyz").unwrap();
        assert_eq!(response(&mut browser), (200, "/b xyz".into()));

        // A request that never ends, and an oversized body. The server
        // accepts in order, so the slow connection was accepted at 0 s.
        let mut slow = connect();
        slow.write_all(b"GET /slow HTTP/1.1\r\n").unwrap();
        let mut large = connect();
        write!(
            large,
            "POST /c HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        )
        .unwrap();
        assert_eq!(response(&mut large).0, 413);
        assert!(closed(&mut large));

        // At 29 s the browser's connection still serves, and its request
        // gives it another 30 s.
        advance(IDLE_TIMEOUT - Duration::from_secs(1));
        browser.write_all(b"GET /c HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(response(&mut browser), (200, "/c ".into()));
        // At 30 s the unfinished request is dropped, at 59 s the idle
        // browser connection.
        advance(Duration::from_secs(1));
        assert!(closed(&mut slow));
        settle("the slow connection's thread ends", || server.peers() == 1);
        advance(IDLE_TIMEOUT - Duration::from_secs(1));
        assert!(closed(&mut browser));
        settle("the idle connection's thread ends", || server.peers() == 0);

        // Many connections opened and dropped: every thread is joined; past
        // the per-address limit a connection is closed at once.
        for _ in 0..5 {
            let mut open: Vec<_> = (0..LIMITS.per_address).map(|_| connect()).collect();
            settle("every connection is served", || {
                server.peers() == LIMITS.per_address
            });
            assert!(closed(&mut connect()));
            open.truncate(1);
            open[0].write_all(b"GET /last HTTP/1.0\r\n\r\n").unwrap();
            assert_eq!(response(&mut open[0]), (200, "/last ".into()));
            assert!(closed(&mut open[0]));
            settle("a thread outlived its connection", || server.peers() == 0);
        }
        server.stop();
        assert!(TcpStream::connect(server.addr()).is_err());
    }
}
