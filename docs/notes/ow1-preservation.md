# ow1-preservation findings

Every fact a 104319 server can use from [p3n1cillin/ow1-preservation](https://github.com/p3n1cillin/ow1-preservation) (AGPL-3.0, `main` at `89d94263`), restated in our words with line citations into that repository. `related-projects.md` summarises the most important ones next to the other projects' findings.

How far to trust it:
- The game code (responder, AI, navigation, mode host) is private (`.local/`). The public files are notes written by AI agents (Claude and Codex) during the work: `HANDOFF.md`, `CLAUDE.md`, `DEVLOG.md`, `docs/*.md` and the numeric layouts in `protocol/*.json`.
- Evidence classes: **live** = seen with the original 104319 client in one of their runs; **emulated** (or emu) = the client's own code run on a memory dump; **static** = read from the client code or data; **claimed** = stated without evidence. Later corrections are marked **RETRACTED**.
- Addresses are client RVAs for 104319 unless written as absolute.
- Line numbers inside a section can be off by a few lines.
- Their results are for their own server; anything taken from here must be checked against our graphs and with our client tests.

`DEVLOG.md` is summarised in `related-projects.md` (section "ow1-preservation's development log"); `research_notes/` is in `game-rules.md`.

## `HANDOFF.md`, lines 1–1100 (2026-10-16 and earlier)

### Current state summary (`HANDOFF.md:50-63`)
- Working flow: load-in, the original SELECT A HERO screen, pick, first-person spawn in the spawn room, move and look, fire. Firing works for Tracer, Soldier: 76 and McCree, with tracers, impacts, ammo and reload. Native Tracer Blink works with collision and server-tracked charges. 13 static training bots are present. Pressing H in the spawn room reopens hero select repeatedly: run 218 went Tracer → Soldier → McCree. Hero Gallery lists all heroes (run 198). live
- Later runs add these, all live: full Tracer HUD including the portrait, Soldier first-pick HUD, Pulse Bomb throw, and two clients seeing each other (372-374, pair-e3-367).
- Hero re-pick **regressed in run 375**. Authored bot movement and full combat and roster behaviour are unresolved. Old teleport recipes are not a good baseline. live / claimed

### Nearby Players discovery (Codex, runs 380-382) (`HANDOFF.md:65-113`)
- Server message **20815** (schema 446200228) carries an array of embedded player records at +0x78. It fills the native Social → **Nearby Players** tab. `:67-69`, live (381)
- In Nearby Players, right-click → Invite to Group sends a real **22102**. The follower's Yes sends a real **22110**. Both clients then get the two-member roster. The leader (Mercy) alone starts Training / Practice; both enter, pick Tracer, and spawn with full HUD and weapons. The follower's Blink consumes 3 → 2 charges. `:69-75`, live (381)
- Each client got its own instance referral on its own port (1119 / 1129). Both responder sessions were bound, with no bind refusals and all 12 cleanup fields true. `:72-74`, live
- Run 382: when 20815 is sent at login, Nearby Players is already filled when Social opens. The native Invite / Yes / roster / leader-only queue path works again. The leader's volley and volley-end events were logged. `:76-83`, live
- How the probe works: each client receives the other configured account once. The seeded `suggest` control is no longer needed. Routing of 22102 no longer requires `party_suggested`. `:84-90`, static
- Run 380 failed before launch: the full RE descriptor set exceeded the codec's **64-message limit**. They fixed it by sending a 41-message subset. A full extraction has 126 messages. `:91-95`, live negative
- The public `protocol/nearby-players-104319.json` holds only the verified 20815 layout. **20809 / 20810 / 20816** are candidates that were NOT sent, so do not give them friend or recent-player meaning. `:96-98`, static
- **Friends still spins and Recent Players is empty.** Requests seen around the menus: **22207** and **58400**. Nothing shows that the empty **22210-22213** requests load those lists. `:98-100`, live
- The nearby list is a configured two-account fixture, not live presence. The peer still floats in a fixed pose. Still to do: automatic presence and removal, party leave and re-invite, peer movement/pose/weapon replication, and shared bot and combat state. `:109-113`, claimed

### Party before Practice (Codex, runs 376-379) (`HANDOFF.md:115-176`)
- The earlier Claude E2/E3 runs entered Practice separately on each client. They did not test invites or party joining. `:117-119`, claimed
- Run 376: both clients reach the menus (accounts Mercy#1002 / Tracer#1001). Friends stays loading, Groups has no active channels, and chat does not open in the initial menu state. `:123-126`, live
- **20701 is a group suggestion, not an incoming invite.** The native banner reads "Mercy suggests Tracer to the group?" with Invite / Don't Invite. Clicking Invite sends **22102** (schema 3003079262) with the target game account identity at +0x78. `:127-132`, live (377)
- The probe sends the suggestion to the LEADER and routes the leader's real 22102 into an incoming **20703** for the other account. It records all native 221xx requests. `:133-139`, static
- The initial login notification list is **20504 / 20505 / 20502 / 20802 / 20700**. A second list is empty, so login data is not repeated. `:140-143`, emulated (offline check)
- **20703 shows a group merge request.** Clicking Yes sends **22110** naming the leader's identity. After the two-member **20700** roster arrives, the follower shows "You have joined a group!", Training is disabled on the follower, and Mercy stays leader. `:144-147`, live (378)
- The leader alone starts Practice. The wrapper forwards the accepted Training queue request to the follower's own session. Each gets its own referral, picks Tracer and spawns with HUD and weapons. The follower sees the leader's proxy. In 378 the incoming request was a server fixture, because the leader's suggestion expired behind the Tutorial. `:147-152`, live
- Run 379 did the full native chain: dismiss tutorials, seed the suggestion, Invite → 22102 → 20703 → Yes → 22110 → automatic 20700 to both. The leader-only Practice start pulls the follower in automatically. Both get the full HUD (portrait, name, health, crosshair, ult, ability, ammo). Follower Blink goes 3 → 2. `:153-163`, live
- Limits: the bots still exist per session, not shared. Proxies float and have no locomotion, no weapon animation and no PvP damage. 20703 working does not mean all invite variants work. `:164-170`, live / claimed
- Offline checks pass: initial data is sent once, Invite goes only to the configured target, only the matching leader can accept, there is exactly one leader, and there is one follower referral on its own port. The public `protocol/party-104319.json` has five numeric descriptors. `docs/PARTY_PROTOCOL.md` documents the behaviour. `:171-176`, static

### Master goal and runs 327-331 (Claude) (`HANDOFF.md:178-248`)
- The master goal is complete OW1 with 6v6; priority 1 is hero HUD ownership and routing (`docs/MASTER_GOAL.md`). `:180-182`, claimed
- Run 327 created controller graph **0x0bcf** (instance 5, state 1). Only its stack node 2 runs. HUD_Root shows no content and no warning icon, so run 231 did not reproduce. Of all **1586 graphs**, only 0x0bcf and 0x120b contain STU_501733D7. Function 0x9cf9b0 creates a render target for the replay/highlight viewer; it is not a general HUD_Root show. `:183-187`, live + static
- The 0x0cc9 Stack sets ShowHeroHud (UI var **0x34f5**) to false, but only while it is active. `:188`, static
- Presenter bindings: m_EC32138E = {screen 0a88…, binding list of id → expression over vars}; m_45216F79 = two UX ids. In 294, hero 0x01cf instance 11 had 0x2af6 = 1, and the main presenters #155/#235 are gated on NOT 0x2af6. It also had 0x7c20/21/22 = 1, the 0x2ad4/0x2e27 handles present, and 0x73ed = hero. `:189-193`, emulated
- Runs 328/329: server message **21616** carrying game message 0x0717 to the hero or controller has no effect (0x2af6 stays 1). Creating **0x0cb6 on the hero** (emulation aligned 471/471) makes the client drop the hero's 0x01cf/0x0cb6 instances and the pistols. **Do not put 0x0cb6 on the hero.** `:194-198`, live
- Senders of 0x0717: 0x0cb6 (client, target mode 1), 0x0cbb (Practice tree, server-side, target 0), and 10 others. `:199-202`, static
- Client send path: send node vt 0x263ac80 → 0xa06fc0 → 0xbe9cb0 (entity comp47 vt+0x60) → per-manager dispatcher 0x1b8d150 (16 callers). `:203-204`, static. This was later reinterpreted: 0x1b8d150 is a variable-write dispatcher, not a message path (`:236-237`).
- Module vtable roles:
  - Slot 0x30 is the frame-prefix reader: m0 0xc6a480, m3 0x7c0630, m4 0x787e70, m5 0x7e37c0; m1 and m2 have none.
  - Slot 0x38 is the per-entity reader: m1 0xa13b90, m2 0xb3a490, m3 0x7c06a0, m4 0x787f20.
  - m2 slots 0x60/0x68 = 0xb3b0d0/0xb3b1f0 are the tick functions. `:205-208`, static
- Module-2 per-entity records (0xb40130 → 0xd81f80) look like **combat events**: entity ids, two vec3, u16s, an entity list. The m2 tick applies them via 0xb40d40 → 0xb22900. `:208-210`, static (interpretation is claimed)
- Movement state encoding: the position is in qwords +0x70/+0x78, XORed with a rolling key at +0x80. The key steps before each word by one of 8 constants chosen by key & 7. A scanner finds encoded states in the authoritative history, the rewind/recorder nodes, and three states in controller comp59 at +0x70/+0x3f0/+0x770. **No state holds the hero's current predicted position.** `:211-216`, emulated
- Server placeables **0x80000300..303** (comp1 +0x90) sit at (28.02,6.31,3.73), (50.44,2.06,23.02), (54.57,1.0,42.15) and (34.24,1.99,22.97). The third is the claimed team-0 spawn point (comp 58). None of them are health packs. `:217-219`, emulated
- Run 330: spawning at (54.565,1.05,42.148) dropped the hero under the map to y −18.8, so placeable height is not a standing position. Use PickSpawn **(48.71,2.05,37.77)**. `:219-221`, live negative
- **Feed-based hit registration (run 331):** a helper reads the client's own position (entity 0x80000305, 60 Hz) and the server's TRACK position uses it instead of dead reckoning. Aimed hold-fire killed bot 6 with **53 hits**, with the server using the true position (49.25,2.0,23.08). Echoing the position back stays off. `:222-227`, live
- **The game-message bus is local** (static):
  - **No network record carries a game-message GUID.** Send-state nodes post messages when their state begins, including when the server activates that state via StDt (vt 0x263ac80, begin → 0xd4c3e0 → registry 0xd9a770).
  - The registry hangs off root 0x2f586e0 via 0xa00b10 and has **64 buckets keyed by entity^msg**.
  - Listeners (GameMessageEntry vt 0x263b408, begin 0xd4bd40, poll 0xd514b0) choose a mode at node+0x248. Client-only listeners in instances where vt+0x148 is true use mode 2 and match only REMOTE posts.
  - So to deliver message X to entity E, the server activates a sender state whose authored config yields X/E. The message section 0xa0cca0 has no GUID field. `:228-239`
- HUD listeners: 0x01cf waits for 0x0717 (#5), 0x1291 (#101), and 0x1509/0x1504/0x150a (#105/#109/#113), among others. 0x20e4 listens for 0x0c9c, 0x0717 and 0x1509/04/0a. `:240-241`, static
- Graph senders: 0x1291 comes from 0x20e4 #187 (state 13, client-only), and 0x0c9c from 0x0233 #1044 (client-only). No graph sends 0x1504/0x1509/0x150a; native code posts them with obfuscated GUIDs. `:242-244`, static
- Pistol child definition 0400000000000017 has a contact set, EE0FB992, model 0d00000000000413 (third-person pistol), and its own children 0x2d33/0x3024. Child 0x0169 is a single effect component. No child of the hero exists in 294. How children are created is unknown. `:245-248`, emulated

### Statescript server ack / reset bit (Claude, runs 307-321+) (`HANDOFF.md:250-352`)
- **Frame header reader 0x9b4160:** a reserved bit, then the CmFD-present bit (+0x4c), then a **reset bit (+0x4d)**. Their responder always sent 0. `:252-253`, static
- In 0xa0e3c0, if the reset bit is set, or a delta frame arrives while history exists:
  - it undoes predictions (0xa0f5d0) and applies the frame;
  - 0xa0a780 then drops the whole history list (mgr+0x90);
  - it replays only local inputs (mgr+0xc8, 0x48-byte items, tick at +8) newer than CmFD (+0x900). `:253-256`, static
- Without the reset bit, 0xa0b200 rolls back every prediction **40 ticks** after the oldest history entry. This is why Recall states always dropped at +0.64 s. Run 307 confirms the tick rewound to the E tick at the drop. `:256-259`, static + live
- Responder option `--hero-reset never|always|after-close` sets the reset bit on hero full frames. Live 308 (after-close): E is predicted and the mirror frame acks it with CmFD = E tick. History clears, and states 3/9/15/17/23 stay active for the full 2 s until mirror-off. **The prediction survives.** `:260-264`, live
- Writing instance var 0x018b (`1/18b=b1`) at +0.25 s does not end state 3. `:265`, live negative
- A server frame that enters state 0 calls its enter handler with **edx=1** (normal path), not edx=2. `:266-267`, emulated (283-e1). This partly contradicts the 299-304 claim that server-entered state 0 returns early (`:373-377`).
- Rewind state 0 (hash 41b3fc2b), enter handler 0x8ed090:
  - End time (+0x238) = start + duration, where the duration comes from def+0xf0 = var 0x00d7 (SetVar #31 = 0.9 s).
  - The state-data s64 then overwrites the end time, so token T is the END time: use T+900.
  - Start time is at +0x978.
  - The recorder node comes via 0xca2660 from def+0x108, state-ref var **0x72a3**. Recorder STU_CF950CB5 (**state 4, node 5**) writes it.
  - History index +0x270 = 0xc90880(recorder, look-back). The look-back is def+0xe8 = var 0x1bd5 = 3 (SetVar #23).
  - On success it sets node bit 9 (per-tick update). `:268-274`, static
- Run 309 had no state 4, so +0x270 = −1. Run 310b added state 4 (aligned 554/554): +0x270 = 0 and the update registered, but there was no movement. Runs 311b/312 (T+1250 / T+900, mirror vars s21=i3, s23=f0.9, aligned 674/674): still no movement, and the client exits on mirror-off. `:275-278`, live negative
- Rewind update 0x8ee160 (vt 0x2620e18 slot 0x80): progress = (clock − start) / (end − start), passed through eased curve 0x8fb1d0 (range −0.285..1.285). 0xc909e0 then needs two recorder samples around the cursor, else it returns null. `:279-281`, static
- **The recorder never samples.** Ring at node+0x1c0: 0x480-byte entries, capacity 101, holds 1 entry. Record function 0xc94a00 (slot 0x70, called each tick from instance tick 0x1b927c0 → 0x1b925f0) samples only when the hero's comp28 movement time is at or past the next time (+0x1d8, step 0.1 s). The comp28 inner ring (+0x400 cap, +0x410 idx, +0x414 count) holds 16-byte {f64 time, u32 seq} entries. `:282-287`, emulated
- In dump 313-r the comp28 ring holds only the **12 spawn-snapshot states** (t 53.44..53.79, seq 3340..3362, step 2). Only these append to it:
  - authoritative movement: 0xc5b640 from module-3 apply 0x64a970;
  - 0x803420 via 0x7fdf10;
  - 0x805970, which is slot 0x48 of interface vt 0x260fdb0 at +0x14f0 of the controller's comp59;
  - module-2 0xb25480.

  **Local prediction does not extend it**, so Recall needs streamed server movement states for the controlled hero, or the comp59 path. `:288-295`, emulated + static
- Movement snapshot sequence: module-3 reader 0x7ca130 uses state +8 (snapshot field 2) as the ring seq. A seq of 0 means the current server tick. Ring time = seq × 16 ms. Apply 0x64a970 → 0xc5b640 skips states whose seq is ≤ the newest ring seq. Client prediction 0x8013d0 runs simulator 0xdaa6c0 but never appends to the ring. `:296-300`, static
- User decision: the private server **may read the client's own predicted position**. The feed reads hero comp1 +0x440 f32 xyz at 60 Hz and re-resolves if entity +0xb0 changes. `:301-305`, static
- `--hero-pos-echo` (run 314) sends the fed position back as position-only snapshots. The hero sinks and drifts because the other snapshot fields reset, and it ends inside geometry at y ≈ 30. **Do not echo.** `:305-308`, live negative
- Server-side Recall rewind (runs 315-320): the responder replays the last 3 s of fed positions in reverse as teleport snapshots (0.9 s, 30 Hz). Outcomes:
  - 315 crashed on an emit bug, since fixed.
  - 316: the path rewinds correctly (z 22.75 → 37.4), but floor-height points sink and the end lands at **y 13.3** (spawn-room roof).
  - 317: lifting every point by 0.3 still ends at 13.3.
  - 318 adds snapshot **field 3** (0xc61b60, state +0xc: changed bit, raw bit 0, u32 monotonic teleport generation; verified 207/207 bits): no change.
  - 319 (6 Hz steps) and 320 (single final teleport): still 13.3. `:309-318`, live negative
- **Snapshot teleports are swept, not hard** (run 321): the client moves the hero from its predicted position to the snapshot over about 0.3-1 s, with collision, rising over obstacles. Results by target:
  - open ground: lands near the target;
  - outdoors near the spawn room (48.7,2.3,37.44): peaks at y 12.4, then falls to the floor (fine);
  - inside the room (48.71,2.05,37.77): climbs onto the roof at y 13.3;
  - a target inside geometry: ends at the fallback (52.4,30.2,14.1).

  This explains the old "spawn on top of the spawn room then fall" report. Ending the mirror before the final teleport (322) still lands on the roof. `:319-327`, live
- **A new generation makes teleports snap** (runs 323-326): with field 3 set to a new generation, the x/z jump is instant. Interpolation ring +0xe040 (0x30-byte {pos, time, gen}; 0xc5bd30/0xc5b820) interpolates only between equal generations. `:328-332`, live + static
- But about 0.3-1.2 s after a ground landing the client launches the hero straight up to a fixed hover height of **y ≈ 30.7**, at any x/z. Teleports while hovering drop correctly. While the same state is re-sent continuously (150 sends) the hero sits about 3.6 m below the target, then pops up once the stream stops. `:332-336`, live
- Cause given: the snapshot's non-position fields are left at the default prior (0x2f82d30). For example stance/height +0x34/+0x38 feed the computed +0x2c/+0x30 = −2.5. These disagree with the client's predicted state. `:336-339`, claimed
- Hard-teleport lead: 0xc5b640 with an equal or greater generation copies the state into the predicted entry at comp28 inner +0x420 + idx × 0x370. Field 1 (state +4 bit) gates velocity derivation in 0x7ca130. `:342-346`, static

### Recall prediction mechanics (Claude, runs 299-305) (`HANDOFF.md:354-390`)
- A live sampler reads memory at about 30 kHz to log state changes (resolving hero instance-1 node, queue and clock addresses). `:356-359`, tool
- **40-tick reconcile:** 0xa0b200 (called from 0xa093e0) computes client tick − 0x28. Client tick comes from 0x979210 → 0x7fc490 → vt+0x10. Once the oldest predicted entry is older than that, it sets instance bit 10 (inner vt+0xd8, 0x97b060), restores (0xa0f5d0) and replays (0xa0ab00). Live 301/302/304: E is predicted (states 17/3/9/15/23 on, queue fills), and **exactly 0.64 s later** everything drops. `:360-364`, static + live
- **CmFD is in client ticks.** The interpreter stores it at manager +0x900 and uses the current tick when it is 0. The hero graph-create frame has no CmFD, so +0x900 starts at the client tick. Their earlier frames sent 4000+seq, a value in the future. `:365-367`, static
- `--hero-cmfd-ticks` sends CmFD = max(prev+1, last command seq + 8) on each hero frame change. Live 302: CmFD 1669, and Ability +0x270 now resets to 0 after the reconcile (before, it stuck at 3). Live 305: Blink, firing and reload all work, so it joined the best recipe. `:368-372`, live
- Rewind state 0's enter handler 0x8ed090 (vt+0x280) returns at once when edx == 2 (server restore). The normal path sets +0x238 (end = now + config × 1000 ms) and +0x270, and the update needs +0x270 ≠ −1. So server-entered state 0 (runs 299/303: active 1.3 s with the correct start time) never moves. `:373-377`, static + live. Partly superseded: emulation shows the server entry uses edx=1, and the real blocker is the recorder (`:266`, `:282`).
- Live 303 (CmFD ticks + mirror + phase B): states 0/1 run server-entered with no movement, and the client exits with code 0x1 about 0.45 s after mirror-off. Live 304 (phase A only, 3 s): states still drop at +0.64 s even though the server frame lists them active, and mirror-off did not crash. `:378-381`, live
- State 3's update (vt+0x60, 0x8e5a90) re-queues records for states 3/31/11. Its runtime def +0xf8 is var 0x018b's config value; +0xf0 is null. What really ends state 3 is not established; the likely candidate is the wind-up animation via DataFlowMapping 10. `:381-385`, static / claimed
- Load-in race: run 300 lost hero select because the graph creates (33 s) went out only 1 s after the controller create (32 s). The best timeline became 31/32/34/36 s, and controller full frames wait for **60 graph-create sends**. `:388-390`, live

### Sessions 297-298 (Claude) (`HANDOFF.md:392-447`)
- Runs 297/298 hit the Windows lock screen (screenshot handle invalid), so no hero pick happened. `:394-396`, live (infrastructure)
- State 3 (54feb21c, the container of 9/10/11/23) ends only through its var-change listener (0x8e5f50 slot 0x90 → 0xc880c0 finish) when instance var **0x018b** is true. 0x018b is used only by state 3: it is not synced and no decoded graph writes it. So the server must drive the 3 → 0 transition. Emulating that finish starts rewind state 0 plus states 1/2/31. `:397-402`, static + emulated
- Data that native writers produce:
  - Stacks 15/28/33/35 = `1,0`;
  - states 1/2/22/37 = one 0 bit;
  - states 17/31/36 = no data. `:402-404`, emulated
- **State 0 data is 80 bits.** The reflected object (vt+0x168 = 0xd44bf0 → node+0x1c0, descriptor image+0x3168080, one s64 at +0x78, hash 0007bf47) is read as 8 raw bytes from the current bit position (the sub-byte offset is kept). The class's u16 (node+0x970) follows. A phase-B frame with `0:64,0:16` decodes 725/725 and starts nodes 1/2/3. `:404-409`, emulated
- The s64 is the client statescript clock when state 0 began (owner+0x1e0), which is the hero's command tick. ms = tick × 16 (+15). Example: dump 283-e1 owner+0x1d0 = 2398, while E was at command seq 2341 about 0.9 s earlier. `:409-412`, emulated
- Live 299 (two-phase): phase A at command seq 2270 and phase B at +0.4 s (T = 36720) were both accepted (81 client datagrams after phase B), but there was **no rewind**. The client exited with 0x1 on mirror-off. The same off frame decodes 600/600 offline, so the exit is semantic, not misalignment. Conclusion: server-driven ability states do not move the hero; do not ship the mirror. `:413-424`, live
- Open question: why the live client drops its predicted Recall states within 0.25 s without any server frame (283-e1). `:424-425`, open
- **HUD gate var 0x2af6:**
  - 0x01cf presenters #155 (runtime 181, layer 05a7) and #235 are gated by NOT 0x2af6.
  - On 294 it is true in hero 0x01cf instance 11 and controller 0x20e4 instance 4.
  - Its only writer is client-only Stack #311 (state 109, node 155; 0x20e4 #399), under GameMessageEntry #5 plug m_452862E6 (0x0717 received). It is not synced.
  - A server pulse that exits state 109 (`--hero-pulse`, emulation 556/554 bits) exits node 155, but 0x2af6 stays true. `:426-435`, emulated + static
- Map tag markers in 294: 15 × [1,110], 18 × [1,111], 10 × [1,38]. The [1,38] set lies at y ≈ 6.5 on a loop (42,−0.5) … (46,−28) … (62,−37) … (95,−12) … (79,10.5), possibly a patrol track. `:436-439`, emulated (meaning claimed)
- Replicated component states (decoded via vtable 0x5d6b20):
  - comp 26 = u64 (filter bits, flags 2);
  - comp 69 = u64;
  - comp 90 = one field;
  - comp 136 = {s32, entity, entity, s32, s32} (hashes 373dbf32/2ec62fac/c7927090/a2141aef/e2894f84; all zero on the hero except the last, which is −1). It is the only candidate for a parent/attachment link and is added by the create-flag 0x80 builder.
  - Components 1/3/4/82 have no replicated state.

  Create records have only placeable and definition kinds. `:440-447`, emulated

### UI root 0x041A is the observer/tournament path (Claude) (`HANDOFF.md:449-516`)
- On 295, UI root switch node 46 and node 52 are false, and node 53 is true. Set the instance owner mode first: state entry copies def+0x74 into inner+0x4c0, and inner vt+0x240 (0x979280) switches on it. With mode 0 the selected entity resolves to nothing; with mode 1 it resolves to controller **0x80000304**. `:451-456`, emulated
- Node 46 = AECBBA41(selected) OR global 0x09ba OR (F94506DB AND B4F19440) OR CEFF290A OR E95A9C77; all five are false. `:457-458`, emulated
- AECBBA41 (0xcec6c0) reads comp44, then comp25 via 0xc1ecd0. It needs comp25+0x58 bit 28 set AND team == −1, where team is bits 23-27 (0..4, else −1; 0xb8af40). `:458-460`, static
- The local-session builder 0x829430 sets filter bits to 0x10040001 | (0x800000 << team). The controller's and hero's comp25+0x58 are 0 because filter bits are never sent. Setters: 0xb8af90 (comp26 state+0x78 or comp25+0x58) from 0xb21790/0x6c9f40, and 0xb8bdf0 (OR) from 0x7e37e0. `:460-463`, static
- **Node 52 = B36E804B = config byte image+0x2e39138** (evaluator 0x91af30). It is 0 at registration. Only the `tank_ShowMode` preset (0x1807e20) and the `tank_TournamentMode` preset (0x1807f10) set it to 1; config keys are checked in 0x17e3d70 via 0x1839600. Node 46's false branch (#25) is empty. So **0x041a → 0x120b HUD_Root is for show/tournament observers. Do not chase it for the player HUD.** `:464-469`, static
- The play HUD must come from the hero/controller presenters (0x01cf, 0x20e4) and the HUD_Root registered at startup (UI manager +0x58, created in 0xafb740 → 0x1dc9c10). `:469-471`, claimed
- Recall's gate 0xc1d860 reads comp25+0x58 bit 10. Hostility and teams depend on filter bits. Their network path (module 2 / 0xb22900 / 0xb21790) is not decoded. `:472-474`, static
- Widget census:
  - All UI widgets use context id 0x1800 (index 3, table image+0x2ebaaf8).
  - Top-level widgets share one array: HUD_Root 0502, hero select 008c, hero HUD 059d/05a7/05d0.
  - The HUD widgets in 294 have +0x90 = 0 (attached by deferred command 0x1c635d0 → 0x1c648a0, vt 0x27c9e50).
  - HUD_Root has no children, so the HUD widgets are top-level, not under HUD_Root. `:475-481`, emulated
- Widget visibility: +0x60/+0x64 is −1/0 when shown and 0x21/1 when hidden. The HUD widgets in 294 read −1, so they are nominally shown but draw nothing. The suspect is data binding to the local player/hero context. References to I+27f7fc0 are not a visibility marker. `:482-486`, emulated
- Presenter vs widget wiring:
  - `m_45017CEA` is the presenter's published widget-handle var, not its parent.
  - A type-7 handle is {state index, instance id}. Controller 0x20e4 instance 4 has 0x2ad4 = 0x900040004 (state 9) and 0x2e27 = 0xa00040004 (state 10). Hero globals hold 0x23000b0004 / 0x28000b0004 (0x01cf instance 11). Hero select's handle var is 0x07b2 (unset).
  - Startup layers by serial: 1 = 0273, 2 = GameLayer 029b, 3 = HUD_Root 0502, 4 = MenuLayer 029c, 5 = 0503, 6 = DialogLayer 0514, 8 = System_Clock 0513. All have show state −1.
  - Visible vs HUD presenter nodes differ at +0xf0 (1 vs 0), at +0x240 (1 vs 0x20/0x23), and at +0x250/+0x258 (one-entry array vs empty). `:487-497`, emulated
- **Muzzle origin ("Tracer shoots from the stomach"):**
  - Graph 0x16 CosmeticEntity states 0/1 (runtime nodes 1/2, vt 0x2630450) look up child pistols by identifiers 0x1b9b/0x1b9c on hardpoints 0x0a/0x0b. They publish them in synced vars s49 = 0x0031 and s50 = 0x0032. Volley #230 fires from hardpoint 0x06 of those entities.
  - On 294 the nodes hold −1 and the vars do not exist.
  - Tracer's definition 0400…0001 lists m_childEntityData: 0x0169 at hardpoint 0x11 and two 0x0017 pistols at 0x0b/0x0a. No child entities are created.
  - The create record has no parent field (0x78dab0 flags: 1 type/skin, 2 movement snapshot, 4 scale, 8 transform).
  - Candidate child list: the runtime STUEntityDefinition descriptor (image+0x3090590, size 0x50) has an array field at +0x38. `:498-511`, static + emulated
- Component mask getters: 0xbcacb0 = comp 25, 0x5d4520 = 26, 0xbd59a0 = 44, 0xbd5ef0 = 50, 0xbc9610 = 57. Module-2 sections (0xb40130, 0xb24600, 0xb22900) work on **comp 50**, which their controller and hero lack. 0xb21790 (comp 26, called from 0xae43f0) looks like local game-mode spawn logic. `:512-516`, static

### Codex runs 292-295 (`HANDOFF.md:518-644`)
- **Correction:** message 1291 and its parameters DO reach the hero (on 264, valid type-7 widget handles). The raw STU labels are byte-swapped: `272e` means var 0x2e27, `d42a` means 0x2ad4, `ed73` means 0x73ed. Earlier lookups used the wrong ids. **This retracts the earlier "1291 parameters unset" diagnosis (`:1040-1042`).** `:520-525`, emulated
- Listener 178 (0x1cf object 101) picks an owner branch:
  - while sender 15 (0x20e4 state 13) exists, Stack 180 sets owner 0x73ed to the selected controller, and the hero Stack 179 and presenters 181/62 are inactive by design;
  - when the sender goes away, Stack 179 selects the hero and its presenters run. `:526-530`, emulated
- Offline, a full controller frame that ends state 13 decodes 170/170 bits and calls the sender exit; the hero listener then switches branch and begins 181/62. `:531-535`, emulated
- Live 294 (`RetriggerExtraStates='4:-13'`): sender 13 ends, hero 179/181/62 become active, and real widgets exist, but **the HUD is still invisible**. Root 0x041a node 46 is false and no 0x120b child exists. `:536-542`, live
- Emulator pitfall: if state-type metadata is missing, a sender exit can decode as data and leave the sender running, even with equal total bit counts. Check state types and the actual exit/enter events, not just alignment. `:543-549`, static
- Live 295, authored Practice wrapper: controller 13c1 state 43 runs graph **288a**, created as instance 5 under 3:43, with runner data naming 5. Native replay is 218/218. Wrapper client nodes start graphs **2951 / 2931 / 2939 / 28fe**. The HUD is still absent, even after Escape menu open/close and Alt+Z. `:550-557`, live
- Client addresses: reference 294 UI comp47 0x15e681e7620, root041a instance 0x15e491dc550; reference 295 UI comp47 0x2758e017620, root041a instance 0x2764fc8cd70. The UI component is reached through root router vt+0x10. `:558-561`, emulated
- **Message 06dd / var 2206 controls distortion, not HUD visibility. Online client mode 6 is correct.** This retracts the HUD reading in the "Not working yet" item at `:1094-1100`. `:562-568`, claimed
- Run 293: the hero HUD parent is corrected to `PickSubgraph='11:0x01cf:6:3'`. Graph 0x4b object 68 has state index 3 and runtime node 7, and PSIn carries the state index. A replay creating child 12 under 6:3 consumes 519/519 header bits and 474/474 interpreter bits, and records parent 3 at instance+0x2c. `:572-580`, emulated
- Live 293: floor spawn, Blink to the doorway, firing, manual reload; no HUD. Audit: hero HUD parent 6 state 3; controller 20e4 parent 3 state 42; controller widgets 11/12/16 exist; hero listeners 97/178 active, and 178 finds sender 15; Stack 179 and presenters 181/62 inactive. **Fixing the parent alone does not fix the HUD.** `:581-590`, live + emulated
- Offline: three instance ticks produce no listener callbacks on 264, and both update lists are empty. Calling native begin does not activate the presenters. A node value f0 = 0 is transient, and registration increments the 1b1 generation counter. `:591-597`, emulated negative
- Run 292 (Recall mirror experiment):
  - An encoder guard makes an active StateStack with fewer than 2 data bits raise an error.
  - The native writer after an emulated E press gives the four Stacks 15/28/33/35 bits `1,0`, not `0,0`. Predicted states 36/37 were missing, and sender 37 needs its zero compact-u16 bit. With these corrections the replay is 643/643.
  - The Ability keeps its verified leading data fields. `:605-619`, emulated
- Live 292: two E presses were accepted (mirror frames 5/6 and 7/8) with no visible rewind. Automated mouse-click firing turns the camera to the ceiling; this is an input artifact. `:623-627`, live
- Dump 292-e2 (PID 21500, hero comp47 0x1f031996640, Recall instance 0x1ef2341edf0): stage 3 / node 4 runs, rewind 0 / node 1 does not, Ability node 24 flag +0x270 = 0, the deferred queue is empty, and nodes 12/13/23/27/59/69/72/73/74/60 are active. `:628-633`, emulated
- Graph state 3's onEnd connects to rewind state 0. State 3's subgraph is 9/10/11/23; state 0's subgraph includes 2/12/27/1/26/5/7/14/18/34. Rewind reader 0x8eeef0 reads a u16 and converts it into a duration-like field at node+0x970; its meaning is not verified. `:634-640`, static

### Load-in, spawn and Recall prediction (Claude, runs 281-291) (`HANDOFF.md:646-712`)
- The user saw a long wait at the default world camera, then a jump to the controller view, then hero select. Old timeline: diagnostic selection 30 s, controller selection 45, controller create 50, graphs 60, hero select 63. New timeline (best, run 291): **30 / 31 / 32 / 33 / 34 s**; hero select follows about 2.5 s after the controller view. `:648-656`, live. This was later adjusted to 31/32/34/36 with a 60-send wait (`:388`).
- Spawn: PickSpawn y = 2.05. The old y = 6.31 dropped Tracer about 4 m. Spawn snapshots start after the second hero create (12 snapshots) instead of after all 40. `-SpawnAt 1,48.71,2.05,37.77,180` puts the point in the hero's create transform. The settled comp1 +0x440 is (48.70, 1.96, 37.45). About 1 s of camera pitch/yaw sweep follows the pick; it is likely a camera transition, not confirmed. Run 290 was a known early instance failure (9 datagrams, exit 1). `:657-667`, live
- Emulated Recall E press:
  - Press handler 0xc847c0 queues a deferred record (state 16 = 1).
  - Gate 0xc85560 → 0xca8d20 qualifies the press (buffered record +0x280, window from def+0x148), then calls 0xc96d40 and 0xc99460. These set node+0x270 |= 1 and fire the Ability's output plug (0x1b73fa0).
  - The links queue deferred begin records on instance +0xc8 (0x1b92990 only if instance vt+0x138 says local). 0x1b921f0 processes them by time (dispatch 0x1b91690 by record type +0x22). `:669-675`, emulated
- Emulated result: states **3, 9, 10, 15, 17, 23, 28, 33, 35, 36, 37** turn on (11 and 31 briefly). Vars change: 0x009a = 1.0f (s26), 0x0021 = true (global), 0x02bb = true (s59). State 0 starts only when state 3 ends. `:676-679`, emulated
- Live (281-e, 283-e1): 0.25 s after E none of these states run, the queue is empty, and the Ability is stuck with node+0x270 = 3. Later presses take the ca87e0 path and do nothing. The server sent no hero frame on E, so the client rolls back its own prediction. This is the same mechanism as the old double blink. `:680-685`, live
- `--recall-mirror` sends one hero frame bump on E with the predicted states, data and vars, and one bump back. It adds a float value kind: kind 5 (u4 → float) or kind 8 (raw f32). `:688-691`, static
- Runs 284-288 crashed (exit 1): the four Stacks had no STATE_DATA, which misaligned the frame and hit the fatal path **0xf661e4**. **Every entered Stack needs `0/1,0/1` data.** `:692-696`, live + emulated
- Run 289 (data fixed): no crash and the Ability flag resets, but there is no rewind. Server-entered states are treated as server-driven. `:697-699`, live
- Body module headers from the client: it sends only module-3 commands and module-0 reliable data. **No upstream statescript is sent on E.** `:710-711`, emulated

### Native Blink (Claude, runs 265-279) (`HANDOFF.md:714-821`)
- Best recipe: run 271's bots without patrol, calibrated Tracer, mode 0x18, native Blink and Recall states, and no server teleports. `:716-718`, live
- **Blink is client-predicted with collision.** Two inputs were missing:
  - (1) Graph **0x24 state 0** (LogicalButton) must be active with **4 data bits** (`24:0=0:4`).
  - (2) The press flow (Wait state 20, then condition #27) needs var **0x000b > 0** (charges; Ability state 38 sync var, slot s11), var 0x2562 (slot s34), NOT global 0x003e, and config #318 (global 0x0021 priority). Fix: s11=i3, s34=i1. `:719-726`, live + static
- Run 274: correct dashes that stop at walls and land on the floor; the user confirmed. `:727-728`, live
- **Charges are tracked by the server** (run 277): −1 per Shift press edge, +1 every 3 s, max 3. The hero frame advances only when the count changes. Holding s11 fixed at 3 made prediction disagree with the server, causing rollback and replay (double blinks). Live: 3 blinks, the 4th only after a recharge. `:729-734`, live
- Input handling:
  - Pressed logical buttons are listed per entity at comp47+0xd0+0x440 (count +0x448). **Shift = 35, E = 36.** Dump 272 shows [35, 114] on both hero and controller; the E-held dump shows [36, 110].
  - LogicalButton node: vt 0x2637e20, update 0x1b9d280, press 0x1b9f160, links 0x1b73fa0 → 0x1b74220.
  - Local activation gate is instance vt+0xc0 = 0x99af30. It allows when +0x318 bit 10 is clear and bit 19 is set, refuses if target def+0x7b is set, and otherwise uses def+0x7a. `:735-743`, emulated + static
- The run-160 crash was missing LogicalButton data. STOP at **0xf661e4** is the generic missing/garbled state-data signature. `:744-745`, emulated
- **Server-driven Blink states do not move the hero** (273: states 25/11/2/3/6/7 entered, no motion). Data forms:
  - Anim 11 = `0:1,0:3,0:1`;
  - states 2/3/7 (types 3bc72de1/75d1c308) = `0:1`;
  - PhysicsExplosion 25 and BlinkEffect 6 have no data;
  - ChaseVar 4/5 are irregular. `:746-750`, live + emulated
- Run 276: server activation of states 17 and 3 on E does not rewind. `:752-754`, live
- The E press calls 0x1b714e0, which queues a state REQUEST (instance, state 16, value 1) on instance +0xc8 for the server (type-1 object via owner vt+0x98, queued by 0x1b92990) instead of activating locally. The server is likely expected to answer with Ability data: flag A=1, f32 +0x80, optional f32 +0x84, and a compact frame offset that becomes the activation time at +0x78 (reader 0xc84ac0). `:755-759`, static / claimed. Contradiction: `:711` says no upstream statescript is sent on E.
- The only caller of Ability vt+0x1d8 (0x8eb450) is 0x8eb424 in slot 0x1b8 (0x8eb3e0). It runs when node+0x2e0 > 0 after gate 0xc85560, which checks instance vt+0x140, +0x318 bit 19, 0xcb00f0, 0x1ba3490/0x1ba4060 and 0xc1d860. `:760-763`, static
- Run 278 swapped in "activated" Ability data `1a:16=0:8,0:1,1:1,0:32,0:1,0:1,0:8` (emulated 461/461; the presence bit must be 0). It was accepted but did not rewind, and the pistols may twitch. The old server Recall teleport threw the hero into the sky. `:764-769`, live
- Dump 279-e (PID 8308, hero comp47 0x20f3f676640): Ability node 24 is running and its press record (+0x290, button 36) has been consumed. States 17 (node 27), 3 (node 4) and 0 (node 1) are not running. All Recall/Blink state defs have +0x78..+0x7b = [1,0,0,0], so the activation gate is not the blocker. `:770-777`, emulated
- Readiness check 0xca8d20:
  - def+0x150 is empty, so it uses def+0x158 = 36 with a hold time from def+0x148, and qualifies via 0xc9e6f0.
  - It sets the start flag only if the press qualifies AND one of several conditions holds. These include entity check 0xc1cc30 and 0x1b95e10 checks on def+0x1c0/0x1c8/0x1d0/0x1d8 objects from 0x8f6b40.
  - All are 0, so it only marks node+0x270 |= 4 ("ready"). `:778-788`, emulated
- Gate 0xc85560: the cooldown is 0 (ready) and 0x1ba4060 = 0, but **0xc1d860 returns 0**. It uses mask comp 25, which is team/filter. So Recall likely depends on the hero's team/filter bits, the same problem as hostility and hit feedback. comp 25's state at comp+0x18 is not plain and its decoder fails. `:789-798`, emulated
- 0xc1cc30 is an anti-tamper return-address check, not gameplay. 0x1b95e10 is the ability priority-stack check (mgr +0x98 → 0x1ba3940, compares priority f32 +0x24). `:799-802`, static
- Ability state 16 (button 36) sits under Switch 22 (false branch) under Stack 29. Data: 29 = `0:1,0:1`, 22 = `0:1`, 16 = `0:8,0:1,0:1` (8+1+1 bits). Var 0x2562 = s34. Activation chain: Ability → state 17 → state 3 (54feb21c, no data) → state 0 (41b3fc2b, 16-bit reader 0x8eeef0; combined emulation over-reads 4 bits). `:803-810`, emulated
- Bot patrol is a sine-wave replication test without collision; the user saw bots clip through walls. Real bots need authored paths or AI. `:813-815`, live

### Mode 0x18 HUD runs 256-271 (Claude) (`HANDOFF.md:823-973`)
- **PSIn is a parent STATE index, not a runtime node index.** Server-creating 0x20e4 under (0x13c1, 101) kills the client within one packet (runs 256/258-261), because 101 is past 0x13c1's 49 states. Creating it top-level (262) or under (3, 42) (263/264) works. `:825-829`, live
- Setting 0x03ac when 0x13c1/0x20e4 are controller graphs is harmless. `:833-834`, live
- Working mode-0x18 recipe (264): GameMode 18; preload 13c1 + 20e4; controller subgraphs `3:0x13c1:0:0;4:0x20e4:3:42`; controller state 3:42; StateData `13c1:42=0:1,4:4,0:1`. Hero select, pick, pistols and firing work; no HUD. `:835-842`, live
- Graph gates:
  - 0x01cf #49 = STU_1FF67CA6 == self, where 1FF67CA6 is the local possessed body.
  - 0x20e4 #159 = selected == self.
  - So **0x01cf belongs on the hero and 0x20e4 on the controller.** `:844-850`, emulated
- Game-message store: a per-world system (store vt 0x2657ed8), 64 buckets × 0x1d0, with 0x70-byte entries keyed {entity, mode, message index}. Sender 0xd4c3e0 with an STU_1D719351 target writes entity = 0 (pushes {0, mode 2, 4753}). def byte +0x7a marks client-only, and client-only senders take only the mode 2/8 branch. `:851-856`, emulated
- Listeners subscribe under their own entity and under 0, and lookup 0xd9ad40 tries own key then key 0. Hero #101 (node 178) finds #187's record (+0x238 = 0x80000304) but takes the non-fire path. Firing needs node+0xf0 ≠ 0 and +0x1b1 == +0x1b2. +0xf0 is a per-frame transient (set by 0x1b72ad0, cleared by 0x1b750c8/0x1ba7230). `:857-866`, emulated. Codex later says f0 = 0 alone proves nothing (`:595-597`).
- Dump 264: hero comp47 0x2343c521b80, controller 0x2343c522da0, PID 18040. `:869-871`, emulated
- Run 265:
  - LogicalButton 0xb480a974 needs 4 bits; the frame aligns 387/387 and BlinkEffect state 6 enters 382/382.
  - Live, the client survives but Shift does nothing: no transition and no message to the server. This was later solved by vars s11/s34.
  - Byte image+0x2f7f840 has no writer (a dev-only check in 0x64d990).
  - Instance +0x318 bit 8 is not a prediction flag; 0x1b92c30 sets it after queuing a type-7 request.
  - Instance creation (0xa0adc0) has optional parts the responder does not encode: PIID/PSIn, then 'SPUI' plus a list of {entity (0xdb0f70), SrcI u16, SrcS u16}, meaning untested.
  - Tracer child entities do not exist in the world (only 0x80000300..308 plus map entities). `:873-896`, live + emulated
- Tracer calibration (268): FireHold = 40 (20 shots × 2), ReloadTime = 1.0. 40 rounds drain in 1 s and auto-reload works. The 0.686 s ammo-restore point is not modelled. `:902-905`, live
- **Comp 26 (filter bits) cannot be sent in a create:**
  - State type 0x30dabc0, one u64 at state+0x78 (comp+0x90), hash 82ce0230, flags 0x2. Comp 44's fields have flags 0.
  - Runs 266/267 crash at the first bot create carrying it.
  - All live entities have filter bits 0.
  - Mask functions 0x5d4520/0xbd5b90 are used by 0xb22900 from module-2 readers 0xb3e8ed/0xb41bac. Module 2, which they send empty, is the likely path. `:906-918`, live negative + static
- **Bots move** (269): module-3 movement items need an **increasing frame in movement field 2**. `--snapshot-frames` writes the reply server frame (emulator-verified with 0xc5f680: 174 bits). `:920-929`, live
- Muzzles: volley 0x16 state 8 #230 uses two {hardpoint 0dc0000000000006, entity handle} pairs from remote-sync vars 0x0031/0x0032 (s49/s50). On 264 they are unset, while ammo var 0x35 = 40 reads fine. `:931-938`, emulated
- Run 270: the server created two 0x17 pistols (def indices 5/6) and set s49/s50 to entity handles. The vars now hold 0x80000309/0x8000030a, but the entities sit unattached at the origin (identity transform, components 1, 3, 4, 22, 25, 26, 40, 97), and shots still start low. `:939-944`, live negative
- Components 3/4 have no state visitor (slot 0x18 = 0x1ce5640), module-4 op 0 is "none", and the create has no parent field. `:948-951`, static
- The Practice map has exactly **15 entities with [1, 110]** (0xc0000004..0xc000020c), the training-bot spawn markers. Floors are at y −4/−2/2/5/11. Run 271 spawns **13 bots** (definition 0x14d2, three skins); definitions are capped at 16, so 2 markers are unused. Bots stand across the range. `:954-965`, live + emulated
- Health packs: the 4 public pack coordinates have bare comp-1 markers (0xc0000042, 0xc0000045, 0xc0000050, 0xc0000129), each next to a class-0 [1,5] entity (probably the pad). The pickup definition is unknown: no graph holds the 75/250 heal constants together (0x0e95 is UI). `:967-973`, emulated / open

### Continuation through 255 (Codex) (`HANDOFF.md:975-1017`)
- Mode 0x18 references controller **0x13c1**: state 42 (runtime node 101) runs 0x20e4 and state 43 runs 0x288a. 13c1 has 64 sync slots / 49 states; 20e4 has 4 / 26. Do not copy the external fork's compressed state numbering. `:983-986`, static (asset decode)
- **0x0cb6 state 3 needs a compact-u16 payload after its active bit** (zero form = 1 extra bit). Without it the replay consumes 474/460 bits and StateHero never enters. Corrected: creation 492/492, activation 461/461, with StateHero enter. Reader 0xd4bac0 calls 0x185bb20. `:988-994`, emulated
- Live 254: controller 01cf stops at node 66 (selected hero ≠ own controller). Hero 01cf passes its gate and waits at node 178 for 1291. Controller 20e4 widgets are active. PID 49200; controller comp47 0x138b9952da0, hero 0x138b9951b80. `:996-1003`, live + emulated
- Ordering hypothesis, not shown: 20e4 might send 1291 before hero 01cf passes 0717 and opens its 1291 listener. `:1005-1009`, claimed
- Run 255: mode 18 fell back to Training because the preload guard allowed only mode 5a. The guard now allows exactly **18 and 5a** for the Practice map. `:1011-1013`, live

### Latest HUD status, runs 235-252 (`HANDOFF.md:1019-1091`)
- Run 250 failure: the close step advanced the HERO_SNAP seq before the first hero full snapshot. The first snapshot had base 3 and both subgraph create flags false, so it updated nonexistent instances 11/12 and decoding returned 0 after 3 bits. With the fix: creation 656/656 and activation 607/607, and instances 11 (0x01cf) and 12 (0x0c90) exist. `:1021-1032`, emulated
- Live 252: StateHero node 0 runs, and 0x0c90 spawns client graphs **0x1a36** and **0x20e4** (instance 65532). No comp47 reset. PID 1400, hero comp47 0x22bbeda1b80. `:1033-1036`, live
- 0x20e4 switch #159 (node 19, condition at def+0xe8) compares the selected controller **0x80000304** with own hero **0x80000305** and evaluates false. Its presenters #166/#178/#183 (nodes 16/11/12) are inactive. Parameters 272e/d42a were reported unset. **RETRACTED by `:520`: the ids were byte-swapped.** `:1037-1043`, emulated
- The idea that 0x0c90 must run on the hero is contradicted by this equality gate. `:1044-1046`, emulated
- A responder bug invalidated runs 240-246: the `_part` variable shadowed the encoder function. `:1054-1057`, static
- Hero HUD chain:
  - Tracer graph 0x4b state 3 (runner #68) runs 0x01cf on the hero.
  - 0x01cf order: #5 waits 0x0717 → #41/#49 switches → #101 waits 0x1291 (params 08fb/08fa) → stack #149 → presenters #155/#159. Stack #84 sets 0x0406.
  - Runtime node index = position in m_nodes: #5 = 97, #41 = 1, #49 = 66, #101 = 178, #149 = 179, #155 = 181, #159 = 62, #84 = 126.
  - 0x1291 is sent only by 0x20e4 #187 (client-only STU_38EE1100). Its target #193 is STU_1D719351, with no broadcast flag. `:1058-1069`, static
- Run 251: the server sent 21616 with game message 0x0240000000001291 and nothing happened. **The client has no handler for the 21600 family**: it is client→server only. Server→client channels are **20300 / 25000 / 27500 / 25800 / 42900**. Message-section mode 1 calls node vt+0x38, which for GameMessageEntry is 0x1ce5640 = `ret 0`. `:1070-1077`, live + static. This contradicts run 328/329's "delivered without effect" (`:194`); 251 shows there is no handler at all.
- Run 250: frames emulated on dump 239 align (432/441/490/499 bits), but live the pistols vanished. This was the scheduling bug above. Run 249 (0x0c90 late on the controller) reached #187, but 1291 did not reach the hero. `:1078-1087`, live
- Still open at that time: Blink (the dead-reckoned server teleport drifted into geometry; later solved natively), projectile origin, hero-select 3D preview, Hero Gallery cosmetics. `:1088-1090`, claimed

### Not working yet (open frontier, cut at line 1100)
- The UI manager sets HUD_Root property **6720** from UI-entity var 0x2206 (0xafd420) and property **6729** from var 0x34f5 (0xafd4a0, default true). `:1094-1096`, static
- Only UI root graph **0x041c** writes 0x2206: StateStack state 9 sets it true under GameMessageEntry state 8, which waits for message 0x02400000000006dd. In the dump that entry is running and waiting (instance 65478, node 19). `:1097-1100`, emulated. **Codex says this var controls distortion, not HUD visibility (`:565-566`).**

### Contradictions to watch
1. Server-entered rewind state 0: the 299-304 notes say its enter handler takes the edx==2 early return, but emulation shows edx=1 (`:373` vs `:266`). The real blocker is that the recorder has no samples (`:282`).
2. Whether E sends a request to the server: 0x1b714e0 "queues a state REQUEST for the server" (`:755`), but the header scan saw no upstream statescript on E (`:711`).
3. 21616 game message: "delivered without effect" (`:194`) vs "the client has no handler for the 21600 family" (`:1073`).
4. HUD parameters 272e/d42a: "unset" (`:1040`) vs "byte-swapped labels, present" (`:520`).
5. Hero HUD parent: `6:7` was replaced by `6:3` (`:830`, `:572`). The 0x041a/0x120b HUD_Root and the 06dd/2206 chain were both dropped as player-HUD paths (`:464-469`, `:565`).

## `HANDOFF.md`, lines 1090–2169 (2026-10-15 to 10-17)

### 1. Open-frontier HUD hunt (runs 219–234), lines 1092–1237

**HUD_Root properties and message 0x06dd**
- The UI manager sets HUD_Root property 6720 from UI-entity var 0x2206 (code 0xafd420). It sets property 6729 from var 0x34f5 (0xafd4a0, default true). `:1095` static.
- Only UI root graph 0x041c writes 0x2206: StateStack state 9 sets it true, under GameMessageEntry state 8, which waits for message 0x02400000000006dd. On the dump that entry is running and waiting (instance 65478, node 19). `:1097` emu.
- 0x06dd appears in no code and in none of 466,358 records, so the claim was that the server supplies it. `:1101` static.
  - **RETRACTED at `:1205`:** 0x06dd drives the `ShowDistortion` effect, not the HUD gate.
- Sending 0x06dd as server message 21616 to the controller or the hero does nothing (run 220). `:1106` live.
  - Elsewhere in the file the client has no handler for the 21600 family; see line 1176.

**Relay graphs that failed**
- Candidate relay graph 0x1289, state 9 (STU_38EE1100 client send state, remote sync), sends the message held in sync var 0x3803 (slot 1) to the local UI target STU_1D719351. `:1104` static.
- Creating 0x1289 on the controller breaks the whole controller frame. In emulation the interpreter returns 0 after 3 bits. `:1108` live+emu.
  - The graph is not resident, even with `-PreloadGraph`. Runs 221/222 lost hero select.
- 0x1289 is map-specific. Its only reference is per-map package 0x0dd0000b0000079f; 0x0dd0 records are per-map packages indexed by map id. Practice map 0x688 has no 000b package. `:1113` static.
- Map 0x79f is Rialto (the Archives PvE event), so 0x1289 is event-mission UI. `:1143` static.

**Senders whose message comes from a variable**
- Of 36 send nodes with a variable message, only two graphs are resident:
  - 0x05ad, a server-side gameplay flow.
  - 0x2fca, a client STU_38EE1100 sender. Its message is an array lookup `var c125[var 967c]`, probably the comm or emote wheel, with literal entries 0x17ab, 0x17ac and 0x15d4.
  - `:1116` static.
- Emulator control: resident graph 0x0cb6 created in the same controller frame parses 114/114. 0x1289 fails only because it is not loaded. `:1120` emu.
- 0x2fca (#16, message var 0x25c1[var 0x7c95]) creates on the controller (emulator 115/115). `:1150` live+emu.
  - Setting its instance vars or sync slots s0/s1 crashes the native value reader (undeclared vars or wrong slots).
  - Run 223 exits 0x1 at the pick.

**Scans that found nothing for 0x06dd**
- Searched:
  - 466k raw records, including the Practice map packages.
  - 4,885 decoded mode, ruleset, map, definition and UI records.
  - All 1,586 graphs; the only hit is the receiver 0x041c.
  - Code immediates, and .rdata as 64-bit and 32-bit values.
  - `:1128` static.
- No pair of 64-bit code immediates XORs, adds or subtracts to 0x06dd (77,721 movabs scanned). `:1144` static.
- A full demangled scan of 467,972 records (every field hash, raw, and with bits 32–47 wildcarded) finds only receiver 0x041c. Live dump 222 holds the value once, in a heap listener, never in image .data. `:1171` static+dump.

**Native "send to UI" path and UI entity**
- Native send-to-UI API: build message (0x1ba7970), set GUID (0x1ba7e20), deliver via root vtable +0x110, then +0x18. `:1132` static.
  - All 50 call sites use fixed GUIDs, script-node vtables (0x2622280.., 0x2792fb0..) or unrelated data (43301 = clock sync).
- There is exactly one UI manager (vtable 0x26552a8) and the root owns it. It creates the UI entity in slot 15 (0xb0aee0) with its own owner, so no world-owned UI context exists. `:1136` static.
- The UI entity is in no world entity table, so message 21616 cannot address it. `:1139` static.
- UI flag updater 0xafd550 on dump 222: 0x08e3=1, 0x0a0d=1, 0x0522=0, 0x1df3=0. Only front-end graph 0x0151 uses the last two. `:1140` emu.
- Channel-0 handlers 20303..20322 send no UI messages directly. `:1146` static.
- Client UI listeners are global: the root message router dispatches by GUID, so any client-side send of 0x06dd would reach 0x041c. `:1148` static.

**Responder options added (private)**
- `--pick-controller-vars 'inst:key=g<hex>|i<n>|L<hex,..>|b0/1'`. GUID lists are kind 15/6 arrays. `:1153`.
- `--pick-controller-guid-vars`. `:1153`.
- GRAPH_COUNTS now includes 0x2fca and 0x1289. `:1153`.

**Game-server reliable channels (client receive side)**, `:1156` static

| Channel | Family / schema | Dispatcher | Contents |
|---|---|---|---|
| 2 | 25000 / 8f89e0df | 0x7d1dc0 | 19 handlers (0x6c2xxx..0x6c4xxx). 25003/4/5/7 raise UI messages, all with fixed GUIDs 0x8cb, 0xf72, 0xf76, 0xf7b, 0xf7c |
| 3 | 27500 / 82a2f991 | 0x7d2400 | Handlers 0xb12cd0 and 0xb12d60. Records hold three entity ids and u64s (27501 adds floats and a bool). 0xb11070 appends them to world+0x138. These are combat events, intended later for damage numbers and the kill feed |
| 4 | 25800 / 9f0a9f88 | 0x7d22a0 | 0xaeb2e0..0xaebd80: stats ("Stat: %s", "Shots Fired", time played) |
| 6 | 42900 | 0x7d1c70 | 7 table entries, no direct calls |

- Families 30700 (78 messages), 22300, 25700, 42800 and 42700: only encoder/decoder switches were found, in the 0x4b..0x4f range. `:1163` static.
- 30700's id switch is 0x4c1b00 (77 cases, table 0x4c231c). Each case calls a per-message decoder (0x4bb240..0x4bd890). The matching encode switch is 0x4be180. `:1166` static.
- Server-to-client handled families are only 20300, 25000, 27500, 25800 and 42900 (registrar 0x7d5100). `:1176` static.
  - 21600, 30700, 42800, 22300, 25700 and 42700 register codecs with no client handler, so they are client-to-server only.
  - Field layouts: `.local/runs/instance-families-224.json`.

**Other relays and the UI root**
- Generic client relay graph 0x1d9e (message var 0x3b4e, send var 0x3023; both remote-sync) exists only in other maps' packages: definition 04000000000030b8, packages 0dd0000b00000afe/0b56. `:1180` live+static.
  - Preloading that definition does nothing (run 224: typed cache absent).
  - Creating it fails the controller frame and hero select is lost.
- UI root 0x041a created on the controller (GRAPH_COUNTS (16,33)), run 225: hero select and spawn still work, but none of its nodes run because its entries are not client-only. `:1187` live.
  - Activating state 25 (the 0x120b runner, state data `0:1,3:4,0:1`) makes the runner node run but spawns no child (run 226).
  - Creating 0x120b as a child (3:0x120b:2:38) plus state 15 (HUD_Root STU_501733D7) makes the client exit 1 (run 227).
  - Conclusion: do not relocate UI graphs.
- UI manager slot 6 (0xafb740) creates HUD_Root (layer 0502, 0a88..01c3) at startup, so HUD_Root always exists. `:1194` static.
- New private responder option `--controller-guid-vars` (driver `-ControllerGuidVars`): GUID instance vars sent in the first controller frame. `:1196`.

**Frame alignment fix (session 228–234)**
- 0x288b state 35 is a StateStack. The native StDt reader (0x1b96460) reads 2 state-data bits after the entry: +0x238 is-top and +0x239 is-under. `:1198` emu+live.
  - The recipe sent none, so every controller frame since hero select was 2 bits short. This was harmless only while all following bits were zero.
  - It explains the run-227 crash.
  - Fix: `StateData='288b:35=0:1,0:1'`; emulation then aligns 116/116.
- HUD_Root binding names in layer 0502: `ShowDistortion` (prop 6720 = var 0x2206, message 0x06dd) and `ShowHeroHud` (prop 6729 = var 0x34f5, default true). `:1205` static.
- HUD_Root is displayed only by node type STU_501733D7. `:1208` static.
  - Mode switch 0x9c2380: mode 4 calls 0x9cf9b0, which shows HUD_Root.
  - The node waits in mode 0 until its load input plug STU_2561471D fires (handler vt+0x198 0x9c2490 → 0x9cad70, job +0x1c0).
  - Only two graphs contain this node:
    - 0x120b, the UI root 0x041a branch. It is gated on client flags and a selected-entity term that are false for the root-owned UI entity.
    - 0x0bcf.
- 0x0bcf is the highlight/replay viewer graph (resident; 128 sync vars, 86 states). `:1214` live.
  - Creating it on the controller and activating StateStack 1 with is-top (`bcf:1=1:1,0:1`) shows HUD_Root: the orange network-warning icon appears top left (run 231).
  - State 3 is its HUD_Root node (enters, stays in mode 0, no load).
  - States 22/25 start "LOADING HIGHLIGHT" (run 234).
  - This is not the live-play HUD path.
- The StDt mask covers every state in `m_states` (graph data +0x60). Entries of a newly created instance are not read in its creation frame. `:1220` emu.
- New public probe option `--lobby-preload-stack-item` (raw 0x0d60; these are IDs and no records exist). `:1222`.
- New private option `--pick-controller-messages 'inst:node:width'`: one-shot mode-0 entry triggers. `:1222`.

**Open bugs and remaining ideas**
- User-reported bugs, run 229 `:1226` live:
  - Blink can launch the hero into the air (server teleport to an estimated position).
  - Tracer's tracers start at her stomach (no weapon child entity or muzzle hardpoint).
- Remaining hypotheses `:1230` claimed:
  - The server delivers UI messages through an undecoded channel, for example the statescript frame's remote-message section RMFr/TrID (code 0xa0cca0).
  - Or a native path uses an obfuscated GUID constant.
- Older 0x041a state-20 terms `:1234` static:
  - AECBBA41: the UI entity is root-owned.
  - 0x09ba: 0x0bce front-end/menu paths.
  - Special-client flags, a replay string, and a constant false.

### 2. Hero changes, weapons and hero-select preview, lines 1238–1260

- McCree's vanishing revolver was a state-encoding bug (Codex, runs 204–213). `:1239` live+emu.
  - There are 483 one-bit state types.
  - BooleanSwitch is encoded as active + branch.
  - Inactive two-bit entries keep their data.
- Re-pick weapon regression (runs 215–218) `:1243` live+emu:
  - The replacement hero's statescript manager is at frame 2 after its graph-create frame (base 1).
  - The client queues any full frame whose base is above current+1.
  - HERO_SNAP seq was not reset, so every base was ≥4 and frames queued forever (dump 215: 13 pending records, bases 4..18).
  - Fix: reset the sequence on re-pick.
- The H key (hero select) works only while var 0x6321 is false. It is 0x288b switch #160 state 96, with the H LogicalButton in the false branch. `:1247` live+static.
  - The open handler sets 0x6321, so the close step must clear it. Otherwise the second H does nothing.
- StateStack needs two state-data bits; totals alone can hide missing bits. `:1250` emu.
- Hero select shows no 3D model, and the gallery shows 0/0 cosmetics. The backdrop (the map's fixed placed view) is probably correct. `:1251` live.
  - None of these helped: lobby 24900 unlocks with real default unlocks, hero-progression preload (run 203), preview-entity preload (run 195), UI context var 0x07b2 (run 196 hid the header).
- Open items: other heroes' abilities, bots placed further into the range, damage numbers. `:1260`.

### 3. Run recipe, lines 1262–1297

- Responder `.local/re/udp_welcome_reply.py`; driver `.local/re/run-live-preload.ps1`. The window is 2560×1080. `:1264` claimed.
  - Roster row is at y=928: Tracer x 1700, Soldier 1485, McCree 1000.
  - CONTINUE is at (1280, 1011).
- Best-recipe parameters (the long hashtable) `:1273` static:
  - Definitions: DefinitionGuid `0e, 03cf, 14d2×3`; Preload `0017, 0018, 04bf, 130d`.
  - Selection and timing: SelectedEntityValue 3221225473 (0xC0000001), delay 30; CreateFlags `0:0x30, 1:0x80`; Statescript `0, 0x288b` with delay 60; CreateSends 150; LateDefinition `0,50`.
  - Controller: ControllerComp74 `2=0,6=1`; ControllerStates `1:35` at 63; ControllerVars `g:7acb=1,0986=1`.
  - Hero: HeroHealth `1:200`; HeroOnPick 1; CloseHeroSelect `1,36`.
  - Pick vars: `g:001f=i1;10:s29=i1,s53=i40,s6=i40,s5=i1`.
  - PickStates: `9:39,43;10:0,1,30,31`.
  - StateData: `288b:35=0:1,0:1; 16:8=0:1,0:1,0:8,0:1,0:1,0:6; 16:9=1:1,0:1,10:4,1:6,0:3,0:1; 16:18=0:1,0:1`.
  - Weapon: FireVolley `10:8`; ReloadStates `10:9,18,26`; ReloadTime 1.4; Ammo `10:53:40:1`; FireHold 36.
  - Skins: `0a50..1afd/1aff/1b00`.
  - Bots at (53|56|59, 2.0, 22.8) with 200 HP.
  - Blink 7.5, Recall 3.0, RunFor 300.
- Intermittent "early instance failure": the client exits 0x1 after about 5–11 datagrams. Retry. `:1286` live.
- The diagnostic bound was raised to 300 s; the driver now forwards RunFor (it was hardcoded to 180). 25 CTests pass. `:1293`.
- Runs 206/207/208/209/210/212 do not count as weapon tests. `:1297`.

### 4. Analysis tools, lines 1299–1329

- Codex tool `stdt-native204.py` runs the native one-bit predicate; `ssfull204.py` tests two full snapshots including inactive WeaponVolley data. `:1301`.
- Dump 200 baseline: PID 30652, hero comp47 0x181436c1b80, base 8 / cmfd 4005. `:1306` emu.
  - Runtime node index ≠ STU state index: states 0,1,2,4,11,12,25,27,34 map to nodes 1,2,4,8,30,31,48,50,61.
- The native global-var reader updates only the listed UVGu entries and does not clear other globals. `:1309` emu.
- Dump scripts: `ssnodes.py`, `ssvars.py`, `nodedefs.py`, `uicheck.py`/`uicond3.py`/`cvreads.py`, `ssfull.py`/`ssfull2.py`. `:1313`.
  - Find the hero comp47 with `inst_list.py <dump> <pid> 80000305`; the controller is 80000304.
- Image base for absolute pointers: 0x7ff6a1790000. `:1322`.
- Raw GUID bits 48–59 are the 12-bit reversal of (type−1). `:1327` static.
  - Examples: 0x0400 = 3 (entity definition), 0x0580 = 0x1B (graph), 0x02e0 = 0x75 (hero), 0x0250 = 0xA5 (unlock), 0x0a50 = 0xA6 (skin theme).

### 5. HUD transparency and teams (runs 332–342), lines 1331–1449

**Why the HUD widgets were invisible**
- HUD widgets exist and are visible, but fully transparent. `:1333` static+emu.
  - Visibility is property key 0xffffffffffffffff (property map at widget+0x40) = 1 for HUD_Hero_Common 059d and HUD_Tracer 00f3.
  - Their STUConfigVarColor properties 2eab/2eac/2ead/4498/4499 are RGBA ffffff00 (alpha 0).
- Colour chain on hero graph 0x01cf (client-only stacks):
  1. #76 var 0x4769 = team(self), read from comp25+0x58 bits 23..27 (getter 0xb8af40). It was −1.
  2. #80 global 0x45c2 = session team-colour GUID T+0x2c8[team] (0x641b70). It was null.
  3. #84 global 0x0406 = colour-record lookup (0x923170 against 0x0460000000000105). No match gives 0x00ffffff.
  - Patching 0x45c2 to 0x0b3000000000001a in emulation gives opaque colours. `:1337` emu.
- Two inputs are missing:
  - Team filter bits on the hero and controller: 0x10040001 | (0x800000<<team), as built locally by 0x829430.
  - The session team-colour table (context slot 0x4e, getter 0x641d50, vft 0x2611ad8), which is empty.
  - `:1341` static.
- Online layer 1 of that table is filled by queue component (vft 0x2610c68) methods 3/4, 0x5fa9a0/0x5f9bb0, from lists of 0x48-byte {GUID +0x10, team +0x18}. 0xb87060(team, guid) is another writer (callers 0xa2cc48/0xa2cc87). `:1344` static.
- Layout corrections `:1347` static:
  - Presenter +0x240 = registration index; +0x248/+0x250 = child list; +0xf0 = transient flag.
  - Widget +0x60/+0x64 = property-map free list.

**Component updates and team bits**
- New option `--component-update 'IDX:EL:TYPE:U64[:SENDS]'`: a module-4 op-2 update of one u64 field. `:1351`.
- Run 333b: comp26=0x10840001 sent as a raw op-2 update to the controller (EL 80) and hero (EL 95). The client survives, but comp26+0x90 and comp25+0x58 stay 0 (team −1). The update is silently ignored. `:1369` live+dump.
- comp26's u64 field has flag 0x2, so reader 0x3f0c20 expects one presence-mask byte followed only by the non-zero little-endian bytes. Team 0 encodes as `0d 01 84 10`. `:1389` emu.
  - Raw 8-byte values misdecode: runs 266/267 crashed and 333b was a no-op.
- Team-colour GUIDs come from lobby message 23310 (+0x138 list of 0x48-byte {GUID +0x10, team u8 +0x18}, teams 0/1). `:1393` emu.
  - Path: 0x6ce7f0 → queue slot 4 (0x5f9bb0) → layer 1.
  - Layer 1 is used only when world job byte +0x6e8 is set. That byte is written from welcome +0x518.
  - Driver: `-QueueFirst 23310 -TeamColors 0b300000000000f6,0b300000000000f7 -W518`.
- Run 336 (PID 31988, dump kept as HUD reference) `:1400` live+dump:
  - 23310 accepted (165 B) before the referral; welcome +0x518 = 1.
  - comp26 = comp25+0x58 = 0x10840001 (team 0) on both controller and hero.
  - Widget colours now opaque: 2eab/2eac/4499 = 27aae1ff (friendly blue), 2ead = ffffffff, 4498 = 000000ff.
  - Still no HUD on screen.
- Run 337: comp26 in **creates** works with the flagged encoding (`-FilterBits '0:0x10840001;1:0x10840001;2..14:0x11040001'`). Controller and hero are team 0; bots 0x80000306/07/10 are team 1. `:1410` live.
  - **RETRACTS** line 906 ("comp26 cannot be sent in a create"); that was an encoding bug.

**Teleport launch**
- The frame domain is not the cause. `:1355` live+static.
  - `--snapshot-frame-source cmd|cmdlast` puts the newest command (+1) into movement field 2.
  - Run 332: x/z snap, then y rises 3.5 → 13.3 within 0.3 s, the same as with server frames.
- Snapshot apply 0x7ca130 `:1358` static:
  - The template frame must match history ([rax+0x58]).
  - The new frame must be above the newest ring seq (0x7947e0 +8), or it is skipped.
  - Time = frame×16 ms (0xbb7100), or the server tick when frame is 0.
  - Velocity = dpos/dt only when the gen field (+0xc) is unchanged; otherwise it uses the zero constant 0x3241710.
  - Create flag bit 1 (0x78dab0 → 0xc5f680, prior 0x2f82d30) carries an initial movement state at create descriptor +0x1b0. Untested.
- The launch is placement about 10 m above the target, then settling onto the first surface below (runs 334/335c). `:1376` live.
  - Targets: 3.0 → 13.2 (roof), 15.3 → 30.6, 31.0 → 30.7.
  - The first spawn also appears at y 12.0 and falls to 1.75 in 1.3 s.
  - **Explanation superseded by `:1417`** (see below).
- Ring append 0xc5b640 `:1381` static:
  - For local-predicted entities (+0xec68), frames ≤ the newest predicted go to reconcile 0xc5c3e0. 0xc5c330 finds the newest stored state with seq ≤ frame and 0x64a530 overwrites it, then interpolation ring +0xe040 is updated.
  - Newer frames append.
  - Older frames return 2; the same frame with a lower gen returns 3.
  - The 10.0 vector in 0xc5a140 is unrelated grid code.
- **Solved, run 340:** the spawn-room barrier ejects heroes that have no team. `:1417` live.
  - Run 339 (targets 10 m below, no team): snaps, then depenetrates up to y ≈ 1.0.
  - Run 340 (with team bits): all three teleports land exactly (3.02, 3.96, 2.43) and fall about 1 m to the floor.
  - Generation teleports are now usable for Recall, respawn and Blink.
- `hero_pos_feed.py` needs SYNCHRONIZE in OpenProcess, or it never exits. It writes `.hist.jsonl` at 10 Hz. `:1366`.

**Recall**
- Recall works (run 341), server-side `:1426` live:
  - Settings: `Recall=3.0 RecallCooldown=3.0 RecallRewind=0.9 RecallStepHz=0 TeleportGen TeleportSends=4 TeleportDy=0.3` + RecallMirror.
  - E at 42.74 s; at 43.71 the hero is at its 39.78-s feed position (3 s earlier).
  - The native screen effect is not visible.

**Hide-HUD message 0x0717 (sent by the responder itself)**
- UI hierarchy: GameLayer 029b hosts HUD_Root 0502, which hosts 05a7/05d0/00f3/0571; 05a7 hosts 059d. `:1434` static+emu.
- UI manager 0xafb990 shows HUD_Root from 0xa7f5e0 = UI global var 0x0734.
- Graph 0x0cc9 node 6 (client-only, on message 0x0240000000000717) stacks 0x0734=false at priority 20 over 028c's true.
- 0x20e4 #399 and hero 0x01cf set 0x2af6 on 0x0717, which hides 05a7/05d0.
- 0x0717 came from the recipe's `RetriggerControllerState='2:3:4'`: 0x0cb6 state 3 is a client-only send of 0x0717.
- Run 342 (without the retrigger; dump kept) `:1443` live+dump:
  - 0x0734 = 1, the gate = 1, and every widget is visible with opaque colours.
  - The cyan chevron returns.
  - Still no HUD content.

### 6. Bots, combat and HUD content (runs 343–375), lines 1450–1740

**Bot hits and death**
- Run 343: a bot at (48.71, 2.0, 29.0), team 1. 8 bursts give server hits 53/106/159, 3 kills, 2 respawns. `:1450` live.
  - Tracers and impact sparks show on the bot.
  - Destroying the entity makes it vanish instantly, with no death animation.
  - No enemy health bar or outline.
- Run 344 `--bot-death-hold 2.5` (replicate 0 HP, keep the entity 2.5 s): the bot stands at 0 HP. `:1457` live.
  - Bot definition 04000000000014d2 has STUHealthComponent m_73AEA60D [STU_9EA55F3D {05c0..0015, 0da0..000d}].
  - Its graphs: 0x33 (StatescriptStateHealthPool, ModifyHealth), 0xbe2, 0xa5b, 0x2206.
- Run 353 `--bot-death-fields` (pool 0 = 0, fields 48–52 = u32 0, u32 0, killer id, u8 1, u8 1): still no death presentation. `:1526` live+static.
  - Ragdoll chain 0x1b18330 ← 0x1ae7790 ← 0x1b0e280 ← scene/model code, with no network trigger found.

**Out-of-bounds respawn**
- Run 345: `--respawn-below Y --respawn-delay S` generation-teleports the hero to the pick spawn. `:1463` live.
  - Out of bounds at 35.7 s, back at spawn at 37.9 s.
  - A 2-s grace period was added after a stale-sample retrigger.
  - The recipe uses Y = −15. This approximates the real kill volume; the kill-volume rule is not implemented.
- Clients that hit the early instance failure send the same 21627/21600/21610 as good runs, then exit 1. `:1470` live.

**HUD content behind server-driven switches**
- The cyan chevrons about 80 px under the crosshair are HUD_Tracer's BlinkCharges control (1950 px vs 0 in 0x0717 runs). `:1472` live+emu.
- The remaining HUD content sits under server-driven BooleanSwitch false branches:
  - Ammo: inst10/0x16, switch 2 (true) + 29 (false) → presenter 31 (slot 68d4).
  - Blink: inst1/0x1a, switch 21 → 32 (5fdb).
  - Recall: inst2/0x24, switch 21 → 32.
  - Ultimate: inst3/0x34, switch 10 → 13 (6869).
  - Health card: inst11/0x01cf, 15 → 66 → 139, then client switch 79 (slot 15 = var 0x070f > 0) → 37 (2d11).
  - Crosshair: inst9/0x15, 11 → 23 → 42, then client 28/26 → 38 (reticle 02bc).
- Recipe bug: apply_hero_config replaced pick_states; the parser now merges them. `:1481`.
- Key to command bit (run 346): fire 0x1, F 0x4, Shift 0x8, E 0x10, Q 0x20, Space 0x40, Ctrl 0x80, R 0x400, V 0x800; C has no bit. `:1487` live.
- Tracer ultimate = graph 0x34 node #38 STUStatescriptStateAbility, logical button 0x25, condition (0x0156 op35 0x0152) && 0x1db5. `:1488` static.
  - Graph 0x4b copies 0x0152; 0x34 resets 0x0156 (a float).
  - Logical buttons: 0x23 Shift (graph 0x24 Blink), 0x24 E (0x1a Recall), 0x25 Q, 0x1f (graph 0x43, probably melee).
- Run 348: the original hero HUD is live: health card, ult meter with Q, Blink ×3 and Recall icons, ammo 40/40. `:1494` live.
  - Firing 40 → 21; Shift spends a charge 3 → 2; R restores 40.
  - Health reads 0/200 and there is no crosshair.
- Run 350: forcing Ability state 9 on Q makes the client exit 1 immediately. Do not force Ability states. `:1502` live.
  - **Cause given later at `:1644`:** the 10-bit Ability state data was missing.

**Combat records**
- Module 2 = projectile replication `:1506` emu+static:
  - Section A: volley/projectile-spawn records (emulator bit-exact 169/172).
  - Section B: projectile physics.
  - Keep `1,0,0`.
- Channel 2, family 25000 `:1509` static:
  - **25001 hit confirm** {target s32, amount s32, flags u8, frame offset s32} → controller comp59 hit ring +0x12d0, counters, world notification 7.
  - **25000 combat log:** a 0x450-byte event on the hero health component list +0x4c0; arrays +0xa8 (≥3 items) and +0xd8 (≥2).
  - No native death record exists; health 0 alone never starts death.
- Health component bug: the mask must be [1] (pool 0, 20 bytes). The old 9-bit mask over-read 9 pools (320/248 bits). `:1515` emu.
- Run 351: the card still reads 0/200, so the card's value is not comp51 pool 0. `:1517` live.
- Run 352: `--hit-confirm-every 5` sends `<iiBi` target, amount, 0, 0. 31 were sent; the comp59 ring filled (cap 32, idx 25, count 26). No visible hit marker. `:1520` live+dump.

**Health number and ultimate meter**
- Run 355: CurrentHP is an expression over hero globals 0x01de, 0x8398 (must be false), 0x06c9 (current) and 0x06c4 (max). `:1531` live+static.
  - Ult meter = 0x0156 / 0x0152. Unset cost shows "ready at 0%".
  - `g:06c4=f200,06c9=f200,0152=f1000,0156=f350` gives 200/200 and 35%.
- Crosshair is gated on 0x0ebc, which only StateStack 23 writes with is-top=1. Rule: StateStacks whose value matters need is-top 1. `:1535` static.
  - **Superseded at `:1594`:** is-top alone does not set it.
- Run 356: `--ult-charge 'COST:START:PER_HIT'` with 1000:0:4 makes the meter climb 0 → 63% from hits. The values approximate the real charge rules. `:1539` live.
- Run 357 (meter full, Q): the client predicts the ultimate. `:1557` live.
  - Pistols holster and the HUD is replaced by a red prohibited marker.
  - Setting cost 0x0152 was the missing condition (run 349 had it unset).
  - The server never confirms it.
- `--extra-for-def GUID` limits Tracer-specific extras to Tracer (0400000000000001). `:1564`.

**Two clients: mutex question**
- Static design: the client has no single-instance check (583 IAT slots traced). `:1546` static.
  - The suspected blocker was the TACT/CASC shared-memory mutex `Global\<storage path>` (name at offset 8 of each `shmem` file; 0x3347f0 CreateMutexW ×2, then OpenMutexW).
  - **RETRACTED by E0 `:1579`.**
- E0 self-test: inside an AppContainer, `Global\` maps to the package's own AppContainerNamedObjects. `:1579` live (launcher test).
  - Different profiles get separate mutexes (no error 5). A descriptor create fails with 1314; a default create succeeds.
  - Processes in the same profile share one mutex; host-precreated mutexes are invisible.
  - So the mutex does not explain the second-client exit.
  - Fix: put both clients in one shared profile.

**Muzzle origin**
- The client creates all m_childEntityData children locally (0x17d3480 → 0x1830a50) when their definitions are preloaded (0400000000000017). The server must not create them, and comp136 is unrelated. `:1569` emu.
- Volley muzzles read hardpoint 0dc0000000000006 on the entity named by vars 0x0031/0x0032. `:1572` emu.
  - The vars hold CosmeticEntity node handles {u16 1, u16 inst, u16 state, u16 0}, resolved through comp47+0x11b0 (0x9a0c90 → 0x9a0db0).
  - Encoding is value kind 14, 27 bits including the kind nibble and the StEC bit.
- Run 361: `10:s49=h10.1,s50=h10.0` puts the muzzle flashes at both pistols. `:1589` live.
  - Generalized: each hero's CosmeticEntity output var is set to {1, weapon inst, state, 0}; the generator emits `cosmetic_handles`. `:1601`.

**Crosshair, name and portrait**
- Run 367: `g:0ebc=b1` shows the reticle. Controller comp75 field 0 = hero GUID (0x02e0000000000003, flagged u64) shows the name "TRACER". `:1594` live.
- Portrait path via var 0x3457 `:1618` static/emu:
  - Server-activated stacks never push their value (0x1b95ff0 modes 1/2 skip it).
  - So the plan was to deliver 0x02400000000009cc via sender graph 0x0b6c state 7.
- Run 369 (0x0b6c on the hero, GRAPH_COUNTS (4,35)): shows the badge "6 | TRACER" but no portrait. `:1633` live.
- Run 372: hero sync var 0x070f (0x01cf slot 15) **is the portrait texture**. `:1665` live.
  - `11:s15=g0c0000000000128e` (= hero-select property 4447 / hero record m_8203BFE1 entry 0d80..40c9) shows the face.
  - The 0x3457/0x0b6c path only feeds the role icon.

**Soldier first pick**
- Runs 363–366: the Soldier first pick exits the client 0.7 s after the first hero full frame. `:1605` live.
- Fixed in run 370b: Tracer's BlinkChargesVar ('2:11:3:3.0') was being written into Soldier's instance 2 (graph 0x4b). `:1653` live.
  - Soldier's rifle muzzle handle s50=h11.11 works.

**Pair launcher (two clients)**
- `--retail-private-pair`: one shared profile, one ACL capture/restore, per-client containers 1119/1129. `:1609` claimed/tests.
- E1 (run pair-e1-363): two clients at the main menu for 300 s. `:1623` live.
  - Mercy#1002 on 1119 → 21119; Tracer#1001 on 1129 → 21129.
  - The client honours the --BNetServer port.
  - PowerShell 5.1 trap: wrapping ConvertFrom-Json in @() makes the array one element.

**Pulse Bomb planning**
- The 0x34 Ability (state 9, node 31) is in the false branch of BooleanSwitch 11 (global 0x1880). `:1639` emu.
  - Path: server Entry #1 → SetVars 0x18e0/0x1cdd/0x00d7 → Stack 12 → switch 11.
  - Ability state data = 10 bits, `0:8,0:1,1:1,0:32,0:1,0:1,0:8`.
  - States: 0 WeaponVolley (projectile 0400000000000045, graph 0x0c1b, explode on 0x02400000000005d4); 1 Anim; 2 Wait 0.15; 3 CosmeticEntity bomb in hand (04000000000000ea); 5 Wait 0.2; 14 Stack (0x034a).
  - End action #128 clears 0x0021.
- Run 371: Ability state 9 ran and consumed Q, but no projectile appeared. The meter showed a big Pulse Bomb icon. Likely rolled back. `:1660` live, `:1672` emu.
- Run 373: AbilityPress `'34:9:0x20:0.6:0:8,...'` + UltUseButton 0x20 works. `:1690` live.
  - Meter → 0, holster, throw, the orb flies.
  - The client predicts the chain; the server confirms it for 0.6 s.

**Multi-client responder and proxies**
- `udp_pair_reply.py` runs one exec of the unedited single responder per client, with shims. `:1677` claimed/offline tests.
  - Each client's port is bound by its own +0xae key.
  - Index bases: c0 0x300, c1 0x340; heroes 0x80000305 and 0x80000345.
  - `--proxy-heroes` creates the other player's hero with flags 0x41 + skin + comp26/51.
- E2 (pair-e2-366): two clients in one instance session, both with the full HUD. `:1698` live.
  - UI clicks must wait for the `bound` event.
- E3 (pair-e3-367): each client sees the other as a proxy. `:1713` live.
  - The proxy uses the same entity id as in the owner's world and skin 0a5000000000170f, and moves by module-3 snapshots.
  - It floats, has a fixed pose and no weapons.

**Per-hero HUD and the hero-change regression**
- Per-hero HUD configs for 32 heroes (`hero-hud-configs-372.json`). Tracer was checked with the native decoder; the others only by the encoder. `:1708` claimed.
- Run 374, Soldier `:1723` live:
  - States `2:3;5:17,31;6:7,16;7:15,32;10:11,23,42;11:0,9,40,67,69,70;12:15,66,139,138,67`, subgraph `12:0x01cf:2:3`, comp75 0x02e000000000006e.
  - The full HUD works, but ammo max is the generic 40 (this bullet says Soldier's magazine is 25).
- Run 375 regression: Tracer → Soldier re-pick gives no weapon and no HUD. `:1729` live. Fixed at `:1757`.

### 7. 2026-10-17: party, remote player, Pulse Bomb, PvP, maps, AI, lines 1742–2131

**Party and hero change**
- Party flow (Codex), per docs/PARTY_PROTOCOL.md `:1744` live:
  1. 20815 Nearby Players.
  2. Native Invite 22102.
  3. Merge 20703.
  4. Yes 22110.
  5. Two-member roster.
  6. The leader alone starts Practice; each client is referred to its own endpoint.
- Hero change fixed (run 387): cmfd_seq/cmfd_tick survived the re-pick, so the new hero got a stale CmFD. They are now reset; a fresh hero_cmfd (2311) appears and Tracer → H → Soldier works. `:1757` live.

**Remote player floor fix**
- The remote hero draws about 0.93 m above the snapshot y: comp1+0x440 = (48.73, 2.93, 38.13) for snapshot (48.71, 2.00, 37.77). `:1748` dump+live.
  - `--proxy-dy` defaults to −0.93.
  - pair-e3-386: the remote stands on the floor (but see `:1783`, where the "stride" is probably the default pose).

**Remote player facing, weapons and animation**
- Yaw goes in movement field 13: s16 Euler yaw, +y rotation, forward = (sin, 0, cos). `:1763` emu.
  - `--proxy-weapons` creates 0x15/0x16 on the proxy (instances 1/2) with states 39,43.
- pair-e3-391: no pistols visible and no turn. `:1781` live.
- Dump 391 `:1788` emu:
  - The pistol children exist but sit near the feet.
  - The proxy lacked comps 69/86/122/136, which come from create flag 0x80.
  - Proxy create flags are now 0xC1; movement field 19 = velocity (state +0x90).
- Dumps 401/403 `:1809`/`:1816`/`:1824` dump+live:
  - The proxy's comp1 world transform stays frozen at the create pose and is not linked in the transform graph (own hero has comp1 +0x2b8/+0x4c0/+0x850 set; proxy has null).
  - The mover (comp31/86) never runs. comp4/comp24 follow the movement, which is how bots render.
  - `--proxy-controller` (def 0e, flags 0x30, comp44 = proxy hero, comp74 team 0) changed nothing visually.
- pair-e3-405: per-viewer transform/remote-bit experiments were inconclusive. `:1830` live.
- **Remote player animates (pair-e3-410)** `:1839` live:
  - Uses the movement-state slot-1 "remote bit" (+4 = 1, positions q/256, the client derives velocity).
  - The proxy follows a circle, turns, runs and holds a pistol.
  - So the remote bit selects the remote-character render path, and comp1 is irrelevant.
  - `--proxy-test-motion` syntax is at `:1835`.
- The proxy weapon script activates the cosmetic BooleanSwitch (Tracer 0x16 state 2). The remote bit is now the default. A 44° look did not visibly turn the body. `:1858` emu+live.
- pair-e3-414: a stationary spin turns in place with both pistols. `:1863` live.
- pair-e3-415: a real 180° owner turn shows the remote facing with pistols raised. `:1871` live.

**Pulse Bomb**
- Run 392 (server Pulse Bomb `350:2:4:0.2:1.0:15:30:5`, mirror reset frame at CmFD 1821) `:1770` live:
  - Event order: pulse_bomb (point (48.71, 1.93, 32.46), fuse 1.33 s) → mirror on → ult_used → reset frame → phase 2 → mirror off → detonation (bot at 2.53 m, 70 damage).
  - The meter stays at 0, because the reset stops the 0.64-s rollback.
  - The run-389 "orb" was the HUD ult-ready burst, not an explosion.
- Client volley fire (0xae6df0 → job 0xae88a0) queues a projectile request in world-context slot 44 (+0x20 list). `:1795` emu.
  - Key = hero<<32 | inst<<19 | node<<8 | shot = 0x8000030500180001.
  - The entity list stays empty. Module-2 update 0xb3b1f0 → 0xb3e490 only simulates linked server entities.
  - So the server must create the 0x45 entity and link it with a section A record.
- Runs 408/409: the section A link works (comp50 +0x40 key, +0x76 = 1, pending list consumed) but was aimed +z, backwards. `:1847` emu+live.
- Run 411: aimed with the command yaw/pitch, the bomb flies. `:1850` live.
  - Blue explosion at ~0.79 s, smoke at ~1.0 s; the server kill at 1.24 s.
  - The visual is ~0.45 s ahead of the server damage, and the throw doubles.
- Run 416, promoted: `mirrorreset2` (the CmFD of the reset ack frame never goes backwards) + MirrorOnRelease + PulseBombAt 0.8 + BombEntityAuto/BombLink 3. `:1866` live.
  - Result: one throw, explosion and kill together, meter stays at 0, no rollback.

**Weapon parameters**
- Per-hero weapon params: Soldier 30 rounds / 1.5 s reload; Tracer 40 / 1.0. `:1785` claimed.
- Run 393: Soldier HUD 30/30 with auto reload. `:1808` live.
  - Contradiction: `:1728` says Soldier's magazine is 25.

**PvP damage between clients**
- pair-pvp-417 `--pvp-dps N` ray test `:1876` live:
  - Shooter: volley aimed with the newest command yaw/pitch, eye = feed +1.5 m.
  - Target: the victim's proxy as the shooter sees it: snapshot − dy, body centre +1.0 m, radius 0.5, half-height 1.0.
  - On hit: subtract dps×dt, send 25001 to the shooter (target = proxy id), and push the victim's health via hero global 0x06c9 plus own comp51 and the proxy's comp51.
  - Kill → 3 s → respawn teleport.
  - Result: 89 hits / 6 confirms; the victim's HUD drops to 58 with the native low-health red vignette.
- pair-pvp-418 (200 dps): kill in ~1.0 s (75 hits / 11 confirms). `:1889` live+dump.
  - The card froze at 163/165 because own-hp was marked as sent before the plan was dropped. Fixed: commit happens in commit(), with a 1-s refresh.
  - The dump read comp51 as current/max (+0xdc / +0xe0). **Corrected next run.**
- pair-pvp-419: the card shows "−1/0" after the kill. `:1899` live.
  - Pool order is actually **(max, current)**: "/N" follows the first float.
  - The big number is CurrentHP(0x06c9, 0x06c4); a non-integer gives −1.
- **Native player death (pair-pvp-420):** hero global **0x01de = true** at the kill, cleared on respawn. `:1907` live.
  - Victim sees 0/200, grey segments, red vignette, pistols removed, camera leaves first person.
  - On respawn: 200/200 with the equip animation.
- pair-pvp-421: the dead flag mirrored into the proxy's frame (svars[0]) makes the proxy's pistols vanish and hands drop. `:1918` live.
  - There is no ragdoll or kill feed.
  - `-PvpCombatLog` sent 25000 records (98 B each) with no errors and no visible damage arc.
- pair-pvp-423: mutual PvP, 189 hits, 3 kills both directions. `:1927` live.
  - Run 422 was an intermittent c1 startup exit.

**Kill-related channel-2 records**
- **25011** (0x6c3940) is not the kill feed. It stores one killed-by record in global-root slot 12 (vt 0x2611310). `:1933` emu.
  - Read by the Kill_Cam_Overlay presenter (0xace460) and the killcam follow target.
  - Fields: +0x78 killcam entity, +0x7c killer controller (if local, the attacker is hidden), +0x80 AttackerName, +0xb0 killer hero GUID, +0xd8 kind (5 hides).
- 25012 (0x6c3fe0) and 25013 (0x6c29c0) are slot-13 highlight/POTG records plus cosmetic preloads. Do not send them for kills.
- The kill-feed source is still unknown. Candidates: 25010 (0x7a5740) and the consumers of channel 3 world+0x138.

**Map loading**
- Runs 424..424f: map GUIDs in lobby 20505 lists crash the client. `:1943` live.
  - Map 0800 in list 2: exit after 4 datagrams.
  - Header in list 1: exit at lobby phase 6.
- Headers are preloaded at login from 20505's third array (offset 168, "list 2"). `:1952` emu.
  - Chain: 0x77a000 → 0x57bba0 → 0x57c700 (type 0x9F, provider+0x98 set) → 0x171a9b0 → 0x171cca0, priority 4.
  - Completion: 0x57ce60 → 0x57c2d0 → 0x57ca70 → 0x57c540 (state 3; 0x7f0970 registers the header) → 0x57cba0, which requests the modes (|0x10000000000).
  - List-1 crash: NULL at 0x57c54a. No queue/20600/20300 code requests a 0x9F header.
- **Original map preload works (run 427):** list 2 = [0x0790000000000688] only, with no native control. `:1974` live.
  - The Practice panorama appears in hero select; 13,087 datagrams.
  - The list sits at +0xa8.

**Three clients**
- pair-trio-426: three clients, containers 1119/1129/1139, 45-s stagger (a 20-s stagger timed out the TLS CA setter). `:1962` live.
  - The dead proxy followed the death camera up to y 3.41; now it is frozen at `dead_pos`.
  - The machine has 31 GB RAM: three clients fit, twelve will not.
- pair-trio-428: integrated run with original map preload. 240 proxy creates + 240 proxy-controller creates, 125 hits. `:1982` live.

**Teams**
- pair-team-429: session i plays on team i%2. `:1988` live.
  - Team 1: filter bits 0x11040001 and comp74 field 2 = 1. Proxies carry the owner's comp26.
  - Same-team hits are ignored.
  - Enemy outline visuals are unverified.

**Anubis**
- Map catalogue: 132 headers, 25 modes. `:1995` live.
- Recipe: MapHeader 079000000000005b, MapCanonical 080000000000005b, Skirmish mode 0x0230000000000010.
  - Placeable cap raised 8 → 64.
  - 29 zero-dependency pending keys must be read live; Practice's catalogue keys were 5 too low.
  - Run 430b with wrong keys aborted to Training after ~4.5 s.
- Run 432: "DEFEND – TEMPLE OF ANUBIS"; spawn at (14.04, 0.9, −28.94).
  - Spawn clusters: (14, 0.8, −30), (−13, 4.3, 139), (−68, 2.5, 19).
  - The Practice hero-select overlay persists (graphs 13c1/20e4); Skirmish graph 0d76 is not driven.
- Anubis floor feed y ≈ 1.75. `:2031` live.

**Practice vs AI**
- vs-AI is not a mode; it is normal modes with server-side bots. `:2007` live.
  - Each AI is a virtual session (team 1, entity base 0x3c0 + 0x10k).
- Run 434: damage before the hero's first full frames broke the HUD. `:2016` live.
  - Gate: wait for counts['pick_full'] ≥ 40, and HUD-hp sequence bumps only when ≥ 20.
- Run 435 works: 7 AI kills, 6 respawns. `:2020` live.
- Session bound raised 300 s → 14,400 s. `:2026`.
- Run 439b: per-bot heroes, e.g. Soldier 03cf/006e, 0022/0002, 0141/0005. `:2029` live.
  - The player sometimes has no HUD (runs 434/436/438) even with identical counters. Unresolved.

**Lobby and queue messages**
- From the user exploration session `:2035` live:
  - 24000 Training (Tutorial and Practice Range; the mode field differs). Only one instance per session; a second entry hangs.
  - **44100/44102** = queue join/leave for QP role queue, Competitive and vs-AI. The 32-byte struct plus 3 u32 holds queue type and difficulty.
  - 26500 Shop; 38201 settings blobs (×9 at once); 22200 when leaving Options; 22206 → 20807 (Career Profile); 39200/39201 periodic stats; 21801 leave game.
  - Arcade, Loot Box, Patch Notes, Social and similar menus send no request.
- vsai-441: Practice vs AI Easy sends 44100; the probe answers 23310 + 20600. `:2048` live.
  - Loading screen "VERSUS AI / EASY / TEMPLE OF ANUBIS".
  - Firing is blocked until Escape closes the overlay.
  - 44100 first u64 = 0x0630000000000070; u32s 0xf38640 / 0xf47266.
  - Lobby loop bound raised 1024 → 200,000 packets.
- OW1 bot roster (2018 research): Roadhog, Zarya, Ana, Lucio, Zenyatta, McCree, Reaper, Soldier, Mei. `:2056` claimed (web).
  - Contradiction: the `:2101` roster has 12 heroes, including Bastion, Sombra and Torbjörn.
- boi027 (MIT project for the same build) mapping `:2065` claimed (third party):
  - 20813 leave policy {0, 20, 0.3, 0.2}.
  - 44201 / 44202 reason 1 before 20600 (QUEUE 0xB4F8BC62).
  - Arcade 39802: QP card 0x06300000000000ED at +0x78+0x18; vs-AI cards 0x0630000000000070..72.
  - 24300 account record; 24900 +0x80 hero records (the 0/0 gallery).
  - Retail 20505 list 1 = 2457 skin themes.
  - 55500 features, 27100 friends, 35500 empty data centres, 20704 invite popup.
  - Game side: mode entity 16B2 + comp114, countdown var 0x0517 via an H=0 full frame, team-list graph 0x288D under 288A state 1, 20304 {0,F} send home, CmFD-only owner acks.
  - boi027 calls comp74 field 2 "slot"; this project calls it team.
- Arcade agent `:2077` emu+static:
  - **39802** (schema 0x00A8654E) handler 0xba84b0 fills root slot 74; 0xba8ad0 resolves cards through catalogue 0x046000000000010b.
  - Cards: DM 0x0630..3b, TDM ..34, Elim ..1e.
  - Entry +0 = game type; +0x18 = Quick Play (0x0630..ED); +0xD0 = Competitive.
  - 44100 key = {card GUID, 0, 0, 0 | flags | platform 0xF38640 | pool 0xF48644}.
  - Reply sequence: 44201 (echo the key), then 44202 {key, 1}, then 20600. The client ignores 44203–44207.
  - No 20813 means a NaN leave rate, which shows "−75% XP / 0 MATCHES".
  - 26500 → 26400 (+0x9C = 0).
- vsai-443 live: 44201 echoes the 48-byte key, then 44202 + 20600. `:2090` live.
  - 20813 after 20802 removes the leaver label.
  - Six roster AIs fight.
  - "VERSUS AI EASY" was no longer shown, because that label came from the still-open queue entry.
- OW1 reference `:2101` claimed:
  - vs AI: 6v6; Easy/Medium/Hard = 25/50/100% AI damage; random Assault/Control/Escort/Hybrid map.
  - Timers: assemble 30 s, setup 45 s, respawn 10 s, Assault 4:00 (+3:00 after point A).
  - Leaver rule: last 20 games.

**Navigation data**
- Map chunk 0x12 (`0dd00012_<map>`, all 132 maps) holds two DUTS documents. `:2109` static.
  - Doc 1, root 5ae64839: AI tactical volumes in an AABB tree. Each volume has a floor point, an s32 neighbour list and a walkable voxel mask (1×1.4×1 m, bit = z + Sz·(x + Sx·y); fit 116/116 and 60/63).
  - Doc 0 (3bc32a6e) is not decoded.
  - Anubis: 117 nodes, main component 85, 113 portal edges; spawn → A 125 m, → B 206 m.
  - Practice: 67 nodes.
  - Node y = floor rounded up to 0.35 m; feed y ≈ node y + 0.8.

**Assault flow on Anubis**
- Mode graph 0f4a runs on the server-created game-mode entity, definition 04000000000016b2. It is the only definition with STU_F00E5A06 = component 114. `:2119` static+emu.
  - Config var A3DEF368 resolves through 0xcdd340 → 0xd0a0a0 (type getter 0xbcb240 = 0x72) to the first entity with comp114.
  - Without it MATCH TIME = `A3DEF368.v0517` reads 0.0.
- Phase var v003b:
  - 0 waiting
  - 1 assemble (30 s, later 25)
  - 2 setup (45 s, first point only)
  - 3 round (+240 s for A, +180 s for B)
  - 4 round end (11.6 s)
  - 5 match end (R13 S20, message 0695 = winner)
- Phase messages 0700/0692/0701/0694/0695 drive graphs 1feb (objective HUD), 288b (S32 → S35/S36 opens hero select; 0701 → "SETUP:"), 0c90 and 0cbb.
  - Client-only S79 sends 071d ATTACK/DEFEND.
- Anubis objectives: A idx698 / B idx723 (def 1c22, 0e8e + 0f4c); doors 0d32; spawn-room tags 0x341/0x3cc/0x310.
  - No map entity has comp47 client-side.

### 8. Runs 445–448: AI navigation, tactics, reply cap, game-mode entity, lines 2132–2169

- ainav-445: six AIs walk 18 waypoints from the spawn to A. They bunch within 0.4 m and deal far more than Easy damage. `:2134` live.
- Tactics `:2138` live:
  - `--ai-difficulty` 0.25/0.5/1.0.
  - `AI_BASE_DPS` table uses public numbers, not client data.
  - `--ai-spread 6` nav cells, 1.5-s stagger, hold range +0/+3/+6, lateral slot 2 clamped to walkable voxels.
  - Run 447b: bots spread out; 6 player kills and 19 AI kills.
  - Run 447 was a TLS CA-setter startup failure.
- **Reply cap:** run 446 dropped back to the menu after ~13 min because the responder stopped at 20,000 replies. The bound is now 1<<31 (u32 GCM nonce sequence, never reused). Run 447b passed 21,810 replies. `:2147` live.
- Mode host options `--mode-host/--mode-graphs 0xf4a/--mode-delay/--mode-graph-types/--mode-timeline` `:2150` claimed (not emulated):
  - One full frame per phase, base 2 + phase; earlier states are sent inactive.
  - Recipe: Assault mode 0230000000000014, definition 16b2 at index 14, preload graphs 0f4a/0cbb/1feb/0c35/1b47/0c8f.
  - Pair-responder definition bound = 15 entries.
- assault-448 (16b2 entity + 0f4a only) `:2159` live:
  - Header becomes **ATTACK** (S79: team == v2a2b, default 0).
  - The mode HUD shows "0.0 | UNSPECIFIED" (objective name var unset).
  - Dump kept: PID 11620; mode entity index = create base + 29 placeables + 14.
- Next step `:2165`: on dump 448, emulate the phase frames with `ssfull2.py` before any live phase test.
  - Always-on states S52/S58/S12+0cbb/S178.
  - Vars: g:2a2b=1, 2a2c=0, 1ddb (objective name), 0517 (clock).
  - Phase 1 states: S43/S7/S10/S8/S9.
  - Settle the ChaseVar/Wait/GME/team/string value forms first.

### Contradictions and retractions in this range
- **0x06dd as the HUD gate** (`:1101`) was retracted at `:1205`: it drives the distortion effect.
- **"comp26 cannot be sent in a create"** (line 906) was retracted at `:1410`: it was an encoding bug.
- **The teleport launch explanation** (`:1376`, "placed 10 m up") was replaced at `:1417`: the spawn-room barrier ejects heroes with no team.
- **The CASC mutex as the second-client blocker** (`:1546`) was retracted by E0 (`:1579`).
- **Stack is-top setting 0x0ebc** (`:1535`) was superseded at `:1594`: it needs `g:0ebc=b1`.
- **comp51 pool order**: read as (current, max) at `:1892`, corrected to (max, current) at `:1903`.
- **"Remote player runs"** (`:1752`) was qualified at `:1783`: it is probably the static default pose.
- **Soldier's magazine** is 25 at `:1728` but 30 at `:1786` and `:1808` (30 was seen live).
- **The vs-AI bot roster** has 9 heroes at `:2056` but 12 at `:2102`.

## `CLAUDE.md`

### Lobby / BGS connection (TLS, credential, gateway)
1. The 1.74.0.0.104319 client reaches TLS 1.2, WebSocket RPC, a local "Pro" account login and a Balancer referral. It needs a mapped-loopback certificate with CN/SAN `::ffff:127.0.0.1`, the CA imported, and the native socket CA setter. BNetServer must stay IPv4. — `CLAUDE.md:101-104` live. Older "handshake-only / error 1016" notes are superseded.
2. The lobby credential passes the original native RSA/address/HMAC checks only because the launcher installs one guarded heap RSA-2048 modulus (e=65537) per launch. Without that key the client closes right after the credential, sends nothing more, and retries the referral. — `:105-108,124-127` live.
3. The lobby server must send one Panama-LE encrypted 292-byte (0x124) packet: credential[256], two 128-bit ids, one u32. The client's receive key is HMAC(k2, CR|SR) and its send key is HMAC(k3, SR|CR); the key also serves as the IV. The native transport at RVA 1909547 waits for exactly 0x124 bytes. Sending only the 256-byte credential times out. — `:109-112` live.
4. The 36-byte credential suffix is currently `((13<<40)|1,1), ((13<<40)|cid,1), 0` (server-local group-13 ids). The other field meanings are unverified. — `:~208-210` claimed/live.
5. After the handshake (native handshake state 6 / connection phase 6) the client sends 13 encrypted bytes: a 3-byte big-endian length plus a 10-byte body, the bootstrap offer `01 00 | u32 1 | 0x11d82194` (entry byte, message id minus base, u32 hash vector). — `:113-116` live.
6. **Gateway negotiation:** reply `01 02` (empty message id 3). The client (ctx+0x84 bit 0 clear) enters negotiated state 5 and sends `01 03` heartbeats every 5 s. Echoing them keeps the lobby open (112 s, 22 heartbeats). An id-1 echo or decoded id-2 replies close the connection at once (role / bit-0 checks). — `:117-123` live.
7. Native gateway codec: send 0x560d90 → 0x568690 → 0x5652b0; receive 0x561ff0 → 0x567de0 → 0x5670e0; per-state timers 0x560b50 / 0x5609c0. — `:~198-200` static.
8. With peer group 13 and the gateway catalogue, the client offers all 86 lobby protocols, negotiates, and sends hello 21800 + 21809 on protocol 0x3c7e3468 (entry 25). The private hello body is 216 bytes and round-trips exactly. — `:152-153,~196` live.
9. Initial identity reply 20500 (protocol 0xf5ff548a, sorted entry 83). Handler 0x774710 calls 0x6261e0, which installs identities/strings and raises event 13. The UI then shows "ENTERING GAME…". With identity only, the client sends nothing further. — `:154-158` live.
10. Initial data 20504 (empty key lists), 20505 (empty hotfix lists) and 20502 (player record): all three handlers are traced. Key context +0x40 changes 0→1. The client then sends 60002 (entry 29, schema 0x49963fa6) and repeating 42600 (entry 80, schema 0xeb45ad29). The screen stays black. — `:159-164` live.
11. Adding 20802 (default account preferences) is accepted. The client then sends 58400 (0x692c511b) about every 30 s. — `:173-175` live.
12. **Main menu requirement:** the full initial data including 20802 gives the original main menu plus the first-run TUTORIAL prompt (run screen8). — `:176-178` live.
13. Earlier "time out communicating with Battle.net" screens came from leftover clients whose server had stopped. — `:~181` live.
14. 20801's traced handler only prints account information. 59900-family handlers print account-migration data; 42400-family handlers process tournament data. — `:191-194` static.
15. The slot-22 +0x88 "player marker" is transient with unknown meaning. It must not be called a persistent receipt. — `:~166-167` live/static.
16. Reliability: 27 of 27 unattended launcher sessions reached the menu. The game is single-instance, so the launcher retries when the client exits at once. — `:~187-190` live.
17. 20500 / 20502 / 20504 / 20505 / 21800 / 21809 descriptors are numeric only. — `:~195` static.

### Map/Hotfix, world manager, resource gates
18. Map track: the Practice header loads and the default loading context activates (updates return state 4), but readiness and cache stay zero. SDK kind 13 is LOCAL Hotfix enumeration, with error status/tag/code 3/2/-1 at +18/+19/+20; +138/+140 hold successful data/count. — `:145-148` live.
19. The world-state manager has vtable 0x2655d88 and its state at +0x18 (names SHUTDOWN…WORLD_RUNNING). At ENTERING GAME it is WAITING with no request. — `:168-170` live.
20. World requests enter through 0xb4b1c0. Callers: 0x82c3e0 (game-server join), 0x8359a0 (local world), 0xb97f40 (from the 20300 dispatcher). — `:171-172` static.
21. World tick 0xb4b5f0 → switch 0xb4cca8 → WAITING handler 0xb52b20. That handler:
    - normalizes the request's low u32 to map-header type 0x9F;
    - checks catalogue availability with 0x171b800 and polls status with 0x171c0b0;
    - returns while status < 2; on status 3 calls 0xb523f0 and enters state 3 (ABANDONING SCENES); other statuses ≥ 2 take the error path.
    — `:480-485` static.
22. In udpapp11/21 the Practice header and the mode resource are in the catalogue but have no load-cache object (status 0). — `:486-490` live (dump).
23. Native resource request 0x171e8e0 (structured callee 0x171cca0) can load the header; standalone PracticeResources proves this. — `:502-505,578-580` static/live.
24. 20318 (channel 0, schema 0xc77f6403) → handler 0x7d8010 → registrar 0x684310. Field +0x98 is format 1, not the build number. — `:506-515` static.
25. Metadata entry fields:
    - +0x80 is a retry deadline (correction: NOT an action pointer);
    - +0x88 is a completed-attempt counter (NOT a state marker);
    - flags: 1 = content present, 0x40 = in flight, 0x80 = failed.
    Consumer 0x67fd70 / 0x68018b drives these. — `:516-523` static.
26. A kind-14 request (ctor 0x81c270, vft 0x26188d8, worker 0x81dfc0) opens a Hotfix temp file through 0x1858f30 with flags 0x321. Six live requests all returned 3/2/-3; formatter 0x185e3a0 labels -3 "failed to open". Run 22 showed HF-503. Empty-dir and ACL "fixes" did NOT work. — `:524-535` live.
27. Context slots: staged +0x1a0 is promoted to ACTIVE +0x198 (0x68362f..0x68370a); 0x6855a0 installs a staged context. The default identifier returns completion 2 without setting +0x98, so "default not ready" is not a failure. Run 28 errors became 9/2/6. — `:552-567` static/live.
28. Engine preloader: slot 39, vft 0x2610910, owner xor 0xf3b530a5300e4c1a.
    - Configured 64-byte descriptors at +0x2678 (count +0x2680); retained 72-byte descriptors at +0x58 (count +0x60).
    - Builder 0x67b250; 0x677190 adds, 0x67a7e0 consumes, producer 0x6775d0..0x679de0.
    - The queues were empty in the saved dumps. +0x44 is "submitted", not "loaded".
    — `:578-606` static.
29. Getter 0x1b53d00 "190GUID" means member offset +0x190 (correction). 0xb4b1c0 only copies the request; 0xb52630 calls no loader. — `:~596-601` static.
30. The welcome's extra resource array lives at request +0xf8 / count +0x100, not +0x68/+0x70 (request copy 0x590d50, consumer 0x7d9990..0x7d99be). — `:~614-618` static/live.
31. Hero Gallery with no hero list was empty (runs 35/36) even with every filter enabled. — `:~626-629` live.
32. A one-shot diagnostic 0x171e8e0 (priority 4) header preload takes header and mode to cache raw state 3 and both managers to WORLD LOADING (6). It shows the Practice loading screen (map art, name, tip). Transitions: WAITING → WAITING(pending) → ABANDONING SCENES → WORLD LOADING. — `:640-665` live.
33. World-owner decode is ONE xor with image[0x2ee48a1] and 0x4dfd564467c27b96 (not the engine mask). — `:~666-668` static.
34. Corrected dispatch: WORLD LOADING (6) → 0xb4ccfb → 0xb54a80; 0xb54370 is WORLD LOADED (7).
    - The 0xb55bbf path checks world resource arrays +0x50/+0x60/+0x88 (via 0xb4d910, object+0xac ≥ 2), then scene virtual +0x140.
    - The owner getter 0x5855f0 decodes owner+0x1d0 with the PID (GS:[0x40]), seed routine 0xb7c60 and table 0x2ee46d0.
    - Scene secondary vft 0x2655fc8; 0xb59220 returns scene+0xa0.
    — `:682-692` static.
35. State 7 handler 0xb543f2 calls 0xb4ffd0 (content gates: active/staged ids, provider readiness for types 0x75/0x77 via 0x57b9f0, then extra resources). It needs manager +0x41 and owner virtual +0xc8 (0x587170 reads owner+0x2a8). — `:~669-675` static.
36. Scene readiness: 0xb5b78e sets primary+0x128 only when a scene exists, 0x1b53d80 returns 5 and 0x1b40cf0 succeeds. Request method 0xb59680. The null global 0x32213e8 falls back to 0x3225890 (FNV-1a32 over 8 GUID bytes). — `:693-702` static.
37. The header asset +0x50 names 0x0800010000000688 (variant bit 0x10000000000). Using it, scenes stall at controller state 1. — `:714-747` live.
38. Typed asset status from 0x173b910: 0 invalid, 2 unfinished, 3 finished+failed, 4 OK. Finished is control+8 bit 16, failure is bit 15. Do not confuse with raw status 3. — `:756-762` static.
39. **Canonical map 0x0800000000000688** in both welcome fields reaches WORLD LOADED (7) / WORLD REPLICATING (8): scene ready, controller 5, typed status 4. So the header's variant reference was the blocker. — `:750-788` live.
40. Scene state getters 0x1b53d80 / 0x1b53d30 decode object+0x458 with xor 0xe4d560d2fc1e2c2b; controller +0x1a8 is state, +0x1b0 completion data. Scene update 0x1b57ff0 → 0x1b5f230; state-1 path 0x1b412d0 / 0x1b416a0. The constructor requests two raw resources plus a typed handle (0x173b8f0 → 0x1740bd0). — `:~728-731,752-758` static.

### Instance (game server) UDP transport
41. **Referral 20600** (schema 0x074dad18 → handler 0x775cd0):
    - outer +0x78 must be false;
    - host +0x2e / port +0x2c are formatted with "%s:%d";
    - nested +0x6e is an IPv4:port endpoint (correction: not a token).
    Job string at +8; callback 0x7daf70 → 0x7cc690 (ConnectToInstance) → parser 0x41ef00. — `:229-232,268-272` live/static.
42. 0xe3e940 creates the referral job through 0xe1e6f0 in component slot 120 (vft 0x26775f0, job at +0x20). Consumer 0xe3e700. Queue slot 38 has vft 0x2610c68; root+0x220 queue service (vft 0x260cd68) owns four lists. — `:233-245` static/live.
43. Training sends 24000 (mode 2 or 3, empty name, final integer 4). All 34 23300-family layouts are known. 23332 queue notification is accepted (it is the tournament path). Referral kind 1 left the client in the Training menu (run 180). — `:226-249,463-470,~1700` live.
44. Connect path: 0x7cc690 → 0xc68980 → 0x3fc550 → 0x3ff920. Selector byte RVA 0x2e2f308 = 0 selects peer path 0x40009d → 0x3f8990 (UDP). — `:273-276` static.
45. The client then sends 34-byte UDP datagrams (39 of them before any reply). — `:~290-293` live.
46. **Packet format:**
    - AES-256-GCM with key = referral +0xae for client→server and +0xce for server→client;
    - nonce = 8 zero bytes ‖ u32 LE sequence;
    - 22-byte header used as AAD: (ver2<<4 | type), seq, ack (+0xa8), u64 bitmap (+0x140), flags, 0xad;
    - 12-byte tag at the front.
    — `:307-310,335-339` emulated/live.
47. Codec roles: peer+0x10 SEND (+0xae), peer+0x18 RECEIVE (+0xce). Earlier slot labels were reversed. Key de/obfuscation is at 0x3fa810 / 0x3fa380. — `:328-339` static/emulated.
48. Receive chain: 0x4027f0 → open → 0x401880 queues at peer+0x188 → 0x401500 → 0x3f94f0.
    - Peer state 1 needs (flags & 3) == 3 (at 0x3f95c4).
    - State 3 checks mask 2 plus ack sequences.
    - 0x3f9fc0 keeps the received-sequence BITMAP (+0x140, not a connection id) and sets +0xa8 = received seq + 1. Bit 0 acks next seq − 1; old sequences are rejected.
    — `:340-344,~385-388` static.
49. Reply type 0 flags 3 (ack = client seq, bitmap 0, empty body) → the client moves to flags 2 and nonempty type-1 packets. Replies with flags 1 were ignored. Never reuse sequence 0 with the same key. — `:310-313,354-358,~386-390` live.
50. Client body sizes: 13 / 33 / 43 / 63 / 73 / 93 / 103 / 123 bytes; with receipts mostly 0 (75), 13 (4) and 33 (145). — `:~362,~395` live.
51. Active transport vtable 0x2616458 with seven modules. Event 4 path: 0x401740 → 0x3fc1b0 → core+0x3a0 → 0x3fc650 → 0xc68c30 → vtable+0xb0 = 0x7ce110 (the body receiver). 0xe3af00 (32-byte control packets) is a separate, not-active path. — `:~382-385` static.
52. Connection +0xa0 holds the u32 bits consumed per module, a useful framing-debug lever. — `:~880` static.
53. Intermittent early instance failure: the client resends 33-byte join packets, then exits 1. Seen in runs 124/128/134/142/184/186/188/210. Cause unknown. — `:~1519,1700,1752` live.

### Instance body framing
54. The receiver 0x7ce110 reads:
    - a compact u32 frame;
    - prefixes for modules 0..5;
    - a 7-bit entity count;
    - per-entity encoded ids and modules.
    Bit reads are LSB-first (0x78bdb0). The compact u32 reader 0xdb0e60 has widths 18/24/32; the first field seen was 19 bits. — `:~400-405` static/live.
55. Module vtables in order: 26167c0, 263c3c8, 260e2b8, 2616350, 2615850, 2617c58, 2615fe0. Module 0 receiver 0xc6a480 / writer 0xc6ade0; module 3 receiver 0x7c0630 / writer 0x7c0930 → 0x7cc060. — `:~406-410` static.
56. Server→client prefixes:
    - mod0: present bit, 4-bit primary, 4-bit secondary, byte-aligned records;
    - mod1 and mod2: empty;
    - mod3: compact ack frame + 4 flag bits + time bit (+16);
    - mod4: the bit must be 1;
    - mod5: bit-terminated lists.
    — `:427-431` static.
57. Correction: the module-5 prefix is ONE 0 bit outside hot reload (two bits only worked with zero entities). — `:~870-873` live.
58. There are 11 reliable channels. Channel 0 is the 20300 family (schema 0xc77f6403); channel 1 carries 21600 / 21610 / 21627. — `:432-433` static.
59. 20300 handler 0x7d9400 checks +0x500 == 16000 (quanta) and +0x519 == false, then calls the world-request path 0xb97f40. — `:433-435` static.
60. **Welcome fields:**
    - +0x4a8: map GUID (becomes the world request's first field);
    - +0x128: settings map GUID;
    - +0x130: mode GUID 0x023000000000005a (Practice);
    - +0x520 (u64[]): extra resources;
    - +0x110 and +0x4d8: strings;
    - +0x78, +0x4b0, +0x518, +0x138 and 32-byte +0x4b4: changed nothing.
    — `:452-476,~1702` live.
61. The client answers the welcome with reliable 21600 (local name, zeros, empty blob) = join, then sends 21610 pings (echoing pongs changed nothing) and 21627. — `:~444-446,454` live.
62. 20301 is a settings update (installer 0x587e10). 20302/20322 were tried. A second join flow exists: queue session start 0x828580 → 0x8357a0 → local-world builder 0x8359a0. — `:456-462` static.
63. Mode GUIDs are type 0xC5 (0x0230…); low part = public mode id; Practice = 0x5a, the other Practice mode = 0x18. — `:472-474,1797` static.
64. **20302 selected entity:**
    - dispatcher 0x7d2000 (table 0x7d2244, index 2) → 0x7d6c10 → 0x5881c0 writes owner+0x298 (u32; schema is signed i32, keep the bits);
    - getter 0x582340;
    - invalid if equal to image[0x3241288] or if the low 29 bits are all ones.
    — `:796-802` static/live.
65. Entity table lookup (0x78abe1..0x78acb5):
    - table = owner+0x18 XOR (0x042b51ef9b84a901 + 2·ror64(image[0x2ee53a8], 4));
    - bucket (sel & 0xfff) × 16;
    - 16-byte rows: key u32 at +0, object pointer at +8;
    - the full id including generation must match.
    — `:803-808` static.
66. Replication job: slot 79 (0x5fd5a0, vft 0x2610cf0) +0x18 points to the job.
    - job+8 = context; +0x18 = queued count; +0x128 = pending count; +0x148 = completion marker;
    - pending rows are 24 bytes (dependency array +8, count +0x10).
    — `:822-839` static.
67. Completion path: 0x78ab81 → 0xb4a810 → 0x1836820 → tailjump 0xb523f0. State 8 → 9 is set at 0xb52548. — `:828-834` static.
68. **The four pending rows are map placeables** with ids 0x88, 0x22e, 0x234, 0x289 (producer 0x787f50, visitor 0x78b9a0, components 0x5e/0x3a). Only an entity create naming the placeable removes its row (0x78c360 → 0x788480). — `:862-866` live.
69. Entity id encoding (0xdb0f70): 2-bit class giving top bits 1gg, then a varint index of 8/16/32 bits. Created ids use class 0b100 (e.g. 0x80000300+). — `:~868-870` static.
70. Module chain per entity:
    - mod0: nothing;
    - mod1: one 0 bit;
    - mod2: TWO 0 bits (0xb3a490 has two optional sections);
    - mod3: one 0 bit;
    - mod4: 2-bit op, 14-bit length, payload.
    A minimal placeable create is `1 | u32 placeable | 0 | u8 0`, 42 bits. — `:868-874,905-908` live.
71. Creates are queued but processed only after the 20302 selection resolves. Then pending goes 4 → 0, the marker is set, and the world goes GAME MODE LOADING → WORLD_RUNNING (state 10). Geometry renders: Practice snow, rocks, building, sky. — `:875-878,912-918` live.
72. Escape menu works (Social / Career / Options / LEAVE GAME / Exit), but the camera stays fixed. — `:919-923` live.
73. Module-4 ops: 1 create, 2 update (component body 0x78e5d0), 3 destroy (0x78e4e0). — `:1615` static/live.
74. 20303 → 0x7d8870 → world component 0xbfd760. 20304 → 0x7d6830 → 0xe3e4d0 sends a byte to slot-120 callbacks. 20308 → 0x7d88b0 → root slot 29. 0x7d89e0 is the 20306 handler. None of these were sent. — `:~958-962,~985-991` static.
75. 20315 = {string +0x78, u64 +0xa0, selected entity +0xa8, u32 +0xac, f32 +0xb0} → string compare 0xe3dca0 → local message 0x02df. 20317 = bool + f32 → local 0x08cc / 0x08cd. Channel 0 has no generic game-message record. — `:~1680-1682` static.
76. The module-3 server time field is accepted (+70 = 49.085 s); this is not clock sync. — `:~963-966` live.

### Entity definitions, components, hero creation
77. Non-placeable definition create (51 bits): bit 0, resource selector 00 + index32 for type 3, flags (8), component count (8). The engine builds the definition's components itself; the controller definition gave 22/25/26/29/44/45/47/73/76/84/85/90/96/97/137. — `:1037-1043` live.
78. Typed cache: primary 0x173ba30 at image+0x321f3e8, fallback image+0x3225890. 0x1738160 uses FNV-1a with capacity +58, buckets +48, node GUID +10, next +18. — `:1022-1027` static.
79. Serialized→runtime component types (0xf525e0): 445e7678→76, 7a36ae06→29, dae477d5→73, fd024f42→47 (89 mapped, 11 unresolved). — `:1015-1021` static.
80. Hardcoded 0x0400000000002299 is the AUTOMATION entity, not the player. A missing definition gives native error 0x40131. — `:~950-953` static.
81. Post-create callbacks: 0x787b20 walks +90 / count +98. 0xb8c530 needs components 25/26; 0xbae5e0 needs 84/85. — `:~954-958,~983` static.
82. The first-person reader 0x7c7800 → 0x582b10 → 0xda69d0 needs component 44 (getter 0xbd59a0). 0xbd4d40 resolves the actor through +d4. — `:967-972,1058-1064` static.
83. There are 56 hero records (type 0x75, 0x02e0…) mapping to gameplay definitions; e.g. Soldier 0x02e000000000006e → 0x04000000000003cf. — `:1077-1079` static.
84. Creating a definition that is not resident gives "Mirroring mismatch" (0x78dd60 → 0x78b059) and abort 0xb64af0. The client sends lobby 21801 + 21802, notifies the BNet object, stops keepalives and leaves. — `:1080-1086` live.
85. **Preload input = lobby 20505:**
    - list 0 → hero set (0x77a000 → 0x57bba0 → 0x57c700, provider +0x88);
    - list 2 → maps;
    - list 1 → requests (0x171ace0; used for graphs 0x1b, definitions, progression).
    — `:1087-1091,1240-1241` live.
86. Component state rides on the create's trailing u8 count:
    - per entry: u8 runtime type → component slot 0x18 → visitor 0x78b870;
    - byte-align at the ABSOLUTE body bit offset;
    - u8 mask length, then mask bytes (1 bit per field, LSB first);
    - raw little-endian values; field kinds are CRC32 of lower-case type names (teEntityID → 4 bytes).
    — `:1109-1114` static/live.
87. Component 44 holds three teEntityIDs at state +0x78/+0x7c/+0x80 (component +0xd0/d4/d8; hashes 1ab5e773 / 4d1ffc51 / b1ca772f). Setting them links the controller to the hero; 0x82a5e0 sets all three to one entity. — `:1115-1122` live.
88. Create flags:
    - 4: f32 at +0x10;
    - 8: transform (ignored for the hero);
    - 0x10: builder 0x2615ae8 → 0x64ee90 adds components 59/63 if the id equals the selected entity;
    - 0x20: adds 74;
    - 0x80: non-ownership builder (adds 86 / 136);
    - 0x41: u32 skin-theme reference plus 1 bit.
    — `:1135-1137,1286-1288,1567-1568,1599-1600` live.
89. **First-person recipe:** select the controller (20302), create it LATE with flag 0x10 after the selection, and link component 44 to the hero. Result: Soldier first-person view, mouse-look, and the client streams input. — `:1290-1295` live.
90. Component 1 position is at comp+0x90; the render position at comp1+0x440 or comp4+0x160. The spawn-room interior is about (48.71, 6.31, 37.77); the floor is y = 2.0. — `:1319-1323,1604` live.
91. Component 74, the 9-field state at comp+0x70:
    - field 0: u64 hero GUID;
    - field 2: s8 TEAM (−1 = none);
    - field 6: u8 at +0x86 = spawn-room flag.
    — `:1334-1338,1569` live.
92. Without a skin, non-controlled entities are placed but NOT drawn. Bots need definition 04000000000014d2 plus skin 0a50…1afd. — `:1599-1606` live.
93. GUID bits 48-59 = bitrev12(type − 1): 0x0250 = unlock 0xA5, 0x0a50 = skin theme 0xA6, 0x0ea0 = progression. — `:~1724` static.
94. Remote entities do not fall (no client physics). Bots have 25 components including 28/43/47/51. — `:1584,1604-1606` live.

### Movement snapshots and commands
95. Module-3 per-entity item: `[1][len:12 including itself][payload]`. Applied by 0x7c7f50 / 0x7ca130; it needs components 28 and 43. Minimal payload = 36 zero bits. — `:1155-1159` emulated/live.
96. Snapshot field map: 35 "changed" bits. Fields 16-18 are position, q/1024 as raw s32 (selectors 000); 7/8 are floats (view angles?). The float decoder 0xc67260 adds prev + q/1024 with widths 8/10/24. — `:1163-1180` emulated.
97. **Teleport:** a snapshot changing only fields 16-18 (140-bit movement block) moves the controlled hero. Send a few, then stop; client prediction continues. — `:1312-1316` live.
98. **Client commands** (0xc2ddc0, exact decoder):
    - each block is 42 + header u32 bits;
    - movement: 0 = none, else 1 + 3-bit index;
    - buttons: 0x1 fire held, 0x8 Shift, 0x10 E, 0x40 jump, 0x400 R, 0x2000 press edge, 0x4000 release edge;
    - an earlier map (0x1000 / 0x100 …) was a misalignment and is retracted.
    — `:1584-1588` live.
99. Calibration:
    - yaw: θ = a·τ/65536; −32768 at spawn faces −z;
    - pitch: negative = up;
    - forward = (sin, 0, cos), right = (−cos, 0, sin);
    - about 6 m/s at 62.5 commands/s.
    — `:1617-1620` live.
100. Prediction gate 0x64d990 needs global byte image+0x2f7f840 (0 here). Component 60 (vft 0x260e8e0) exists on no entity. — `:1561-1566,1706` static.

### Statescript replication
101. Component 47 is the statescript component (vft 0x2611f68; manager at +0x690). Module 1 carries statescript, reader 0xa16ad0. — `:1199-1207` static.
102. Module-1 framing (38 bits when empty): presence; bit 0x787be0; SFSF flag + u16 base frame; SFDt flag + u8 delta; Part; Size → raw blob. — `:1220-1231` emulated.
103. Field tags (FourCC readers):
    - PIID / PSIn / SPUI / SrcI / SrcS: instances;
    - TmrU / DelT: timers;
    - UVGu / IVDt / StDt: variables and states;
    - RMFr / RMSz / TrID: messages;
    - StEC / SpEC / …: state events;
    - Inst / EIns: instance list;
    - ArrC / ArrS / ArrE: arrays;
    - ResV / HndI / …: handles;
    - CmFD / LBSS: full frame.
    — `:1232-1239` static.
104. Processing rule (0xa0e630):
    - discard if base + delta ≤ current frame;
    - wait if Part ≥ 0 or base > current frame + 1;
    - otherwise apply with 0xa0e3c0.
    The component frame is at +0x908 (live 0→2). — `:1256-1264` live.
105. 'Inst' is an instance-id delta (0 ends). Each create = `0, 1, u16 graph, 4 absent flags`. The instance table is at manager+0x58 (0x40-byte entries); duplicate ids are rejected. — `:1270-1276` live.
106. Destroy = instance bit 1; update = 0,0. A full snapshot (CmFD greater than the current frame) makes StDt cover every state. — `:1280-1284` emulated/live.
107. **StDt form:** most state types (483 of 486) read ONE active bit. Only 691bfa55, b480a974 and eed6d952 (WeaponVolley) read has-data + active. The data reader still runs when has-data = 1 and active = 0, so keep the payload on stop. The earlier "458 types" count is corrected. — `:1455-1459,1734-1745` emulated/live.
108. StateStack needs two data bits (1,0 native). Missing bits misaligned runs 113-130 and 284-288. — `:1736,1807-1812` emulated.
109. Var ids printed in graph dumps are byte-swapped. Raw two-byte STU labels are little-endian (272e → 0x2e27). — `:1404,1848-1849` static.
110. Global (m_60DB8F99) vars are written through UVGu; value = kind(4) + payload + StEC 0. Kinds: bool 0; GUID 9 (ResV); entity 13 (IDVd); array 15 subkinds 4-7. — `:1404-1407,1545,1468` emulated.
111. Only `m_clientOnly` entries run on the client; others are server-driven. — `:1302-1303` static.
112. **Message section:** EIns bit, continuation bit, then RMFr/RMSz. Mode 0 (8-bit) triggers entry nodes; mode 1 (17-bit) calls vtable +0x38. — `:1490-1500` emulated.

### Hero select, spawn, hero change
113. Spawn-room test: config var E4C6AAB4 → component 74 byte +0xf6. — `:1334-1336` static.
114. H opens SELECT A HERO when graph 0x288b is created, component 74 field 6 = 1, and the hero is in the spawn room. With all 56 records in the 20505 list, the full 32-hero roster shows. — `:1339-1342` live.
115. Client 21616 = {u32 entity, blob, u64 game message, i16}. The blob is u32 0, u8 parameter count, GUID byte-presence mask plus bytes, then values. 0x1360 is a toggle (05a2 = !0x0986); **0x1361 {005d = hero}** is the pick. — `:1343-1356,1408` live.
116. Opening at load-in: set globals 0x7acb = 1 and 0x0986 = 1. CONTINUE (UX 65d0) sends 0x1361. Clearing both closes the UI. — `:1423-1430` live.
117. On pick: create the definition, set 0x3369 = hero and 0x0842 = 1, then send spawn snapshots. This gives first-person Tracer. — `:1409-1414` live.
118. **Re-pick condition:** avail && !0x0842 && 0x0986 && 0x3748 != 0x3369 && !0x03ac. A null GUID in 0x03ac crashes the client. — `:1712-1716` live.
119. The red no-entry icon comes from game-mode graphs 0x13c0 / 0x0c8e / 0x0c90; dropping them clears it. — `:1440-1450` live.

### Weapons, abilities, bots
120. Weapon manager 0x15 states 39/43 plus cosmetic 0x16 states 0/1 make the pistols render. The "39" on the right pistol is a fixed marking, not an ammo counter. — `:1469-1474` live.
121. **Firing:** state 8 active for 250 ms per click, with zero data `0:1,0:1,0:8,0:1,0:1,0:6`. Ammo var 0x0035; reload states 9 / 18 / 26 with a reference to cosmetic state 1. — `:1525-1530,1552-1558` live.
122. Server-side Blink/Recall teleports work as an emulation. Generic BlinkEffect states crash the client. — `:1592-1594,1683-1688` live.
123. A two-pulse native Recall "mirror" was accepted but did not rewind (stage 3 running, rewind 0 inactive). — `:1813-1818` live.
124. The shoot-to-kill loop works with server-side ray tests: health updates, destroy at 0, respawn after 3 s. Three bots are supported. — `:1621-1625,1689-1690` live.

### HUD (unresolved), client modes, party
125. 0x106b is the objectives HUD, not the hero HUD. The UI root chain stays false: graph 0x041a state 20 → 0x120b → HUD_Root. — `:1631-1645` static/emulated.
126. Client mode 6 is the normal online mode (set by 0xb456b0). The mode-5/9 path was retracted. — `:1717-1722` static.
127. Hero widgets are created when the controller sender 1291 is removed (hero ownership), but no HUD appears even with the 288a wrapper or a menu open/close. — `:1850-1864` live.
128. Practice mode 0x18 uses controller graph 13c1, whose state 42 runs 20e4. — `:1795-1799` static.
129. **Parties:**
    - 20815 (schema 446200228; player records at +0x78) creates the Nearby Players tab;
    - Invite sends 22102; Yes sends 22110;
    - 20700 is the two-member roster; 20701 a suggestion; 20703 a merge request;
    - after one leader queue request, both clients spawn as Tracer.
    — `:5-30,32-37` live.


## `docs/*.md` and `protocol/*.json`

### A. Schema field encoding (`protocol/*.json`)

1. Field kinds: 0 is a bool packed into bits LSB-first in a shared byte. 1–11 are little-endian scalars: i8, u8, u8, i16, u16, i32, u32, u64, i64, f32, f64. 12/13 are NUL-terminated strings, 14 is a nested record, 15 is a blob (u32 length + bytes). `tools/gateway_codec.py:9-10,69-88` [static/codec]
2. `count` 0 is a scalar, a positive count is a fixed array, and −1 is an array with a u32 LE item count. Any non-bit field byte-aligns the bool stream. `tools/gateway_codec.py:60-62,90-99` [codec]
3. Schema files are keyed by (schema_hash, message_id), checked against build 104319, and capped at 64 messages and 4096 fields. The codec's body bound is 16 KiB, because 24900 needs more than 4 KiB. `tools/gateway_codec.py:11-15,24-54`; BGS:1447

### B. Gateway catalogue (`protocol/gateway-104319.json`)

4. 87 protocol entries, each with schema_hash, first_id, end_id, flags and group. One is group 0 (hash 299377044, ids 1–5, flags 3); the other 86 are group 13. `protocol/gateway-104319.json:4-10` [static]
5. Flag 1 = client→server, flag 2 = server→client, and some families use flag 10. Paired families sit in adjacent hundreds (e.g. 59900 f2 / 60000 f1). Only flag-2/10 families have a client receive dispatcher (rva 0x007a8120–0x007ab630). BGS:347-350,462-463 [static]
6. The client's protocol offer is 350 bytes and lists all 86 group-13 hashes, sorted by unsigned hash. An entry number is the 1-based position in that sorted list, so never hardcode it. BGS:335-336; `docs/PARTY_PROTOCOL.md:5-6` [live]
7. The negotiation reply `00 02` completes the handshake; heartbeat `00 03` follows every 5 s. BGS:337 [live]
8. Examples of family ranges (hash in decimal): 20500–20510 lobby (4127151242, f2); 20600–20603 referral (122531096, f2); 20700–20713 party (605150806, f2); 20800–20826 profile/social (446200228, f2); 21800–21816 hello (1014903912, f1); 22100–22115 party requests (3003079262, f1); 23300–23334 queue (556297033, f2); 24000–24029 (2800040086, f1); 24900–24909 unlocks (1894097479, f2); 44100–44114 (477021970, f1); 44200–44208 (3036200034, f2). `protocol/gateway-104319.json:54-63,180-200,236-249,166-168,327-339` [static]

### C. Lobby messages before the instance (game-relevant)

9. On entry 25, the client sends 21800 (218 bytes, account record with build 104319 at +0x1bc and i32 1 at +0x1c0) and 21809 (one string). BGS:338-340,357-361 [live]
10. 20500 is the initial identity reply (handler 0x00774710). Fields: two {u64,u64} ids at +0x78/+0x88, a two-u64 record at +0x98, four strings at +0xa8/+0xd0/+0xf8/+0x120, then packed bools and i32s up to +0x156. The live reply was 81 bytes and reached ENTERING GAME…. BGS:369-385,402-404; `protocol/lobby-login-104319.json:1` [live]
11. 20504 carries a u64[] and a u8[]. It imports 16-byte key entries and sets key-context +0x40 (0→1 seen live). BGS:424,431-432,437-439 [live]
12. 20505 carries four u64[] lists (offsets 120/144/168/192): list 0 = Hero (type 0x75) set, list 1 = per-GUID resource requests (priority 2), list 2 = map (0x9f) set, list 3 stays in the handler. BGS:425; `protocol/lobby-login-104319.json` [static+live]
13. 20502 is one 104-byte nested player record at +0x78. Sending it triggers client 60002 (migration query) and a repeating 42600 (tournament poll, about every 12 s). BGS:426,440-444,473 [live]
14. 20501 is a hello error/disconnect, so never send it as success. 20506 is ignored; 20507 logs a dynamic-config update result. BGS:467 [static]
15. 24000 is the Training request: u8 at +0x78, string at +0x80, u32 at +0xa8. `protocol/lobby-login-104319.json` (24000 entry); BGS:488 [live]
16. 20802 (profile, "Low Violence") has nested records at 120/264/269/288 and an array at 304. 20807 has a bool at 136, a 392-byte record at 144 and a blob at 536. 20813 has two u64 and two f64. 22206 has two ids and a u32. `protocol/lobby-login-104319.json` [static]
17. 21801 is empty and 21802 is a bool. The client sends them in pairs from its abort routine 0x00b64af0, and they need no reply. BGS:939-943 [live]
18. With the full initial-data sequence the main menu appears (PLAY/TRAINING/HERO GALLERY/…); this was run screen8. `docs/PROJECT_STATUS.md:11` [live]
19. A hotfix state machine near rva 0x00b4cd70 re-polls every 15000 ms. In state 4 it reports "hotfix timeout" after 81.0 s; whether our runs reach that state is unchecked. BGS:478-482 [static]

### D. Queue, unlock and VS-AI schemas

20. `protocol/queue-104319.json` holds all 34 messages 23300–23333 under schema 556297033. 23311 and 23332 have no fields. `protocol/queue-104319.json:4-37`; `docs/PROJECT_STATUS.md:160` [static]
21. 23332's handler 0x006ce750 sets component-38 +0x10f9. Sending it before 20600 sets that marker, but queue records stay empty (udpapp19/20). BGS:684-686,754-758 [live]
22. Recurring queue layouts: 23307/23315/23323/23329 = {i32,i16,u8}[] + i16 record, then u32 at 152, u32 at 156, bool at 160. 23318 = string + u64. 23319 = u64 + {u64,u64}. `protocol/queue-104319.json:11,19,22-23,27,33` [static]
23. 24900 (unlock state; this document lists schema 0x70e5a247, the JSON uses 1894097479): u32 at 120.
    - +0x80: array of 160-byte per-hero entries. Each has a {u64,bool}[], a {u64,u64}[], five u64 and three u64[].
    - +0x98: array of groups. Each holds {u64,u64,u32,i16,u8,bool}[] plus a u64 key.
    - +0xb0: {u64,u64}[].

    `protocol/unlocks-104319.json:5-187`; BGS:1390-1394 [static]
24. Sending 128 default unlocks plus 34 progression records (run 203) still showed 0/0 cosmetics in Hero Gallery. BGS:1442-1446 [live]
25. VS-AI queue: 44100 (join) and 44102 (leave) carry a 48-byte record: 4×u64 then 3×i32. 44201 (queue entry) adds 3×u64, u64, f32, u8 and two bools. 44202 is queue end. `protocol/vs-ai-queue-104319.json:3,6-262` [static]

### E. Instance referral 20600 (`protocol/instance-104319.json`)

26. 20600 (schema 122531096 = 0x074dad18): bool at 120 (+0x78); a 240-byte record at 128. Record contents:
    - a {u64,u64} id, three u64;
    - i32 at +40, i16 at +44;
    - two 64-byte i8 strings at +46 and +110;
    - two 32-byte u8 arrays at +174 and +206;
    - a bool at +238.

    `protocol/instance-104319.json:5-100` [static]
27. Handler 0x00775cd0. Outer bool +0x78 = false skips a record transform. Nested +0x2e is the 64-byte host, +0x2c the port, and +0x6e a second 64-byte "IPv4:port" string (not a token). BGS:491-497 [static+live]
28. Referral field +0x28 must be 2 for the Practice Range; with 1, the client stays in the Training menu (run 180). BGS:1309-1310 [live]
29. The referral creates a component-slot-120 job (vft 0x026775f0), consumed only on instance teardown 0x007cdb60. BGS:505-507,775-777 [live+static]
30. The first 32-byte input (+0xae) is the client→server AES key; the second (+0xce) is the server→client key. BGS:553-558 [static+offline]

### F. Instance datagram envelope

31. Header: 12-byte AES-GCM tag; u32 `(version<<4)|type`; u32 send sequence; u32 next received sequence; u64 receipt bitmap; u8 flags; magic 0xad at byte 33; payload from byte 34. BGS:537-547 [offline: every packet of two runs]
32. Encryption is AES-256-GCM. Nonce = 8 zero bytes + u32 LE sequence. AAD = the 22 header bytes (offsets 12–33). The tag is 12 bytes. BGS:549-551 [offline]
33. The client's connect packet is version 2, type 0, flags 1, empty. It is resent every ~257 ms, 39 times (~10 s), then abandoned. BGS:540,560-561 [live]
34. Peer state 1 advances only when `(flags & 3) == 3`. A server reply with flags 3 moved the UI to ENTERING GAME (udpflags7). BGS:590-592,599-610 [live]
35. Receipt rules: shift the bitmap by `recv − next + 1`, set bit 0, then next = recv + 1. A gap of 64 or more clears the bitmap; older packets are rejected. The sender reads its ack from bit `next − 1 − seq`. BGS:633-638 [static+live]
36. Working server pattern (udpack8): flags 3 on the connect reply, flags 2 after, with proper next/bitmap. All 224/224 packets authenticate. BGS:646-650 [live]
37. Codec factory types: 2 plain, 0 HMAC-MD5, 1/3 AEAD, 5 BCrypt AES-GCM. BGS:552-553 [static]

### G. Bitstream primitives

38. Compact u32 (0x00db0e60), LSB-first: selector `1` + 18 bits; `0,1` + 24 bits; `0,0` + 32 bits. The first field of all 287 captured type-1 bodies uses 19 bits. BGS:672-678 [offline]
39. SFDt-style ids and values come in 8, 16 or 32-bit forms. BGS:1007,1073 [emu]
40. Statescript bitmask (0x00d530e0): up to 8 bits is sent raw. Above that, a ceil(n/8)-bit byte-presence mask is followed by each present 8-bit chunk. BGS:1021-1022 [emu]
41. Unary count (0xd53400): zero is a single 0 bit. BGS:1076-1077 [emu]
42. Sign-magnitude int (0x1b736a0): sign bit, then a width selector (`0` → 8, `10` → 16, `11` → 32 bits), then the value. BGS:1148-1149 [emu]
43. Instance-id varint (0x1b75190): `0` → 4 bits, `1,0` → 8 bits, `1,1` → 16 bits. BGS:1165-1166 [emu]

### H. Reliable records and channels

44. Client→server body: present bit, 4-bit primary count, 4-bit secondary count, then byte-aligned records `u32 seq | u8 channel | u8 msg delta | fields`. One sequence counter covers all channels and starts at 0. BGS:691-695 [offline: 138+149 bodies]
45. Primary records are delivered in order (older dropped, newer buffered); secondary records are unsequenced. BGS:706 [static]
46. Server→client body (callback 0x007ce110): compact u32 server frame, then module prefixes 0–5, then a 7-bit entity count. BGS:700-702 [static+live]
47. Module prefixes:

    | Module | Prefix |
    |---|---|
    | 0 reliable | as fact 44 |
    | 1, 2 | none |
    | 3 | compact u32 ack frame, 4 flag bits, time-present bit (+16 bits) |
    | 4 | one bit, 1 = normal |
    | 5 | bit-terminated list of 8+8-bit items; a single 0 bit when empty |

    BGS:704-710 [static+live]
48. 11 channels: 0 = 0xc77f6403 (20300–20323), 1 = 0xa7fbbd0c (21600–21634), 2 = 0x8f89e0df (25000–25019), 3 = 0x82a2f991 (27500–27502), 4 = 0x9f0a9f88 (25800–25810), 5 = 0xa9385a52 (30700–30777), 6 = 0x398e145e (42900–42907), 7 = 0x741ec7b2 (42800–42803), 8 = 0x716ad5d9 (22300–22308), 9 = 0x145ce2da (25700–25708), 10 = 0xcf043764 (42700–42701). BGS:712-727 [static]
49. 21610 (channel 1) is an i32 counter plus a u64[] of times, every 250 ms. 21627 is one f32, 15.0, about every 10 s. BGS:696-697; `protocol/instance-common-104319.json:20-49` [offline]
50. 21600 is the client join after a 20300. Fields: NUL string at 120, u32 at 160, four bools at 164–167, f32 at 168, u32 at 172, blob at 176. The native reader ignores the upper four bits of the bool byte. BGS:806-810; `protocol/instance-common-104319.json:5-17` [offline: 1003 bodies]
51. The client handles no incoming 21600-family records; it only sends them (including 21616 game messages). BGS:1252-1253 [static]
52. Native hit confirms arrive on channel 2 as message 25001 and fill the controller hit ring (run 352). `docs/PRACTICE_RANGE_PLAN.md:16` [live]

### I. World messages (channel 0, 20300 family)

53. Dispatcher 0x007d2000. 20300 is the game-server welcome, handled by 0x007d9400. BGS:729-730 [static]
54. The welcome is rejected unless +0x500 equals the client's simulation quanta (16000) and the +0x519 hot-reload flag is false. BGS:730-731 [static+live]
55. The world request is built from +0x4a8 (map-header GUID), two settings u64, a 32-byte array at +0x4b4, a 1,064-byte settings record and a u64[] at +0x520. BGS:732-733,767-768 [static]
56. Practice header GUID `0790000000000688` at +0x4a8 produces a pending world request in both world managers. BGS:742-745 [live udpapp]
57. Settings record +0x128 holds the map and +0x130 the game mode. Practice Range game mode id = 90 (type 0xC5). BGS:746-747,759-763 [live]
58. 20301 re-installs the same 1,064-byte settings record (installer 0x00587e10, from +0x78). BGS:766-767 [static]
59. Welcome +0x518 feeds team colours, together with lobby 23310. `docs/PRACTICE_RANGE_PLAN.md:12` [live run 348]
60. 20302 sets the selected entity, stored at world-owner +0x298. Diagnostic id 0xc0000001 was used to drain the create queue. BGS:881,1046-1050 [live]
61. 20315 is (string, u64, selected entity, u32, f32). It compares a string and raises one fixed local message; there is no generic "game message to entity" record. BGS:1250-1253 [static]
62. 20322 carries an i64. BGS:751 [live, no visible effect]
63. 20318 is content metadata (schema 3347014659): a 32-byte id at +0x78 and a 76-byte record at +0x98 (u32, u32, i32, two 32-byte arrays). Body is 108 bytes; +0x98 must equal 1. BGS:818-824; `protocol/instance-metadata-104319.json:5-19` [static+live]
64. Metadata cache entry flags: 1 = content present, 0x40 = in flight, 0x80 = failed. +0x80 is a retry deadline and +0x88 an attempt counter (10 attempts seen). BGS:835-842,828-829 [live]
65. In WAITING, handler 0x00b52b20 polls the map-header load state. It advances only at raw state 3; below 2 it waits, other values ≥2 are an error. BGS:783-788 [static]
66. Practice has 4 server-replicated placeables (components 0x5e/0x3a). Only after creates for all four (and a resolved 20302) does the world reach WORLD_RUNNING. BGS:877-883 [live livecreate54/55]

### J. Resource GUIDs

67. GUID type = 12-bit bit-reversed field + 1 (TankLib convention): raw bits 48–59 hold reverse(type − 1). BGS:759-760,1432 [static]
68. Prefix → type: 0x0400 → 3 (entity definition), 0x0580 → 0x1B, 0x02e0 → 0x75 (hero record), 0x0a50 → 0xA6 (skin theme), 0x0250 → 0xA5 (unlock); map headers are 0x9F and game modes 0xC5. BGS:1433-1434,760 [static]
69. Unlock records (0xA5) point to skin themes (0xA6) through a 16-byte asset reference. BGS:1435-1436 [static]

### K. Entity section and creates (module 4)

70. Entity identifier (0x00db0f70): 2 class bits give top bits `1gg`. Then a varint index: `0` + 8 bits, `10` + 16 bits, `11` + 32 bits; index 0 is invalid. BGS:859-862 [live]
71. Module chain per entity: bit 1 means module k has data, bit 0 ends the entity. Empty forms: module 1 = one 0 bit, module 2 = two 0 bits, module 3 = one 0 bit. BGS:862-866 [live]
72. Module 4 op is 2 bits: 1 create, 2 update, 3 destroy. Create and update carry a 14-bit length plus payload; destroy has none. BGS:866-870,1229-1230 [live]
73. Placeable create: presence bit, u32 placeable id, extra bit, then an update body. Placeable entities must use id class 0b100. The payload is 42 bits. BGS:872-875,902 [live]
74. Definition create: placeable bit 0, two zero selector bits, 32-bit type-3 resource index, 8-bit flags, 8-bit component count (51 bits minimum). BGS:912-916 [live livedefcreate64]
75. The definition must already be resident, because create only looks up the cache. Otherwise the client aborts with "Mirroring mismatch." / "Missing entity definition" and leaves. BGS:931-944 [live]
76. 129 type-3 definitions are resident by default, but none of the 56 hero gameplay definitions. Preloading the hero record via 20505 list 0 fixes this (150 resident). BGS:945-947,955-959 [live]
77. Soldier: 76 gameplay definition = type-3 index 0x3cf. BGS:929 [live]
78. Create flags byte (0x0078dab0, record +0x60):
    - 0x10 selects the ownership builder, which adds components 59 and 63.
    - 0x20 adds component 74.
    - 0x80 adds 86/136 (plus 69/122) on a hero with component 31.
    - 0x01 means a resource reference follows.
    - 0x41 makes that reference a skin theme.

    BGS:1043-1045,1191-1192,1211-1213 [live]
79. Skin reference: u32 (bit 31 = flag, low 31 bits = skin index), then one bit. Without a skin, non-controlled heroes are positioned but never drawn. BGS:1213-1217 [live liveskin162]
80. Tracer's first skin is `0a5000000000170f`. The training bot is hero record `02e0000000000170`, definition `04000000000014d2`, skin `0a50000000001afd`. BGS:1219-1222 [live]
81. Ownership order (liveowned93): select the controller id via 20302 first, then create it with flag 0x10 and its comp-44 actor link. This gives first person and mouse-look. BGS:1049-1054 [live]
82. Op 2 update runs the same component-state body as a create's tail. Op 3 removes the entity (bot disappears). BGS:1231-1234 [live livebot165]

### L. Component state

83. Component body (0x0078e5d0): u8 count, then per component a u8 runtime type, byte alignment (relative to the frame body start), u8 mask length, mask bytes, and field values. BGS:966-979,1232-1233 [live]
84. Mask bits are one presence bit per reflected field, LSB-first. Values are raw LE bytes, with special forms for strings, 128-bit values and compact u64. BGS:974-978 [static]
85. Reflected type kinds are CRC32 of lowercased type names (e.g. `teentityid`). BGS:980-981 [static]
86. Component 44 (controller actor link): state size 0x88, with three teEntityID fields at +0x78/+0x7c/+0x80 (hashes 0x1ab5e773, 0x4d1ffc51, 0xb1ca772f). Encoding: count 1, type 44, padding, mask length 1, mask 0x07, then three equal 4-byte ids. BGS:983-988 [live livecontrol72]
87. Component 74: s8 field 2 (+0xf2) is the team (−1 = none). Bots are hostile as team 1. BGS:1189; `docs/PRACTICE_RANGE_PLAN.md:16` [live]
88. Component 51 holds health. Bots are killed by op-2 health updates, then destroyed and re-created. Health 0 does not trigger death presentation. BGS:1243-1246; `docs/PRACTICE_RANGE_PLAN.md:16` [live]
89. Position probes: component 1 +0x440 (render), component 4 +0x160, component 24 history; component 28 holds movement history. BGS:1223-1224,1566 [static]
90. Component 59 (local command target) is never serialized; only create flag 0x10 adds it. Component 60 gates "locally owned" statescript and nothing adds it. BGS:1040-1042,1184-1187 [static]

### M. Statescript frames (module 1)

91. Sync header: `SFSF` base frame, `SFDt` delta, part-present bit, `Size` = blob length in bits. BGS:1006-1007 [emu+live]
92. Blob header: one bit, then a full-snapshot bit. A full snapshot adds a bit, `CmFD` (absolute frame, must exceed manager +0x900) and, if base ≠ 0, `LBSS` (bits to skip). BGS:1008-1011 [emu]
93. Instance list: `Inst` id delta (0 ends). Per id: 1 = destroy; 0 then 1 = create; 0 then 0 = update. BGS:1012-1014 [emu]
94. Every listed instance, created or updated, is followed by its IVDt and StDt blocks. BGS:1107-1109 [emu+live]
95. Body order: global variables, then per instance the `IVDt` sync-var mask with values, then the `StDt` mask over remote-sync nodes (over all states in a full snapshot). BGS:1015-1020 [emu]
96. Frames with base greater than current + 1 are held. Each replacement hero must restart at base 2, or frames queue forever. BGS:1507-1512 [live run 216]
97. Subgraph creates must go in the first full send. Sequence-based gating skipped them in run 250; fixed, creation consumes 656/656 bits. BGS:1522-1532 [emu+live run 252]
98. Instance-create option bits: owner entity, a flag, `PIID`/`PSIn` (parent instance/state), `SPUI` + (entity, `SrcI`, `SrcS`) triples. BGS:1110-1113 [emu+live]
99. The flag bit after the entity bit in an instance create is copied to entities the instance spawns. It does not enable prediction. BGS:1179-1182 [live]
100. Remote messages: `RMFr` frame, `RMSz` size, then per message a 17-bit id, `TrID` target instance and a node reference. BGS:1121-1123 [emu]

### N. Variable scopes and value encodings

101. Global section: present bit, then `UVGu` ids (SFDt) each with a value, ending with id 0. Values are stored under instance id 0 (entity-global). BGS:1072-1074 [emu+live]
102. Global UVGu writes update only the listed variables; the table is not cleared first. BGS:1495-1496 [emu]
103. Per-instance `IVDt` values are instance scope. BGS:1074-1075 [emu]
104. `STUConfigVarDynamic` (0x18 bytes): u16 id at +0x10, scope flag `m_60DB8F99` at +0x12. Flag set → entity-global table `[[table+8]+0x1f0]`; clear → the instance's own table. BGS:1066-1070 [static]
105. Text dumps show raw ids byte-swapped (`4208` = 0x0842). Use LE numeric ids: `272e` = 0x2e27, `d42a` = 0x2ad4, `ed73` = 0x73ed. BGS:1067-1068,1602-1604 [static]
106. Sync-var entries are 0x18 bytes with GUID `0x0d80…|id` at +8. A graph has up to 32 `m_syncVars`. BGS:1070 [static]
107. Variable GUIDs are type 0x1C (`UVGu` id ORed into a type-0x1C GUID). BGS:1015-1016 [emu]
108. A value is a 4-bit kind plus payload, then a `StEC` state-event count. Kinds: 0 Bool (1 bit); 1 int (4 bits); 4 int (32 bits); 9 GUID (`ResV`: present bit, high u16, SFDt low). BGS:1076-1078,1115 [emu]
109. Kind 15 subkinds (3 bits): 0 = u64 as four u16; 1 = u32; 2 = string; 3 = null; 4–7 = arrays. Subkind 6 = `ArrC` count, list/map bit, same-type bit, elements. Entity-id values use tag `IDVd`. BGS:1024,1078-1079,1114-1115 [emu]

### O. Expression opcodes (retail 104319)

110. Retail numbering differs from beta: 0 end; `2 n` AND-jump; `3 n` OR-jump (jumps relative to the opcode); `8 k` dynamic variable; `14 k` config variable; 30 NOT. BGS:1080-1082 [static/emu]
111. Hero-select conditions were checked by calling the client's own expression evaluator on saved dumps. BGS:1353-1355 [emu]

### P. State entries (StDt)

112. Predicate 0xf5e960 run over 486 state types: 483 use one bit (active only); 3 use two bits (has-data + active): 0x691bfa55, LogicalButton 0xb480a974, WeaponVolley 0xeed6d952. BGS:1456-1460 [emu run 204]
113. BooleanSwitch (0x76f0cf47) is one bit plus a branch bit; its state data reads no other bits. BGS:1463-1465,1448-1450 [emu]
114. Inactive two-bit entries (has-data 1, active 0) still carry state data; one-bit inactive entries carry none. BGS:1481-1487 [emu+live run 211]
115. Stack (594c2d80): two flag bits. Wait (6540c278): no data. BGS:1162-1163,1474-1476 [emu]
116. Anim (ca60e86e): optional state reference (present bit, instance varint, state index of `m_statesBitCount` width), a 3-bit field, then a flag. With no reference, nothing visible plays. BGS:1164-1170 [emu+live]
117. Subgraph runner STU_37D754C8: 6 bits = flag, 4-bit child instance, flag. BGS:1104-1106 [emu]
118. WeaponVolley data: sign-magnitude start time, flag (+int+Part), flag (+Part), then 6 bits; the zero form is 18 bits. BGS:1147-1150 [emu]
119. LogicalButton reader reads a 4-bit counter. BGS:1424-1425 [emu]
120. A message-sender state reads a compact u16 after its active bit; the zero form is one extra bit (e.g. 0x0cb6 state 3). BGS:1545-1551 [emu+live run 254]
121. Recall audit gives Stack flags 1,0 on stacks 15/28/33/35; the frame aligns at 643/643 bits, but rewind still does not start. BGS:1584-1590 [emu+live]
122. Total bit consumption alone is not enough: a missing Stack payload can still end at the right length by eating later flags. Check entry/data events too. BGS:1474-1477 [emu]

### Q. Hero select

123. Controller graph `0x058000000000288b` holds both the client hero-select UI and the server half. BGS:1062-1064 [static]
124. H in the spawn room makes the client set 0x7acb = !0x0986 and send game message `0x0240000000001360` with parameter `0x0a400000000005a2`. The server stores the value in global 0x0986 ("hero select open"). BGS:1083-1085,1339-1340 [live]
125. Picking a portrait sends `0x0240000000001361` with parameter `0x0a4000000000005d` = hero GUID (local 0x3748). The server sets 0x3369 = hero and 0x0842 = true. BGS:1085-1089 [live]
126. Initial pick (UX event 0x65d3) needs `!0x2222 && 0x6321 && 0x0842`. In-game change (0x65d0) needs available && !0x0842 && 0x0986 && 0x3748 ≠ 0x3369 && !0x03ac; on failure the client sends local 0x1402. BGS:1341-1347 [emu]
127. The server must clear 0x0842 when select closes and clear 0x6321 (switch #160, state 96), or H stops working. 0x03ac must stay unset; a null GUID there makes the client exit. BGS:1348-1351,1513-1515 [live]
128. Game-message parameter blob: u32 0, u8 count, per parameter a GUID byte-presence mask and the present bytes, then `0c 01 ff <type>` + 1 byte (type 1, bool) or 8 bytes (type 4, GUID). BGS:1091-1093 [live]
129. Remote-sync state 36 holds 0x0986 true and, through state 142, opens hero select without H. BGS:1089-1090 [static]
130. On a re-pick the server destroys the old hero and repeats create, spawn, graphs and full snapshot on the same entity index. Tracer → Soldier → McCree worked in run 218. BGS:1351-1353,1516-1518 [live]
131. Global 0x07b2 is a UI context entity, read by every hero-select presenter, but no graph writes it. BGS:1384-1386 [live run 196]

### R. Client commands and movement (module 3)

132. Encoder 0xc2ddc0. Header: 10 bits, u32 (bit length of what follows), u16, u8, u8 count. BGS:1128-1129,1141-1143 [offline+live]
133. First command in full: u32 seq, movement, s16 yaw, s16 pitch, u24 buttons, u8, extras if button bit 16 is set, u16. BGS:1129-1131 [offline]
134. Later commands are deltas: seq as one bit (+1) or varint (2/6/32 bits). Then a change bit; if set: movement, signed deltas (8/10/16 and 5/8/16), button presence + 24 bits, +0x1c presence + 8 bits. BGS:1132-1135 [offline]
135. Movement: changed bit, table bit. Table form: 0 = no movement, else 1 + 3-bit (index − 1) into a 9-entry table of axes 0/±127. Otherwise per-axis presence bits + 8-bit values. BGS:1136-1140 [offline]
136. Buttons: 0x0001 primary (held), 0x0008 ability 1 (Shift), 0x0010 ability 2 (E), 0x0040 jump, 0x0400 reload, 0x2000 press edge, 0x4000 release edge. BGS:1144-1146 [live]
137. Yaw θ = a·2π/65536 (−32768 at the practice spawn); pitch negative = up. Forward = (sin θ, 0, cos θ), right = (−cos θ, 0, sin θ). Axis 2 is forward (W = +127), axis 1 is right (D = +127). BGS:1235-1238 [live livecal166]
138. Commands arrive at 62.5/s. W moved 5.94 m/s and D 6.39 m/s (≈ 6 m/s). The spawn-room floor is at y ≈ 2.0. BGS:1238-1240,1225 [live]
139. Remote movement items apply only if their movement frame (field 2) is newer than the stored component-28 frame. BGS:1562-1567 [static+live run 269]
140. Field 2 encoding: `0` = unchanged; `1` then `1` + 4-bit delta, `01` + 10-bit, or `00` + 32-bit. frame = prior − delta; the default prior is 0. BGS:1568-1573 [static]
141. Putting the server frame in field 2 moves training bots continuously. BGS:1576-1578 [live]
142. Module-3 time field (receiver 0x007c7800) records the controlled actor at module +0x64. BGS:993-996 [live livecontrol73]
143. Statescript prediction first needs global byte image+0x2f7f840 (0 in all sessions), then component bit 60; no writer has been found. BGS:1311-1314 [static]

### S. Weapons and abilities

144. Weapon models are CosmeticEntity states (Tracer `m_entityDef` 0x0400000000000017, must be preloaded). Primary fire is the first WeaponVolley. BGS:1116-1119,1318-1320 [live]
145. Tracer weapon graph 0x16: volley = state 8; reload = Stack 18 (var 0x0021) + Anim 9 (alias `0c80000000000004`, speed var 0x0245) + Wait 26, then refill 0x0035 from 0x00c6. BGS:1152,1171-1173 [live]
146. Sync vars: ammo 0x0035, max ammo 0x00c6, weapon index 0x001c, anim speed 0x0245; Tracer's sync slots are 5/6/29/53. Weapon manager graph 0x15 states 39/43 are shared by all heroes. BGS:1322-1326 [static, 189 graphs/33 heroes]
147. Tracer tuning: 40-round magazine, 40 rounds/s, 1.0 s reload. BGS:1200-1203; `docs/PRACTICE_RANGE_PLAN.md:14` [live run 268]
148. Server-side Blink: 7.5 m along input, 3 charges at 3 s each. Recall: back 3 s, 12 s cooldown. Both are sent as position snapshot fields 16–18. BGS:1280-1284 [live]
149. Native Blink with collision and server-tracked charges works (runs 274/277). Recall lands correctly (run 341). The Tracer ultimate is not client-predicted, and forcing it exits the client. `docs/PRACTICE_RANGE_PLAN.md:15` [live]
150. Activating BlinkEffect (0x24 state 6) directly through a snapshot makes the client exit. BGS:1204-1207 [live+emu]

### T. HUD and client mode

151. Client mode values: 3 RequestExit; 5 normal default; 6 inside an online world (set by world join); 7 replay. Unloading the world restores 5. BGS:1360-1372,1418-1419 [static]
152. UI layers: 0571 HUD_Elements_Objectives, 0502 HUD_Root, 059d/05d0 HUD_Hero_Common, 05a7 HUD_Hero_Common_Animated, 00f3 HUD_Tracer, 008c HeroSelection2. BGS:1263-1265 [static]
153. Local message 0x0240000000000717 hides the HUD; the server must not cause it. `docs/PRACTICE_RANGE_PLAN.md:12`; BGS:1124 [live]
154. Controller message 0x1291 supplies both widget handles (vars 0x2e27/0x2ad4) to the hero listener. BGS:1595-1597 [emu]
155. 0x20e4 checks owner = selected controller; 0x01cf checks owner = selected hero. BGS:1556-1558 [emu+live]
156. HUD renders as of run 348 (health card, ult meter, abilities, ammo, Blink charges). It needs team filter bits, team colours (23310 + welcome +0x518), no 0x0717, and server-driven presenter states. The health value still reads 0, and there is no crosshair or hit marker. `docs/PRACTICE_RANGE_PLAN.md:12` [live]

### U. Party and social (`docs/PARTY_PROTOCOL.md`, `protocol/party-104319.json`, `protocol/nearby-players-104319.json`)

157. Server party messages use schema 605150806; client requests use 3003079262. `docs/PARTY_PROTOCOL.md:16-17` [static]
158. Party messages:
    - 20700 snapshot (160-byte record at +0x78): member list at +0x00 holding player records; status +0xe0; leader flag +0xe2; party id +0x60.
    - 20701 suggestion: proposer record +0x78, candidate record +0xe0 (104 bytes each).
    - 20703 merge request: proposer +0x78, 160-byte party snapshot +0xe0.

    `docs/PARTY_PROTOCOL.md:10-13`; `protocol/party-104319.json` [live]
159. 22102 (Invite) and 22110 (Yes) each carry a {u64,u64} identity at +0x78. `docs/PARTY_PROTOCOL.md:12,14` [live]
160. 104-byte player record: two {u64,u64} ids, three u64, two u32, NUL string at +0x40 (BattleTag). Shared by 20701, 20815 and 23302. `protocol/nearby-players-104319.json:9-82` [static]
161. 20815 (schema 446200228): array at +0x78 of player records (game id +0x00, account id +0x10, BattleTag +0x40). It populates Nearby Players, and Invite to Group then sends 22102. `docs/PARTY_PROTOCOL.md:21-26` [live run 381]
162. Two clients, one launch: the leader's queue request is forwarded, each client gets its own referral (ports 1119/1129), and both spawn as Tracer with HUD (pair-party-378/379, pair-social-381/382). Bots are not shared, the peer proxy has a fixed pose, and there is no PvP. Friends does not populate. `docs/PARTY_PROTOCOL.md:31-70` [live]

### V. Status (`docs/PROJECT_STATUS.md`, `docs/PRACTICE_RANGE_PLAN.md`)

- M1–M6 are partly done. M7–M12 (authoritative 6v6, matchmaking, stable hosting) are not implemented. `docs/PROJECT_STATUS.md:9-16`
- 13 static training bots. Normal map loading still relies on an opt-in native header preload. `docs/PRACTICE_RANGE_PLAN.md:17-18`
- Instances still sometimes fail early: the client sends 21627/21600/21610, then exits with code 1 (runs 335/335b). `docs/PRACTICE_RANGE_PLAN.md:19`

The `protocol/` folder also has `protocol/legacy_wire.h`, which is not JSON. It defines an older message enum (ChangeHero 1, PositionUpdate 2, JoinInstance 3, LeaveInstance 4, AssignPlayerID 5) with a 16384-byte cap (`protocol/legacy_wire.h:14-16`). `docs/PROJECT_STATUS.md` calls the BSON relay a separate historical implementation, not final-retail gameplay (`docs/PROJECT_STATUS.md:182-183`), so it is not part of the 104319 protocol.
