# Moving work from Rust into the game's graphs: plan

The server does much in Rust that the game's own graphs (statescript) should do. The full inventory, row by row with effort and blockers, is `D:/Work/Coding/refs/reports/RustInsteadOfGraphs.md`. Things the retail server did natively (hit registration, movement, netcode, damage and death messages, ultimate charge, voice line choice, area tests, spawning) stay in Rust by design.

Most rows wait on a few shared pieces. Each enabler below unlocks several rows.

| # | Enabler | Unlocks | Needs IDA? |
|---|---|---|---|
| 1 | **`ModifyHealth` as a real state**, applying to the targets of its parent search (today it is in `ASSUMED_INERT_STATES`) | quick melee (`0043`, with its damage tags), the spawn room heal, the Biotic Field heal, enemy-room damage | No |
| 2 | **A controller world:** run `0C90` (and `288A`/`288B`/`288D`) on the player entity in the VM, with message routing (`037F`, `0687`, `0093.025`) | hero select, hero change rules, the respawn hand-off and its camera (st99), the hero switch notice, multikills (`0215`) without hand-built frames | Partly: the revive node (enabler 4). First slice done (agent ControllerWorld, 2026-11-13): `0C90` runs in the VM per player and drives the multikills; next steps in `docs/dev/loose-ends.md` |
| 3 | **Run `0C8E`** on every hero body (the team's `m_bodyScript`) | the respawn countdown and death HUD, spawn room effects, the `0685` hand-off | No (agent ModeBody, 2026-11-13) |
| 4 | **The revive node `STU_621AF478` and StateHero** | same-body respawn for heroes (`0C90`) and Training Bots (`0BA8`) | Yes (IDA list §17) |
| 5 | **`0F4A`'s 15 missing node classes**, plus native capture/escort components | the whole match flow from the mode graphs (`0F4A`, `0EC2`, …) instead of `match_flow.rs` | Partly: `STU_6A926B21` ForCode and capture residency |
| 6 | **A script world for spawned and map entities** (stage B of [map-entities-plan.md](map-entities-plan.md)) | Biotic Field `0258`, bomb fuses, spawn rooms `0D62`, capture points `0E8E`, payload `1225`, doors, the Training Partner | No for the world itself; some node classes may |
| 7 | **A status and aura system** (`STU_9028E035`, ApplyAura) | spawn room invulnerability, the Biotic Field, Biotic Grenade's anti-heal and heal boost | Partly: what status type 0 means |
| 8 | **The AI definitions:** derive each hero body's `m_04A8896C` → `STUAIBehaviorComponent` per-mode graphs and `STUAIDifficultyProfile`, and the AI node classes | AI heroes driven by their authored graphs instead of `ai.rs` | Unknown until the classes are listed |

Order: enablers 1, 7 and 6 build on each other (the Biotic Field needs all three); 2 and 3 meet at the respawn hand-off; 5 needs 6 for the objectives' entities.
