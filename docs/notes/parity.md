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
- Full player and bot scripts and replication, native combat with stock bodies, observer projectile launch, scripted flight, healing fields.
- Matchmaking queues and PvP modes beyond the solo Practice Range.
- Social actions.
- Dashboard actions that depend on missing features: opening loot boxes, the shop (`purchase` and item prices), the bot's group, matchmaking settings (`matchmaking`, `mode_settings`, `set_map`, `end_matches`), `reconnect` and `start_game`. The native dashboard answers them with 501. Its profile, account, frame, item and loot box actions work, but a logged-in game only sees their changes at its next login; the Python server pushed them to the live session. A new default account takes effect when the server starts again. The dashboard accepts only JSON posts and loopback `Host` headers, so another web site can't change profiles through it.
- Host mode (bind beyond loopback), join mode (a local Battle.net emulator pointing at a remote lobby) and tournament mode. The Python CLI modes are `retail`, `tournament`, `host`, `server`, `join` and `join-tournament`.
- Starting the client and injecting the relay: `orpheus play` and the Rust relay exist, but no original client has logged in through them yet. The C++ relay and the Python launcher stay until one has.
- A second game on the same PC (the dashboard's `start_game`) and telling two local games apart by their connection owners.
- Open verification items: on-fire scoring and decay, the tips registry, network serialization of map values, instance overrides and foreign entities for target tags, named pools and pool resize rules, the randomized timer's client parity.

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
