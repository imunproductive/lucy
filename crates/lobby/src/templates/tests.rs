use super::*;
use lucy_wire::jam::codec::Schemas;

fn entry(key: u64, value: &str) -> Value {
    Struct(Fields::from([
        (0x0, Unsigned(key)),
        (0x8, Value::String(value.into())),
    ]))
}

#[test]
fn the_config_sets_this_build_the_menu_hero_and_the_console() {
    assert_eq!(
        menu_config(None, false),
        Fields::from([(0x78, Array(vec![entry(SUPPORTED_BUILDS_KEY, "[104319]")]))])
    );
    assert_eq!(
        menu_config(Some(0x02e0_0000_0000_0002), true),
        Fields::from([(
            0x78,
            Array(vec![
                entry(SUPPORTED_BUILDS_KEY, "[104319]"),
                entry(MENU_HERO_KEY, "0x02e0000000000002"),
                entry(DEV_FEATURES_KEY, "true"),
                entry(ENGINE_CONSOLE_KEY, "true"),
            ])
        )])
    );
}

fn header(name: &str, variant: Option<&str>, kind: &str, state: &str, map: &str) -> MapHeader {
    MapHeader {
        name: Some(name.into()),
        variant_name: variant.map(Into::into),
        state: Some(state.into()),
        map_type: Some(kind.into()),
        map_guid: Some(map.into()),
        game_modes: Some(vec![Json::Null]),
    }
}

/// One map per name and variant, the first header by GUID; no menu scenes,
/// tutorial, header without modes or disabled PvE mission.
#[test]
fn custom_games_list_each_map_once() {
    let without_modes = MapHeader {
        game_modes: Some(Vec::new()),
        ..header("Paris", None, "OffenseDefenseMap", "Enabled", "891.002")
    };
    let headers = vec![
        (
            "0002.09F".into(),
            header("Busan", None, "MirroredMap", "Enabled", "A14.002"),
        ),
        (
            "0001.09F".into(),
            header("Busan", None, "OffenseDefenseMap", "Enabled", "7E2.002"),
        ),
        (
            "0003.09F".into(),
            header("Busan", Some("Lunar"), "MirroredMap", "Enabled", "A86.002"),
        ),
        (
            "0004.09F".into(),
            header(
                "Menu/Paris",
                None,
                "OffenseDefenseMap",
                "Enabled",
                "AF0.002",
            ),
        ),
        (
            "0005.09F".into(),
            header("Tutorial", None, "OffenseDefenseMap", "Enabled", "661.002"),
        ),
        ("0006.09F".into(), without_modes),
        (
            "0007.09F".into(),
            header("Rialto", None, "PVE", "Disabled", "79F.002"),
        ),
        (
            "0008.09F".into(),
            header("Junkenstein", None, "PVE", "Enabled", "6C9.002"),
        ),
    ];
    let maps = |indexes: &[u64]| {
        Array(
            indexes
                .iter()
                .map(|&index| Unsigned(0x0800_0000_0000_0000 | index))
                .collect(),
        )
    };
    assert_eq!(
        custom_game_maps(headers).unwrap(),
        Fields::from([
            (0x78, maps(&[0x7e2, 0xa86, 0x6c9])),
            (0x90, Array(vec![Unsigned(PRACTICE_RANGE)])),
        ])
    );
}

fn content() -> InstallContent {
    InstallContent {
        preload: Fields::from([
            (0x78, Array(vec![Unsigned(1)])),
            (0x90, Array(vec![Unsigned(42), Unsigned(43)])),
            (0xa8, Array(Vec::new())),
            (0xc0, Array(Vec::new())),
        ]),
        custom_game_maps: Fields::from([(0x78, Array(Vec::new())), (0x90, Array(Vec::new()))]),
        stat_catalogs: StatCatalogs::default(),
    }
}

#[test]
fn the_preload_adds_extra_resources_once() {
    assert_eq!(
        content().preload(&[43, 90, 90, 7])[&0x90],
        Array([42, 43, 90, 7].map(Unsigned).to_vec())
    );
}

/// Built messages take their constant's place; the ones without a constant
/// must be built.
#[test]
fn the_login_sends_constants_and_built_messages_in_order() {
    let built: BTreeMap<_, _> = [
        (LOBBY, 20812),
        (LOBBY, 20813),
        (PASSES, 58501),
        (ARCADE, 39802),
        (ARCADE, 39810),
        (ARCADE, 39804),
        (LOBBY, 20814),
    ]
    .into_iter()
    .map(|key| (key, Fields::from([(0x78, Bool(true))])))
    .collect();
    let messages = content().at_login(None, false, 7, "EU", &built).unwrap();
    let keys: Vec<_> = messages.iter().map(|&(crc, id, _)| (crc, id)).collect();
    let expected: Vec<_> = AT_LOGIN
        .iter()
        .flat_map(|&(crc, id)| match (crc, id) {
            (OWL_LIVE, 43200) => vec![(crc, id); 2],
            (lookups::STORE, 26400) => vec![(crc, 26404), (crc, 26400)],
            _ => ids(crc, id).into_iter().map(|id| (crc, id)).collect(),
        })
        .collect();
    assert_eq!(keys, expected);
    assert_eq!(messages[0].2, built[&(LOBBY, 20814)]);
    let value = |id| &messages.iter().find(|message| message.1 == id).unwrap().2;
    assert_eq!(value(36602)[&0x78], Unsigned(7));
    assert_eq!(value(26400)[&0x78], Unsigned(978));
    assert_eq!(*value(36600), menu_config(None, false));

    let mut missing = built;
    missing.remove(&(ARCADE, 39802));
    assert_eq!(
        content()
            .at_login(None, false, 7, "EU", &missing)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

/// The install's preload (build 104319): the 56 hero records, their 2,483
/// skin themes (the 2,457 of the 1.68 retail capture and 26 newer), the 132
/// map headers and the one `10D` record; the custom game maps are the 70 of
/// the capture. Every static message encodes.
#[test]
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn the_installs_preload_and_custom_game_maps() {
    let content = InstallContent::load(&lucy_testkit::runtime_dir().join("data")).unwrap();
    let count = |fields: &Fields, offset| match &fields[&offset] {
        Array(values) => values.len(),
        _ => panic!("a list"),
    };
    let preload = content.preload(&[]);
    assert_eq!(
        [0x78, 0x90, 0xa8, 0xc0].map(|offset| count(&preload, offset)),
        [56, 2483, 132, 1]
    );
    assert_eq!(preload[&0xc0], Array(vec![Unsigned(0x0308_0000_0000_0001)]));
    let Array(themes) = &preload[&0x90] else {
        panic!("themes")
    };
    // The Anniversary menu themes are among them.
    for index in [
        0x49e1, 0x48a9, 0x49cf, 0x49e4, 0x49d8, 0x49d9, 0x49d1, 0x49a8,
    ] {
        assert!(themes.contains(&Unsigned(0x0a50_0000_0000_0000 | index)));
    }
    assert_eq!(count(&content.custom_game_maps, 0x78), 70);
    let Array(maps) = &content.custom_game_maps[&0x78] else {
        panic!("maps")
    };
    assert_eq!(maps[0], Unsigned(0x0800_0000_0000_005b));
    assert!(maps.contains(&Unsigned(PRACTICE_RANGE)));

    let built: BTreeMap<_, _> = [
        (LOBBY, 20812),
        (LOBBY, 20813),
        (PASSES, 58501),
        (ARCADE, 39802),
        (ARCADE, 39810),
        (ARCADE, 39804),
    ]
    .into_iter()
    .map(|key| (key, Fields::new()))
    .collect();
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    for (crc, id, fields) in content.at_login(Some(2), true, 7, "US", &built).unwrap() {
        if !built.contains_key(&(crc, id)) {
            schemas.encode(crc, id, &fields).unwrap();
        }
    }
    schemas.encode(IN_CONNECT, 20505, &preload).unwrap();
}
