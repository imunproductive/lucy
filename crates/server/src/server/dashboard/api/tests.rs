use super::*;
use orpheus_content::catalog::{ItemDb, UNLOCK_BASE};
use orpheus_lobby::templates::RetailTemplates;

const ICON: u64 = UNLOCK_BASE | 0x10;
const FRAME: u64 = UNLOCK_BASE | 0x20;

struct Fixture {
    state: tempfile::TempDir,
    accounts: Accounts,
    selected: Mutex<Arc<Account>>,
    online: Mutex<HashMap<u32, Arc<AtomicBool>>>,
    collection: Collection,
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
        let templates = RetailTemplates::from_reader(
            format!(
                r#"{{"format":1,"server":[
                    ["70E5A247",24900,{{"+0x80":[],"+0x98":[]}}],
                    ["779A8581",24300,{{"+0x78":{{"+0x30":[]}},"+0xF8":{{"+0x0":[
                        {{"+0x0":{FRAME},"+0x10":601,"+0x14":0,"+0x16":0}}
                    ]}}}}]
                ]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let mut items = ItemDb::default();
        for (guid, name, item_type) in [(ICON, "Ring", "Icon"), (FRAME, "Silver", "PortraitFrame")]
        {
            items.unlocks.insert(
                guid,
                Unlock {
                    guid,
                    name: name.into(),
                    item_type: item_type.into(),
                    rarity: "Common".into(),
                    hero: None,
                    source: String::new(),
                    box_type: None,
                    categories: Vec::new(),
                    available_in: String::new(),
                    skin_theme: 0,
                    esports_team: String::new(),
                },
            );
        }
        let collection = Collection::new(&templates, Arc::new(items)).unwrap();
        Self {
            state,
            accounts,
            selected,
            online: Mutex::new(HashMap::new()),
            collection,
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
    assert_eq!(icons["total"], 1);
    assert_eq!(icons["items"][0]["guid"], guid_text(ICON));
    assert_eq!(icons["items"][0]["purchasable"], false);
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
