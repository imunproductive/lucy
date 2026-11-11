# Hardcoded game values

This page lists the game values and rules that the server still keeps in code, and whether the install has the real value. The list comes from an audit on 2026-11-11 of `crates/sim`, `crates/script`, `crates/physics`, `crates/content`, `crates/assets` and `crates/lobby`. The audit used the derived data, the DataTool dump, OWLib's type names and small probes of the install. It did not use the client binary.

"Code" is what the server uses now. "Install" is what the game data says. Paths are relative to `crates/`. Graphs are named by their hex index, and `v<number>` is a statescript variable.

Engine tuning inside the client's program code is not listed one by one: the mover's ground probes, contact tolerances, pitch clamp and slot defaults. No game asset holds them. Only reverse engineering can recover them.

When you replace a value, remove its row here or move it to the right section.

## 1. In the install and confirmed

### Values the server has wrong

| Item | Where | Code | Install |
| --- | --- | --- | --- |
| Control: capture time with 2 players | `sim/src/objectives.rs:118` | 5 s | 5.33 s. Graph `0EC2` uid 657 gives 0.125, 0.1875 and 0.25 per second for 1, 2 and 3 players. |
| Control: wait after a round | `sim/src/match_flow.rs:178` | 11.6 s for every mode | 10.9 s for Control (`0EC2` uid 359). 11.6 s is right for Assault, Escort and Hybrid (`0F4A` uid 359). |
| Overtime fuse | `sim/src/match_flow.rs:200` | 5 s, then 3 s after 20 s, then 1.5 s after 30 s | 5 s for the first 15 s of overtime, then it falls smoothly to 1.5 s at 30 s (`0F4A` uids 1090-1123, same in `0EC2`). |

### Values the server has right, but as copies in code

| Item | Where | Install |
| --- | --- | --- |
| Setup time 45 s | `sim/src/match_flow.rs:166` | `0F4A` uid 895 sets v1303 (the countdown) to 45. |
| Assemble 30 s, then 25 s in later rounds | `sim/src/match_flow.rs:160-162` | `0F4A` uid 896. |
| Control unlock 30 s | `sim/src/match_flow.rs:183` | `0EC2` uids 310, 830 and 831. |
| Control score rate, 1% per 1.2 s | `sim/src/objectives.rs:350` | `0EC2` uid 511. |
| Control overtime hold at 99% | `sim/src/objectives.rs:352` | `0EC2` uid 879. |
| Time bank: cap 120 s, floor 60 s | `sim/src/match_flow.rs:206-207` | `0F4A` uids 1297, 1298, 671, 673 and 698. |
| Payload rollback speed, 0.3 m/s | `sim/src/objectives.rs:28` | `1225` uid 44: −0.3 × the payload speed setting (ruleset entry 9520). |
| Elimination: 3 rounds to win, 135 s rounds, 105 s tiebreak, 20 s assemble | `sim/src/match_flow.rs:164,194-197` | Already derived: `game_rulesets_174.json`, schema `0x0A30…000D`. |
| Control: 2 rounds to win | `sim/src/match_flow.rs:185` | Already derived: schema `0x0A30…000C`. |
| Deathmatch 20 kills, Team Deathmatch 30 kills, 10 minutes | `sim/src/match_flow.rs:186-189` | Already derived: schemas `0x2A` and `0x28`. |
| Capture progress marks at 1/3 and 2/3 | `sim/src/objectives.rs:162,178` | `STUCaptureAreaComponent.m_segments` in `0x37.003`. |
| Leave policy: window 20, thresholds 0.3 and 0.2 | `lobby/src/progression.rs:85,90` | `054/0008` `m_94EA7D40`, `m_84F13CFF`, `m_398CB1D9`. The thresholds' meaning is not known. |
| Group XP 120%, leaver XP 25% | `lobby/src/progression.rs:66,70` | `054/0008` has 1.2 (three unnamed fields) and 0.25. It is not known which 1.2 is the group bonus. |
| Practice Range record | `content/src/maps.rs:120-137` | `game_maps_174.json` entry 0688 and mode 0018. A game-tier test pins the copy to the data. |
| Practice Range bot bodies | `assets/src/graphs.rs:99` | `ai_spawners_174.json` lists the same six bodies. |
| The six Tab board stats | `content/src/stats.rs:49-54` | `054/0051` lists the same six, in order. |
| Special map names | `assets/src/game_maps.rs:95-102` | The `09F` map name plus its `0C5` mode name. |

### Values the server does not use yet

| Item | Where | Install |
| --- | --- | --- |
| Arcade loot boxes at 3, 6 and 9 wins | Not built | `054/0018` `m_rulesetWinRewards`: unlock `03E1` at 3, 6 and 9 wins. |
| Training Bot respawn, 3 s | `sim/src/bots.rs:57` | The practice modes have 3 s in `game_respawns_174.json`, but the bot timer does not read it. |

## 2. In the install, but the field meanings are not confirmed

These look findable from the data alone, by comparing values across maps and modes. No reverse engineering should be needed.

| Item | Where | Code | What the install has |
| --- | --- | --- | --- |
| Capture area size | `sim/src/objectives.rs:16-20` | 8 m circle, 2 m below, 5 m above | Each capture point's `STUECCaptureAreaInstanceData` in the map's ENTITY chunk holds 8.0 or 8.5, then numbers that look like height and times. |
| Assault and Hybrid capture times | `sim/src/objectives.rs:112-116` | 30/20/15 s and others | The ×1, 1.5 and 2 multipliers per player are confirmed (`0F4A` uid 614, and the capture area component). The base time is probably in the capture point's data. |
| Time added per checkpoint, and the 240 s start | `sim/src/match_flow.rs:168-176` | +180, +150, +90 | `0F4A` uid 647 adds v10552 or v10553 of the current objective. The map's objective objects set these values (`0F4C` uids 5 and 6), so the real seconds are per map. |
| Payload speed and pushers | `sim/src/objectives.rs:30,200-202` | 1.4 to 2.25 m/s by map; ×1, 1.165, 1.33 | `1225` uid 34: a base of 1.5 m/s and ×1, 1.167, 1.333 per pusher, capped at ×2. There is also a ±10% factor of unknown source. Per-map values may be overrides on the payload. |
| Payload push radius | `sim/src/objectives.rs:23` | 5 m | `STUEscortComponent` has two radius pairs: 3.5 / 6.75 and 2.25 / 6.75. Which one is the push radius is unknown. |
| Loot box odds | `lobby/src/lootbox.rs:60-61` | One set of odds for every box type | `054/011D` `STULootBoxRarityRules` has separate values per box type: 1/20, 1/40, 1/100 for the base box and 1/5, 1/12, 1/24 for event boxes. The fields are not decoded. |
| Credit share in loot box slots | `lobby/src/lootbox.rs:70-75` | 5%, 5%, 50% | `m_currencyRarityRules` has 11/30, 13/42, 91/250. Not decoded. |
| Level-up loot box | `lobby/src/progression.rs:82` | Box type 0 | `054/0018` `m_C84D463F` names unlock `03E1`. It is probably the standard box. |
| Shop prices | `lobby/src/shop.rs` | From the captured shop catalog (see below) | `054/0018` `m_0C6A363E` prices by rarity: 25/75/250/1000 credits for normal items, 75/225/750/3000 for event items, 3000 for golden weapons. The install does not say which event items cost the event price. In the capture, most event items cost the normal price, and only some cost 3×. |

The captured shop catalog is `crates/fixed/data/retail_templates.json`. The upstream project decoded it from traffic recorded on the real 1.68 servers before they closed. See [data sources](data-sources.md). For shop prices, keep the capture until there is a way to pick out the event-price items.

## 3. Not in the install

These were rules of Blizzard's servers, live-service data, or code inside the client. They stay as written, each recorded as an assumption, until reverse engineering or another source gives the real value.

| Item | Where | Code | Why it stays |
| --- | --- | --- | --- |
| Passive ultimate charge | `script/src/ultimate.rs` | 5 points a second | The retail server added the charge. The ruleset multipliers are already read. |
| Armour damage reduction | `sim/src/vitals.rs:10` | 5 per hit | Not found in any asset or graph. |
| Pulse Bomb fuse | `script/src/weapons.rs:103` | 1 s | Graph `0034` has a 1 s wait, but it cannot be tied to the bomb. |
| World gravity | `physics/src/mover/kinematics.rs:22` | 10 m/s² | In the client's program code. Each hero's gravity scale is already read from the data. |
| Crouch-again delay | `physics/src/mover/posture.rs:37` | 0.15 s | No asset field found. |
| AI difficulty: damage share, aim error, reaction time, behaviour | `sim/src/ai.rs` | 25/50/100% and guesses | Not found in the AI graphs. |
| AI hero roster | `sim/src/ai.rs:44` | 12 heroes | Not found. |
| Overtime respawn +3 s, extra-round +30 s, match end 20 s, rollback delay 10 s, Deathmatch top 4 | `sim/src/match_flow.rs`, `sim/src/objectives.rs` | As in the community rules | Not found in any graph. |
| Hitscan reach and projectile lifetime when a weapon has none | `sim/src/hitscan.rs:24`, `script/src/weapons.rs:87` | 200 m, 5 s | Fallbacks. Not found. |
| Training Bot eye height | `sim/src/bots.rs:82` | 1.55 m | The bot asset says 2.0 m, but the Python server aimed from 1.55 m. Hit shapes now decide headshots for bots. |
| Training Bot patrol timings | `sim/src/bots.rs:62-73` | First wait 3 s, walk gap 1.2 s, turn rate | The patrol graph `0B8E` is read, but these timings were not found in it. |
| Ranked: tier floors, placements, ±25 SR, leaver loss, demotion, competitive points | `lobby/src/ranked.rs` | Community rules and guesses | Server-side rules. No ranked asset is in the install. |
| Leaderboard rule | `lobby/src/leaderboard.rs:51-53` | Top 500, 3000 SR, 25 wins | Server-side rule. |
| Endorsement shares | `lobby/src/player_messages.rs:56-60` | 40/35/25 | Only the category ids are in the install. |
| Practice vs AI XP | `lobby/src/progression.rs:68` | 90% | Not in the XP gains asset. |
| Loot box slots | `lobby/src/lootbox.rs:35` | 4 | Not found. |
| Arcade rotation, queue waits, priority passes | `lobby/src/arcade.rs`, `lobby/src/queue.rs`, `lobby/src/passes.rs` | Captures and the Python server | Live-service data. |
| Starting profile and starting loot boxes | `lobby/src/accounts.rs` | Development defaults | Server policy. |
| Challenge wins per tier | `lobby/src/celebrations.rs:75` | 9 | `STUCelebration` has a list that looks like tiers, but no celebration asset was found. |

## Suggested order

1. Fix the five wrong values in section 1.
2. Read the copies in section 1 from the data, and add the Arcade win loot boxes.
3. Confirm the capture area and payload fields in section 2.
4. Decode the loot box odds per box type.

Section 3 waits for reverse engineering of the client.
