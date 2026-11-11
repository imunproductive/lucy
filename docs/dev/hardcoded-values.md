# Hardcoded game values

This page lists the game values and rules that the server still keeps in code, and whether the install has the real value. The list comes from an audit on 2026-11-11 of `crates/sim`, `crates/script`, `crates/physics`, `crates/content`, `crates/assets` and `crates/lobby`. The audit used the derived data, the DataTool dump, OWLib's type names and small probes of the install. It did not use the client binary.

"Code" is what the server uses now. "Install" is what the game data says. Paths are relative to `crates/`. Graphs are named by their hex index, and `v<number>` is a statescript variable.

Engine tuning inside the client's program code is not listed one by one: the mover's ground probes, contact tolerances, pitch clamp and slot defaults. No game asset holds them. Only reverse engineering can recover them.

When you replace a value, remove its row here or move it to the right section.

## 1. In the install and confirmed

### Values the server has wrong

None left: the Control capture time, the round waits, the overtime fuse, match XP, the level curve, the capture times, the time per objective and the payload's speeds, range and timers are now read from the install.

### Values the server has right, but as copies in code

| Item | Where | Install |
| --- | --- | --- |
| Leave policy: window 20, thresholds 0.3 and 0.2 | `lobby/src/progression.rs:85,90` | `054/0008` `m_94EA7D40`, `m_84F13CFF`, `m_398CB1D9`. The thresholds' meaning is not known. |
| Group XP 120%, leaver XP 25% | `lobby/src/progression.rs:66,70` | `054/0008` has 1.2 (three unnamed fields) and 0.25. It is not known which 1.2 is the group bonus. |

### Values the server does not use yet

| Item | Where | Install |
| --- | --- | --- |
| Arcade loot boxes at 3, 6 and 9 wins | Not built | `054/0018` `m_rulesetWinRewards`: unlock `03E1` at 3, 6 and 9 wins. |

## 2. In the install, but the field meanings are not confirmed

These look findable from the data alone, by comparing values across maps and modes. No reverse engineering should be needed.

| Item | Where | Code | What the install has |
| --- | --- | --- | --- |
| Capture area shape | `sim/src/objectives.rs` (`Area::Boxes`) | Read: the boxes of the area placeable (map chunk `0x0A`) that the point's `STUTargetTagInstanceData.m_2746D7E4` names | Strong guess that this area is the capture volume: the boxes fit each point's floor ring and the legacy capture area names the same area, but no reader of the field was seen (refs `CaptureAreas.md` §3). |
| Payload speed units | `sim/src/objectives.rs` (`Track`) | Read: `1225` uid 34 times v7779, in the track's stored-length metres a second | Strong guess: with the stored lengths (`m_9741DF92`) the speeds give the community's m/s within 0.04. The ±10% factor (`STU_B182C5C9`, a field-less config var) is read at its middle, so the server has no spread; when the client draws it is not known. |
| Payload range shape, rollback end | `sim/src/objectives.rs` (`Payload`) | Read: a 7 m sphere 2 m above the payload; rollback stops at the last barrier waypoint | Strong guesses: the sphere from the TrackTargets' 360° angles, the barrier from the waypoints' missing previous links (refs `Payloads.md` §3-4). |
| Target search filters (`TrackTargets`) | `script/src/vm/targets.rs` | `m_66747E7D` 1 allies, 2 enemies; `m_89831602` bit 4 leaves the searcher out; `m_84065FD1` 1 and 64 take players' bodies and bots; `m_4EA686DB` bit 16 leaves the dead out; a body counts when its capsule is within the radius | Each search's `STUConfigVarFilter`. The meanings come from comparing the 292 uses (heals use 1, enemy abilities 2, beams to others bit 4); the module doc lists the evidence. Other bits and fields are not read, and searches that set them find nothing. |
| Loot box odds | `lobby/src/lootbox.rs:60-61` | One set of odds for every box type, fitted by Python to Blizzard's 2017 China disclosure (an epic about every 5.5 boxes, a legendary about every 13.5) | `054/011D` `STULootBoxRarityRules` gives each box type a fraction per rarity (rare, epic, legendary): base 1/20, 1/40, 1/100; the six event boxes 1/5, 1/12, 1/24; golden 1/8, 1/20, 2/30; legendary boxes 1/4, 1/6, 1/15. They are most likely chances, but no model (per item, per box, or rising each box without the rarity) gives both disclosed rates. `m_24391F9B` = 21 and `m_53B8C818` = 50 are the same on every box type; they might be bad-luck limits. The 1.74 client has no odds screen. Box contents were rolled on Blizzard's servers, so the client may never read these fields. Settling it needs IDA (code that reads the field hashes) or logs of many retail box openings. Report: `refs/reports/LootOdds.md` outside the repo. |
| Credit share in loot box slots | `lobby/src/lootbox.rs:70-75` | 5%, 5%, 50% | Only the base box has `m_currencyRarityRules`: 11/30, 13/42, 91/250. A weak guess is the chance that a slot of that rarity gives credits (about 37%, 31%, 36%). Not confirmed. |
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
| Overtime respawn +3 s, match end 20 s, Deathmatch top 4 | `sim/src/match_flow.rs` | As in the community rules | Not found in any graph. |
| Objective details the graphs leave to native code | `sim/src/objectives.rs`, `sim/src/match_flow.rs` | Every attacker advances a point (0E8E ignores players with v10414 set; the server sets none); a Control tie picks team 0 (the graph picks at random); a payload event fires once; Hybrid's unlock run starts after the waits; a match with a ranked player plays with "Competitive Rules" | The graphs read native state the server doesn't have. |
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

1. Done: the wrong values in section 1 are read from the install.
2. Read the copies in section 1 from the data, and add the Arcade win loot boxes.
3. Confirm the capture area and payload fields in section 2.
4. Loot box odds per box type: tried from the data alone (see the row above); settling them needs IDA or retail logs.

Section 3 waits for reverse engineering of the client.
