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

- The stock Soldier: 76 body: its preflight stops at the first unsupported operation: the crouch branch's effect action (`STUStatescriptActionEffect`) at graph `01C7`, node `74`, with 1 initialization diagnostic (checked 2026-11-10). The VM gets that far only when the hero bundle carries the modes' variables and the bodies' target tags. Without target tags it stops at graph `004B`, node 21, an entity filter (`STU_AC829876`) that needs them; with target tags but no mode variables, at graph `0259`, node 31, the game mode entity (`STU_A3DEF368`). The server then runs Soldier: 76 without his statescript. Its startup tree already encodes byte for byte (3,261-bit owner and 3,067-bit observer frames).
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
