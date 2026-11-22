use std::fs;

use lucy_casc::{Game, guid_type};
use lucy_testkit::casc::{DEFINITION, GRAPH, MiniInstall, guid};

#[test]
fn reads_stored_and_bundled_assets_by_guid() {
    let root = tempfile::tempdir().unwrap();
    let graph = guid(GRAPH, 0x1D8E);
    let definitions = [guid(DEFINITION, 7), guid(DEFINITION, 8)];
    MiniInstall::default()
        .asset(graph, b"graph bytes")
        .bundled(definitions[0], b"first definition")
        .bundled(definitions[1], b"second")
        .write(root.path());

    let game = Game::open(root.path(), "enUS").unwrap();
    assert_eq!(
        game.storage().info().get("Version"),
        Some("1.74.0.0.104319")
    );
    assert_eq!(game.guids(GRAPH), [graph]);
    assert_eq!(game.guids(DEFINITION), definitions);
    assert_eq!(guid_type(graph), GRAPH);
    assert_eq!(game.read(graph).unwrap(), b"graph bytes");
    assert_eq!(game.read(definitions[1]).unwrap(), b"second");
    let all = game.read_all(&[definitions[1], graph, definitions[0]]);
    let all: Vec<_> = all
        .into_iter()
        .map(|(guid, bytes)| (guid, bytes.unwrap()))
        .collect();
    assert_eq!(
        all,
        [
            (definitions[0], b"first definition".to_vec()),
            (definitions[1], b"second".to_vec()),
            (graph, b"graph bytes".to_vec())
        ]
    );
    assert!(game.read(guid(GRAPH, 1)).is_err());
}

#[test]
fn damaged_bytes_fail_their_content_key() {
    let root = tempfile::tempdir().unwrap();
    let graph = guid(GRAPH, 1);
    let definition = guid(DEFINITION, 2);
    MiniInstall::default()
        .asset(graph, b"intact graph")
        .bundled(definition, b"bundled definition")
        .write(root.path());
    let archive = root.path().join("data/casc/data/data.000");
    let mut data = fs::read(&archive).unwrap();
    for needle in [&b"intact graph"[..], b"bundled definition"] {
        let at = data
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap();
        data[at] ^= 1;
    }
    fs::write(&archive, data).unwrap();

    let game = Game::open(root.path(), "enUS").unwrap();
    assert!(
        game.read(graph)
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
    // The bundle itself fails its key, so the asset in it can't be read.
    assert!(game.read(definition).is_err());
    assert_eq!(game.unreadable_bundles().len(), 1);
}
