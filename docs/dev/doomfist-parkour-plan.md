# Plan: Doomfist parkour (custom games, Workshop, Doomfist movement)

Goal (user, 2026-11-20): a player creates a custom game, loads a Doomfist parkour Workshop script (first target: Hanamura, code `C32S2`, `D:/Downloads/brave/personal/dfp-archive/workshop.codes/C32S2_OG_doom_parkur_hanamura.txt`) and plays it: the rules run, the HUD, effects and checkpoints show, and Doomfist moves exactly as the client predicts.

Earlier findings this starts from: `D:/Work/Coding/refs/reports/GameSettings.md` (§3 lobby side, §4 Workshop, "Client tools pass" 15.4 custom game messages 24000-24028 / 23300-23333, 15.7 `STU_FC3921B1`), `AdminMode.md` (`CGGameRuleset`, `globalAdminVars`), `TrainingPartner.md` (dummy bots), `LobbyGap.md`. The archive's README lists 7 full DPK scripts and the OverPy decompiles (`.opy`) next to each.

## What C32S2 needs

- **Settings:** Assault on Hanamura only; team 1 up to 12 players, team 2 none; Doomfist only; no hero switching, kill cam, kill feed, health packs or ult generation; respawn time 0 %; game mode starts at once; return to lobby never.
- **Workshop:** 3540 lines, about 90 global and 20 player variables, subroutines. Actions it uses, by count: If/Else If/While/Skip/Abort/Loop (control flow), Call Subroutine, Wait/Wait Until, Create HUD Text (16), Create Icon (24), Create Effect/Play Effect, Create In-World Text, Small/Big Message, Modify/Set variables (incl. At Index), Chase Player Variable At Rate, Teleport, Start/Stop Forcing Player Position, Set Ability Cooldown, Set Move Speed, Set Secondary Fire/Jump/Crouch/Ultimate Enabled, Allow/Disallow Button, Press/Start Holding Button, Cancel Primary Action, Start/Stop Camera, Start/Stop Facing, Set Facing, Set Invisible, Start Scaling Player, Start Forcing Player Outlines, Disable Movement Collision With Environment, Respawn, Resurrect, Kill, Set Status, Set Damage Dealt, Set Match Time, Set Player Allowed Heroes, Set Ultimate Charge, Start Forcing Dummy Bot Name. Values: Is Button Held, Is On Ground/In Air, Is Firing Secondary, Distance Between, Position Of, Speed Of, Throttle Of, vector math, Custom String, Input Binding String, Ability Icon String, arrays, Is True For Any, Map.

## Unknowns that decide the design (research first)

1. **How the client handles Workshop rules in a match.** Does the client compile rules into statescript (then the server must build the same graph and run it in our VM, and the client syncs it like any graph), or does the server run a rule engine and send only results (HUD text, effects, icons) through dedicated messages? Where the client parses the Workshop text (Paste), its in-memory form, and how the rules travel (custom game settings `CGGameRuleset` / 23300 / 24008 …). This decides phase 3. Per AGENTS.md: if the client runs graphs for the Workshop, we run the same graphs, not a Rust copy.
2. **The custom game lobby protocol:** create (24000 kind 1/3), settings messages (24008-24028 ↔ 23300-23309), "Import code" (share code lookup), the lobby screen's team slots, Start, and the handoff of the settings to the game server (20300/20301 game struct, 53001).
3. **Doomfist on the server vs his client:** each ability (Hand Cannon, Rocket Punch incl. wall impact, Seismic Slam, Rising Uppercut, Meteor Strike, the passive) and the parkour techniques (punch bounce, slam distance, uppercut-cancel, "bullet punch" cancel): which graphs and mover natives run, where the server's movement differs from the client's prediction. Known bug: the Rocket Punch works on the server but the client's camera sits out of bounds (`loose-ends.md`).

## Phases

1. **Research (parallel):** unknowns 1, 2 and 3, each into a report under `D:/Work/Coding/refs/reports/` (`Workshop.md`, `CustomGames.md`, `Doomfist.md`) and the index.
2. **Custom games:** create a custom game from the client, its lobby screen with the host seated, settings changed from the client and from imported text or code (the server answers share codes from the archive), Start → a match on the chosen map with those settings applied (hero pool, respawn scalar, ult generation, kill feed, health packs, team sizes, mode start).
3. **Workshop runtime:** by what unknown 1 finds: either the client's compile-to-graph path reproduced on the server, or a server rule engine plus the result messages. Build it up action by action, driven by C32S2, with each action tested against the client's behaviour; then the other DPK scripts in the archive.
4. **Doomfist movement:** fix each ability's server movement to match the client's prediction (owner corrections must not snap him), starting with the Rocket Punch camera.
5. **End to end:** C32S2 played through level 1 in the game: checkpoints, HUD, teleports, timers; then the other levels.

Phases 2 and 4 don't depend on unknown 1 and run alongside it; phase 3 starts when unknown 1 is answered.

## Status

- 2026-11-20: plan written; phase 1 research started (agents WorkshopClient, CustomLobby, DoomfistMoves).
