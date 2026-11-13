# Loose ends

Small things left undone by finished work. Each entry says what is missing, where, and where the evidence is. Bigger plans live elsewhere (the netcode rework: steps 2 and 3, rewinding and corrections for every hero). Remove an entry when it is done. Questions only the client's code can answer are on the IDA list instead (see [AGENTS.md](../../AGENTS.md#leftovers)).

## Combat and kills

- **Fall and environment deaths aren't credited to the last attacker.** The kill feed shows the victim alone. The stat for it exists; the time window isn't in the data. `crates/sim/src/combat.rs` `environment_kill`; `docs/notes/behavior.md` (kill feed entry).
- **The "died" message `003E.025` still leaves out `006B`, `0142` and `03E8`.** Meaning unknown. `combat::death_message`.
- **Team kill, kill streaks, the hero's own multikill line.** In graph `0215` but not built: "TEAM KILL!" (`01A3`, 3+ enemies all down, `0646.025`), the kill-streak text every 5 kills, the hero's line 1.6 s after a third kill (`00C9`), and the multikill career stats. `crates/sim/src/announcer.rs`.
- **No kill feed line in the game.** The server sends the dead body's pulse at once to the victim and observers (loopback). Next: in the game, kill yourself (own Helix rocket or a kill volume) to see if the victim-only line shows; then the IDA list, section 17. `docs/notes/behavior.md` (kill feed entries), `D:/Work/Coding/refs/reports/KillFeed.md`.
- **Death lines.** Bots: `0C17` raises `05FB`/`1C92` on `003E` with tags the server doesn't pass. Players: `01CA` raises death cries, but the match drops voice lines from a dead body. `crates/sim/src/voice_lines.rs`.
- **No loopback test of an AI hero hurting the player.** The path is the human attacker's (tested); only the slow vs-AI match test covers AI heroes, and it checks no pain line. Easy AI hits (a quarter of the damage, `ai::scale_damage`) rate under 0.1 alone, so they raise no pain line until the player has lost about 40 health (VM test `a_hit_the_hero_survives_raises_his_pain_line_and_knockback`). `docs/notes/behavior.md` (Player hurt feedback).
- **Quick melee carries no damage tags**, so it gives no flinch. `combat.rs` (melee hits).
- **A killing hit sends no `0011.025`** (only the death message). `WorldDelivery::damage_body`.
- **Assists:** graph `1901` (attackers of the last 5 s) is a PlayScript the VM doesn't run, so no assists are credited.
- **Damage direction arcs and the screen hit flash** aren't sent; likely combat event 25000 to the victim (IDA list, section 13).
- **Enemy explosions don't push other players**, only the shooter himself and bots. `ProjectileFlight::blast_push`.
- **No aura system.** Biotic Grenade's aura `0267.053` (graphs `0C71`, `0C72`, `0B99`) sets `#8202e` / `#8203e` on the hit body for the anti-heal and heal boost status icons over the bar (not the outline colour); the server doesn't run it. `docs/notes/behavior.md` (Nameplate and bar look).
- **The mode body script's `0685` goes nowhere.** `0C8E` sends it to the mode entity when a dead body's countdown ends; retail's `0C8F` (a child of the mode graph) answers with `0687` to the controller, whose `0C90` spawns the new body (and plays its st99 camera when #22241 > 0, which needs `0DFE` from `0F4A`). The server still respawns on its own timer (`WorldDelivery::respawn_due`), which is the same 3 s in the Range. `crates/sim/src/body_script.rs` (`ModeBody`); `refs/reports/DeathCam.md`.
- **Soldier: 76's death path has two VM gaps** that a debug or `gametest` build refuses, so there the dead body's script stops at death and its owner frames (with `0C8E`'s countdown) stop too: `01C7` node 68 `STU_3716E1D4` and `01D8` node 98 `STU_309B0570` (a velocity). Release builds tolerate both. `STU_3716E1D4` is also in `0C8E` st7/st25 (on `0713`/`072A`). `body_script` test `a_dead_body_counts_its_respawn_down_in_the_mode_body_script`.
- **`06FD` carries only the seconds.** The server sends each body `06FD.025` with `00B7` as it starts; `0F4A` also sends `04F0` (#12392, the random respawn scale of `0C8E` st44) and `055D` (#13159). If the mode graph ever sends to bodies itself (its `STU_1D719351` targets), drop the server's copy. `PlayerBodyFactory::start`.
- **`STU_DB742151` of a player's body is 0 in the VM** (read as the AI control entity), but `0C8E` uses it as the player's controller (`0669` goes to it, `0685`'s `0221`; ElimName.md: the client finds the controller). `crates/script/src/player.rs` `related_entity`.
- **Training Bots and AI heroes don't run the team body script** (`m_bodyScript`); only player bodies do. `crates/sim/src/body_script.rs`.
- **AI heroes deal player damage times a share** (`ai::scale_damage`, 25/50/100 %); retail's weapon graphs give an AI its own damage per difficulty (`STU_D3EB8D87(STU_DB742151(self)) ? STU_4049CF77(param) : player damage`, the profiles in `game_ai_definitions_174.json`). Needs the AI body's possessor to be an AI entity with component 92 (see the `STU_DB742151` entry above). Plan step 1 in `D:/Work/Coding/refs/reports/AiDefinitions.md`.
- **AI heroes run no AI graphs.** Each hero's AI definition (`0456`, the per-mode behaviour lists) is derived but not run; `ai.rs` stands in. Plan steps 3-6 in `D:/Work/Coding/refs/reports/AiDefinitions.md`; IDA list, section 19.

## Movement and collision

- **Character collision gaps:** standing on an enemy's head, slowly pushing an enemy, bots being blocked by players, and placing a moving enemy where the client sees it (a view delay late). `crates/physics/src/mover`, `replication::blockers`.
- **Widowmaker's Grappling Hook** isn't run on the server (`005A` st1), so she gets no corrections. Hero by hero: `docs/notes/behavior.md` ("Owner corrections, hero by hero").
- **A respawn makes a new body; retail revives the same one.** 0C90 node 4 / 13C1 node 73 run `STU_621AF478` on the possessed body at the chosen spawn point #8370 with its heading; only a hero change makes a new body (StateHero). We (and Python) create a new body every time. Also, whether the respawn's controller frame turns the view is not checked in the game. `WorldDelivery::switch`; `D:/Work/Coding/refs/reports/RespawnFacing.md`; IDA list, section 17.

## Maps

- **Spawn door barriers outside the Practice Range:** the `0D73` / `0FA8` placeables are created now (map entities stage A); send them `072F` from the match flow, and make `0FA8` walls block the other team.
- **Server placeables in their default state:** every server-owned placeable is created but none runs its graphs (stage B of `docs/dev/map-entities-plan.md`). Not checked in game: what the Volskaya, Rialto and Gibraltar doorways (`0D73`), team walls (`0FA8`) and spawn doors (`0D32`) look like with no frame, and whether Rialto's boats (`25BF`/`25C0`/`25C1`, 30 at water level) show still. `D:/Work/Coding/refs/reports/MapEntities.md` (Stage A). `docs/notes/behavior.md` (Spawn door barriers outside the Practice Range).
- **Collision builder packing bug:** `props.rs` `FLAG_BITS` swaps two fields' bits and packs a third to bit 13 instead of 5 on 13,751 meshes. No server query depends on it yet; fixing it changes the Range's `0688.col`. `D:/Work/Coding/refs/reports/WeaponRays.md`.
- **Training Partner Spawner** (the Practice Range console that spawns a hero dummy): large; needs server-run map entity scripts. `D:/Work/Coding/refs/reports/TrainingPartner.md`.
- **The Range as `005A` is not checked in the game** (2026-11-13): the load, bots, spawn room, hero change, the bot plate tool, and two players of one party in one Range. What the client's 288A does only in `0018` (its st3 runs `292E` there) no longer runs; what `292E` shows is not known. The hero switch notice (`037F`) now shows in the Range, since both teams have `0C90`. `crates/sim/src/controller.rs`, `docs/notes/behavior.md` (Practice Range mode).

## Interface and voice

- **"Need Healing" plays no voice line:** its line comes from `05AD`'s `0307` entry, routed by the engine (`STU_57692020`). IDA list, section 8.
- **Communication wheel:** sprays, emotes and loadout plumbing aren't done; sprays are refused at the server (their placement state isn't run).
- **Live bot plate tool:** `message` can't reach the nameplate for any message but `0C3C` (the others are taken by client-only states). `crates/sim/src/bot_plate.rs`.
- **ALLIED HEALTH BARS and SHOW FRIENDLY OUTLINES** have no visible effect in game, though the server applies them. `D:/Work/Coding/refs/reports/AllyBars.md`.

## Tools and tests

- **`ORPHEUS_DEBUG_BOT_BODY`** crashes the game: it changes only the bot's create while the server runs the old body's graphs. Fix: choose the body when the bot list loads. `D:/Work/Coding/refs/reports/HitPredict.md`.
- **Data cache hardening.** `cache::ensure` now derives each run into `104319.new.<pid>`, so runs at the same time can't mix their files (fixed after a stamped half pack stopped the user's server, 2026-11-13). Still open: a killed run leaves its `104319.new.<pid>` folder behind (about 1.7 GB; nothing deletes stale ones), and a pack is trusted on its stamp alone. Checking that every file the server requires exists before trusting the stamp would make any future partial pack re-derive. `crates/assets/src/cache.rs`, `apps/orpheus/src/serve.rs`.
- **Client shots pass through server-created entities.** In game (2026-11-13) the client's own hitscan passes through the Practice Range's moving platforms that stage A creates (they move on the client though their scripts don't run on the server), as it passes through the Training Bots (bullet holes behind them; projectiles do hit bots). The map's own client-built props stop shots. Possibly one shared cause in how the client's hit test treats entities the server creates. `D:/Work/Coding/refs/reports/MapEntities.md` (In-game result of stage A), `HitPredict.md`.
