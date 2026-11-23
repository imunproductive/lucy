//! The files the server reads from a derived game data folder.

use std::{fs, io, path::Path};

/// The key the lobby signs its state blob with: the client's own, which
/// `lucy-assets` reads from the client program.
pub fn lobby_state_blob_key(root: &Path) -> io::Result<[u8; 64]> {
    let path = root.join("data/lobby_state_blob.key");
    fs::read(&path)?.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not 64 bytes", path.display()),
        )
    })
}

/// Every file under `data/` that the lobby and the match worker read. The
/// install derivation (`lucy-assets`) writes them all.
pub const FILES: &[&str] = &[
    "extracted_items.json",
    "lobby_schemas.json",
    "game_schemas.json",
    "bnet_descriptors.pb",
    "lobby_state_blob.key",
    "resource_keys.json",
    "extracted_general_unlocks.json",
    "extracted_heroes.json",
    "extracted_maps.json",
    "arcade_cards_174.json",
    "career_stats_174.json",
    "economy_174.json",
    "competitive_seasons_174.json",
    "lobby_preload_174.json",
    "game_progression_174.json",
    "game_queues_174.json",
    "game_eyes_174.json",
    "game_hitboxes_174.json",
    "game_bot_animation_174.json",
    "game_anim_lengths_174.json",
    "ai_spawners_174.json",
    "collision_props_174.json",
    "game_ai_definitions_174.json",
    "game_graphs_174.json.gz",
    "game_heroes_174.json",
    "game_maps_174.json",
    "game_respawns_174.json",
    "game_spawns_174.json",
    "game_modes_174.json",
    "game_movers_174.json",
    "game_objectives_174.json",
    "game_rulesets_174.json",
    "game_stat_rules_174.json",
    "game_stats_174.json",
    "game_tunables_174.json",
    "game_voice_sets_174.json",
    "workshop_catalog_174.json",
];

/// The files of [`FILES`] missing from the game data folder `root`, as
/// `data/<name>`.
pub fn missing_files(root: &Path) -> Vec<String> {
    FILES
        .iter()
        .map(|name| format!("data/{name}"))
        .filter(|name| !root.join(name).is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_only_the_missing_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("data")).unwrap();
        for name in &FILES[1..] {
            std::fs::write(root.path().join("data").join(name), b"").unwrap();
        }
        assert_eq!(missing_files(root.path()), ["data/extracted_items.json"]);
    }
}
