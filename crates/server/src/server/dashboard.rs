//! The dashboard's loopback HTTP server: the embedded page and its JSON API.
//! The connections are served in parallel (`http.rs`); the requests one at a
//! time.
//!
//! The page is the build of `web/dashboard` (Svelte), committed in its
//! `dist/` and embedded by `build.rs`. Its scripts and styles have content
//! hashes in their names, so browsers keep them; `index.html` answers every
//! path outside `/api/` and `/assets/`, where the page routes itself.

mod api;
mod bot_group;
mod events;
mod http;
mod live;

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    io,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
};

use serde_json::{Map, Value as Json, json};

use api::{Api, ApiError, ApiResult};
use http::{HttpServer, Method, Request, Response};
use lucy_lobby::{
    accounts::{Account, Accounts},
    clock::Clock,
    collection::Collection,
};

use crate::lobby::services::Services;
use crate::platform::Platform;

mod files {
    include!(concat!(env!("OUT_DIR"), "/dashboard_files.rs"));
}

/// How long a browser keeps the page's hashed files (a year: a new build has
/// new names).
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub struct Dashboard {
    http: HttpServer,
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
}

impl Dashboard {
    pub fn start(port: u16, parts: DashboardParts) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        let clock = parts.clock.source();
        // One request at a time, as the API was written for.
        let serial = Mutex::new(());
        let revisions = live::Revisions::default();
        let http = HttpServer::start(listener, clock, move |request| {
            let _one_at_a_time = serial.lock().unwrap_or_else(PoisonError::into_inner);
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
            handle(&api, parts.lobby.as_deref(), &revisions, port, request)
        })?;
        Ok(Self { http })
    }

    pub fn addr(&self) -> SocketAddr {
        self.http.addr()
    }

    /// Close the listener and every connection, and join their threads.
    pub fn stop(&mut self) {
        self.http.stop();
    }
}

fn handle(
    api: &Api,
    lobby: Option<&crate::lobby::Lobby>,
    revisions: &live::Revisions,
    port: u16,
    request: Request,
) -> Response {
    // A page on another site can reach this loopback server: through DNS
    // rebinding (refused by the Host check) or with a form post, which
    // cannot send a JSON content type without a preflight this server never
    // answers.
    if !local_host(&request, port) {
        return json_reply(Err(ApiError::new(403, "Use http://127.0.0.1")));
    }
    if request.method == Method::Post && !json_body(&request) {
        return json_reply(Err(ApiError::new(
            415,
            "Send the request as application/json",
        )));
    }
    let (path, query) = request.url.split_once('?').unwrap_or((&request.url, ""));
    match request.method {
        Method::Get if !path.starts_with("/api/") => page(api.assets, path),
        Method::Get => {
            let query = parse_query(query);
            let account = query.get("account").map(String::as_str);
            json_reply(match (path, live::match_route(path)) {
                ("/api/state", _) => api.state(account),
                ("/api/status", _) => api.state(account).map(|state| state["profile"].clone()),
                ("/api/collection", _) => api.collection(&query),
                ("/api/live", _) => live::live(lobby, revisions, &query),
                ("/api/lobby/chat", _) => live::lobby_chat(lobby, &query),
                (_, Some((id, "chat"))) => live::match_chat(lobby, id, &query),
                _ => Err(not_found()),
            })
        }
        Method::Post => {
            let result = read_body(&request.body).and_then(|data| post(api, lobby, path, &data));
            json_reply(result)
        }
        Method::Other => json_reply(Err(ApiError::new(405, "Method not allowed"))),
    }
}

/// The Host header names this server on loopback.
fn local_host(request: &Request, port: u16) -> bool {
    request.header("Host").is_some_and(|host| {
        let host = host.trim().to_ascii_lowercase();
        host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
    })
}

fn json_body(request: &Request) -> bool {
    request.header("Content-Type").is_some_and(|value| {
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
    match (path, live::match_route(path)) {
        ("/api/bot_group", _) => bot_group::bot_group(api, lobby, data),
        ("/api/update_profile", _) => api.update_profile(data),
        ("/api/apply_to_all", _) => api.apply_to_all(data),
        ("/api/add_boxes", _) => api.add_boxes(data),
        ("/api/set_frame", _) => api.set_frame(data),
        ("/api/grant_skin", _) => api.grant_skin(data),
        ("/api/select_account", _) => api.select_account(data),
        ("/api/default_account", _) => api.default_account(data),
        ("/api/start_game", _) => api.start_game(data),
        ("/api/reconnect", _) => Ok(api.reconnect()),
        ("/api/purchase", _) => api.purchase(data),
        ("/api/open_all_boxes", _) => api.open_all_boxes(data),
        ("/api/matchmaking", _) => api.matchmaking(data),
        ("/api/mode_settings", _) => api.mode_settings(data),
        ("/api/set_map", _) => api.set_map(data),
        ("/api/set_workshop", _) => api.set_workshop(data),
        ("/api/end_matches", _) => api.end_matches(),
        ("/api/broadcast", _) => live::broadcast(lobby, data),
        (_, Some((id, "chat"))) => live::say(lobby, id, data),
        (_, Some((id, "kick"))) => live::kick(lobby, id, data),
        (_, Some((id, "flow"))) => live::flow(lobby, id, data),
        (_, Some((id, "stop"))) => live::stop(lobby, id, data),
        _ => Err(ApiError::new(404, "Action not found")),
    }
}

/// The body as a JSON object; the server limits its size
/// (`http::MAX_BODY_BYTES`).
fn read_body(body: &[u8]) -> Result<Map<String, Json>, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Map::new());
    }
    match serde_json::from_slice(body) {
        Ok(Json::Object(data)) => Ok(data),
        Ok(_) => Err(ApiError::new(400, "A JSON object is expected")),
        Err(error) => Err(ApiError::new(400, format!("Invalid request data: {error}"))),
    }
}

/// A GET outside the API: a file of the page, a picture from the assets
/// folder, or the page itself for the paths it routes (`/matches/<id>`).
fn page(assets: &Path, path: &str) -> Response {
    let path = if path == "/" { "/index.html" } else { path };
    if let Some(&(_, body, mime)) = files::FILES.iter().find(|(url, _, _)| *url == path) {
        // The built scripts and styles have content hashes in their names.
        let cache = if path.starts_with("/assets/") {
            IMMUTABLE
        } else {
            "no-cache"
        };
        return reply(200, Cow::Borrowed(body), mime, cache);
    }
    if let Some((file, mime)) = picture(assets, path) {
        return match std::fs::read(file) {
            Ok(body) => reply(200, body.into(), mime, "no-store"),
            Err(_) => json_reply(Err(not_found())),
        };
    }
    // A missing file (a name with an extension) stays missing.
    let file = path.starts_with("/assets/")
        || path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('.'));
    if file {
        return json_reply(Err(not_found()));
    }
    let &(_, index, mime) = files::FILES
        .iter()
        .find(|(url, _, _)| *url == "/index.html")
        .expect("build.rs checks that the page has index.html");
    reply(200, Cow::Borrowed(index), mime, "no-cache")
}

/// A picture inside the assets folder: `boxes/<a-z0-9_>.png` (not a Windows
/// device name such as `con`) or `previews/<16 upper-case hex digits>.webp`.
fn picture(assets: &Path, path: &str) -> Option<(PathBuf, &'static str)> {
    let relative = path.strip_prefix("/assets/")?;
    let (folder, name) = relative.split_once('/')?;
    let valid = match folder {
        "boxes" => name.strip_suffix(".png").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
                && !lucy_lobby::account_name::is_reserved_device_name(stem)
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

fn json_reply(result: ApiResult) -> Response {
    let (status, value) = match result {
        Ok(value) => (200, value),
        Err(error) => (error.status, json!({"error": error.message})),
    };
    reply(
        status,
        value.to_string().into_bytes().into(),
        "application/json",
        "no-store",
    )
}

fn reply(status: u16, body: Cow<'static, [u8]>, mime: &str, cache: &str) -> Response {
    let text = mime.starts_with("text/") || mime == "application/json";
    let content_type = if text {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_owned()
    };
    Response {
        status,
        headers: vec![
            ("Content-Type", content_type),
            ("Cache-Control", cache.to_owned()),
            ("X-Content-Type-Options", "nosniff".to_owned()),
        ],
        body,
    }
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
    use lucy_content::catalog::ItemDb;
    use lucy_lobby::economy::ItemPrices;
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };

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
        let (status, _, body) = exchange(addr, method, path, host, content_type, body);
        (status, body)
    }

    /// A GET: status, head and body.
    fn get(addr: SocketAddr, path: &str) -> (u16, String, String) {
        exchange(addr, "GET", path, &addr.to_string(), "text/plain", "")
    }

    fn exchange(
        addr: SocketAddr,
        method: &str,
        path: &str,
        host: &str,
        content_type: &str,
        body: &str,
    ) -> (u16, String, String) {
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
        let (head, body) = reply.split_once("\r\n\r\n").unwrap();
        (status, head.to_owned(), body.to_owned())
    }

    fn parts(state: &Path) -> DashboardParts {
        let accounts =
            Arc::new(Accounts::new(state.join("profiles"), state.join("profile.json")).unwrap());
        let collection = Collection::new(Arc::new(ItemDb::default()), ItemPrices::default());
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
        }
    }

    /// The live routes answer 503 without a lobby; the debug routes are
    /// gone.
    #[test]
    fn the_live_routes_need_the_lobby_and_the_debug_routes_are_gone() {
        let state = tempfile::tempdir().unwrap();
        let mut dashboard = Dashboard::start(0, parts(state.path())).unwrap();
        let addr = dashboard.addr();
        let id = "0123456789abcdef0123456789abcdef";
        for (method, path) in [
            ("GET", "/api/live"),
            ("GET", "/api/lobby/chat"),
            ("GET", &*format!("/api/matches/{id}/chat")),
            ("POST", "/api/broadcast"),
            ("POST", &*format!("/api/matches/{id}/kick")),
            ("POST", &*format!("/api/matches/{id}/stop")),
        ] {
            assert_eq!(request(addr, method, path, "{}").0, 503, "{path}");
        }
        for (method, path) in [
            ("GET", "/api/debug/messages?id=25000"),
            ("POST", "/api/debug/send"),
            ("POST", "/api/debug/kill"),
            ("GET", "/api/bot_plate"),
            ("POST", "/api/bot_plate"),
            ("POST", "/api/matches/0123/kick"),
        ] {
            assert_eq!(request(addr, method, path, "{}").0, 404, "{path}");
        }
        dashboard.stop();
    }

    #[test]
    fn serves_the_built_page_its_files_and_its_routes() {
        let state = tempfile::tempdir().unwrap();
        let mut dashboard = Dashboard::start(0, parts(state.path())).unwrap();
        let addr = dashboard.addr();

        let (status, head, index) = get(addr, "/");
        assert_eq!(status, 200);
        assert!(
            head.contains("Content-Type: text/html; charset=utf-8"),
            "{head}"
        );
        assert!(head.contains("Cache-Control: no-cache"), "{head}");
        assert!(index.contains("<title>Lucy</title>"));
        // The page names its script and style by content hash: browsers keep them.
        for (attribute, mime) in [
            ("src=\"", "text/javascript; charset=utf-8"),
            ("href=\"", "text/css; charset=utf-8"),
        ] {
            let start = index
                .find(&format!("{attribute}/assets/"))
                .unwrap_or_else(|| panic!("{attribute}/assets/ in {index}"))
                + attribute.len();
            let url = &index[start..start + index[start..].find('"').unwrap()];
            let (status, head, _) = get(addr, url);
            assert_eq!(status, 200, "{url}");
            assert!(
                head.contains(&format!("Content-Type: {mime}")),
                "{url}: {head}"
            );
            assert!(
                head.contains(&format!("Cache-Control: {IMMUTABLE}")),
                "{url}: {head}"
            );
        }
        let (status, head, _) = get(addr, "/favicon.svg");
        assert_eq!(status, 200);
        assert!(head.contains("Content-Type: image/svg+xml"), "{head}");
        // The page's own routes load the page, which routes itself.
        for route in [
            "/index.html",
            "/matches",
            "/matches/0123abcd",
            "/collection?kind=skins",
        ] {
            let (status, _, page) = get(addr, route);
            assert_eq!((status, page == index), (200, true), "{route}");
        }
        // Missing files and API routes stay missing.
        for missing in [
            "/assets/index-missing.js",
            "/robots.txt",
            "/api/missing",
            "/assets/boxes/golden.png",
        ] {
            let (status, head, _) = get(addr, missing);
            assert_eq!(status, 404, "{missing}");
            assert!(head.contains("application/json"), "{missing}: {head}");
        }
        dashboard.stop();
    }

    #[test]
    fn serves_the_api_and_refuses_unported_actions() {
        let state = tempfile::tempdir().unwrap();
        let mut dashboard = Dashboard::start(0, parts(state.path())).unwrap();
        let addr = dashboard.addr();

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
            "/assets/boxes/con.png",
            "/assets/boxes/com1.png",
            "/assets/previews/0250000000001149.png",
            "/assets/previews/0250000000001i49.webp",
            "/assets/dashboard.js",
        ] {
            assert_eq!(picture(assets, path), None, "{path}");
        }
    }
}
