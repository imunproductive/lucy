# Behavior and acceptance

What the server must do, what has been seen with the original client, and the known defects. Saved from the inherited documentation. New client acceptance results are added at the end.

## Acceptance targets

The first acceptance group is Soldier: 76 and the training bots in the Practice Range. Doomfist is the next hero.

| Milestone | Exit criteria |
| --- | --- |
| M0 Research and replay | Reproduce a known baseline with shared entity, time, input, event and state contracts. Track unsupported protocol and graph behavior. |
| M1 Body and weapon | Soldier: 76 keeps the expected model, weapon and ammo HUD for 60 seconds, with ten switch-and-return cycles and ten exits and rejoins. |
| M2 Rifle and prediction | Hold and release fire, empty the magazine and reload repeatedly. Client ammo and effects agree with the server, also with lost and repeated input. |
| M3 Single-player combat | Repeatedly hit, miss, kill and reset a training target. Its visible health, identity and combat feedback agree with the server. |
| M4 Movement and two clients | Two clients agree on movement, line of sight, combat and respawn. Loss, delay and reordering recover without extra damage. |
| M5 Match rules and heroes | Complete a full match in one mode on one map and return to the lobby. Each supported hero's declared abilities work. |
| M6 Fidelity and reliability | Timing and behavior compared against references. Long sessions, recovery, performance and repeatable deployment verified. |

Areas still to cover beyond the milestones:

- Login, profiles, progression, cosmetics, parties and other social features.
- Queues, team assignment, game setup, hero selection, handoff, joining, leaving, reconnecting and returning to the lobby.
- Movement authority, prediction and correction, input timing, and consistency for players and observers under delay, loss, reordering and repeated connections.
- Weapons, reloads, aiming, movement abilities, passives, cooldowns, resources, ultimates, transformations and deployables.
- Hit detection, projectiles, explosions, damage, healing, armor, shields, barriers, status effects, eliminations, assists and respawns. Projectile and hitscan penetration by weapon and surface.
- Map collision, movement surfaces, spawn rooms (placement, team permissions, effects), doors, health packs, destructibles, out-of-bounds death.
- Character collision for players and bots: allies pass through each other, live opponents block. Check player/player, player/bot and bot/bot in both directions, free-for-all, crouching, simultaneous movement, death and body replacement. Check the authored character-collision setting separately from map barrier masks.
- Objectives, scoring, timers, overtime, rounds, victory and results across modes.
- Training and tutorial bots: identity, routes, combat, spawning, death, respawn and resets.
- First- and third-person models, skins, weapon handling, aiming, ability animations, spawn, death and respawn sequences.
- HUD: health, ammo, overhead bars, reticles, hit markers, material effects, elimination names, kill feed, on-fire meter, scoreboards, match screens.
- Audio: weapon, hit, critical-hit and death sounds, multikill and match announcements, ambient audio, music, contextual and randomized dialogue.
- Tick and packet budgets, long matches, recovery. Measure tick tail latency, world packet gaps, delivered owner corrections and client frame time during sustained fire, with one and then two clients. Compare first and repeated hero selections on the same map: pick processing, body creation, initial script delivery, first playable frame.

## Seen with the original client

With the Python server:

- The real `_retail_/Overwatch.exe` logs into the local emulator without the Battle.net launcher.
- The Practice Range loads, hero selection appears, and a hero switch was recorded by the server.
- Training bots were killed. The log shows damage (25001) and elimination (25005) notifications, bot deaths and respawns.
- Missing: the enemy overhead health bar, the expected hit effect, and the bot's name in the elimination label. A bot's hit effect must be checked separately from flesh blood effects.
- Never established: a stable weapon and ammo HUD, a full shoot and reload cycle, ten repeated hero switches and reentries, multiplayer agreement.

With the Rust server: the client entered the Practice Range, but weapons were missing and abilities unusable. Gallery equip and in-match skin replacement pass loopback tests and still need a client retest.

## Upstream author's claims (not independently verified)

The imported game-server branch (LizRollin `9f8f3bcb`) claimed these work in the game:

- Matchmaking that fills teams, keeps parties together and honours role queue. "Game Found!" for 4 s before a queue match; the Practice Range loads at once.
- Hero select in the Practice Range and PvP modes with skins; a skin change during a match changes the body; H reopens hero select. "Assemble your team" with a 30 s countdown.
- Movement on each map's collision staying in sync (Practice Range, Kanezaka).
- Soldier: 76 weapon, abilities and HUD; hits where the shooter saw the target; damage, death, respawn and kill notices; other players see his weapon and shots.
- Training bots walk their routes, die and come back. Per-hero health bars. Chat lines on join, leave and hero switch. Enemy players have a name, health bar and outline. F1 shows your hero. The competitive loading screen shows both teams. Leaving a match alone or as a group.

Written but never checked in the game: kill feed, Helix Rockets and Biotic Field on other players' screens, no General chat during a match, a queue match found while in the Practice Range, a hero voice line on spawn (18 heroes have a networked spawn voice stimulus; Soldier: 76 does not).

## Known defects

- Soldier: 76 in the Practice Range: after a kill the fire button can stay pressed and ammo stops going down; Biotic Field does not show with some skins; Sprint stops working after a while; a crash after kills. Default skins avoid some of this.
- The HUD health number does not follow damage.
- Tab does not open. The competitive loading screen shows no ratings.
- Soldier's ultimate, Helix self-damage and rocket jumps are missing. Other players' shots can point the wrong way.
- Training bots have no names and no outline. The bot body cannot accept the player-only component 123.
- Kanezaka free-for-all always respawns you at the same point.
- Never tested over the internet.
- Not done: other heroes' abilities (their statescript must run on the server), match phases after "Assemble your team", objectives, spawn room doors, health packs, breakables, end of match, custom games, the custom game browser, event modes that wait for a mode script.

## Server coverage and open items

Moved here from the parity note when the Python server was deleted (phase 8 of the [refactor plan](../REFACTOR.md)). "Python" and "the reference" mean the frozen Python server, still reachable at the git tag `python-reference`. None of this is verified with the original client unless [Seen with the original client](#seen-with-the-original-client) or the acceptance records say so.

### What the Rust server does

- Runs the local Battle.net listener, the encrypted lobby and the UDP match worker on loopback.
- Login subset: presence, progression, the hero collection, the dated Arcade roster, priority passes, career mode rules, celebration content, party and ranked messages. Gallery equip, also while hero selection is open; a skin change of the active hero replaces the body.
- Selects a profile from `--account`, `default_account.txt`, the first saved profile, or the local template, in that order.
- Serves the solo base Practice Range. A headless test logs in over the encrypted lobby, requests a solo seat and uses the returned keys to authenticate UDP and receive the map load.
- Player movement on prepared collision: ordered per-body contacts and the same pose and wire quantization as the reference. Input work is round-robin, up to 16 poses or events per peer and 64 per event-loop turn. A 96-frame lead limit and a bounded input queue protect the worker. A correction history selects the eligible pose; sending owner corrections still needs authored hero movement parity.
- Statescript: expression evaluation (map building, angle and trigonometry opcodes, random floats), startup and same-time actions, scoped message entries, static and re-evaluated waits, server-frame waits, the deferred update phase (`Defer`), logical button edges and buffering, button blocks (`DisableLogicalButton`), AI weapon buttons (`AIFireWeapon`), variable stacks and keyed map writes, part of ability activation and `StopAbility`, `CycleIndex`, client-only pulsers and the kill feed pulse gate (`ProjectedSnapshot::gate_kill_feed`), `MoveTo` requests, owner and observer full and cumulative frames, authored target tags, health pools in `Vitals`, delayed conditional states, the operand-free instance exit. Every hero's and mode's ruleset defaults, read from the install (`orpheus_script::rulesets`, `game_rulesets_174.json`). The crate also has, for the sim to wire up: the ultimate charge (`ultimate::UltimateCharge`) and game messages between bodies (`PlayerWorld::take_messages`, `Component::receive_message`).
- Soldier: 76 rifle, reload and Helix cooldown run in reference tests. A fixture weapon damages a bot, retries a dropped hit, updates its health and respawns the bot over encrypted loopback. Helix flight and splash use the shared health path with a fixture body.
- The training bots run their body scripts (`bot_script.rs`; match.py `_prepare_bot_script`, `_pending_bot_scripts`, combat.py `_bot_weapons`, `_bot_died`, `_advance_bot_death`, `_respawn_bot`): one script starts per update and the world waits for them; each client gets every bot's H=0 frames once its create arrived, the Practice Range nameplate (0D24) once the mode UI arrived, the firing bots' live frames, the death script's frames with the kill feed pulse gated, and the respawned bot's new script. The Practice Range's three firing bots (0B92) fire their rockets along their yaw at the players and the other team's bots.
- Several players in one world on any map and mode of the catalog (`MapCatalog::game_map`; scripted-loading modes are refused as in Python). The matchmaker opens the match with `GameService::open_world`: the seats come back at once and the scene is prepared on its own thread while the players load, its collision built from the install into the cache's `collision/` when the map has none yet (`collision.py` `_read_cache`); when that fails, the games go back to the menu (`_collision_failure`). In the PvP modes (controller 0C90) the first spawn starts "assemble your team": v1303 of the game mode entity counts down every second, hero select stays open after a pick with the skin selector, and closes after 30 s. Players join a running match (`GameService::join_match`, the matchmaker's fill-running), and the dashboard lists the matches with their players' heroes and states (`GameService::snapshot`). Each client gets the other players as `ow174/game/match.py` sends them: their 20308 cards (sent once more after 2 s, "joined the game" for a later arrival, "left the game" when they leave), controller entities, 288D team list entries, possession (44/75) once entity and body arrived, body creates, a movement record of each other body in every send frame, their body statescript as H=0 full frames and deltas (`observers.py`), and the destroys on a hero switch or a leave. Loopback tests check the order, and a PvP match on Workshop Chamber with a third player joining.

### Open items and known differences

- The stock Soldier: 76 owner script runs on the server path (checked 2026-11-10 without the client): a scripted session (fire through a reload, move, sprint, crouch, Helix, melee) runs with no gaps and every frame encodes for the owner and observers (`stock_soldier_plays_a_scripted_session`). Where the server can't do what the graphs ask yet, it does what the Python reference did. The VM records each such answer in `Snapshot::assumptions`, and the session test lists all 13 it reaches:
  - no-op actions: the unnamed `STU_E9694EFD` (sprint), `STU_6E5C5F66` and `STU_1C2CC6D5` (melee). Python's reasoned server no-ops (effects, criteria, stats, client-only messages, `PlayScript`, `STU_7106C356`, `STU_C43F9944`) aren't recorded: they change nothing the server tracks;
  - `STU_ED45C5E8` (graph `01CA`, node 126, server-only; moving and firing reach it) answers no entities, so message `0BD` goes nowhere; a message to nobody doesn't read its parameters;
  - an animation without a length in its graph (graph `02C5`, node 51) doesn't end by itself. Its length is that of the animation an alias (`0C8`) resolves to, but an alias asset holds no data (`STUAnimAlias` has no fields): the alias is resolved elsewhere, through the animation blend trees or client code, so reading assets alone may not give the length;
  - target searches (`TrackTargets`) find nothing, and melee's knockback, hit list and three unnamed states are on/off only; the match applies quick melee's damage itself (`MELEE` in `ow174/game/combat.py`);
  - a map value (graph `05AD`, variable 29859) is left out of frames: its wire form isn't known, and the client builds the same map.
  The stock body turns on owner corrections (`replication.rs`). None of this has been checked with the client yet.
- Other heroes' owner scripts (checked 2026-11-10 without the client, by starting each body through `PlayerBodyFactory` in a debug build). Reaper, Pharah, Cassidy and Soldier: 76 start without a gap; `scene_assets.rs` runs only Soldier: 76. Every ruleset default now comes from the table, so Tracer's pistols load 40 rounds (`tracer_pistols_load_forty_rounds_from_the_ruleset_defaults`). A body with any startup gap gives no shots (`take_shots` waits for a complete start), so one unsupported node in an ability graph also disarms the gun. What still stops the others in debug builds is all graph content the Python reference didn't model either, so release builds run it the reference's way (see Statescript below):
  - config values Python answered null: `STU_E72F6348` (12 heroes), `STU_9B213168` (7), `STU_D3685C04` (5), `STU_71A87E92` (5), `STU_9B7648C3`, `STU_ED807E07`, `STU_038C32E8`, `STU_967463AB`, `STU_06610EBC`, `STU_60146072`, `STU_082FA44E` and a few single ones; forms of `STU_783B4CF0`, `STU_32E6E4D8` and `STU_AC829876` the evaluator doesn't compute;
  - state classes Python ran as plain states, with no name or model: `STU_75D1C308`, `STU_441B50E4`, `STU_D1BF5FA6`, `STU_431F3FCC`, `STU_160F2578`, `STU_64CFBC4E`, `STU_D1D0DED6`, `STU_E9485CAF`, `StateRayCast`, `StateBlinkEffect` (whose payload neither server can write), and movement samplers of other shapes (`STU_96F1DBCF`, Doomfist and Baptiste);
  - one action: `STU_064A9E0D` (Bastion).
  Tracer's other gaps: Blink's action `STU_A807207A` (the move itself, with parameters 0.1 and 1); Recall's container `STU_54FEB21C` (node 4), which ends only when instance variable 395 (`0x018B`) turns true, as ow1-preservation found; nothing writes it, so the server must. Outside the VM: Blink and Recall move the player, so the mover must apply them (ow1-preservation did both as server teleports, Recall from 3 s of position history); the pistols' muzzle variables `0x31`/`0x32` stay empty, so shots would start at the body; Pulse Bomb's projectile `0x0400000000000045` sticks and explodes (graph `0C1B`), which the projectile flight doesn't model.

The rest, from a module-by-module comparison of `ow174/` with the crates (2026-11-10). "Builder only" means Rust can encode the message but nothing on a live path sends it.

- **Statescript.** Python runs every hero's body script and the bots' scripts; Rust runs both. The bots' come from the bot bundle (`game_graphs_bots_174.json.gz`), and the stock ones reach content the VM doesn't know (`STU_F76C486C` in graph 0033), so debug builds and tests start them only with `SceneAssets::set_bot_unknowns(Unknowns::Tolerate)`. Python treats an unknown state as a plain state, an unknown action as "follow the out plug" and an unknown native as null. Rust does the same in release builds and records each case as an assumption, logged once per server run; debug builds and tests refuse the body instead (`vm::Unknowns`). The node classes, opcodes and configs Python modelled are all in the VM now; Raycast follows its out plug without casting, as the live Python server had no cast provider. The sim runs the ultimate charge (#342 over 60 s, paused while the ultimate runs), the kill feed pulse gate for owner and observer frames, game messages between players' bodies and `MoveTo` requests on the mover. Soldier: 76's death path reaches an action the VM doesn't implement (`STU_4C3509FB`, graph 01CA node 118), so debug builds drop the dead body's script; release builds follow its out plug as Python did. Assumptions the new code records: `WaitForServerFrames` takes a server frame as a 16 ms command frame, `STU_4E2BCAAB` answers false, and a player's entity filter (`STU_AC829876`) the evaluator can't compute answers false, as Python's driver does.
- **Match.** Several players share a world on any map and see each other (bodies, cards, team list, possession, observer frames, join, leave and hero switch notices), shoot and melee each other and the bots, die (25004, 25005, the 003E kill feed line, the dead flag in their movement records) and respawn after the mode's time as a new body; the Tab board, ultimate charge and spawn voice lines are live. The Biotic Field opens, heals the caster's team and goes for every client (03DC with graph 0258's frame, parented to the owner's canisters), and the other clients see a player's predicted projectiles (a server entity and the shooter's channel-2 shot record). The training bots run their scripts, show nameplates, fire, die through their death script and respawn; every client with a firing bot's statescript sees its rockets the same way (`projectiles.py` 349-361). Missing: a volley that leaves another entity (Torbjörn's turret, Zarya's ultimate) flies only without an arc and keeps the explosion release Python leaves out.
- **Lobby.** After login the Rust lobby answers every message Python's does.
- **Platform.** `--game-port 0` gives a random port instead of turning the game server off (the lobby needs it). The research `inject.jsonl` watcher is not ported (a research tool, not server behaviour). The multi-PC modes (`host`, `server`, `join`, `join-tournament`) and the second game on one PC work on loopback but are not yet verified with a second PC or a second real client.
- **Game data.** Derived from the install at run time (`crates/assets`); the prepared pack is only the oracle of `just test-assets`. Derived: the ruleset defaults and mode facts (`game_rulesets_174.json`, `game_modes_174.json`: scripted loading, hero switch notices), every hero body's eye heights (`game_eyes_174.json`), each mode's respawn time (`game_respawns_174.json`, from the 06FD.025 its mode graph sends), the Biotic Field's graph (`game_spawns_174.json`) and numbers (read from that graph), quick melee's sphere (graph 0043), the kill feed pulsers (found in the graphs), the movers' fallbacks and gravity, and the loot box economy's credits and event boxes (`economy_174.json`). Still hard-coded: the training bots' eye (`BOT_EYE`, kept at Python's 1.55: the bot asset says 2.0 and the Python reference tests need 1.55), the loot box odds and slots (the install's `054/011D` numbers aren't decoded), shop prices, ranked tiers (not in the install), and weapon fallbacks no volley uses.
- **Open verification items:** on-fire scoring and decay, the tips registry, network serialization of map values, instance overrides and foreign entities for target tags, named pools and pool resize rules, the randomized timer's client parity.
- **Ported test that differs.** `TakeOutTests::test_a_player_taken_out_before_his_game_connects...` (Python `test_leaving.py`) was not ported: the Rust server behaves differently when a player is taken out of a match before his game connects. Decide which behaviour the client needs before porting it.

### Still needs a client test

Everything above that was checked only on loopback or in tests: the Soldier: 76 stock owner script and its owner corrections, other heroes' bodies, the training bots' scripts and rockets, several players in one world and "assemble your team", the multi-PC modes (`host`, `server`, `join`, `join-tournament`) and the second game on one PC, and the open verification items. The client acceptance records below are the only client results so far.

## When debugging against the client

- If the server log agrees but the client rolls back weapon or ammo state, check body initialization, state layout and processed-input acknowledgement before looking at damage.
- Prefer target-client evidence when cross-version sources disagree. Mark every cross-version inference until it is checked on 104319.
- Don't infer a client restart from a low frame number in a late datagram.

## Client acceptance records

Add one entry per check: date, commit, client build and locale, hero, map, exact inputs, expected and observed result, number of runs that agreed.

- **2026-11-10, Rust relay to the lobby (phase 5 gate).** Commit `aa07704` plus the uncommitted phase 5 cleanup (no relay or server change), debug build. Client 1.74.0.0.104319, enUS, KR branch install. Runtime pack assembled from LizRollin's public data at `9f8f3bcb` plus `0688.col` built by `tools/build_collision.py` (see `docs/notes/data-sources.md`). Command: `orpheus play --runtime <pack> --game <install> --state <fresh folder>`. Expected: the client logs in through the Rust relay and reaches the lobby. Observed: the relay connected the client's Battle.net stream on port 1119 to the RPC port 21119 and carried the WebSocket upgrade and the logon exchange. The client then held a lobby connection on 3724 and showed the main menu's Options screen as RESEARCHER. One run.
- **2026-11-10, Soldier: 76 with the forced `041A` HUD mount.** Commit `5bbbdb0` (gun preload plus AipNooBest's world UI entity mount, since replaced), release build, Practice Range, 445-graph pack. Client 1.74.0.0.104319, enUS. Observed (one run, screenshot): the rifle in first person; the hero card with portrait and 200/200 and its health segments; the ultimate ring at 0%; Sprint, Biotic Field and Helix Rockets icons; ammo 30/30 with the rifle icon; the crosshair; training bots. The user called it partly working without listing what was missing.
