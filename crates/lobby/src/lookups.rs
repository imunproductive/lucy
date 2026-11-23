//! Requests that ask for information: player names, the store and Overwatch
//! League live matches. Ports `ow174/lobby/handlers/lookups.py`; career
//! profiles are in `mode_rules`, the leaderboard in `leaderboard`.

use std::io;

use crate::{accounts::Accounts, transport::LobbyMessage};
use lucy_content::catalog::region_of;
use lucy_wire::jam::codec::{
    Fields,
    Value::{Array, Bool, Signed, Struct, Unsigned},
};

pub const NAME_QUERY: u32 = 0x46dc_9706;
pub const NAMES: u32 = 58202;
pub const NAME_REPLY: u32 = 0x981d_6a52;
const NAMES_FOUND: u32 = 58301;
pub const OWL_POLL: u32 = 0xeb45_ad29;
pub const OWL_MATCHES: u32 = 42600;
pub const OWL_LIVE: u32 = 0x713d_7589;
const OWL_LIVE_MATCHES: u32 = 43200;
pub const STORE_QUERY: u32 = 0x5217_e4cd;
pub const STORE_PAGE: u32 = 26500;
pub const STORE: u32 = 0x4bad_7a7e;
const STORE_STATE: u32 = 26404;
const STORE_PRODUCTS: u32 = 26400;

/// 58301: the name of each account id the client asked about (58202 +0x78);
/// an unknown one gets "Player" and its low 16 bits.
pub fn player_names(request: &Fields, accounts: &Accounts) -> io::Result<LobbyMessage> {
    let ids = match request.get(&0x78) {
        Some(Array(ids)) => ids.as_slice(),
        _ => &[],
    };
    let mut names = Vec::with_capacity(ids.len());
    for id in ids {
        let account_lo = match id {
            Struct(fields) => match fields.get(&0) {
                Some(Unsigned(lo)) => *lo,
                Some(Signed(lo)) => *lo as u64,
                _ => 0,
            },
            _ => 0,
        };
        let known = match u32::try_from(account_lo) {
            Ok(lo) => accounts.by_id(lo)?,
            Err(_) => None,
        };
        let name = known.map_or_else(
            || format!("Player{}", account_lo & 0xffff),
            |account| account.name.clone(),
        );
        names.push(Struct(Fields::from([
            (0x00, id.clone()),
            (0x10, Unsigned(0)),
            (0x18, lucy_wire::jam::codec::Value::String(name)),
        ])));
    }
    Ok((
        NAME_REPLY,
        NAMES_FOUND,
        Fields::from([(0x78, Array(names))]),
    ))
}

/// Retail's answer to 42600: no live match.
pub fn no_owl_match() -> LobbyMessage {
    (
        OWL_LIVE,
        OWL_LIVE_MATCHES,
        Fields::from([(0x78, Bool(true)), (0x80, Array(Vec::new()))]),
    )
}

/// The real-money store: no products to sell, in the account region's
/// currency (Msg26400_Handler 0x7FF7899FDBC0). Its other fields are the
/// request it answers (`+0x98`, 1, as retail sent), the status (`+0x9C`,
/// 0: no error; another value shows `+0xA0` as an error) and that text.
/// 26404 comes first, empty, as retail sent it. They answer the store page
/// (26500) and go out at login.
pub fn store(region: &str) -> Vec<LobbyMessage> {
    vec![
        (STORE, STORE_STATE, Fields::new()),
        (
            STORE,
            STORE_PRODUCTS,
            Fields::from([
                (0x78, Unsigned(region_of(region).currency)),
                (0x80, Array(Vec::new())),
                (0x98, Unsigned(1)),
                (0x9c, Unsigned(0)),
                (0xa0, lucy_wire::jam::codec::Value::String(String::new())),
            ]),
        ),
    ]
}

#[cfg(test)]
mod tests;
