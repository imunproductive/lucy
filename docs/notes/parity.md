# Python and Rust parity

The frozen Python server (`ow174/`) is the behavioral reference until the Rust server covers what it does. This note lists the gap and the cutover criteria. Phase 8 of the [refactor plan](../REFACTOR.md) fills in the test inventory and then deletes this file.

## What the Rust server does today

- Runs the local Battle.net listener, the encrypted lobby and the UDP match worker on loopback.
- Login subset: presence, progression, the hero collection, the dated Arcade roster, priority passes, career mode rules, celebration content, party and ranked messages. Gallery equip, also while hero selection is open; a skin change of the active hero replaces the body.
- Selects a profile from `--account`, `default_account.txt`, the first saved profile, or the local template, in that order.
- Serves the solo base Practice Range. A headless test logs in over the encrypted lobby, requests a solo seat and uses the returned keys to authenticate UDP and receive the map load.
- Player movement on prepared collision: ordered per-body contacts and the same pose and wire quantization as the reference. Input work is round-robin, up to 16 poses or events per peer and 64 per event-loop turn. A 96-frame lead limit and a bounded input queue protect the worker. A correction history selects the eligible pose; sending owner corrections still needs authored hero movement parity.
- Statescript: expression evaluation, startup and same-time actions, scoped message entries, static timers, logical button edges and buffering, variable stacks, part of ability activation, owner and observer full and cumulative frames, authored target tags, health pools in `Vitals`, delayed conditional states, the operand-free instance exit.
- Soldier: 76 rifle, reload and Helix cooldown run in reference tests. A fixture weapon damages a bot, retries a dropped hit, updates its health and respawns the bot over encrypted loopback. Helix flight and splash use the shared health path with a fixture body.

## Still Python-only or open

- The stock Soldier: 76 owner script runs on the server path (checked 2026-11-10 without the client): a scripted session (fire through a reload, move, sprint, crouch, Helix, melee) runs with no gaps and every frame encodes for the owner and observers (`stock_soldier_plays_a_scripted_session`). Where the server can't do what the graphs ask yet, it does what the Python reference did. The VM records each such answer in `Snapshot::assumptions`, and the session test lists all 13 it reaches:
  - no-op actions: the unnamed `STU_E9694EFD` (sprint), `STU_6E5C5F66` and `STU_1C2CC6D5` (melee). Python's reasoned server no-ops (effects, criteria, stats, client-only messages, `PlayScript`, `STU_7106C356`, `STU_C43F9944`) aren't recorded: they change nothing the server tracks;
  - `STU_ED45C5E8` (graph `01CA`, node 126, server-only; moving and firing reach it) answers no entities, so message `0BD` goes nowhere; a message to nobody doesn't read its parameters;
  - an animation without a length in its graph (graph `02C5`, node 51) doesn't end by itself. Its length is that of the animation an alias (`0C8`) resolves to, but an alias asset holds no data (`STUAnimAlias` has no fields): the alias is resolved elsewhere, through the animation blend trees or client code, so reading assets alone may not give the length;
  - target searches (`TrackTargets`) find nothing; melee's health change, knockback, hit list and three unnamed states are on/off only, so melee hits nothing;
  - a map value (graph `05AD`, variable 29859) is left out of frames: its wire form isn't known, and the client builds the same map.
  The stock body turns on owner corrections (`replication.rs`). None of this has been checked with the client yet.
- Other heroes' owner scripts (checked 2026-11-10 without the client, by starting each body through `PlayerBodyFactory`). Only Soldier: 76, Pharah and Cassidy start without a gap; `scene_assets.rs` runs only Soldier: 76. Two blockers are common to all of them:
  - ruleset values: `PlayerWorld::ruleset` answers only Soldier: 76's few defaults and 0 for everyone else, so for example the clip size multiplier `0x2819` is 0 and Tracer's magazine is 0. The Python server had the full table (`ow174/game/script/rulesets.py`: the `0C6` ruleset schemas each hero `075` and mode `0C5` lists);
  - a body with any startup gap gives no shots (`take_shots` waits for a complete start), so one unsupported node in an ability graph also disarms the gun.
  Tracer, with the ruleset defaults supplied, reloads to 40 and empties her pistols; her remaining gaps are all in her ability graphs:
  - at startup: Recall `001A` node 12 (`STU_60146072` about her own entity), Blink `0024` node 80 (`STU_B9898052`, named ClientOnlyPulser in LizRollin's list, yet networked), node 63 (`STU_038C32E8`), node 9 (`StateBlinkEffect`, effect `0x0300000000000190`), Pulse Bomb `0034` node 51 (`STU_082FA44E`, a lookup in `0x0F4800000000015D`);
  - in use: Blink's action `STU_A807207A` (the move itself, with parameters 0.1 and 1) and `STU_71A87E92`; Recall's `STU_9B213168` message target and its container `STU_54FEB21C` (node 4), which ends only when instance variable 395 (`0x018B`) turns true, as ow1-preservation found; nothing writes it, so the server must;
  - outside the VM: Blink and Recall move the player, so the mover must apply them (ow1-preservation did both as server teleports, Recall from 3 s of position history); the pistols' muzzle variables `0x31`/`0x32` stay empty, so shots would start at the body; Pulse Bomb's projectile `0x0400000000000045` sticks and explodes (graph `0C1B`), which the projectile flight doesn't model.
The rest, from a module-by-module comparison of `ow174/` with the crates (2026-11-10). "Builder only" means Rust can encode the message but nothing on a live path sends it.

- **Statescript.** Python runs every hero's body script and the bots' scripts; Rust runs Soldier: 76's only (above). Python treats an unknown state as a plain state, an unknown action as "follow the out plug" and an unknown native as null; Rust refuses the body instead, so each of these stops a hero:
  - states: Defer `STU_9C9A422A`, ClientOnlyPulser `STU_B9898052` (kill feed pulses), DisableLogicalButton `STU_E4A30BCF`, AIFireWeapon `STU_D7F9E1C3`, WaitForServerFrames `STU_E3CEF6B9`, MoveTo, Wait with a re-evaluated timeout (`m_BF1A93B0`), and any presentation state not on the allow-list;
  - actions and conditions: CycleIndex `STU_BC5E4622`, StopAbility `STU_187D5125`, Contains `STU_821116A6`, button held `STU_7DEB522D`, `STU_4E2BCAAB`, Raycast `STU_08D6BA6E`, SetVar with a map key;
  - expressions: opcode 19 (build a map), opcodes 40–49 (angle conversions and trigonometry), the random float `STU_BE0BD267`.
  Python also charges the ultimate (#342 over 60 s; Rust keeps it at 0), gates kill feed pulses, and routes game messages between bodies.
- **Match.** Only the solo Practice Range with Soldier: 76 has a world. Missing: other maps and modes (`MapCatalog::game_map` is test-only); other players' bodies, controllers, team list entries, cards (20308) and observer frames (the encoder exists, nothing calls it); damage to players, player death and respawn, the 25004 death notice and the 003E kill feed message; quick melee damage (0043 state 4); Soldier: 76's Biotic Field (03DC, graph 0258); projectiles shown to other clients; bot weapons, bot scripts, bot death and nameplates (a pack with bot graphs is refused); the PvP assemble countdown; live Tab board updates; join, leave and hero switch notices; spawn voice lines.
- **Lobby.** After login the Rust lobby answers only 21800, gallery equip (24500) and the Practice Range form of 24000; every other message is dropped. Missing:
  - matchmaking: 44100–44107 queues, the matchmaker, Game Found (53000/53003, builder only), the VERSUS lineup (53001/53002), priority passes (58501), match lobby and leaving (52902, 21801);
  - party and group finder: 22102–22111, 52200–52205 (the party member layouts exist, unused);
  - chat and friends: 21700/21701, whispers (27004), `.hero` and `.leave`, friend requests (27000–27002), status (27011), the friends list (27100) and cards (20809) at login, 55500 account features, rejoining General after a match;
  - loot boxes (24201), the shop (24203) and the store page (26500);
  - saving client settings (22200–22204, 22207, 36200), so they reset at every login;
  - lookups: career profile (22206), leaderboard (39100), player name (58202), the OWL poll (42600); Top 500 places are always empty;
  - event greetings and login gifts (38901), challenge rewards (24901), team-skin pairs;
  - pushing dashboard edits to a connected game (they apply at the next login).
- **Dashboard.** `open_all_boxes`, `bot_group`, `purchase`, `matchmaking`, `mode_settings`, `set_map`, `end_matches`, `reconnect` and `start_game` answer 501; item prices are empty; port 0 doesn't turn it off.
- **Platform.** Every listener binds to 127.0.0.1 and the handoff names 127.0.0.1, so no other PC can play: the `host`, `server` (for others), `join`, `tournament` and `join-tournament` modes are missing, with their prompts and the public address. No second game on one PC (Battle.net always logs in the dashboard's account). No remembered game path or file picker, no log rotation.
- **Game data.** Every runtime file still comes from the Python extractors (`tools/extract_*.py`, `build_collision.py`); `orpheus runtime prepare` only copies them, and Rust can't build a missing collision cache. `crates/casc` and `crates/stu` are not used by the server yet (phase 6c, 6d).
- **Open verification items:** on-fire scoring and decay, the tips registry, network serialization of map values, instance overrides and foreign entities for target tags, named pools and pool resize rules, the randomized timer's client parity.

## Cutover criteria

| Area | Exit criteria |
| --- | --- |
| Transport | Rust reproduces the reference bytes and fragmented streams, rejects malformed records, and handles loss, duplicates and rollover. |
| Login and lobby | The reference service requests and replies match. Existing profiles and runtime formats load. A lobby client acquires a valid match seat. |
| Match simulation | Registered state and packet order match encrypted Practice Range and two-peer replays. Sustained-fire tick time and snapshot gaps are measured with the same inputs. |
| Tools and dashboard | Replaced as described in the refactor plan (phases 4, 6 and 7). |
| Deployment | The launcher and relay work from a source build with the original client. |
| Cutover | Broader verified coverage than Python, the replacement workflows verified with the original client, and a server-only smoke test. |

When comparing the two implementations, use the same clock, input, collision data and packet-loss schedule. Compare bytes where the format is fixed, and decoded event order and registered state where runs contain nondeterministic values. Measure tick latency and snapshot gaps before claiming a performance improvement.

## Python test inventory

Filled in during phase 8: each of the 114 files in `tests/` is covered (Rust test named), ported (new Rust test named) or dropped (reason).
