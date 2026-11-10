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

## When debugging against the client

- If the server log agrees but the client rolls back weapon or ammo state, check body initialization, state layout and processed-input acknowledgement before looking at damage.
- Prefer target-client evidence when cross-version sources disagree. Mark every cross-version inference until it is checked on 104319.
- Don't infer a client restart from a low frame number in a late datagram.

## Client acceptance records

Add one entry per check: date, commit, client build and locale, hero, map, exact inputs, expected and observed result, number of runs that agreed.
