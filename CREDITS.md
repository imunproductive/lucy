# Credits

This repository is a fork of [ORPHEUS](https://github.com/leafall903/ORPHEUS) and is being rewritten. Code from these projects is still in it. Their MIT notice is kept in [LICENSE](LICENSE).

- **[ORPHEUS](https://github.com/leafall903/ORPHEUS)** (leafall903 and contributors): the Rust port, the dashboard and the server integration this fork starts from.
- **[Boi-027, Overwatch 1.74 Lobby Research](https://github.com/Boi-027/Overwatch-1-v1.74-Lobby-Research)** (copyright Arlecchino): the lobby protocol, handshake, JAM cipher and framing, and the lobby services.
- **[LizRollin, overwatch-1.74-lobby-server](https://github.com/LizRollin/overwatch-1.74-lobby-server/tree/9f8f3bcb5eccab6fb7738bee3c2371fcf0623f29)**: the game transport, statescript runtime and payload layouts, collision, character mover and combat, and the fixed build data in `crates/fixed/data`.
- **[AipNooBest, overwatch-1.74-lobby-server](https://github.com/AipNooBest/overwatch-1.74-lobby-server/tree/860bab45)** (copyright Arlecchino, MIT): the live finding that a gun shows only when its definition is preloaded at login.
- **[TACTLib](https://github.com/overtools/TACTLib/tree/a00459fe0885a58da19bceb0a366605c92f1927b)** (overtools, MIT): the CASC storage, BLTE, encoding table, bundle and content manifest formats, the build 104319 manifest key procedure, and the 1.74 encryption key from its static `Overwatch.keyring` in `crates/casc`.
- **[OWLib](https://github.com/overtools/OWLib/tree/7afe5f52706e7002ed8c2717299cd60846f3fa3a)** (overtools, MIT): the structured data (STUv2) format in `crates/stu`, and the type table it generates from TankLib's STU types; DataTool's hero, map and unlock lists and its `GUIDNames.csv` names in `crates/assets`; TankLib's model skeleton chunk reader (`teModelChunk_Skeleton`, the bone matrices as `ExportFormats/OverwatchModel.cs` reads them) in `crates/assets/src/hitboxes.rs`; TankLib's animation reader (`teAnimation`: the clip header, track tables and packed rotations and scales) in `crates/assets/src/animation.rs`.
- **The authors of the community TLS relay**: the basis of the TLS relay in `crates/relay`.
- **[ORPHEUS](https://github.com/leafall903/ORPHEUS/tree/c239a84)** (leafall903, MIT), commit c239a84: the state-input join (`STU_1113EBD5`), the StateElapsed condition (`STU_4E2BCAAB`), the indexed action Switch, the ability's early-press fix and the finding that `STU_6E5C5F66` only records trace data, in `crates/script/src/vm` (`flow.rs`, `abilities.rs`, `nodes.rs`).

The upstream projects built on research shared by Xref, the AyakaPS community, Logo2K, Zagrion, Sidiusz and Blizless. The in-game HUD's team colours follow research published by [p3n1cillin/ow1-preservation](https://github.com/p3n1cillin/ow1-preservation) (no code taken).
