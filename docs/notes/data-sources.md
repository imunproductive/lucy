# Data sources

What the server reads, where each input comes from, and the facts needed to rebuild it from the game install. Saved from the inherited documentation and tool comments.

## Runtime files

Today the server reads a prepared runtime pack (`ORPHEUS_RUNTIME_DIR`). Phase 6 of the [refactor plan](../REFACTOR.md) replaces the derivable files with reads from the game install.

| File | Content | Made by | Inputs |
| --- | --- | --- | --- |
| `schemas_174.json` | Layouts of 707 lobby messages in 87 protocol groups | `dump_schemas.py` from a running client or a memory dump | Client memory (fixed data) |
| `game_schemas_174.json` | Layouts of the game link messages | `dump_schemas.py --game-link` | Client memory (fixed data) |
| `statescript_codecs_174.json` | What the client reads for each statescript state class in a frame (property block and class reader) | `extract_statescript_codecs.py`, an IDA script over a client memory image (base `0x7FF788F10000`) | Client code (fixed data) |
| `bnet/descriptors.pb` | Battle.net protobuf descriptor set | Extracted from the client | Client binary (fixed data) |
| `lobby_state_blob.key` | 64-byte signing key for the lobby state blob | — | Fixed data |
| `retail_templates.json` | Decoded server templates, including catalogs and prices. Display-name markers are anonymized. | Decoded from 1.68 reference traffic with the 1.74 schemas | Captured server data (fixed data) |
| `resource_keys_174.json` | Content keys for event resources, sent by the server to the client. Not account credentials. | — | Fixed data |
| `extracted_heroes.json` | Hero identifiers and names | No producer in the repository | To classify (phase 6c.0) |
| `extracted_maps.json` | Map metadata and seasonal variants, English names | No producer | To classify |
| `extracted_items.json`, `extracted_general_unlocks.json` | Cosmetic names, types, rarity, categories and rewards | No producer | To classify |
| `collision_keys_174.json` | Storage keys of each map's collision chunk | No producer | To classify; expected unnecessary once the manifest resolves GUIDs |
| `collision_props_174.json` | Storage keys of each map's props and their models, and which modes keep which props | No producer | To classify |
| `game_heroes_174.json` | Each playable hero's GUID, name and the `003` body the server creates | `extract_game_heroes.py` | `075` hero assets, `extracted_heroes.json` names |
| `game_graphs_174.json.gz` | The statescript graphs the server runs: everything reachable from the 32 hero bodies (statescript component initial graphs, weapon scripts), the controller, hero select, HUD and mode graphs, following every `01B` reference | `extract_game_graphs.py` | `01B`, `003`; optional `0C5` mode variables (`--mode-xml`), extra `003` entities (`--entity`), map AI spawner graphs (`--ai-spawners`) |
| `game_maps_174.json` | Per map: modes, server-owned placeables, spawn points | `extract_game_maps.py` | `003`, `09F`, `0C5`, strings, raw map chunks `0BC`, map entity dumps, `extracted_maps.json` |
| `game_queues_174.json` | Per queue card: name, rulesets (mode, players per team, free-for-all), role slots, maps | `extract_game_queues.py` | `0C7`, `0C0`, `0C5`, `039`, `09F`, `0D9`, `0EE`, `0CC`, `07C` strings, plus `retail_templates.json` |
| `arcade_cards_174.json` | Arcade cards | `extract_arcade_cards.py` | `0C7`. Identifier, display entry, reward unlocks and seven of eleven flags come from the asset. How long a card stays and whether it is permanent are set by the server. |
| `game_movers_174.json` | Character mover values (speeds, jump, gravity, capsule) for each hero and bot body | `extract_game_movers.py` | `003` (`STUCharacterMoverComponent`, `0x6EFDBD73`), `game_heroes_174.json`, `ai_spawners_174.json` |
| `game_stats_174.json` | Stats on the Tab board: player stats and per-hero stats | `extract_game_stats.py` | `054`, `075`, `062`, `game_heroes_174.json` |
| `ai_spawners_174.json` | Per map: AI spawners (hero, team, place, route) and hint points; the bot heroes' bodies, health and speed | `extract_ai_spawners.py` | Map ENTITY chunks from the install, `075`, `003`, `collision_props_174.json`, `extracted_heroes.json` |
| `collision/<map>.col` | Movement collision cache (format 5) | `build_collision.py` | Map collision chunks and prop models from the install |
| `collision/<map>.rays.col` | Optional static ray geometry | `build_collision.py --ray-geometry` | Same, plus optional exported prop records |
| Raycast metadata | Raycast filters and receivers | `extract_raycast_metadata.py` | `097`, `098` |
| Score events | `003F.054` label rows and `StatEventScoreScaler` rows | `extract_game_stats.py --score-events` | `054` |

Research-only files: `statescript_names_174.json` (names for `ssq.py`), `announced_crcs_174.json` (protocol identifiers for `dump_schemas.py`), `extracted_events_174.json` (verified scene mappings used by tests).

The fixed data most likely came from client dumps and traffic captured while the live servers were still running. Its origin is not tracked further.

## Asset formats

- **ENTITY map chunk** (type `0x0B`, as TankLib's `teMapChunk` reads it): header `{u32 placeable count, u32 instance data offset, u32 placeable offset, u32}`. Each placeable has a 24-byte header `{uuid, u16, u8, u8 type, u32 size}` and a body `{u64 definition, u64 identifier 1, u64 identifier 2, f32 translation[3], scale[3], rotation[4], u32 instance count, …}`. From body `+112`, one `{u32 type, u32 offset}` per instance, each an STUv2 blob (`"DUTS"`, u32 version, u32 instance offset).
- **AI spawner** instance `STUECAISpawnerInstanceData` (`0x99257985`): `+8` the hero (`075` asset reference, GUID in the second half), `+24` an identifier (`01C`; 0 means every mode). Hint instance hash `0xF199249B`.
- **Map placeables:** the client gives map placeables one flat index over the placeable types whose flag is set in its type table (light, area, entity, … in that order), so the first entity's index is `count(light) + count(area)`. Placeable type is byte 19 of each placeable: light 7, area 8, entity 9. Spawn component bit 58 (`0x737C5C41`), required component bit 94 (`0xCA1FB0B8`).
- **CASC (local install):** the local index `data\casc\data\XX*.idx`. Bucket XX is the first 9 bytes of the encoding key XORed together and folded to 4 bits; the newest version of each bucket wins. Entries are 18 bytes: 9 bytes of key, 5 bytes big-endian `archive << 30 | offset`, u32 size. In `data.NNN` at that offset sits a 30-byte header (the key reversed, u32 size, …) followed by the BLTE file: `'BLTE'`, u32 header size, then blocks stored (`N`) or zlib (`Z`). The content key is the file's MD5. An asset inside a bundle has its own content key and size, plus the bundle's keys, size and the asset's offset.
- **CASC GUID lookup (`crates/casc`):** `.build.info` (active row) names the build config and keyring under `data\casc\config\ab\cd\<hash>`. The build config names the encoding table (by encoding key) and the root file (by content key). The root file lists the `RDEV` content manifests (`.cmf`): one base, plus text and speech per locale. A 104319 CMF header is twelve u32s ending in asset count, entry count and magic; the body is AES-256-CBC encrypted with TACTLib's `ProCMF_104319` key and an IV from the SHA-1 of the manifest's file name. Assets are 29 bytes `{GUID, u32 size, u8, content key}`. Bundles (`09C`) start `{i32 count, u8 offset size}` with `{GUID, offset}` entries; the asset size comes from the manifest. Encrypted BLTE blocks (`E`) are Salsa20 with a keyring key; the IV's low 32 bits are xored with the block number.
- **What the 104319 install can't provide:** 8 bundles are encrypted with key `28B8EBB462B83EEA`, which the install's keyring lacks (OWLib's support keyring labels it "[1.74] ???"). With them, a few assets of other types are unavailable: 1 of 1,587 graphs (`1D6D`), 19 of 3,528 definitions, maps `0EEE` and `0F14`, and some models and strings. Nothing in this repository references them. Every other graph and definition reads and matches its content key (`just test-game`).

## Pinned tools and the recorded extraction

- OWLib `overwatch/1.74` at `7afe5f52706e7002ed8c2717299cd60846f3fa3a` (DataTool, TankLib, STU types).
- TACTLib at `a00459fe0885a58da19bceb0a366605c92f1927b`. The build manifest parser is `TACTLib/Core/Product/Tank/CMF/ProCMF_104319.cs`.
- revorbstd at `8204be4f86811aa71007cd0d0ca8a3a198ea4273`.
- DataTool ran fully offline: language and speech language zhCN, no language registry, no cache, no CDN data, no manifest fallback, online off. It identified build 1.74.0.0.104319 and its TRG/CMF procedures.
- Output: 3,528 `003` definition files and 1,587 `01B` graph files. The XML uses dot decimals.
- The graph bundle for 32 hero bodies and 373 graphs has the normalized SHA-256 `febfbcc8d406ea59f76b10029cc825b765b49ece29c8020b9ba7929b3671104f`. Graph `0F9C` is missing. Graphs `288B`, `05AD` and `0D24` have input plugs with unknown ownership; their targets are not guessed.
- Practice Range bot bodies: `003` indexes `1403`, `143E`, `146E`, `14A9`, `14D2`, `14ED`.

## Collision caches

- Practice Range `0688.col`: format 5, 470,397 bytes, SHA-256 `dfdb71fbe9ad079d9d0b56fe12bd7c992a76c6ec942084d86f6388827f36a169`.
- All 95 catalog maps have integrity-checked format-5 caches. Workshop Green Screen uses a checked slice of its CASC bundle; both the container and the asset content keys are verified.
- Ray caches: sphere and capsule props with positive uniform scale use analytic intersections. Nonuniformly scaled round props use triangle projection. A ray cache verifies the map and the original chunk key and selects props for the requested mode. Attached child geometry, server-generated and dynamic objects are not covered.
