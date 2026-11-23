use super::*;
use lucy_content::catalog::{ItemDb, UNLOCK_BASE};
use lucy_lobby::economy::ItemPrices;

const ICON: u64 = UNLOCK_BASE | 0x10;
const FRAME: u64 = UNLOCK_BASE | 0x20;
/// Sold for 100 credits.
const SPRAY: u64 = UNLOCK_BASE | 0x30;
/// An Overwatch League icon, sold for 50 league tokens; never in boxes.
const OWL_ICON: u64 = UNLOCK_BASE | 0x40;

struct Fixture {
    state: tempfile::TempDir,
    accounts: Accounts,
    selected: Mutex<Arc<Account>>,
    online: Mutex<HashMap<u32, Arc<AtomicBool>>>,
    collection: Collection,
    platform: Platform,
    services: Services,
}

impl Fixture {
    fn new() -> Self {
        let state = tempfile::tempdir().unwrap();
        let accounts = Accounts::new(
            state.path().join("profiles"),
            state.path().join("profile.json"),
        )
        .unwrap();
        accounts.get("Alpha").unwrap();
        let selected = Mutex::new(accounts.get("Beta").unwrap());
        let mut items = ItemDb::default();
        // The frame unlocks at level 601; the spray is a base box item, the
        // Overwatch League icon no box's.
        for (guid, name, item_type, categories, box_type, level) in [
            (ICON, "Ring", "Icon", &[][..], None, None),
            (FRAME, "Silver", "PortraitFrame", &[], None, Some(601)),
            (SPRAY, "Tag", "Spray", &[], Some("Base"), None),
            (OWL_ICON, "Team", "Icon", &["OWL"], None, None),
        ] {
            items.unlocks.insert(
                guid,
                Unlock {
                    guid,
                    name: name.into(),
                    item_type: item_type.into(),
                    rarity: "Common".into(),
                    hero: None,
                    source: String::new(),
                    box_type: box_type.map(Into::into),
                    categories: categories.iter().map(|&category| category.into()).collect(),
                    available_in: String::new(),
                    skin_theme: 0,
                    esports_team: String::new(),
                    level,
                },
            );
        }
        let prices = ItemPrices {
            credits: [("Common".into(), 100)].into(),
            competitive_points: 3000,
            league_tokens: [("Common".into(), 50)].into(),
            ..ItemPrices::default()
        };
        let collection = Collection::new(Arc::new(items), prices);
        Self {
            state,
            accounts,
            selected,
            online: Mutex::new(HashMap::new()),
            services: Services::new(&collection, Arc::default()),
            collection,
            platform: Platform::new(std::net::Ipv4Addr::LOCALHOST, String::new(), None),
        }
    }

    fn api(&self) -> Api<'_> {
        Api {
            accounts: &self.accounts,
            selected: &self.selected,
            online: &self.online,
            collection: &self.collection,
            data: self.state.path(),
            assets: self.state.path(),
            lobby_port: 3724,
            platform: &self.platform,
            services: &self.services,
            lobby: None,
            uptime: Duration::ZERO,
        }
    }

    fn saved(&self, name: &str) -> Profile {
        let path = self
            .state
            .path()
            .join("profiles")
            .join(format!("{name}.json"));
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }
}

fn data(value: Json) -> Map<String, Json> {
    match value {
        Json::Object(map) => map,
        _ => panic!("object"),
    }
}

#[test]
fn a_rejected_field_leaves_the_saved_profile_unchanged() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let error = api
        .update_profile(&data(json!({
            "account": "alpha", "credits": 500, "endorsement_level": 9
        })))
        .unwrap_err();
    assert_eq!(
        error,
        ApiError::bad("Endorsement level: allowed range is 1 to 5")
    );
    assert_ne!(fixture.saved("Alpha").credits, 500);

    let state = api
        .update_profile(&data(
            json!({"account": "ALPHA", "credits": "500", "level": 2}),
        ))
        .unwrap();
    assert_eq!(state["profile"]["credits"], 500);
    assert_eq!(fixture.saved("Alpha").credits, 500);
    assert_eq!(fixture.saved("Alpha").level, 2);
}

#[test]
fn numbers_reject_booleans_fractions_and_unknown_fields() {
    let fixture = Fixture::new();
    let api = fixture.api();
    for (value, message) in [
        (json!(true), "credits: enter a whole number"),
        (json!(2.5), "credits: enter a whole number"),
        (json!(-1), "credits: allowed range is 0 to 2147483647"),
    ] {
        let error = api
            .update_profile(&data(json!({"credits": value})))
            .unwrap_err();
        assert_eq!(error.message, message);
    }
    let error = api
        .update_profile(&data(json!({"stats": {}, "xp": 1})))
        .unwrap_err();
    assert_eq!(error.message, "Unknown field: stats, xp");
}

#[test]
fn wins_cannot_exceed_matches_in_the_same_queue() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let error = api
        .update_profile(&data(json!({"matches_tank": 3, "wins_tank": 4})))
        .unwrap_err();
    assert_eq!(error.message, "Tank wins can't be more than tank matches.");
    let state = api
        .update_profile(&data(json!({"matches_tank": 4, "wins_tank": 4})))
        .unwrap();
    assert_eq!(state["profile"]["wins_tank"], 4);
}

#[test]
fn events_dates_and_challenges_accept_only_known_values() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let state = api
        .update_profile(&data(json!({
            "events": " LUNAR ",
            "server_date": "2022-02-01",
            "challenge": "Kanezaka Challenge",
        })))
        .unwrap();
    assert_eq!(state["profile"]["events"], json!(["lunar"]));
    for (field, value, message) in [
        (
            "events",
            json!(["lunar", "winter"]),
            "Pick an event from the list.",
        ),
        ("events", json!(["easter"]), "Pick an event from the list."),
        ("server_date", json!("2022-02-30"), DATE_HINT),
        ("server_date", json!("2022-2-01"), DATE_HINT),
        (
            "server_date",
            json!("1999-12-31"),
            "Server date must be between 2000-01-03 and 2106-02-06",
        ),
        ("challenge", json!("Unknown"), "Challenge not found"),
    ] {
        let error = api
            .update_profile(&data(json!({field: value})))
            .unwrap_err();
        assert_eq!(error.message, message, "{field} {value}");
    }
}

#[test]
fn a_level_change_returns_the_level_frame_and_set_frame_checks_borders() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let reply = api
        .set_frame(&data(json!({"guid": guid_text(FRAME)})))
        .unwrap();
    assert_eq!(reply["frame"], guid_text(FRAME));
    assert_eq!(fixture.saved("Beta").frame_guid, FRAME);
    let error = api
        .set_frame(&data(json!({"guid": guid_text(ICON)})))
        .unwrap_err();
    assert_eq!(error.message, "Invalid frame");

    api.update_profile(&data(json!({"level": 900}))).unwrap();
    assert_eq!(fixture.saved("Beta").frame_guid, 0);
}

#[test]
fn granting_and_revoking_items_keeps_a_sorted_unique_list() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let reply = api
        .grant_skin(&data(json!({"guids": [guid_text(ICON), ICON]})))
        .unwrap();
    assert_eq!(reply["granted"], json!([guid_text(ICON), guid_text(ICON)]));
    assert_eq!(
        fixture.saved("Beta").unlocked_items,
        [json!(guid_text(ICON))]
    );
    let reply = api
        .grant_skin(&data(json!({"guid": guid_text(ICON), "revoke": true})))
        .unwrap();
    assert_eq!(reply["unlocked_count"], 0);
    let error = api.grant_skin(&data(json!({"guid": "0x1"}))).unwrap_err();
    assert_eq!(error.message, "Invalid item");
}

#[test]
fn selecting_and_defaulting_accounts_use_saved_names() {
    let fixture = Fixture::new();
    let api = fixture.api();
    api.select_account(&data(json!({"name": "alpha"}))).unwrap();
    assert_eq!(fixture.selected.lock().unwrap().name, "Alpha");
    let reply = api.default_account(&data(json!({"name": "beta"}))).unwrap();
    assert_eq!(reply["message"], "Beta is the default account now.");
    assert_eq!(
        fs::read_to_string(fixture.state.path().join("default_account.txt")).unwrap(),
        "Beta\n"
    );
    let error = api
        .select_account(&data(json!({"name": "Gamma"})))
        .unwrap_err();
    assert_eq!(error, ApiError::new(404, "Profile not found."));

    let state = api.state(None).unwrap();
    let rows = state["accounts"].as_array().unwrap();
    assert_eq!(
        rows[0],
        json!({"name": "Alpha", "online": false, "selected": true, "default": false})
    );
    assert_eq!(rows[1]["default"], true);
}

#[test]
fn apply_to_all_changes_every_account_and_the_template() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let reply = api
        .apply_to_all(&data(json!({"events": ["winter"]})))
        .unwrap();
    assert_eq!(reply["accounts"], 2);
    assert_eq!(fixture.saved("Alpha").events, ["winter"]);
    assert_eq!(fixture.saved("Beta").events, ["winter"]);
    let template = load_or_create_profile(&fixture.state.path().join("profile.json")).unwrap();
    assert_eq!(template.events, ["winter"]);
    let error = api
        .apply_to_all(&data(json!({"events": [], "credits": 5})))
        .unwrap_err();
    assert_eq!(error.message, "Send an event or a challenge");
}

#[test]
fn collection_pages_filter_by_kind_and_report_ownership() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let query = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect::<BTreeMap<String, String>>()
    };
    let icons = api.collection(&query(&[("kind", "icons")])).unwrap();
    assert_eq!(icons["total"], 2);
    assert_eq!(icons["items"][0]["guid"], guid_text(ICON));
    assert_eq!(icons["items"][0]["purchasable"], false);
    assert_eq!(icons["items"][0]["price"], Json::Null);
    assert_eq!(icons["items"][1]["price"], 50);
    assert_eq!(icons["items"][1]["currency"], "league_tokens");
    assert_eq!(icons["items"][1]["purchasable"], true);
    let sold = api.collection(&query(&[("currency", "credits")])).unwrap();
    assert_eq!(sold["total"], 1);
    assert_eq!(sold["items"][0]["guid"], guid_text(SPRAY));
    assert_eq!(sold["items"][0]["price"], 100);
    let none = api
        .collection(&query(&[("currency", "comp_points")]))
        .unwrap();
    assert_eq!(none["total"], 0);
    let frames = api.collection(&query(&[("kind", "frames")])).unwrap();
    assert_eq!(frames["items"][0]["name"], "Silver · level 601+");
    let error = api.collection(&query(&[("kind", "hats")])).unwrap_err();
    assert_eq!(error.message, "Unknown item type");
}

#[test]
fn frame_names_follow_the_level_tiers() {
    assert_eq!(frame_name(1), "Bronze · level 1+");
    assert_eq!(frame_name(1441), "Gold ★★ · level 1441+");
    assert_eq!(frame_name(2401), "Diamond · level 2401+");
    assert_eq!(frame_name(3600), "Diamond ★★★★★ · level 3600+");
}

/// Second games that "start" at once and play until the test ends.
#[derive(Default)]
struct Started(Mutex<Vec<String>>);

impl crate::platform::LocalGames for Started {
    fn start(&self, account: &str) {
        self.0.lock().unwrap().push(account.into());
    }
    fn account_of(&self, _: u16, _: u16) -> Option<String> {
        None
    }
    fn second_accounts(&self) -> std::collections::HashSet<String> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
}

#[test]
fn a_second_game_starts_once_for_an_account_that_is_not_playing() {
    let fixture = Fixture::new();
    let error = fixture
        .api()
        .start_game(&data(json!({"name": "Alpha"})))
        .unwrap_err();
    assert_eq!(error.status, 409);

    let games = Arc::new(Started::default());
    let platform = Platform::new(
        std::net::Ipv4Addr::UNSPECIFIED,
        String::new(),
        Some(Arc::clone(&games) as _),
    );
    let api = Api {
        platform: &platform,
        ..fixture.api()
    };
    let started = api.start_game(&data(json!({"name": "alpha"}))).unwrap();
    assert_eq!(started["message"], "A second game is starting as Alpha.");
    assert_eq!(*games.0.lock().unwrap(), ["Alpha"]);
    let again = api.start_game(&data(json!({"name": "Alpha"}))).unwrap_err();
    assert_eq!(
        (again.status, again.message.as_str()),
        (409, "Alpha is already playing")
    );

    let beta = fixture.accounts.get("Beta").unwrap();
    fixture
        .online
        .lock()
        .unwrap()
        .insert(beta.account_lo, Arc::default());
    assert_eq!(
        api.start_game(&data(json!({"name": "Beta"})))
            .unwrap_err()
            .status,
        409
    );
    assert_eq!(games.0.lock().unwrap().len(), 1);
    let server = api.state(None).unwrap()["server"].clone();
    assert_eq!(
        (server["host"].as_str(), server["second_games"].as_bool()),
        (Some("0.0.0.0"), Some(true))
    );
}

#[test]
fn a_purchase_charges_its_own_currency_and_refusals_change_nothing() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let before = fixture.saved("Beta");
    let reply = api
        .purchase(&data(json!({"guid": guid_text(OWL_ICON)})))
        .unwrap();
    assert_eq!(
        reply["receipt"],
        json!({"guid": guid_text(OWL_ICON), "price": 50, "currency": "league_tokens", "also": []})
    );
    let after = fixture.saved("Beta");
    assert_eq!(after.league_tokens, before.league_tokens - 50);
    assert_eq!(after.credits, before.credits);
    assert_eq!(after.unlocked_items, [json!(guid_text(OWL_ICON))]);

    for (guid, status, message) in [
        (
            json!(guid_text(OWL_ICON)),
            409,
            "You already have this item.",
        ),
        (json!(ICON), 400, "This item is not available for purchase."),
        (json!("-5"), 400, "Invalid item identifier."),
        (json!(0), 400, "Invalid item identifier."),
        (json!("ring"), 400, "Invalid item"),
        (Json::Null, 400, "Invalid item"),
    ] {
        let error = api.purchase(&data(json!({"guid": guid}))).unwrap_err();
        assert_eq!(error, ApiError::new(status, message), "{guid}");
    }
    api.update_profile(&data(json!({"credits": 99}))).unwrap();
    let error = api.purchase(&data(json!({"guid": SPRAY}))).unwrap_err();
    assert_eq!(error, ApiError::new(409, "Not enough funds."));
    assert_eq!(fixture.saved("Beta").credits, 99);
}

#[test]
fn opening_every_box_grants_new_items_and_counts_them_for_the_menu() {
    let fixture = Fixture::new();
    let api = fixture.api();
    let boxes = fixture.saved("Beta").loot_boxes.len();
    let reply = api.open_all_boxes(&data(json!({}))).unwrap();
    assert_eq!(reply["opened"], boxes);
    let saved = fixture.saved("Beta");
    assert!(saved.loot_boxes.is_empty());
    assert_eq!(saved.stats["boxes_opened"], boxes);
    assert_eq!(reply["new_items"], saved.unlocked_items.len());
    assert_eq!(
        saved.settings["+0x130"],
        json!([{"+0x0": boxes, "+0x8": BOXES_OPENED_KEY}])
    );
    let error = api.open_all_boxes(&data(json!({}))).unwrap_err();
    assert_eq!(error, ApiError::bad("No boxes to open"));
}
