//! Fixed data for build 104319 that the game install does not contain, so it
//! can't be read from CASC. It is embedded in the binary. Where each file comes
//! from is recorded in `docs/notes/data-sources.md`.

#![forbid(unsafe_code)]

/// Layouts of the lobby messages, read from a running client by
/// `tools/dump_schemas.py`.
pub const LOBBY_SCHEMAS: &str = include_str!("../data/lobby_schemas.json");

/// Layouts of the game link's messages (`tools/dump_schemas.py --game-link`).
pub const GAME_SCHEMAS: &str = include_str!("../data/game_schemas.json");

/// What the client reads for each statescript state class in a frame, from
/// its code (`research/ida/extract_statescript_codecs.py`).
pub const STATESCRIPT_CODECS: &str = include_str!("../data/statescript_codecs.json");

/// The protobuf descriptor set of the client's Battle.net services.
pub const BNET_DESCRIPTORS: &[u8] = include_bytes!("../data/bnet_descriptors.pb");

/// The client's fixed HMAC key for the lobby handshake's state blob.
pub const LOBBY_STATE_BLOB_KEY: &[u8; 64] = include_bytes!("../data/lobby_state_blob.key");

/// Server messages decoded from 1.68 reference traffic: catalogs, prices,
/// rules and defaults the client never contained.
pub const RETAIL_TEMPLATES: &str = include_str!("../data/retail_templates.json");

/// Content keys of event resources, as the server sends them in
/// `lobby_celebrations`.
pub const RESOURCE_KEYS: &str = include_str!("../data/resource_keys.json");

/// Research names for the statescript graphs (`orpheus script query`): class
/// aliases, the STU fields that hold a variable, logical buttons, the hero
/// select and HUD graphs, each game mode's scripts and the variable names the
/// server uses.
pub const STATESCRIPT_NAMES: &str = include_str!("../data/statescript_names.json");
