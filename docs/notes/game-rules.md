# Overwatch 1 game rules from public sources

Rules a server needs for match flow, combat and progression, as compiled from the wiki and patch notes by p3n1cillin/ow1-preservation's `research_notes/` (AGPL-3.0; restated here as facts with citations). None of this is read from the 1.74 client: where the client's own mode and statescript data give a value, that wins. A check against this repository's authored data: the Soldier: 76 numbers in `heroes_combat_data.md` (20 to 6 damage over 30–50 m, 9 shots a second, Helix 6 s cooldown, Sprint +50%) match his graphs.

Each fact cites a line of the notes and the source that line names. Marks:
- ⚠️OW2: the value comes from an Overwatch 2 era page or patch; don't use it for 1.74.
- ⚠️HIST: an older OW1 value a later patch replaced.
- ⚠️UNVERIFIED: the notes say no source confirms it.

File aliases, under `research_notes/` in that repository:

| Alias | File |
| --- | --- |
| GMR | `Overwatch 1 complete game reference/game_modes_rules.md` |
| HCD | `Overwatch 1 complete game reference/heroes_combat_data.md` |
| WCA | `Overwatch 1 complete game reference/workshop_customgames_ai.md` |
| RSU | `Overwatch 1 complete game reference/replay_spectator_ui_flows.md` |
| BNM | `Overwatch 1 complete game reference/battlenet_matchmaking.md` |
| PCS | `Overwatch 1 complete game reference/progression_cosmetics_social.md` |
| MAP | `Overwatch 1 complete game reference/maps.md` |
| GNR | `Overwatch 1 server emulation research/gdc_netcode_replay.md` |
| TRR | `Overwatch 1 server emulation research/transport_replication.md` |
| EMU | `Overwatch 1 server emulation research/emulator_projects.md` |

## 1. Match phases and common timers
1. Hero select ("Assemble Heroes") at match start lasts 30 s on Assault, Hybrid, Control and Escort. It was 40 s, then 35 s (Feb 2020), then 30 s (Mar 2020). — GMR:27-28 (patch notes Feb/Mar 2020)
2. Hero select before round 2 on Assault, Hybrid and Escort lasts 25 s; it was 10 s before Oct 2018. — GMR:26 (patch notes Oct 9 2018)
3. Setup phase on Assault, Escort and Hybrid lasts 45 s; it was 60 s before Oct 2018. Defenders may leave spawn; attackers stay behind the doors. — GMR:16, 26, 29 (patch notes Oct 2018; Assault/Escort OW1 wiki revisions)
4. Hero select only opens after loading finishes (a Feb 2018 fix stopped its use during the loading screen). — GMR:34 (patch notes Feb 2018)
5. Skin selection is part of the start-of-match hero select screen (Feb 2018). — GMR:33 (patch notes Feb 2018)
6. Mystery Heroes has no "Assemble Your Team" screen (Sept 2017). — GMR:31 (patch notes Sept 2017)
7. The notes' proposed state machine: LoadIn → AssembleHeroes → Setup → InProgress → [Overtime] → RoundEnd → side swap → MatchEnd → POTG (~17 s) → cards/endorsements → lobby or requeue. — GMR:124 (inference)
8. No source gives the hero select time between Control rounds. — GMR:129
9. A 1.74 Python server reports a working "Assemble your team" 30 s countdown; a separate variable (v8535) hides the "ASSEMBLE YOUR TEAM" banner. — EMU:142, 267 (Boi-027 server WIP)

## 2. Respawn and spawn rooms
10. Base respawn is 10 s. — GMR:37 (Respawn wiki oldid 117428)
11. Overtime adds 3 s to respawn per the wiki; the Jul 19 2016 patch says 2 s. This conflict is unresolved, and the notes favour +3 s. — GMR:37-38, 125 (wiki; patch notes Jul 2016)
12. Defender respawn has been 10 s on every map since Aug 2016. — GMR:39 (patch notes Aug 2016)
13. Attacker-advantage delay (Assault, Escort, Hybrid; added Feb 2017): when attackers outnumber defenders at the objective, defender respawn grows. — GMR:41 (patch notes Feb 2017)
14. Its ramp starts once the advantage timer passes 10 s and reaches +5 s at about 33 s. The timer counts back down if the advantage is lost. It ramps earlier since Aug 2017 and faster on final objectives since Jul 2018; the exact curve is unknown. — GMR:42-44, 130 (Respawn wiki oldid 117428; patch notes Aug 2017, Jul 2018)
15. Maximum respawn is 18 s (10 base + 5 advantage + 3 overtime). — GMR:44, 125
16. On Assault, after point A falls, dead defenders (and those who die shortly after) respawn in at most 3.5 s (Jun 2019). — GMR:45, 167 (patch notes Jun 2019)
17. Group respawn (non-competitive modes): if a death's timer is within 5 s of a dead teammate's remaining timer, the timers are averaged (10 s and 6 s become 8 s each). Groups chain, and it does not apply in overtime. — GMR:46-49 (Respawn wiki, current revision, History section)
18. ⚠️OW2: Wave Respawn and a 12 s base respawn (OW2 Season 12), and 13/14 s overtime respawn. Do not use for OW1. — GMR:50, 68, 641
19. Deathmatch: a player can skip the respawn timer at 3 s by pressing Jump. Spawn points are fixed and chosen relative to other players' positions. — GMR:51 (Respawn wiki oldid 117428)
20. Each spawn room has a healing zone and a barrier. Enemies inside an enemy healing zone take damage until they die, and this is not counted as environmental damage. — GMR:54; MAP:160 (Spawn Room wiki oldid 121624)
21. ⚠️OW2 page / UNVERIFIED for OW1: spawn heals 400 HP/s and players there are invulnerable. Intruders take 400 HP/s as self-damage, so it gives no ult charge. — GMR:56-57, 126; HCD:575 (Healing wiki oldid 121867 also lists 400 HP/s)
22. ⚠️OW2 page: when a spawn moves, the old room stays partly active for 7 s (heals, doors locked to enemies, no hero swap, Interact teleports to the new spawn). — GMR:58
23. Heroes can only be swapped in the spawn room. In Role Queue, players may only pick heroes from their queued role for the whole match. — GMR:61 (Competitive Play wiki oldid 128194)
24. The custom-game option "Respawn time scalar" can only lower respawn (0–100%) since Sept 2018. — GMR:537, 545 (patch notes Sept 2018); WCA lobby schema (OverPy)

## 3. Overtime
25. Overtime starts when an attacker touches the objective with 3 s left. While an attacker is on the objective the fuse bar stays full; after more than 20 s of overtime it burns faster. — GMR:64-65 (Overtime wiki oldid 115974; patch notes Jul 19 2016)
26. ⚠️OW2 page (mechanics probably OW1): the fuse is 5 s, then 3 s after 20 s of overtime, then 1.5 s after 30 s. There is no hard cap. — GMR:66
27. Control: overtime is checked at 99%, giving a window of about 4.2 s (≈96.5%). The team losing the point counts as contesting, so retaking it at 99% triggers overtime. — GMR:67, 270, 281 (Overtime wiki, current; Control wiki oldid 121392)
28. Escort: reaching an intermediate checkpoint during overtime adds time and ends overtime. — GMR:70, 217 (Escort wiki oldid 125449)
29. Assault: if A is captured in overtime, play continues with 3:00 on the clock. — GMR:71, 158 (Assault wiki oldid 123635)
30. ⚠️HIST: Competitive Sudden Death existed in Season 1 only (one short round, coin-flip attacker, 1:30–1:50 per map). Since Sept 2 2016 a tied Competitive match is a draw. — GMR:74-89 (patch notes Jul/Sep 2016)

## 4. Assault (2CP)
31. Attackers get 4:00 for point A; capturing A adds 3:00 for B. The A bonus was 4:00 before Jun 2019, and QP/Arcade had 5:00 per point until Jul 2018. — GMR:139, 158-161 (Assault wiki; patch notes Jun 2019, Jul 2018)
32. Capture progress rises only while attackers are on the point and no defender is. Speed scales up to 3 attackers. — GMR:146 (Assault wiki oldid 123635)
33. Point A capture time with 1/2/3 attackers is 30/20/15 s; per tick it is 10/7/5 s, cumulative 20/13/10 s. — GMR:149-153
34. Point B capture time with 1/2/3 attackers is 18/12/10 s; per tick it is 6/4/3 s, cumulative 12/8/6 s. — GMR:154-156
35. The bar has 3 ticks (33/66/100%) that act as checkpoints. Without attackers, progress falls back only to the last tick; the decay rate is not documented. — GMR:141, 146, 174, 177
36. Assault has 3 spawn rooms: 2 attacker, 1 defender. The attacker spawn moves forward after A; defenders always spawn behind B. — MAP:118, 128; GMR:164
37. Since Jul 2018, invisible Sombra cannot contest objectives. — GMR:166 (patch notes Jul 2018)
38. ⚠️Never shipped: the Jul 2020 Experimental Assault rework (6-min time bank, points per capture, 8 s / 11.5 s respawns). — GMR:168

## 5. Escort
39. The payload moves when an attacker is in range; any defender in range stops it. After 10 s with no attacker nearby it rolls back toward the last checkpoint. — GMR:196 (Escort wiki oldid 125449)
40. Speed multiplier by pusher count: 1 = 100%, 2 = 116.5%, 3+ = 133%. — GMR:208; MAP:138
41. Payload speed in m/s (1/2/3 pushers): Havana and Route 66 1.4/1.65/1.85; Junkertown and Dorado 1.55/1.8/2.05; Gibraltar and Rialto 1.7/2.0/2.25. — GMR:199-206 (Escort wiki oldid 125449)
42. Attackers near the payload heal 10 HP/s (OW1). HCD gives the range as 5 m. ⚠️OW2 lowered it to 5 HP/s. — GMR:189, 196, 216; HCD:575 (Healing wiki oldid 121867)
43. Competitive starting time is 4:00 (5:00 before Sept 2016). — GMR:192, 214 (patch notes Sept 2016)
44. Route 66 Competitive gives +60 s at checkpoint 1 (Mar 2017). — GMR:215 (patch notes Mar 2017)
45. ⚠️OW2: "4:00 / +2:30 / +1:30" and Unranked "5:00 / +3:00 / +2:00", plus 0.3 m/s rollback speed. Per-map OW1 checkpoint additions are not found. — GMR:216, 223-227; MAP:141
46. ⚠️OW2 page: on some sections the payload moves as if 3 attackers push and cannot be stopped. — GMR:219
47. Escort has 3 checkpoints and 6 spawn rooms (3+3); both teams' spawns move at each checkpoint. — MAP:119, 133
48. Not found: checkpoint pause duration and payload capture radius. — GMR:228-229

## 6. Hybrid
49. Point A capture with 1/2/3 attackers: tick 1 = 11/7/5 s, tick 2 = 21/14/10 s, tick 3 = 32/21/15 s. Speed caps at 3× even though the HUD shows up to 6. — GMR:239; MAP:145 (Hybrid wiki oldid 125447)
50. Payload speed in m/s (1/2/3 pushers): Hollywood, King's Row and Numbani 1.55/1.8/2.05; Blizzard World and Eichenwalde 1.7/2.0/2.25. — GMR:242-248
51. The payload can only be pushed after A is captured; it rolls back after 10 s and heals 10 HP/s. — GMR:250
52. On Blizzard World and King's Row the payload must be unlocked; on Eichenwalde, Hollywood and Numbani it starts as part of the objective. The A→CP1 leg is longer than CP1→CP2. — MAP:144 (Map wiki rev 2022-09-23)
53. Competitive objective time on Hollywood, King's Row and Numbani is 4:00 (5:00 before Sept 2016). Starting time and checkpoint additions per Hybrid map are not found. — GMR:255

## 7. Control
54. Matches are best of 3 in QP and Competitive (⚠️HIST: Competitive was Bo5 until Season 5). — GMR:262, 272 (Control wiki oldid 121392)
55. The point unlocks 30 s after the round starts and both teams have left spawn. Whether this also applies to rounds 2 and 3 is undocumented. — GMR:263, 270, 296
56. Capture time with 1/2/3+ players is 8/5/4 s. Any enemy on the point blocks the capture. — GMR:264, 271
57. The owning team's score rises 1% every 1.2 s, so 120 s of uncontested ownership wins the round. — GMR:265, 291
58. There is no round clock. — GMR:273 (Game Modes wiki oldid 124830)
59. Each map has 3 sections, picked at random per round. All sections sit in one map instance (Workshop `Objective Position(0..2)`). — GMR:267, 274-279; MAP:149-152
60. Each team has one spawn per stage, 6 per map; Control maps are symmetrical. — MAP:151, 198
61. Mystery Heroes resets heroes between Control rounds (Aug 2019). — GMR:282
62. Custom defaults: score to win 2 (range 1–3); capture and scoring speed 100% (range 10–500% in the OverPy schema; the wiki says 0–500%). — GMR:283-287; WCA Control settings (OverPy customGameSettings)

## 8. Competitive scoring, time bank and tiebreaks
63. Escort: the farther push wins. Equal distance, or no movement by either team, is a draw. The best distance counts even if the payload later rolls back. — GMR:310 (Competitive Play wiki oldid 128194)
64. Assault: more points or more progress wins. Progress only counts at ≥33.3% (a tick); if neither team reaches it on the same point, or both are exactly equal below 100%, it is a draw. — GMR:311
65. Hybrid: same rule as Assault. It is a draw if neither team reaches 33.3% on A or moves the payload. — GMR:312
66. ⚠️HIST: the tiebreak threshold was 1% (Apr 11 2017) and became 33% in late Apr 2017. — GMR:313 (patch notes Apr 2017)
67. Time bank for Escort/Hybrid when both teams complete the map:
    - Both teams ≥120 s: the lower bank is cut to 120 s and the other is cut by the same amount.
    - Both 60–119 s: no change.
    - Either below 60 s: the lower bank is raised to 60 s and the other gets the same increment.
    - The team with less time attacks first; no time is added at checkpoints.
    — GMR:314-319
68. Mar 2020 rule: if both banks exceed 2:00, both are cut equally until one reaches 2:00 (4:37/2:50 becomes 3:47/2:00). — GMR:325 (patch notes Mar 2020)
69. Assault/Hybrid extra rounds:
    - A team with 0 s against one with ≥60 s: only the other team attacks, and it wins with ≥33.3%; otherwise the match is a draw.
    - Both at 0 s: both get 60 s.
    - In Assault extra rounds, capturing A adds only 30 s; in Hybrid nothing is added.
    — GMR:320-324
70. One copy of each hero per team in Competitive (Jul 2016). — GMR:327
71. Role Queue is 2 Tank / 2 Damage / 2 Support with heroes limited to the queued role (beta Aug 13 2019, official S18 on Sept 3 2019). Open Queue Competitive is a separate queue with its own SR (Jun/Jul 2020). Party Flex since Jan 2020. — GMR:329-332; PCS:109
72. ⚠️HIST: map pools (S19, 12 maps, 3 per mode) were removed Apr 14 2020; hero pools (S21–22) were removed Jun 8 2020. Horizon and Paris have been out of Competitive since Apr 14 2020. — GMR:163, 334-335

## 9. SR, placements and ranks
73. SR runs 1–5000 (⚠️HIST: 1–100 in Season 1). A draw gives no SR change. New players get larger swings over their first 10–20 matches. — GMR:337-338
74. Placements: 5 per role in Role Queue (it was 10 before S18), 10 in Open Queue. Season 35 notes say 5 placement matches in a selected role. — GMR:306, 330; PCS:101 (Season 35 forum post)
75. Tiers: Bronze <1500, Silver 1500–1999, Gold 2000–2499, Platinum 2500–2999, Diamond 3000–3499, Master 3500–3999, GM 4000+. — PCS:106 (esports.gg; fandom summary)
76. Demotion: Silver to Diamond demote after staying under the tier minimum for 5 games; Master and above demote immediately. — GMR:339
77. The SR gain/loss formula and placement caps are not public. — GMR:361; PCS:120
78. Group SR limits: Diamond and below within 1000; Master within 500; GM/Top 500 within 350 and duos only (GM was 250 from Nov 2017 to Sept 2018). Since Aug 2020, a party with anyone ≥3900 SR is limited to 2 players. — GMR:340 (wiki; patch notes Sept 2018, Aug 2020); BNM:120 (Mercer S8)
79. SR decay: Diamond and above lose 25 SR/day (⚠️HIST: 50/day) after 7 days idle, with a floor of 3000; each match adds 36 h before decay, up to 7 days. Whether decay still existed in 2022 is uncertain (Kaplan's "no more SR decay" statement is undated). — GMR:342-343; PCS:110, 123 (patch notes Sept 2016, Aug 2017)
80. Competitive unlocks at player level 25 (still true in S35). Seasons last 2 months from S6, starting "the first Thursday of every other month". S36 starts Sept 1 2022. — GMR:349; PCS:20, 101, 108
81. Competitive points: 15 per win, 5 per draw, 0 per loss; accrual stops at 6000 unspent (⚠️HIST: S2 paid 10 per win and 3 per draw). — GMR:345, 348; PCS:104
82. End-of-season CP per role (Bronze→GM): 25/50/100/200/300/450/650. Open Queue: 65/125/250/500/750/1200/1750. Maximum total: 140/275/550/1100/1650/2550/3700. GMR gives Master as 500 per role; S35 gives 450. — PCS:102 (S35 forum); GMR:346-347
83. Golden weapons cost 3000 CP each. — PCS:54
84. Top 500 has 5 boards (Tank, Damage, Support, combined, Open Queue). Role boards need ≥25 games, Open Queue needs ≥50, and SMS Protect is required (since Jun 2018). — PCS:103
85. Completing placements grants a season spray and icon; Top 500 adds an icon and an animated spray. — PCS:102

## 10. Leavers and backfill
86. Quick Play leavers are backfilled. Repeated early leaving gives −75% XP on later matches, cleared by finishing matches. — GMR:105 (Quick Play wiki oldid 119861)
87. Competitive leavers cannot join another game until the match ends and are not backfilled. If they never rejoin, they lose 50 SR and get escalating suspensions up to a season ban. — GMR:106 (Competitive Play wiki oldid 128194)
88. Leaving a single match more than 3 times gives a penalty even if the match is completed; leaving without rejoining counts as a loss (Jul 2016). — GMR:108
89. A Competitive leaver waits at least 10 min before the next Competitive match (Aug 2016). — GMR:109
90. Rejoin is allowed any time while the match runs. Teammates of a disconnected player may leave after 2 min with no penalty, but take a loss (Oct 2016). — GMR:110
91. Penalties became more severe in Sept 2020. — GMR:111
92. Priority requeue is granted after a collapsed Competitive game (Jun 2020) and after finishing a backfilled game (Aug 2020). — GMR:113-114
93. Deathmatch AFK timer is 120 s (60 s before Jun 2018). — GMR:115
94. Repeat leavers enter "leaver status", shown on the player card, until they complete enough games. — PCS:157

## 11. Deathmatch, Elimination, CTF and other Arcade modes
95. FFA: 8 players, first to 20, 10-minute limit (highest score wins). +1 per final blow; −1 for an environmental or self-inflicted death. The top 4 get win credit. — GMR:369, 386, 389 (patch notes Aug 2017; Arcade wiki oldid 127559)
96. TDM: 4v4, first to 30, 10 min. A Mercy resurrection before respawn removes the enemy's point. — GMR:387; custom ranges GMR:394-395
97. Killing B.O.B. gives no Deathmatch score (Feb 2020); Ana can Nano Boost herself in FFA (May 2018). — GMR:390, 392
98. Deathmatch maps have no spawn rooms; Elimination and CTF maps have one per side. — MAP:155, 198
99. Elimination: 1v1, 3v3 or 6v6. First to 3 round wins (1v1 is first to 5). No respawn within a round. Lockout: heroes from a won round are banned. — GMR:370, 398
100. Elimination custom defaults:
    - Hero selection 20 s (20–60); score to win 3 (1–9).
    - Capture tiebreaker after 105 s (30–300) with a 3 s capture (1–7).
    - Draw after 135 s (60–300); hero reveal after 75 s (0–180).
    - Limited hero pool = team size +2.
    — GMR:401-408; WCA Elimination settings
101. Arcade duels: Mystery Duel uses the same random hero, excluding Bastion, D.Va, Lúcio, Mercy, Reinhardt, Torbjörn, Winston and Zarya. Limited Duel offers 3 random heroes per round. — GMR:399
102. CTF (final OW1 rules from Feb 8 2018):
    - Pickup is instant; a dropped flag is locked for 5 s.
    - Return takes 4 s and is not interrupted by damage.
    - The match is 8 min and 3 captures win; the flag respawns 15 s after a score.
    — GMR:414-419 (Blizzard forum CTF update Feb 8 2018)
103. Abilities that drop the CTF flag include Tracer Blink and Recall, Reaper Shadowstep and Wraith, Mercy Guardian Angel and Valkyrie, Soldier Sprint, and others listed per hero. Lúcio Wall Ride was added Feb 13 2018. — GMR:420-440
104. CTF sudden death: when time runs out, a new round starts with flags closer to the centre and the first score wins. The game does not end while a flag is outside its base; overtime only burns once both flags are home. — GMR:438, 441 (the wiki's "5-minute timer" conflicts with the official 8 min)
105. CTF custom options:
    - Defaults match the rules above.
    - Pickup time 0 (0–5); respawn speed buff 0 s (0–60).
    - Team does not need its own flag at base to score; Blitz flag locations off by default.
    — GMR:445-457
106. ⚠️HIST: Capture the Rooster (2017) was a 7-minute match where ties were a draw; from Mar 2017 a team's own flag had to be secured before it could score. — GMR:411, 413
107. ⚠️OW2 page: CTF respawn 13 s, Blitz 5 s. — GMR:459
108. Total Mayhem: 200% health, 150% ult gain, 75% faster cooldowns. Low Gravity: 25% gravity. No Limits: no hero limit. — GMR:462-464
109. Mystery Heroes: a random hero on each respawn, never the same hero twice in a row. A self-kill keeps the hero but resets ult and removes deployables (Sept 2017). — GMR:465-466, 619
110. Bounty Hunter (Feb 2021) custom defaults: 100 per kill, 300 for killing the Target, 300 per kill as Target, 1 Target, 10 min, 20 to win. The Target loses status if they take too long to get a kill. — GMR:469-471; WCA Bounty settings
111. Arcade weekly reward: 1 loot box per 3 wins, up to 3 per week, reset Tuesday 00:00 EST. Sources disagree on whether 3 or 9 wins are needed for 3 boxes. — GMR:473; PCS:48, 64
112. Skirmish (and "While You Wait", Dec 2019) has no score; custom-game Skirmish gives no XP (Mar 2017). — GMR:474, 568

## 12. Practice Range, Tutorial and seasonal PvE/PvP
113. Practice Range has no objectives and 4 friendly + 11 hostile training bots that stand still, shoot or move, plus range markers in metres. — GMR:496; MAP:360 (Practice Range wiki oldid 95841)
114. Training bot: 200 HP (⚠️OW2 page says 250). Duo Cannons: projectile, 8 damage, 60 m/s, 4 shots/s, 8 ammo, 1 s reload, can headshot. — HCD:600; WCA:888-892 (Training bot wiki oldid 123913)
115. Practice Range custom settings: Spawn Training Bots on, Training Bot Respawn Time Scalar 100% (10–500%), Training Partner on. — GMR:498-501; WCA:798
116. The Workshop Practice Range mode defaults to 6 players on team 1 and 0 on team 2. Health packs: 2 small and 2 large at known Workshop coordinates. — MAP:362, 365
117. ⚠️OW2 page: bot types (Standard 250, Tank 200/400, Sniper, Rocket, Hero Bot), consoles and the jump pad may postdate OW1. — WCA:894-897
118. Training bots appear in the kill feed when destroyed (Mar 21 2017); bots return to position after displacement (fix May 3 2018). — WCA:899-901
119. Tutorial: Athena guides the player as Soldier: 76, then leads into the Practice Range. The server spawns the hero without a hero-select screen (seen in the 0.8 beta). — GMR:503; MAP:384; EMU:22
120. Lúcioball: 3v3, 4 min (Copa 6 min); a tie goes to overtime that ends at the next goal. The goal-lead mercy rule is reported as 10 or ">5" (conflict). — GMR:505-506
121. Yeti Hunt: 5 Mei against a 5000 HP Yeti (Winston) on Nepal Village. Each meat gives 25% ult; Primal Rage triggers automatically at 100%. The Yeti wins with 5 Hunter kills. — GMR:510-514
122. Uprising: 4 players, 40 s revive window, mission fails if all are down; defend the payload for 4 min. Junkenstein Endless has 12 waves. Difficulties are Normal, Hard, Expert and Legendary. — GMR:517-521

## 13. Practice vs AI and bot behaviour
123. Practice vs AI is 6 players against an AI team. The party leader picks Easy, Medium or Hard, which deal 25%, 50% and 100% damage. Bots never headshot; higher difficulty also improves accuracy. — WCA:853-854; GMR:502 (Practice vs AI wiki rev 2022-03-09)
124. There are 12 AI heroes: Roadhog, Zarya, Bastion, Cassidy, Mei, Reaper, Soldier: 76, Sombra, Torbjörn, Ana, Lúcio, Zenyatta (⚠️HIST: 6 at launch, anecdotal). — WCA:855, 859
125. The mode is random among Assault, Control, Escort and Hybrid. XP is always 90%. It is the only matchmade mode with bots. — WCA:856-857; MAP:390-391
126. Custom "Add AI" lets the host choose hero or role, difficulty, count and team. "AI only works on standard game modes"; OW1 had Easy/Medium/Hard only. ⚠️OW2 added 8 difficulties and a 1/2/2 recommended AI composition. — WCA:824-825, 878, 924
127. Bot behaviour (⚠️current OW2 wiki, "some likely predate"):
    - Never crouch; no movement abilities outside spawn (except Lúcio Amp It Up and Soldier Sprint).
    - Jump only to navigate terrain.
    - Take cover when certain enemy ultimates are activated.
    - Swap heroes after several deaths unless their ult is ready.
    - Ignore enemies slept by Sleep Dart.
    — WCA:866-871
128. More bot behaviour (same source and flag):
    - Greet each other in spawn during setup and call out ult readiness.
    - May retreat to spawn when outnumbered.
    - Damage and support bots follow the tank.
    - Bots cannot be pushed by players but can be body-blocked.
    - Abilities bots never use include Genji double jump, Lúcio Wall Ride, Reaper Shadow Step, Widowmaker Grapple and Zenyatta alt fire.
    — WCA:872-876
129. Workshop dummy bots: at most 6 per team or 12 in FFA, regardless of lobby settings. With no Workshop actions they do nothing; they are driven only by injected input (throttle, facing, button presses). — WCA:880-882
130. The notes infer that hero AI is server-side, uses the same input-injection path as dummy bots, and needs a per-map navmesh connected to spawn points. — WCA:908-909, 913 (inference)
131. Bots have no Connection or InputStream component, so the AFK system skips them. — GNR:33 (Ford GDC 2017, kevinan translation)

## 14. Combat formulas
132. Damage = base × (1 + Σ dealt modifiers) × (1 + Σ taken modifiers) × headshot multiplier − armor. Armor is applied last. — HCD:546, 549 (Damage modification wiki oldid 120815; Hit points wiki oldid 125959)
133. Damage boosts: Mercy +30%, Nano +50% (also −50% damage taken and 250 heal), Orisa Supercharger +50%, Amplification Matrix +100% to projectiles. A projectile is boosted only if the shooter was boosted when it was fired. Discord is +30% taken (the wiki table says 25%). — HCD:547
134. Some damage cannot be boosted: B.O.B., Self-Destruct, Dragonstrike, Steel Trap, RIP-Tire, Flux slam, Symmetra turrets, Torbjörn turret, Molten Core DoT, Venom Mine, Minefield and Graviton DoT. — HCD:548
135. Damage reduction: Fortify 40%, Ironclad 20%, Take a Breather 50%, Nano 50%. Example: a Fortified Orisa hit for 100 takes 60 − 5 = 55. — HCD:549
136. Armor subtracts 5 per damage instance, capped at 50% of the instance; 1 point of armor is enough for the full reduction. Beams (Echo beam, Mei, Moira, Symmetra, Winston, Zarya) are cut by a flat 30% instead. DoT ignores armor. Use OW1 armor rules, not the OW2 rule (Jul 2022 beta only). — HCD:550, 605
137. Damage order: temporary HP first (shields, then armor), then recoverable HP (shields, then armor, then health). Healing restores health, then armor, then shields. EMP damages shields only. — HCD:551
138. Shields regenerate 30/s (5.76 per 0.192 s tick) after 3 s without damage; this counts as healing. Barriers regenerate only while lowered, after 2 s (Reinhardt 200/s, Brigitte 85/s, Sigma 80/s). — HCD:552
139. Temporary HP:
    - Rally: 15 armor per 0.5 s up to 100, lasting 30 s.
    - Overload: 100 armor for 5 s.
    - The Best Defense: 30 per ability hit and 75 per Meteor Strike hit, max 150, decaying 3/s after 1 s.
    - Sound Barrier: 750, decaying over 6 s.
    - Kinetic Grasp: 60% of absorbed damage, max 400, decaying 7/s after 3 s.
    - Adaptive Shield: 100 + 75 per enemy in range, lasting 7 s.
    — HCD:554-559
140. Headshots deal 2.0×. A fully charged Widowmaker scoped shot (and its deflect or Echo copy) deals 2.5×. Deadeye and Tactical Visor always hit the body. Fortify blocks crit damage (since Feb 2021). — HCD:563 (Critical hit wiki oldid 124419)
141. Falloff is linear from 100% at the near distance to 30% at the far distance (e.g. Tracer 6→1.8 between 12 and 20 m; Soldier 20→6 between 30 and 50 m). — HCD:532, 603 (inference from wiki min/max pairs)
142. Health packs: small 75 HP with 10 s respawn; large 250 HP with 15 s respawn. A pack cannot be used at full HP or while anti-healed. Packs cleanse Ana, Ashe and Widowmaker DoTs. Biotic Grenade amplifies packs 50%. A Sombra-hacked pack is denied to enemies and respawns 4× faster. — HCD:574; MAP:163 (Health pack wiki oldid 122064)
143. Health-pack counts (small/large) per map are in MAP:165-183 (workshop.codes), e.g. Hanamura 10/8, Dorado 11/8, King's Row 9/6.
144. Self-heals: Mercy Regeneration 20/s after 1 s without damage; Bastion Self-Repair 90/s; Take a Breather 300 over 1 s; The Reaping 35% of damage dealt; Moira Grasp 24/s; Coalescence 50/s self; Recall restores the highest HP of the last 3 s. — HCD:576-581
145. Movement: forward 5.5 m/s (Tracer and Genji 6.0); backward 90%; crouch 3 m/s. Buffs, enemy slows and self penalties each stack additively within their category; a self penalty and an enemy slow do not stack (the larger applies). — HCD:584 (Movement speed wiki oldid 117302)
146. Environmental kill: the last crowd-control applier gets credit, and the kill counts as damage equal to the victim's remaining HP (it gives ult charge). — HCD:586
147. Low-health signals: ≤50% is "low", ≤25% is "critical", ≤35% plays a gasp and turns the screen edge red. Health bar segments are 25 HP. — HCD:562
148. Crowd-control classes: knockback, knock-up, stun, knock-down, sleep (breaks on any damage), slow, pull, root, anti-heal, and hack/EMP (EMP also destroys barriers). — HCD:587-597 (Status effect wiki oldid 124015)
149. Workshop has 10 statuses: Hacked, Burning, Knocked Down, Asleep, Frozen, Stunned (these four block movement, aim, weapons and abilities), Unkillable (HP stays ≥1), Invincible, Rooted, Phased Out. Discord, Nano and anti-heal are modifiers, not statuses. — WCA:702-720
150. A player can have at most 16 health pools per type (health, armor, shields). — WCA (OverPy actions.js, Add Health Pool limits; §1 Limits)
151. 0.192 s DoT/HoT ticks suggest 16 ms simulation ticks (62.5 Hz) with DoT applied every 12 ticks. — HCD:604 (inference)
152. Final OW1 balance patch: Jan 6 2022 (Earthshatter +200 damage within 1.75 m; Storm Arrows ricochet; Cassidy can roll in the air; Torbjörn alt fire costs 2; Wrecking Ball 6 s grapple cap). Mercy quality-of-life change Jan 25 2022. Later 2022 patches have no balance changes, and the Apr–Aug 2022 notes belong to the OW2 beta. — HCD:530-531, 633, 664

## 15. Ultimate charge
153. Every hero gains 5 charge per second passively. — GMR:596; HCD:565 (Ultimate ability wiki oldid 123286)
154. Damage to enemy heroes gives 1 charge per point, including environmental kills. Self-damage, damage to non-heroes and ultimate damage give nothing. Damage to Roadhog gives 75% (since Aug 2021); damage to Adaptive Shield gives nothing. — GMR:597-599, 603; HCD:566
155. Healing gives 1 charge per point, except The Reaping, Mercy Regeneration, hacked packs, and healing of self-damage or environmental damage. — GMR:600; HCD:567
156. Mercy gains charge equal to the damage her boost added; this is a separate category, unaffected by the custom ult-generation settings. — GMR:601, 621
157. Most heroes cannot charge while their ult is active, except Ashe, Hanzo, Torbjörn, Tracer and Wrecking Ball. Echo charges 6.5× while duplicating. — GMR:602; HCD:569
158. Ult charge is spent instantly on activation (since Oct 2017). — GMR:604
159. Ult costs (points): Call Mech 319.2, Pulse Bomb 1260, EMP 1400, a 1540 group (including Earthshatter), Meteor Strike 1680, Nano Boost 2100, B.O.B. 2240, Amplification Matrix and Configuration: Tank 2310, Rally 2800, maximum 2940. Maximum passive time = cost / 5. — GMR:605-616
160. Ultimates with a cast time can be interrupted during the cast; channelled ones throughout; transformations cannot. Graviton and B.O.B. cannot be interrupted while casting; Meteor Strike cannot be interrupted. — HCD:573
161. ⚠️UNVERIFIED: ult reset on hero switch and carry-over between rounds (common belief: kept between Control rounds, not across the attack/defence swap). — GMR:629-634
162. Custom ult settings: Ultimate Generation 10–500%, Combat 0–500%, Passive 0–500%. — WCA:801

## 16. End of match, POTG, cards and endorsements
163. POTG plays at the end of every match ("Play of the Match" in Competitive). Categories are High Score, Lifesaver, Shutdown and Sharpshooter; the highest score in any category wins. — GMR:93; RSU:77, 82
164. Sharpshooter weighs shooter speed, distance, headshot and airborne victims. Shutdown means killing an enemy about to do something impactful. Lifesaver means saving a teammate from death. Weights are not public (patented). — RSU:83-87, 122
165. Dead heroes cannot get POTG (fix after a beta POTG for a turret whose Torbjörn was dead). Turret kills credit the builder. — RSU:44, 88 (GDC 2017 replay talk via lfzxb)
166. ⚠️OW2 page: POTG is a 5 s intro plus 12 s of play (17 s), and a player who quits cannot get POTG. — GMR:94
167. End-of-match card voting existed in OW1 (a Feb 2018 fix stopped bots voting on cards). Card and screen durations are not found. — GMR:95, 131; RSU:164
168. Endorsements (Jun 2018): Shot Caller and Good Teammate for teammates, Sportsmanship for anyone; the same player at most once per 12 h; levels 1–5 with decay. — GMR:96-101; PCS:151-152
169. Endorsement count conflict: 3 per match with 50 XP each (GMR), against 2 per game per the current wiki, with the 50 XP unconfirmed. No endorsements in custom games. — GMR:98; PCS:31, 152
170. Endorsements were extended to Mystery Heroes, No Limits, Deathmatch, Total Mayhem, Low Gravity and PvE events (Aug 2019). — GMR:102; PCS:153
171. Endorsement-level loot boxes (forum reports): level 2/3/4/5 gives 1/2/3/4 boxes, at an irregular cadence. — PCS:49
172. Medals per stat compare players in the lobby (gold, silver, bronze); only the highest medal earns XP (50/100/150). — PCS:134
173. Kill cam: the server sends about 8–12 s of recorded network data. The client plays it in a second ECS world, and respawn interrupts it. The reel must arrive between death and respawn (effective window ~0.5 s). — GNR:99, 304; RSU:35-41
174. Replays are recorded at 20 Hz deltas with a full snapshot every 1 s, about 1 MB/min per buffer. — RSU:25-27
175. The Replays tab keeps the 10 most recent matches (not Tutorial or Practice Range) until the next patch. Share codes are ~6 characters, valid until the next patch; up to 10 replays can be pinned. — RSU:95, 101-102
176. Highlights: Today's Top 5 kept 24 h or until a patch; manual capture of 12 s, up to 36 stored. — RSU:78, 92

## 17. Progression, XP and loot
177. XP (launch-era fan wiki): 4.01 XP/s in match; medal 50/100/150; match finish 250 (Dot Esports says 150); win 500; first match (or win) of the day 1500; group +20%. — PCS:11-12 (Fextralife; Dot Esports)
178. XP per level (⚠️HIST launch table): L1 1500, rising to 20000 from L19+. Since Nov 2016 levels 2–13 are unchanged, 14–100 are "slightly reduced", and levels after 100 are a flat 20000. The curve no longer resets after each promotion. — PCS:13-14 (patch notes Nov 2016)
179. One loot box per level-up with 4 items, at least one Rare or better. Epic is in about 18.5% of boxes, Legendary about 7.5%. Contents are decided when the box is received, and duplicates pay credits. — PCS:15, 39-42 (Weird Gloop; China disclosure)
180. Credits as loot: 50/150/500. Item costs: Common 25, Rare 75, Epic 250, Legendary 1000; new event items cost 3× during their event. Duplicate refunds: 5/15/50/200. — PCS:43-44
181. Borders change every 10 levels and a star is added every 100. Tiers: Bronze 1–600, Silver 601–1200, Gold from 1201, Platinum 1801–2400, Diamond from 2401. The border stops changing at 2901. Profiles store level (1–100) plus prestige. — PCS:16, 19
182. Group finder (Jun 2018): mode, SR range, voice chat, minimum endorsement, per-slot roles with "Enforce Roles". — PCS:154

## 18. Matchmaking and netcode
183. The matchmaker matches groups of similar size and will not create a game where either team's predicted win rate (from SR) is below 40%. Group size does not change SR math. 2018 stats: 6-stacks met groups of 4+ 84% of the time, with a 52.88% win rate. — BNM:119 (Mercer, "Groups and Matchmaking", Jun 20 2018)
184. Season 8 (Dec 2017): limits on SR spread within a team; performance-based SR adjustments removed for Diamond and above. — BNM:120
185. Netcode: 16 ms command frames (62.5 Hz; 7 ms in tournament mode).
    - The client runs about RTT/2 + 1 frame ahead and sends every input since the last acknowledged one.
    - Under packet loss the server repeats the last input and tells the client to run on 15.2 ms frames.
    - Server→client updates: ~20.8 Hz before 2016, then 63 Hz ("high bandwidth"), scaled down adaptively for weak connections.
    - The server rewinds targets to the shooter's frame; damage is never predicted.
    - Above ~220 ms ping impacts are not predicted, and escape abilities can win over favour-the-shooter.
    — GNR:11-15, 47, 64-66, 83-86, 322, 326; BNM:123; TRR:27-28

## Gaps the notes call out
- Per-map Escort/Hybrid checkpoint time additions, checkpoint pause durations and payload radius (GMR:227-229).
- Assault tick-decay rate (GMR:177).
- Control decay and the time between Control rounds (GMR:295-296).
- Exact attacker-advantage curve (GMR:130).
- POTG, card and end-screen durations (GMR:131).
- SR formula (GMR:361).
- Ult carry-over rules (GMR:634).
- Exact endorsement formula (PCS:164).
- XP table for levels 14–100 after Nov 2016 (PCS:28).
- Elimination round timer and whether ult carries between rounds (GMR:482).
- AI hero roster history and which maps support AI (WCA:922; MAP:400).
