//! The dashboard's loopback HTTP server: the embedded page and its JSON API.

mod api;
mod bot_group;
mod debug;
mod events;

pub(super) use debug::enabled as debug_send_enabled;

use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Read},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
    thread::{self, JoinHandle},
};

use serde_json::{Map, Value as Json, json};
use tiny_http::{Header, Method, Request, Response, Server as HttpServer};

use api::{Api, ApiError, ApiResult};
use orpheus_lobby::{
    accounts::{Account, Accounts},
    clock::Clock,
    collection::Collection,
};

use crate::lobby::services::Services;
use crate::platform::Platform;

const MAX_BODY_BYTES: usize = 65_536;
/// URL path, embedded file and MIME type.
const PAGES: [(&str, &[u8], &str); 5] = [
    ("/", INDEX, "text/html"),
    ("/index.html", INDEX, "text/html"),
    (
        "/assets/dashboard.css",
        include_bytes!("../../../../web/dashboard/dashboard.css"),
        "text/css",
    ),
    (
        "/assets/dashboard-api.mjs",
        include_bytes!("../../../../web/dashboard/dashboard-api.mjs"),
        "text/javascript",
    ),
    (
        "/assets/dashboard.js",
        include_bytes!("../../../../web/dashboard/dashboard.js"),
        "text/javascript",
    ),
];
const INDEX: &[u8] = include_bytes!("../../../../web/dashboard/index.html");
pub struct Dashboard {
    http: Arc<HttpServer>,
    addr: SocketAddr,
    worker: Option<JoinHandle<()>>,
}

/// What the dashboard reads and changes, shared with the lobby.
pub struct DashboardParts {
    pub accounts: Arc<Accounts>,
    pub selected: Arc<Mutex<Arc<Account>>>,
    pub online: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
    pub collection: Arc<Collection>,
    /// The state folder with the profiles and default_account.txt.
    pub data: PathBuf,
    /// Optional pictures: `boxes/<name>.png` and `previews/<GUID>.webp`.
    pub assets: PathBuf,
    /// The lobby port the dashboard reports.
    pub lobby_port: u16,
    /// Hosting and the games this PC started.
    pub platform: Arc<Platform>,
    /// The shop, the loot boxes and the live updates of a connected game.
    pub services: Arc<Services>,
    /// The running lobby, for the actions on its live state (none in tests
    /// of the dashboard alone).
    pub(crate) lobby: Option<Arc<crate::lobby::Lobby>>,
    /// The lobby's clock: the uptime is its time.
    pub clock: Clock,
    /// The marker message routes (`ORPHEUS_DEBUG_SEND=1`, `debug.rs`).
    pub debug_send: bool,
}

impl Dashboard {
    pub fn start(port: u16, parts: DashboardParts) -> io::Result<Self> {
        let http = HttpServer::http((Ipv4Addr::LOCALHOST, port))
            .map(Arc::new)
            .map_err(|error| io::Error::new(io::ErrorKind::AddrInUse, error.to_string()))?;
        let addr = http
            .server_addr()
            .to_ip()
            .ok_or_else(|| io::Error::other("dashboard is not on an IP address"))?;
        let requests = Arc::clone(&http);
        let port = addr.port();
        let worker = thread::Builder::new()
            .name("orpheus-dashboard".into())
            .spawn(move || {
                for request in requests.incoming_requests() {
                    let api = Api {
                        accounts: &parts.accounts,
                        selected: &parts.selected,
                        online: &parts.online,
                        collection: &parts.collection,
                        data: &parts.data,
                        assets: &parts.assets,
                        lobby_port: parts.lobby_port,
                        platform: &parts.platform,
                        services: &parts.services,
                        lobby: parts.lobby.as_deref(),
                        uptime: parts.clock.now(),
                    };
                    let _ = handle(
                        &api,
                        parts.lobby.as_deref(),
                        port,
                        parts.debug_send,
                        request,
                    );
                }
            })?;
        Ok(Self {
            http,
            addr,
            worker: Some(worker),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stop(&mut self) {
        self.http.unblock();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle(
    api: &Api,
    lobby: Option<&crate::lobby::Lobby>,
    port: u16,
    debug_send: bool,
    mut request: Request,
) -> io::Result<()> {
    // A page on another site can reach this loopback server: through DNS
    // rebinding (refused by the Host check) or with a form post, which
    // cannot send a JSON content type without a preflight this server never
    // answers.
    if !local_host(&request, port) {
        return request.respond(json_reply(Err(ApiError::new(403, "Use http://127.0.0.1"))));
    }
    if *request.method() == Method::Post && !json_body(&request) {
        return request.respond(json_reply(Err(ApiError::new(
            415,
            "Send the request as application/json",
        ))));
    }
    let url = request.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let response = match request.method() {
        Method::Get => {
            if let Some((_, body, mime)) = PAGES.iter().find(|(page, _, _)| *page == path) {
                return request.respond(reply(200, body.to_vec(), mime));
            }
            if let Some((file, mime)) = picture(api.assets, path) {
                return match std::fs::read(file) {
                    Ok(body) => request.respond(reply(200, body, mime)),
                    Err(_) => request.respond(json_reply(Err(not_found()))),
                };
            }
            let query = parse_query(query);
            let account = query.get("account").map(String::as_str);
            json_reply(match path {
                "/api/state" => api.state(account),
                "/api/status" => api.state(account).map(|state| state["profile"].clone()),
                "/api/collection" => api.collection(&query),
                "/api/bot_plate" => api.bot_plate(None),
                debug::MESSAGES if debug_send => {
                    debug::messages(lobby.map(|lobby| &lobby.schemas), &query)
                }
                _ => Err(not_found()),
            })
        }
        Method::Post => {
            let result = read_body(&mut request).and_then(|data| match path {
                debug::SEND if debug_send => debug::send(api, lobby, &data),
                debug::KILL if debug_send => debug::kill(api, lobby, &data),
                _ => post(api, lobby, path, &data),
            });
            json_reply(result)
        }
        _ => json_reply(Err(ApiError::new(405, "Method not allowed"))),
    };
    request.respond(response)
}

fn header_value<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str())
}

/// The Host header names this server on loopback.
fn local_host(request: &Request, port: u16) -> bool {
    header_value(request, "Host").is_some_and(|host| {
        let host = host.trim().to_ascii_lowercase();
        host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
    })
}

fn json_body(request: &Request) -> bool {
    header_value(request, "Content-Type").is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    })
}

fn post(
    api: &Api,
    lobby: Option<&crate::lobby::Lobby>,
    path: &str,
    data: &Map<String, Json>,
) -> ApiResult {
    match path {
        "/api/bot_group" => bot_group::bot_group(api, lobby, data),
        "/api/update_profile" => api.update_profile(data),
        "/api/apply_to_all" => api.apply_to_all(data),
        "/api/add_boxes" => api.add_boxes(data),
        "/api/set_frame" => api.set_frame(data),
        "/api/grant_skin" => api.grant_skin(data),
        "/api/select_account" => api.select_account(data),
        "/api/default_account" => api.default_account(data),
        "/api/start_game" => api.start_game(data),
        "/api/reconnect" => Ok(api.reconnect()),
        "/api/purchase" => api.purchase(data),
        "/api/open_all_boxes" => api.open_all_boxes(data),
        "/api/matchmaking" => api.matchmaking(data),
        "/api/mode_settings" => api.mode_settings(data),
        "/api/set_map" => api.set_map(data),
        "/api/set_workshop" => api.set_workshop(data),
        "/api/end_matches" => api.end_matches(),
        "/api/bot_plate" => api.bot_plate(Some(data)),
        _ => Err(ApiError::new(404, "Action not found")),
    }
}

fn read_body(request: &mut Request) -> Result<Map<String, Json>, ApiError> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|error| ApiError::new(400, format!("Invalid request data: {error}")))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ApiError::new(413, "Request too large"));
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Map::new());
    }
    match serde_json::from_slice(&body) {
        Ok(Json::Object(data)) => Ok(data),
        Ok(_) => Err(ApiError::new(400, "A JSON object is expected")),
        Err(error) => Err(ApiError::new(400, format!("Invalid request data: {error}"))),
    }
}

/// A picture inside the assets folder: `boxes/<a-z0-9_>.png` or
/// `previews/<16 upper-case hex digits>.webp`.
fn picture(assets: &Path, path: &str) -> Option<(PathBuf, &'static str)> {
    let relative = path.strip_prefix("/assets/")?;
    let (folder, name) = relative.split_once('/')?;
    let valid = match folder {
        "boxes" => name.strip_suffix(".png").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        }),
        "previews" => name.strip_suffix(".webp").is_some_and(|stem| {
            stem.len() == 16
                && stem
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
        }),
        _ => false,
    };
    let mime = if folder == "boxes" {
        "image/png"
    } else {
        "image/webp"
    };
    valid.then(|| (assets.join(folder).join(name), mime))
}

fn not_found() -> ApiError {
    ApiError::new(404, "Address not found")
}

fn json_reply(result: ApiResult) -> Response<io::Cursor<Vec<u8>>> {
    let (status, value) = match result {
        Ok(value) => (200, value),
        Err(error) => (error.status, json!({"error": error.message})),
    };
    reply(status, value.to_string().into_bytes(), "application/json")
}

fn reply(status: u16, body: Vec<u8>, mime: &str) -> Response<io::Cursor<Vec<u8>>> {
    let text = mime.starts_with("text/") || mime == "application/json";
    let content_type = if text {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_owned()
    };
    Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", &content_type))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

/// A query string as a map; a repeated key keeps its last value and blank
/// values are dropped.
fn parse_query(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_decode(value);
            (!key.is_empty() && !value.is_empty()).then(|| (percent_decode(key), value))
        })
        .collect()
}

/// Decode `%XX` escapes and `+`; an invalid escape stays as written.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], escaped) {
            (_, Some(byte)) => {
                decoded.push(byte);
                index += 3;
                continue;
            }
            (b'+', None) => decoded.push(b' '),
            (byte, None) => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_content::catalog::ItemDb;
    use orpheus_lobby::templates::RetailTemplates;
    use std::{io::Write, net::TcpStream};

    fn request(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, String) {
        raw(
            addr,
            method,
            path,
            &addr.to_string(),
            "application/json",
            body,
        )
    }

    fn raw(
        addr: SocketAddr,
        method: &str,
        path: &str,
        host: &str,
        content_type: &str,
        body: &str,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.0\r\nHost: {host}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let status = reply[9..12].parse().unwrap();
        let body = reply.split_once("\r\n\r\n").unwrap().1.to_owned();
        (status, body)
    }

    fn parts(state: &Path, debug_send: bool) -> DashboardParts {
        let accounts =
            Arc::new(Accounts::new(state.join("profiles"), state.join("profile.json")).unwrap());
        let templates = RetailTemplates::from_reader(
            br#"{"format":1,"server":[
                ["70E5A247",24900,{"+0x80":[],"+0x98":[]}],
                ["779A8581",24300,{"+0x78":{"+0x30":[]},"+0xF8":{"+0x0":[]}}]
            ]}"#
            .as_slice(),
        )
        .unwrap();
        let collection = Collection::new(&templates, Arc::new(ItemDb::default())).unwrap();
        DashboardParts {
            selected: Arc::new(Mutex::new(accounts.get("Alpha").unwrap())),
            accounts,
            online: Arc::default(),
            services: Arc::new(Services::new(&collection, Arc::default())),
            collection: Arc::new(collection),
            data: state.into(),
            assets: state.join("assets"),
            lobby_port: 3724,
            platform: Arc::new(Platform::new(
                std::net::Ipv4Addr::LOCALHOST,
                String::new(),
                None,
            )),
            lobby: None,
            clock: Clock::wall(),
            debug_send,
        }
    }

    #[test]
    fn the_marker_message_routes_exist_only_with_the_switch() {
        let state = tempfile::tempdir().unwrap();
        let send = r##"{"id":25000,"fields":{"#0":1}}"##;
        let mut off = Dashboard::start(0, parts(state.path(), false)).unwrap();
        let addr = off.addr();
        assert_eq!(
            request(addr, "GET", "/api/debug/messages?id=25000", "").0,
            404
        );
        assert_eq!(request(addr, "POST", "/api/debug/send", send).0, 404);
        assert_eq!(request(addr, "POST", "/api/debug/kill", "{}").0, 404);
        off.stop();

        let mut on = Dashboard::start(0, parts(state.path(), true)).unwrap();
        let addr = on.addr();
        let (status, layout) = request(addr, "GET", "/api/debug/messages?id=25000", "");
        assert_eq!(status, 200);
        let layout: Json = serde_json::from_str(&layout).unwrap();
        assert_eq!(layout["link"], "game");
        assert_eq!(
            request(addr, "POST", "/api/debug/send", send),
            (503, r#"{"error":"The lobby is not running."}"#.into())
        );
        assert_eq!(
            request(addr, "POST", "/api/debug/kill", "{}"),
            (503, r#"{"error":"The lobby is not running."}"#.into())
        );
        on.stop();
    }

    #[test]
    fn serves_the_page_the_api_and_refuses_unported_actions() {
        let state = tempfile::tempdir().unwrap();
        let mut dashboard = Dashboard::start(0, parts(state.path(), false)).unwrap();
        let addr = dashboard.addr();

        let (status, page) = request(addr, "GET", "/", "");
        assert_eq!(status, 200);
        assert!(page.contains("dashboard.js"));
        let (status, state) = request(addr, "GET", "/api/state?account=alpha", "");
        assert_eq!(status, 200);
        let state: Json = serde_json::from_str(&state).unwrap();
        assert_eq!(state["profile"]["player_name"], "Alpha");
        assert_eq!(
            request(addr, "POST", "/api/select_account", r#"{"name":"alpha"}"#),
            (200, r#"{"status":"ok"}"#.into())
        );
        assert_eq!(request(addr, "POST", "/api/bot_group", "{}").0, 503);
        assert_eq!(
            request(addr, "POST", "/api/purchase", r#"{"guid":"0x10"}"#),
            (
                400,
                r#"{"error":"This item is not available for purchase."}"#.into()
            )
        );
        assert_eq!(
            request(addr, "POST", "/api/start_game", r#"{"name":"alpha"}"#).0,
            409
        );
        assert_eq!(
            request(addr, "POST", "/api/reconnect", "{}"),
            (200, r#"{"status":"ok"}"#.into())
        );
        assert_eq!(request(addr, "POST", "/api/select_account", "[]").0, 400);
        assert_eq!(request(addr, "GET", "/api/missing", "").0, 404);
        assert_eq!(request(addr, "GET", "/assets/boxes/golden.png", "").0, 404);

        let select = r#"{"name":"alpha"}"#;
        let localhost = format!("LocalHost:{}", addr.port());
        let json = "application/json; charset=utf-8";
        assert_eq!(
            raw(
                addr,
                "POST",
                "/api/select_account",
                &localhost,
                json,
                select
            )
            .0,
            200
        );
        assert_eq!(
            raw(
                addr,
                "POST",
                "/api/select_account",
                &addr.to_string(),
                "text/plain",
                select
            )
            .0,
            415
        );
        let evil = format!("evil.example:{}", addr.port());
        assert_eq!(raw(addr, "GET", "/api/state", &evil, json, "").0, 403);
        dashboard.stop();
    }

    #[test]
    fn query_values_are_decoded_and_blank_values_dropped() {
        let query = parse_query("account=Caf%C3%A9+Bar&page=2&hero=&page=3");
        assert_eq!(query["account"], "Café Bar");
        assert_eq!(query["page"], "3");
        assert!(!query.contains_key("hero"));
        assert_eq!(parse_query("account=100%25+%ZZ")["account"], "100% %ZZ");
    }

    #[test]
    fn pictures_stay_inside_their_folders() {
        let assets = Path::new("assets");
        assert_eq!(
            picture(assets, "/assets/boxes/golden.png"),
            Some((assets.join("boxes").join("golden.png"), "image/png"))
        );
        assert!(picture(assets, "/assets/previews/0250000000001149.webp").is_some());
        for path in [
            "/assets/boxes/../secret.png",
            "/assets/boxes/Golden.png",
            "/assets/previews/0250000000001149.png",
            "/assets/previews/0250000000001i49.webp",
            "/assets/dashboard.js",
        ] {
            assert_eq!(picture(assets, path), None, "{path}");
        }
    }
}
