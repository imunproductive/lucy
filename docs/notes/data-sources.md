# Data sources

What the server reads, where each input comes from, and the facts needed to rebuild it from the game install. Saved from the inherited documentation and tool comments.

## Runtime files

The server derives its game data from the install (`crates/assets`, phase 6 of the [refactor plan](../REFACTOR.md)) into `<state>/cache/104319`, in the format of the prepared runtime pack the inherited server read, so the loaders read it unchanged. A stamp file (`derived.txt`) records the build, build config, locale and derivations version; a cache with another stamp is derived again. The prepared pack stays as the oracle each derivation is checked against (`just test-assets`, `orpheus assets diff --pack`) and as input for the frozen Python server, until phase 8. The fixed data is embedded in the binary (`crates/fixed`).

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
- **Structured data (`crates/stu`):** an STUv2 asset starts with three `{i32 count, i32 offset}` tables (instances `{class hash, field hash, instance, size}`, inline array types, field bags of `{field hash, size}`), then the size and offset of the dynamic buffer and the offset of the data buffer. Each instance in the data buffer is a field bag index and that bag's fields; a size of 0 means the field's size precedes it. Arrays, strings and hash maps live in the dynamic buffer. Asset references are stored xored with the field hash (repeated in both halves) and the CRC-64 of the 36-byte header (reflected polynomial `0x95AC9329AC4BC9B5`, zero start, inverted output), with bytes 0/3, 7/1, 2/6 and 4/5 swapped. Fields an asset doesn't store take the class's declared default.
- **What the 104319 install can't provide:** 8 bundles are encrypted with key `28B8EBB462B83EEA`, which the install's keyring lacks (OWLib's support keyring labels it "[1.74] ???"). With them, a few assets of other types are unavailable: 1 of 1,587 graphs (`1D6D`), 19 of 3,528 definitions, maps `0EEE` and `0F14`, and some models and strings. Those two maps are main menu lobby maps, "OWL Summer Showdown 2022" and "Goodbye" ([Logo2K's lobby map list](https://rentry.co/wcwyv3r8)); nothing in this repository references them. Every other graph and definition reads and matches its content key (`just test-game`).

## Pinned tools and the recorded extraction

- OWLib `overwatch/1.74` at `7afe5f52706e7002ed8c2717299cd60846f3fa3a` (DataTool, TankLib, STU types).
- TACTLib at `a00459fe0885a58da19bceb0a366605c92f1927b`. The build manifest parser is `TACTLib/Core/Product/Tank/CMF/ProCMF_104319.cs`.
- revorbstd at `8204be4f86811aa71007cd0d0ca8a3a198ea4273`.
- DataTool ran fully offline: language and speech language zhCN, no language registry, no cache, no CDN data, no manifest fallback, online off. It identified build 1.74.0.0.104319 and its TRG/CMF procedures.
- Output: 3,528 `003` definition files and 1,587 `01B` graph files. The XML uses dot decimals.
- The graph bundle for 32 hero bodies and 373 graphs has the normalized SHA-256 `febfbcc8d406ea59f76b10029cc825b765b49ece29c8020b9ba7929b3671104f`. Graph `0F9C` is missing. Graphs `288B`, `05AD` and `0D24` have input plugs with unknown ownership; their targets are not guessed.
- Practice Range bot bodies: `003` indexes `1403`, `143E`, `146E`, `14A9`, `14D2`, `14ED`.

## Fixed data (`crates/fixed/data`)

Committed and embedded in the binary. Taken from LizRollin's repository (MIT) at `9f8f3bcb5eccab6fb7738bee3c2371fcf0623f29` and renamed:

| File | Source in that commit | SHA-256 |
| --- | --- | --- |
| `lobby_schemas.json` | `data/schemas_174.json` | `bd0d752bc365df93ab35b3749add3e34c690d06b33ba6ea9293d4589cc2cff53` |
| `game_schemas.json` | `data/game_schemas_174.json` | `904f77bd3d5fc18a5348aa32d5274c2015454aff274c13c7864a24306dccc12d` |
| `statescript_codecs.json` | `data/statescript_codecs_174.json` | `49834e2c3b2a9761011be0b17d5b742e784a148090ed6f0430771bb7e6bea019` |
| `bnet_descriptors.pb` | `ow174/bnet/descriptors.pb` | `c20cac09537c013ef359d3c47aa39b178c51c3efe2258308ca0b1e8ee8f0fb2d` |
| `lobby_state_blob.key` | `BLOB_KEY` in `ow174/jam/handshake.py` | `ab024361c5acd29e3a45548cc675002b762895753fe6dc7613335b1f0ff033b8` |
| `retail_templates.json` | `data/retail_templates.json` | `667cf09afd5a2b889b4c95e7957e0d2d90d5a7a711cb527fc8159bed2c5602e5` |
| `resource_keys.json` | `data/resource_keys_174.json` | `47084948c33f4d2437f0f9c6b452b10a014beedd3a99cef8c3ee3a72e8bd7ba2` |

## Where a runtime pack comes from

The upstream ORPHEUS repository never committed the pack's files: its `data/` held only `README.md` and replays in all 204 commits, and `prepare_server_runtime` only copies files from a private "prepared checkout". The pack now holds only game data, which phase 6 moves to CASC; the fixed data is embedded. LizRollin's repository commits all of its files in `data/`, except the collision caches. The collision caches: `tools/build_collision.py <map>` with `ORPHEUS_RUNTIME_DIR` set to the pack builds them from the install. `0688.col` built this way matches the recorded SHA-256 below.

The pack has two graph bundles, both made by `tools/extract_game_graphs.py` from DataTool XML dumps of the install:
- `game_graphs_174.json.gz`, the hero bundle the server runs: 373 graphs, 32 bodies, `0F9C` missing. `--xml <dump> --owlib <OWLib>` alone gives exactly LizRollin's file. The pack's copy also has the 25 modes' variables (`--mode-xml <dump>/0C5`) and each body's authored target tags (`orpheus runtime prepare --game-graphs <bundle> --target-tags-xml <dump>/003`), as upstream's had: without them the Soldier: 76 VM stops earlier (see `docs/notes/parity.md`). SHA-256 `a1d42e17781904d9a209c432f1655f3504e30c93e3f303a799ec2509dc092431`.
- `game_graphs_bots_174.json.gz`, for VM tests of the bot graphs (`testkit::bot_graphs`): 405 graphs, 8 entity definitions, 25 modes' variables. The same plus `--ai-spawners <pack>/data/ai_spawners_174.json --mode-xml <dump>/0C5 --entity 1403 --entity 143E --entity 146E --entity 14A9 --entity 14D2 --entity 14ED --entity 1404 --entity 14AF` (the six Practice Range bot bodies and the AI definitions their `m_8B9A461F` names). It holds the Practice Range bot graph `0B92`. SHA-256 of the built file: `6821d7a3c53d4efd12a26959164c39545b781604ea4407feb4ac57432f86d990`. Its digest matches the Python reference vector for the 405-graph bundle. The server can't use it yet: with bot graphs the Practice Range scene is not movement-ready.

The dumps: OWLib at the pinned revision, built with the .NET 8 SDK (a `global.json` pinning `8.0.418`; SDK 10's C# 14 breaks TACTLib's `Reverse()` calls) and run with `DOTNET_ROLL_FORWARD=Major`: `dotnet DataTool.dll --language=enUS --speech-language=enUS <install> extract-stu-type <dump> <type> --xml` for `01B`, `003` and `0C5`. It reads 1,587 `01B` and 3,528 `003` files from this install, offline.

The server logs at start that the Practice Range Soldier: 76 owner statescript is unavailable; `docs/notes/parity.md` explains the blocker.

## Collision caches

- Practice Range `0688.col`: format 5, 470,397 bytes, SHA-256 `dfdb71fbe9ad079d9d0b56fe12bd7c992a76c6ec942084d86f6388827f36a169`.
- All 95 catalog maps have integrity-checked format-5 caches. Workshop Green Screen uses a checked slice of its CASC bundle; both the container and the asset content keys are verified.
- Ray caches: sphere and capsule props with positive uniform scale use analytic intersections. Nonuniformly scaled round props use triangle projection. A ray cache verifies the map and the original chunk key and selects props for the requested mode. Attached child geometry, server-generated and dynamic objects are not covered.
