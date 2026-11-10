//! Reads the real build 104319 install: `just test-game <install>`.

use orpheus_casc::Game;

const GRAPH: u16 = 0x01B;
const DEFINITION: u16 = 0x003;

/// DataTool's offline extraction found 1,587 graphs and 3,528 definitions
/// (`docs/notes/data-sources.md`); it also had keys the install lacks.
#[test]
#[ignore = "reads an Overwatch 1.74 install in ORPHEUS_GAME_DIR"]
fn every_stored_graph_and_definition_reads_and_matches_its_key() {
    let game = Game::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    assert_eq!(
        game.storage().info().get("Version"),
        Some("1.74.0.0.104319")
    );
    for (kind, count, not_stored) in [(GRAPH, 1587, 1), (DEFINITION, 3528, 19)] {
        let guids = game.guids(kind);
        assert_eq!(guids.len(), count);
        let stored: Vec<u64> = guids
            .iter()
            .copied()
            .filter(|guid| game.is_stored(*guid))
            .collect();
        assert_eq!(guids.len() - stored.len(), not_stored);
        for (guid, bytes) in game.read_all(&stored) {
            let bytes =
                bytes.unwrap_or_else(|error| panic!("{}: {error}", orpheus_casc::guid_name(guid)));
            assert_eq!(bytes.len(), game.asset(guid).unwrap().size as usize);
        }
    }
}
