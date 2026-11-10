# References

External material worth consulting. Anything from another build is a lead only; check it against 1.74.0.0.104319 before relying on it.

## Architecture

- Timothy Ford, GDC 2017, [Overwatch Gameplay Architecture and Netcode](https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and).
- Dan Reed, GDC 2017, [Networking Scripted Weapons and Abilities](https://www.gdcvault.com/play/1024653/Networking-Scripted-Weapons-and-Abilities).

Both explain the design. Neither is a wire specification or server source.

## Earlier 1.74 servers and research

- [Boi-027, Overwatch 1.74 Lobby Research](https://github.com/Boi-027/Overwatch-1-v1.74-Lobby-Research): the original lobby research and implementation this code descends from.
- [LizRollin, overwatch-1.74-lobby-server at `9f8f3bcb`](https://github.com/LizRollin/overwatch-1.74-lobby-server/tree/9f8f3bcb5eccab6fb7738bee3c2371fcf0623f29): the game runtime the Python server imported (expression interpreter, statescript codecs and driver, collision, character mover, combat). Its expression VM had an unbounded jump loop (`expr.run([1, 0])`); the imported copy bounds execution.
- AipNooBest's notes: [early weapon and snapshot proof of concept at `f8426859`](https://github.com/AipNooBest/overwatch-1.74-lobby-server/blob/f8426859c3195691d379f2fb291a475dfa40139e/docs/STATESCRIPT.md) and [HUD corrections at `860bab45`](https://github.com/AipNooBest/overwatch-1.74-lobby-server/blob/860bab453eaa8b49e5760b155f9a3fa68e164af3/docs/HUD-VARIABLES.md). The later notes retract the queue-mode-5 HUD explanation and the screenshot readings of ammo 30/30, health and armor numbers and ability rows (those digits were duplicate weapon rendering). Their docs and code disagree on the body-create descriptor list; check call sites.
- Logo2K: [1.74 lobby research](https://rentry.co/3bswrgmw#overwatch-174-lobby-protocol-channel-setup-login-and-loot-boxes) and the [lobby map list](https://rentry.co/wcwyv3r8).
- Gamayun (TripleFault): a 104319 server in Go. Useful for comparing VM natives (possessed body and current possessor, health pool selectors, on-fire lookup), states (synced game-code state, tips, input bridge, delayed switch timing, event order) and component 51. **It is AGPL-3.0-or-later**: use it only as a behavior reference, never copy its code.

## Other builds

- [Prometheus at `ad1fea7`](https://github.com/saturn-xvi/prometheus/tree/ad1fea7e6c04c9959c7115cb8ecd9c79c93c1ccb): beta client research. Its voice system triggerer shows a client audio endpoint; its selective map-resource hook shows where the beta client filters loaded entities (placement of several dynamic objects was wrong after the bypass). Its component IDs and memory writes don't transfer to 1.74.
- Claudewatch: Beta 1.0.0.3 leads for attacker-side hit-confirm effects and separate entity and announcer voice messages.
- Client build 155284 was inspected for the instance exit, parent completion, typed zero returns and the movement sampler inputs. Treat those as leads for 104319.

## Tools

- [overtools/OWLib](https://github.com/overtools/OWLib) (MIT): DataTool, TankLib, STU type definitions.
- [overtools/TACTLib](https://github.com/overtools/TACTLib) (MIT): CASC and TACT reading, including the per-build Overwatch manifest procedures.
- [Plasmawatch login-server](https://github.com/plasmawatch/login-server): Battle.net login emulation reference.
- [SplitMix64](https://prng.di.unimi.it/splitmix.c) (public domain): the generator behind the statescript VM's local random expressions.

## Survey of related projects (2026-11-10)

What each project has that this repository lacks, in short. Every finding, with its source and evidence, is in `related-projects.md`; ow1-preservation's full list is in `ow1-preservation.md`, its public rule research in `game-rules.md`.

| Project | License | Worth taking | Evidence |
| --- | --- | --- | --- |
| [AipNooBest, branch `hud-display-fix` at `860bab45`](https://github.com/AipNooBest/overwatch-1.74-lobby-server/tree/860bab45) | MIT | A weapon's `CosmeticEntity` builds nothing unless its `.003` definition is in the 20505 preload list (`+0x90`); the captured skin list has no gun definitions (`docs/HERO-WEAPONS.md`, `ow174/game/loadout.py` `gun_defs`). The in-game HUD: a server-created UI entity (definition `0x04000000000001F1`) mounting `041A` → `120B` beside `20E4`; the match-start message `.025:0C47` that only the server sends, as a remote-sync frame; the ammo widget `20E7`, hero panel `183C`, hero HUD `01CF` and layer `01CB` through dispatcher `004B`; which weapon presenters to light (`.0035` ammo, `.003D` charge, `.001C` gate, `.00C6` magazine); `ALIVE_VAR 0x094C`; the HUD needs a named menu hero at login (`docs/HUD-VARIABLES.md`). | Live client renders; several readouts still don't draw, as the author says. Checked here: Soldier: 76's weapon script `0254` builds its gun from definition `0x04000000000004BF` (node 31), which neither the captured 20505 list (2,457 entries) nor our server sends. |
| [p3n1cillin/ow1-preservation](https://github.com/p3n1cillin/ow1-preservation) | AGPL-3.0 (research only; its game code is not public) | `HANDOFF.md` and `CLAUDE.md`: AI navigation data in map chunk `0x12` (tactical volumes with voxel masks); vs-AI as normal modes with server bots (queue cards `0x0630…70`–`72`); the Assault flow on Temple of Anubis (phase variable `003B`, messages `0700`/`0692`/`0701`/`0694`/`0695`, mode entity `16B2` with `0F4A`, objective entities, spawn room tags); killcam record 25011; the hero dead flag `0x01DE`; `STU_E4C6AAB4` reads the spawn-room flag of component 74; `STU_54FEB21C` is a container the server ends through variable `0x018B`. Branch `claude/blissful-bohr-qodvyf`: public-source rule notes (timers, capture rates, AI difficulty). | Summaries written by AI agents about private code: leads to check, not facts. Live runs, memory-dump emulation and static tracing are marked per item. The rule notes come from the wiki; their Soldier: 76 numbers (20 to 6 damage over 30–50 m, 9 shots a second, Helix 6 s cooldown, sprint +50%) match his authored graphs here. |
| [LizRollin at `9f8f3bcb`](https://github.com/LizRollin/overwatch-1.74-lobby-server/tree/9f8f3bcb5eccab6fb7738bee3c2371fcf0623f29) | MIT | `data/statescript_names_174.json`, which `tools/ssq.py` needs: aliases for unnamed classes (`STU_0A6BE10E` FilterArray, `STU_0C3A5D53` ForProbe, `STU_B5C87F3D` SoundImportanceThreat and others), logical button ids, mode and controller graphs per team. Nothing else: the Python server here is a superset. | Names without client addresses |
| [Prometheus](https://github.com/saturn-xvi/prometheus) (branches `artpop2`, `stu-editor-ui`) | AGPL-3.0 since `artpop2` | Beta-only leads: an instance flag `0x8` that let some beta weapons fire; PvP game component fields (game-state asset, round, overtime). | Beta client trials |
| Boi-027 lobby server (`wip`), KenMizz fork, Boi-027 research repo, Plasmawatch login-server and lobby | MIT / none | Nothing new: older copies of the same lobby code or of what `crates/bnet` does. | |

Logo2K's lobby map list also explains the eight encrypted bundles in `data-sources.md`: maps `0EEE` and `0F14` are the lobby maps "OWL Summer Showdown 2022" and "Goodbye". Gamayun (a 104319 server in Go, AGPL) and Claudewatch (a Beta 1.0.0.3 binary) are not public; the upstream author reviewed local copies. Upstream took from Gamayun the possessed-body and current-possessor queries, health pool selectors, the on-fire lookup, the synced game-code state, tips, the input bridge, delayed switch timing, event order and component 51: the same areas where `PlayerWorld` still has fixed answers.

