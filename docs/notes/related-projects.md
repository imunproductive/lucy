# Findings from related projects

What other 1.74 projects and research found that this repository can use: server gameplay first, then the lobby, client internals and game rules. The project table and licences are in `references.md`.

Each fact names its source and how it was established:
- **live**: seen by that project with the 104319 client;
- **emulated**: run on a memory dump of the client, not live;
- **static**: read from the client code or data, not run;
- **public**: from the wiki or patch notes, not from the client.

Nothing here is verified by this repository unless it says so. ow1-preservation's full fact list is in `ow1-preservation.md`. ow1-preservation's game code is private; its public text is summaries written by AI agents, so treat it as leads. ow1-preservation and Prometheus are AGPL: facts only, no code or text. AipNooBest, LizRollin and Boi-027 are MIT.

## Match flow and game modes

### The Assault flow on Temple of Anubis (ow1-preservation, static and emulated)

Source: `HANDOFF.md:2119-2132`; the full trace is private.

- Mode graph `0F4A` runs on the server-created game mode entity (definition `0x04000000000016B2`, component 114). `STU_A3DEF368` resolves in the client to the first world entity with component 114.
- The phase variable is `003B`:

  | Value | Phase | Time |
  | --- | --- | --- |
  | 0 | waiting | |
  | 1 | assemble | 30 s (later 25 s) |
  | 2 | setup | 45 s, first point only |
  | 3 | round | +240 s for point A, +180 s for B |
  | 4 | round end | 11.6 s |
  | 5 | match end | message `0695` carries the winner |

- Phase messages `0700`, `0692`, `0701`, `0694` and `0695` drive graphs `1FEB` (objective HUD, client-run), `288B` (`0701` shows "SETUP:"), `0C90` and `0CBB`. A client-only state sends `071D` (ATTACK/DEFEND).
- The match clock is `A3DEF368` variable `0517`, read by `288B`. This repository already sends `0517` as the assemble countdown.
- Anubis objectives: A is entity index 698, B is 723 (definition `1C22`, graphs `0E8E` and `0F4C`). Doors use `0D32`. Spawn rooms are found by the tags `0x341`, `0x3CC` and `0x310`.

### The mode entity alone (ow1-preservation, live)

Source: `HANDOFF.md:2150-2165`.

- Creating only the mode entity `16B2` with a `0F4A` instance (mode `0x0230000000000014`) makes the client run its own mode logic: the hero select header reads ATTACK (the team compared with `2A2B`, default 0), and the top-centre mode HUD appears.
- The HUD reads "0.0 | UNSPECIFIED" because the objective name variable `1DDB` is unset.
- Planned and untested: states 52, 58 and 12 of the mode graph, `0CBB` state 178, globals `2A2B = 1` and `2A2C = 0`.

### The Practice Range round machine (AipNooBest, lead only)

Source: `860bab45` `ow174/game/heroselect.py` (`MODE_GRAPH`, `mode_chunk`), `match.py`.

- `13C0` (20 owner states, 3 presence variables) is the Practice Range round machine; quick play uses `0F4A`.
- Its children: `0CBB` (announce listener, under state 5), `0C8F` (re-begin listener, under 16), `0CB6` (widget tree, parent index 21), round timer `0BD6` (under 14).
- The author's notes say fire-while-reloading and the round countdown read the mode graph, and without it the client stays before the round. No live result is reported.
- The mode entity's create must carry only component 114's two fields; the longer field list crashed the client.
- This repository sends only `0CB6` on the mode entity.

### Practice vs AI uses the normal modes (ow1-preservation, live)

Source: `HANDOFF.md:2007`, `:2048-2056`, `:2090-2101`.

- The mode catalogue (132 map headers, 25 modes) has no vs-AI mode. vs AI is a normal mode with server bots.
- Training → Practice vs AI → Easy sends 44100 with queue card `0x0630000000000070`; the vs-AI cards are `…70` to `…72`. The server answers 44201, 44202 and 20600; the client loads Anubis and shows "DEFEND / VERSUS AI EASY" in hero select.
- The "VERSUS AI / EASY" loading label comes from the queue entry still being open; once 44202 closes it the label goes. Their conclusion: retail sends the game type from the game server.

### Loading Temple of Anubis (ow1-preservation, live)

Source: `HANDOFF.md:1995-2006`, `:2029-2032`.

- Map header `0x079000000000005B`, map `0x080000000000005B`, Skirmish mode `0x0230000000000010`.
- The server placeables the client waits for must be read from the live client: 29 keys with no dependencies. The Practice Range catalogue keys were 5 too low there, and adding 5 aborted the load.
- Bodies stand at about y 1.75. Spawn clusters: (14, 0.8, −30), (−13, 4.3, 139), (−68, 2.5, 19).
- The Practice Range controller graphs `13C1` and `20E4` leave the hero select overlay up on other maps.

## AI bots and navigation (ow1-preservation)

### Navigation data ships in the client (static, then live)

Source: `HANDOFF.md:2109-2118`; the reader is private.

- Map chunk `0x12` (`0DD00012_<map>`, present for all 132 maps) holds two STU documents.
- Document 1, root `STU_5AE64839`, is the AI tactical volume set: an AABB tree and volumes. Each volume has a floor point, a signed 32-bit neighbour list and a walkable voxel mask. Voxels are 1 × 1.4 × 1 m; bit index = z + Sz·(x + Sx·y). The formula fits 116 of 116 and 60 of 63 volumes. Neighbour links are symmetric and between adjacent voxels.
- Document 0, `STU_3BC32A6E` (AI cell data), is not decoded.
- Anubis: 117 nodes, a main component of 85, 113 portal edges, capture boxes for A and B. Attacker spawn to A is 125 m, to B 206 m. Practice Range: 67 nodes.
- A node's y is the floor rounded up to 0.35 m; a body stands at about node y + 0.8.
- This repository has the STU types (`crates/stu/data/types.txt`: `m_volumes`, `m_chokes`, `m_cellData`) and reads map chunks from CASC, but nothing reads this chunk yet.

### AI heroes as server-driven players (live)

Source: `HANDOFF.md:2007-2056`, `:2134-2146`.

- Each AI is a virtual session (team, hero, controller, weapon) replicated through the same remote-player path as a human. The server picks the nearest live enemy, fires bursts and applies damage per second × accuracy. Players kill AIs through the normal hit test. Their AI does not run statescript: the server makes the bot hero's frames.
- Six AIs walk the navigation graph from the attacker spawn to point A (18 waypoints). Each gets its own cell near the goal (6 m spread), they leave 1.5 s apart, hold at +0, +3 or +6 m, and a side slot is clamped to walkable voxel columns. Reported: 6 player kills and 19 AI kills in one run.
- Difficulty scales damage by 0.25, 0.5 and 1.0 (Easy, Medium, Hard). Base damage per second per hero comes from published values, times accuracy and range falloff.
- Per-hero bots use definition and record pairs plus a weapon preload; Soldier: 76 is `03CF`/`006E`.
- Bug they hit: the player lost the HUD and pistols when damage arrived before the hero's first full statescript frames had built its HUD and weapon subgraphs. Their fix: no damage or targeting until the victim has had 40 full hero frames. In other runs (434, 436, 438) the HUD and pistols were missing with identical server counters, unexplained.

## Death, killcam and health (ow1-preservation)

- **Killcam record 25011** (emulated; `HANDOFF.md:1933-1942`): not the kill feed. Handler `0x6C3940` stores one "killed by" record in global root slot 12, read by the Kill_Cam_Overlay presenter. Fields: `+0x78` killcam entity, `+0x7C` killer controller (equal to the local player hides the attacker), `+0x80` attacker name, `+0xB0` killer hero GUID, `+0xD8` kind (5 hides). 25012 and 25013 are slot 13 highlight and Play of the Game records; don't send them for kills.
- **Hero dead flag** (live, two clients; `HANDOFF.md:1907-1931`): the hero's global statescript variable `0x01DE = true` at the kill, cleared at respawn, gives the native death view: card 0/200, grey segments, red vignette, weapons removed, camera out of first person. Mirrored into the remote copy, other players see that player's weapons drop. This repository uses component 51 and movement flag `0x2000` instead.
- **HUD health number** (live but contradictory; `HANDOFF.md:1885-1906`, `:1531-1535`): they read the card's number as `0x06C9` current and `0x06C4` maximum, the reverse of this repository. Checked here with our evaluator on the card's binding (`01CF` node 76, variables `01DE`, `8398`, `06C9`, `06C4`): it shows `06C4` while `06C4 ≤ 06C9`, otherwise `06C9 − 1`, and 0 when dead or `8398` is set. Graph `0153` treats `06C4 ≤ 0` as dead and `0109` heals by `06C9`. So `06C4` is current and `06C9` the maximum, as we send; their reading is wrong.
- **Alive variable** (AipNooBest `860bab45` `loadout.py`): `ALIVE_VAR 0x094C = True` on bodies. This repository doesn't set it.

## Weapons and HUD

### AipNooBest's findings (`860bab45`, live unless marked)

Sources: `docs/HERO-WEAPONS.md`, `docs/HUD-VARIABLES.md`, `ow174/game/loadout.py`, `heroselect.py`, `statescript.py`.

Ported here: gun definitions in the login preload (`GraphStore::weapon_cosmetics`). The world UI entity mount was ported and then replaced (see "`041A` and the retail HUD path").

- **Guns.** A weapon script's `CosmeticEntity` builds nothing unless its `.003` definition is in the 20505 preload list (`+0x90`). The picture needs the manager graph `0x15` with its states at positions 29 and 33 (classes `CD46AF93`, `BD02E168`, also checked on a 1.23 server) and every weapon script instance with its cosmetic states. Our VM run of Soldier: 76 lights 29, 32 and 33 and the cosmetic.
- **Per-hero weapon table** (data): for each hero the weapon script, cosmetic slots and definitions, volley slots (the first fires), Anim, UXPresenter and Ability slots. Torbjörn has two scripts (`1683`, `168C`) and lighting both fuses the rivet gun and hammer; Mercy has `6B` and `6D`; Zenyatta has 11 cosmetics; Brigitte, Mercy's staff, Reinhardt and Winston have no volley. Bastion, Doomfist, Moira, Wrecking Ball and Echo build no cosmetic.
- **Traps.** Anim and UXPresenter states that are active at spawn play at once (Soldier spawns mid-visor; Symmetra's teleporter placement screen takes her view); they must enter as transitions. The expiry-tick class `691BFA55` dies without type-3 preloads. Turning on every one-bit state crashes at spawn: a begin action reads a preload the server doesn't send.
- **Gun counters.** A gun script syncs `.001C` (non-zero shows the weapon; shipped as Int 50), `.0035` (current ammo), `.00C6` (magazine) and `.003D` (alternate fire charge, e.g. Helix Rockets, Soundwave). Light the presenters that bind `.0035` or `.003D`; one that binds only `.001F`/`.281C` draws a spare gun card with ∞ (Soldier's "two guns"). Soldier `0254`: counter 66, bare 64, charge 65; our VM lights 65 and 66. Baptiste, Hanzo, Mei, Moira and Sombra have no presenter in the main gun script.
- **Ability bar.** The ability icons are presenters in the ability scripts, not the gun script (Soldier: `0255` slot 28, `0257` slot 16, `0259` slot 32). The icon under the ult ring is still open.
- **Hero HUD.** The body graph `004B` is the hero HUD dispatcher; its launcher state 3 starts `01CF`. The client loads a graph on demand only under a launcher whose override names it. Of `01CF`'s 41 networked states, light the gate (40) and the presenter (28, remote position 13). Entity variables: `.0735` hero asset, `.076C` Float 0, and `.2E27 .0757 .0406 .45C2 .2243 .2AD4 .73ED .001F` true. `.0028` must be non-zero: both HUD gates watch it. Of single-variable rewrites only `.0A14` did anything (0 shows the damage boost glyph, non-zero the heal glyph); `.1B84` is not the ult percentage.
- **Health and respawn layer `01CB`.** 96 states, one presenter, one gate (state 95) watching 13 variables. Health is `.065D` (current) and `.063D` (max); `.0837` is the value the ult ring eases toward. A presenter bound to a logical button paints only while that button's state is on. With its presenter active the background paints a full-width top strip, so the author dropped it; it should be presented on death by the round machine.
- **HUD widgets.** Ammo widget `20E7`: the number is its presence variable `0x35`, then `0xC6` and `0x1C`; the number must stay an integer (a float crashed the client). Hero panel `183C`: one networked gate, nine entity variables. Lighting the panel launcher or the widget's own `.0C47` listener from an owner frame crashes the client.
- **Result.** The author reports the ult ring with its percentage and two weapon-slot icons; ammo digits, health number and bar, ability row and portrait do not draw. Earlier "30/30" readings were a duplicated gun render.
- **Menu hero.** The hero HUD layer starts only if the 36600 login message named a menu hero (`MENU_HERO_KEY`); a "random" lobby hero leaves it dead.
- **Variables.** Presence and entity variables are read only when an instance initializes; to change one, destroy and recreate the instance under a new id. A rewrite must keep the variable's kind. The entity variable block uses 32-bit ids, so 48-bit widget sync ids can't go there; a widget instance is keyed by `id & 0xFFFFFFFF`.
- **Solo queue.** Commit `0588fce` skips the role screen for a solo player; not sourced to retail.
- **Debug chat.** `.hudvar`, `.hudslot`, `.mount`, `.hudroot`, `.mode`, `.pres`, `.banner`, `.hud`, `.pick` rewrite HUD state live in a match: a cheap way to run client experiments.

### The working hero HUD (ow1-preservation, live, runs 332–387)

Source: `HANDOFF.md:1331-1762`. They got the full original hero HUD for Tracer and Soldier: 76 (portrait, name, health, crosshair, ultimate meter, ability icons, ammo), in one client and in two clients at once, without `041A`. What it took, in the order they found it:

1. **Team filter bits** (`:1333-1416`). The HUD widgets existed but were fully transparent: their colour properties (`2EAB`, `2EAC`, `2EAD`, `4498`, `4499`) came out with alpha 0 because the hero's team was −1. Component 26's u64 field is read with a flagged encoding (one presence-mask byte, then only the non-zero little-endian bytes; team 0 is `0D 01 84 10`); raw 8-byte values crashed the client or were ignored. Sending component 26 = `0x10840001` for players (team 0) and `0x11040001` for bots (team 1), in creates or as updates, gives the hero a team. Team bits also stopped teleports and spawns from launching the hero onto the spawn room roof: a hero without a team was being pushed out by the enemy spawn barrier (`:1417-1425`).
2. **Team colours** (`:1389-1398`). Lobby message 23310 (`+0x138`: a list of 0x48-byte {GUID at `+0x10`, team u8 at `+0x18`}) fills the session team colour table, used only when the world job byte `+0x6E8` is set from the welcome's `+0x518`. They sent colours `0x0B300000000000F6`/`F7`.
3. **Never send `0x0717`** (`:1434-1449`). `0x0717` is the game's hide-HUD message: graph `0CC9` node 6 stacks UI variable `0x0734` (HUD_Root shown) false on it, and `20E4` and `01CF` set `0x2AF6`, which hides the hero widgets. Their earlier HUD attempts had been sending it by activating `0CB6` state 3 (a client-only sender). Without it, HUD_Root and the widgets are visible.
4. **Light the presenters behind server-driven switches** (`:1472-1501`). The HUD content presenters sit under BooleanSwitch false branches the server must enter: ammo (`0x16` switch 2 true, 29 false → presenter 31), Blink icon (`0x1A` 21 → 32), Recall icon (`0x24` 21 → 32), ultimate (`0x34` 10 → 13), health card (`01CF` 15 → 66 → 139, then a client switch on sync slot 15 = `0x070F` > 0), crosshair (`0x15` 11 → 23 → 42). Run 348 showed the health card, ultimate meter, Blink and Recall icons and ammo 40/40 that drains and reloads.
5. **Health and ultimate values** (`:1531-1545`). The card's current health is an expression over hero globals `0x01DE` (dead), `0x8398` (must stay false), `0x06C9` (current) and `0x06C4` (max); with `06C4 = 06C9 = 200` it reads 200/200. The ultimate meter is charge `0x0156` / cost `0x0152`; with the cost unset it shows "ready at 0%". Q with a full meter is predicted by the client only once the cost is set (`:1557-1563`).
6. **Crosshair and hero name** (`:1594-1600`). Hero global `0x0EBC = true` shows the reticle (its stack's is-top bit alone did not). Controller component 75 field 0 = the picked hero GUID (flagged u64, `0x02E0…` type) makes the card show the hero's name. Rule they derived: Stack states whose value matters need is-top 1 (`:1538`).
7. **Portrait** (`:1665-1671`). Hero sync variable `0x070F` (`01CF` slot 15) is the portrait texture: Tracer's is `0x0C0000000000128E`, the hero record's `m_8203BFE1` entry for `0x0D80…40C9`.
8. **Per-hero states** (`:1708-1728`). A per-hero table (portrait textures, component 75 hero GUIDs, `01CF` under `004B` state 3, presenter chains) for all 32 heroes; live for Soldier: 76 with states `2:3; 5:17,31; 6:7,16; 7:15,32; 10:11,23,42; 11:0,9,40,67,69,70; 12:15,66,139,138,67`, subgraph `01CF` in instance 12 under instance 2 state 3, component 75 `0x02E000000000006E`: portrait, name, 200/200, crosshair, ultimate, Sprint/Biotic Field/Helix icons, rifle and ammo. Their ammo maximum was still a generic 40.
9. **Hero change** (`:1757-1762`). A re-pick must restart the CmFD bookkeeping for the new body; a stale CmFD left Soldier without weapon or HUD.

Also from this stretch:
- **Muzzles** (`:1569-1593`): the client creates every `m_childEntityData` child itself when the definitions are preloaded; the server must not create them. A volley fires from the entity named by weapon instance variables `0x0031`/`0x0032`, which hold CosmeticEntity node handles {1, instance, state, 0}; unset, shots leave from the hero's origin. Tracer: `s49 = handle(10, 1)`, `s50 = handle(10, 0)`; Soldier: `s50 = handle(11, 11)`.
- **Recall** (`:1426-1433`): with team bits, the server replays the last 3 s of the client's position as generation teleports; works live. Snapshot teleports are swept with collision unless movement field 3 carries a new generation, which snaps (`HANDOFF.md:319-336`).
- **Tracer's Pulse Bomb** (`:1639-1697`): the Ability state (`0x34` state 9) is in the false branch of switch 11; its state data is 10 bits (`0:8, 0:1, 1:1, 0:32, 0:1, 0:1, 0:8`); a forced Ability state without it crashed the client. Confirming the Ability for 0.6 s after Q gives the holster, throw and the projectile.
- **Combat feedback** (`:1506-1530`): module 2 is projectile replication. Reliable channel 2 family 25000: 25001 hit confirm {target s32, amount s32, flags u8, frame offset s32} fills the controller's hit ring; 25000 is the combat log. Health 0 alone never starts a death; bots stood at 0 HP. Component 51's mask must be [1] (pool 0, 20 bytes); a 9-bit mask over-read nine pools.
- **Buttons** (`:1487-1492`): Q `0x20`, V `0x800`, F `0x4`, Ctrl `0x80`, Space `0x40`, Shift `0x8`, E `0x10`, R `0x400`, fire `0x1`. Logical buttons `0x23` Shift, `0x24` E, `0x25` Q, `0x1F` melee.
- **Two clients** (`:1546-1720`, `:1748-1756`): two original clients can share one AppContainer profile; each gets its own lobby port and instance session; the other player's hero is created as a proxy (flags `0x41` plus skin, components 26 and 51) and moved by movement snapshots. Remote heroes draw about 0.93 m above the snapshot y.

### `041A` and the retail HUD path

ow1-preservation (`HANDOFF.md:449-471`, emulated on a memory dump) found that `041A`'s gate (node 46) is an OR of five terms, all false in a normal game, and that node 52 is a config byte set only by `tank_ShowMode` and `tank_TournamentMode`: the `041A` → `120B` `HUD_Root` branch is for show and tournament observers. The play HUD comes from the hero and controller presenters (`01CF`, `20E4`) and the `HUD_Root` the client registers at startup (UI manager `+0x58`, created in `0xAFB740`). They also read that the client has exactly one UI manager, which owns its UI entity (`:1136-1139`). AipNooBest instead forces the gate state from an owner frame on a server-created UI entity.

Both drew a HUD live. This repository first ported AipNooBest's mount (commit `5bbbdb0`); a client test showed the portrait, health, ultimate ring, ability icons, ammo and crosshair (`behavior.md`). It was then replaced by ow1-preservation's path, which is what retail does for a player: the native `HUD_Root`, team bits, the team colour table (lobby 23310 with `0CE` colours `F6`/`F7` and welcome 20300 `+0x518`), and nothing sending `0x0717`. Our VM already supplies the rest of their list: the portrait (`0x070F`), ultimate cost and charge (`0x0152`, `0x0156`), health (above), and the crosshair variable `0x0EBC`, which goes out bound to its stack state (`0015` state 23).

Other HUD facts from the same section:
- Every UI widget uses context id `0x1800`. Top-level widgets (`HUD_Root` `0502`, hero select `008C`, hero HUD `059D`/`05A7`/`05D0`) share one top-level array; the HUD widgets are top-level, not children of `HUD_Root`.
- Startup layers by serial: 1 `0273`, 2 GameLayer `029B`, 3 HUD_Root `0502`, 4 MenuLayer `029C`, 5 `0503`, 6 DialogLayer `0514`, 8 System_Clock `0513`.
- A shown widget has `+0x60/+0x64` = −1/0, a hidden one `0x21`/1. Their HUD widgets were nominally shown but drew nothing; the suspect was the data binding (local player and hero context).
- `m_45017CEA` is a presenter's published widget handle variable. A type-7 handle is {state index, instance id}: `20E4` instance 4 has `0x2AD4` = state 9 and `0x2E27` = state 10. `01CF` presenters 155 and 235 are gated by NOT `0x2AF6`.
- Filter bits: the local session builder sets component 25 `+0x58` to `0x10040001 | (0x800000 << team)`; their controller and hero had 0 there because filter bits are never sent. Recall's gate reads bit 10 of it, and hostility depends on it.
- Tracer's muzzle: graph `0x16` CosmeticEntity states 0 and 1 look up child pistol entities by identifiers `0x1B9B`/`0x1B9C` on hardpoints `0x0A`/`0x0B` and publish them in synced variables `0x31`/`0x32`; the volley fires from hardpoint 6 of those. No child entities were created in their runs, so Tracer shot from the stomach.

## Statescript on the wire (AipNooBest `docs/STATESCRIPT.md`, live)

Client addresses: chunk reader `0x7FF7898A6450`, frame reader `0x7FF78991E3C0`.

- A chunk whose last frame is at or below the highest applied is dropped, so an unchanged chunk can be resent safely. `first 0` is a full frame: the client resets and applies the whole picture, clearing network-set variables and destroying the network instances it doesn't list.
- An owner frame applies only if its CmFD is larger than the last applied one (unsigned, so CmFD 0 never applies) and larger than the graph manager's tick stamp (`+0x900`), which is set to the client's own tick when the entity's graph-info frame arrives. Their rule: CmFD = max(previous + 1, newest command frame + 8). Frames below the tick leave the client waiting for updates that never come: the screen freezes without a log line. This repository uses max(previous + 1, acknowledged command frame), and its controller frames work live.
- LBSS, the varint after CmFD in a delta or in any frame wrapped at a non-zero base, is the body's bit length; the client skips that many bits when it already applied the frame.
- A fresh hero body takes two records: the create carries a graph-info frame (descriptors only, CmFD 0), then the activating full frame at base 0. Listing the instances in both creates them twice (the client dies at null+0x1F8 when the activation wait expires); a full frame wrapped at base 2 or higher is ignored. After a hero pick the body is a new entity; a destroyed id stays ignored for 30 s.
- Hero entity variable `0x03AC` must carry the hero's asset: without it no StateHero node starts; a null GUID makes the client exit.
- State payloads in owner frames: most states are the bare active bit; BooleanSwitch adds one branch bit; Stack two bits; GameMessageEntry a compact u16 (zero is one bit); LogicalButton, WeaponVolley and `STU_691BFA55` have a second bit before the entry, then the payload. A bare active bit for a class with data shifts the rest of the frame.
- Hero select: the open and closed controller frames must carry the same instances (only the screen's presence bits change), or the client keeps the post-pick presentation and never lets the player in. Never write the screen's initial-pick variable `0x0842`: true kills the grid, false freezes the presentation.

## ow1-preservation's development log (`DEVLOG.md`, 2026-10-14 to 10-16)

Game-side entries, in order. "Live" unless marked.

- **Movement records** (static and emulated, `:1762-1778`): module 3 carries per-entity items (presence bit, 12-bit length counting itself, payload); the decoder needs components 28 and 43 and applies to component 28. An all-default movement item is presence 1, length 48, 36 zero bits.
- **Statescript records** (`:1780-1823`): component 47 is the statescript component; module 1 carries its sync records (base frame, frame delta, optional part, size in bits, frame). An empty frame is 10 bits. The instance list's `Inst` field is an id delta (id += delta, 0 ends the list); every create using id 1 was rejected as a duplicate (`0xD55D30`). Creating `13C0`, `0C8E` and `0C90` on the controller with ids 1–3 applied and showed a red "prohibited" HUD icon.
- **Frames** (emulated, `:1825-1833`): instance list bit 1 destroys an instance (updates are `0,0`); per-instance `IVDt` variable masks and `StDt` state masks; full snapshot header `CmFD`/`LBSS`. Remote messages do not fire GameMessageEntry states. A full snapshot with the hero variables activates `0C90` state 0 (`StateHero`).
- **Local control** (`:1835-1844`): movement commands and first-person code need component 59, added only when a create with flag `0x10` arrives for the entity that is already the 20302 selection. Then mouse look works and the client streams commands.
- **Commands** (`:1848-1852`): after the 9-bit module-0 header, `[1][u32 bit length]`, then u16, u8, u8 count, u32 oldest sequence and delta-coded commands (writer `0xC2DDC0`). The server's module-3 compact field acknowledges up to value − 1 (`0x7CADD0`); acknowledging the newest cut resent commands from 32 to 2–3 per packet.
- **Health** (`:1853-1856`): without per-tick "unchanged" hero snapshots the hero moves under client prediction. Component 51 holds 3 × 16 pool structs {u32, f32 value, f32 max, enum, 4 × u8}; a first pool of 200/200 removes the critical-health vignette.
- **Weapons** (`:1857-1861`): component 49 has no replicated state; weapons are statescript. Activating `0254` states 11/68/69/70 in a full snapshot without the gun definition preloaded made the client exit; state 11 is the CosmeticEntity for `0x04000000000004BF`, and the 20505 list accepts type-3 definitions.
- **Hero select messages** (`:1862-1890`): candidates 21605 {u64, u64, i32} and 21607 {u64, bool, bool}. Creating `288B` (from `0C90` state 119 via `288A`) gives H → "You must be in a spawn room to change heroes". The spawn room check reads component 74's replicated field 6; with ownership flags `0x30` and that field set, H opens SELECT A HERO with the full roster. CONTINUE sends 21616, game message `0x1360` (one parameter, variable `CB7A`); the pick is `0x1361` with the hero GUID.
- **Teleports** (emulated then live, `:1872-1877`): a module-3 movement item with only the position fields (140 bits) moves the hero. Map entities with component 1 have their position at component `+0x90`.
- **Reload** (`:1905-1912`): R plays the first-person reload when the server turns on the weapon script's reload stack, wait and animation states, with the animation referencing the pistol cosmetic state; without that reference nothing plays.
- **Visible bodies** (`:1919-1923`): a non-player hero or bot renders only if its create carries the skin: flags bit `0x40` (with `0x01`) carries a skin theme reference.
- **Kills** (`:1930-1933`): the server integrates the client's movement inputs and aim to hit-test training bots; at zero health the bot is removed and returns 3 s later.
- **Client-only send** (`:1942-1944`): activating a client-only "send message" node from server state makes the client send a local game message; this delivered `0x0717` to the HUD graphs.
- **Blink and Recall as teleports** (`:1945-1947`): Blink 7.5 m and Recall 3 s back, done by the server from its own position estimate; without collision they can end inside geometry.
- **Other heroes** (`:1953-1956`): weapon setups generated from each hero's graphs; Soldier: 76 fires with tracers; McCree's revolver vanished on fire until two encoding errors were fixed (one-bit types misclassified, and a stopping state omitted data the client reads) (`:1971-1976`).
- **Client mode** (`:1957-1960`, corrected `:1979-1981`): the in-game UI was thought to need client mode 5 or 9; mode 6 is the normal in-game mode, and the mode-5/9 path does not give the hero HUD.
- **Hero changes** (`:1962-1969`, `:1983-1990`): switching failed while the "initial pick" flag stayed set after spawning and while a hero variable used by only one mode graph was set. The replacement hero's script updates must be numbered for the new body, or the client waits forever; a hero select flag left set disables H.
- **HUD ownership** (`:1992-2000`, `:2061-2073`): the HUD's extra graphs must be created before their states are activated. The hero HUD needs `20E4` on the controller and `01CF` on the hero; `01CF` waits for message 1291 from the controller. While the controller's 1291 sender exists the hero listener selects controller ownership (Stack 180); without it, hero ownership (Stack 179) starts the hero presenters and widgets are created, yet no HUD draws.
- **Payload sizes** (`:2006-2010`, `:2021-2029`): `0CB6` state 3 (sender) has one compact-u16 zero bit after its active bit. An active Stack must carry its two bits; the client writes `1,0` for Recall's Stacks 15/28/33/35 where they had sent `0,0`.
- **Recall** (`:2031-2035`): mirror pulses were accepted but did not rewind; stage 3 ran and rewind stage 0 stayed inactive with Ability flag `+0x270` = 0.
- **Hero HUD parent** (`:2043-2048`): the hero HUD's parent is `004B` state 3 (an authored state index), not runtime node 7.
- **Practice wrapper** (`:2075-2077`): `288A` under `13C1` state 43 starts four child graphs.

## Statescript names and open questions

- **`STU_E4C6AAB4` is "is in spawn room"** (ow1-preservation `CLAUDE.md:1334-1338`, `:1357-1359`; live). The evaluator (vtable `0x26643E8`, evaluate `0xCF1F40`) reads component 74's byte at `+0xF6`, the replicated state field 6, the spawn room flag. Used in `288B` before SetVar #1207 and message `0x13EF`. It is one of the hero select gaps.
- **`STU_E72F6348`** is a generic entity-reference condition (evaluate `0xCE4A60`), not the spawn room test (`CLAUDE.md:1322-1324`; static).
- **`STU_54FEB21C`** (in Tracer's Recall graph) is a container of states that ends only when instance variable `0x018B` becomes true, and no graph writes `0x018B`, so the server must end it (`HANDOFF.md:397-401`, `:678-679`, `:808-810`; emulated). We treat it as "no effect" in melee graph `0043`; a container that never ends keeps its children running.
- **LizRollin's `data/statescript_names_174.json`** (`9f8f3bcb`, MIT; names only, no addresses). `tools/ssq.py` needs it. Besides about 60 aliases it holds which fields hold variables for about 110 classes, logical button ids, UI graph names, the 25 modes with their script, controller and body graphs per team, and variable names. Aliases not found elsewhere:

  | Class | Name |
  | --- | --- |
  | `STU_0A6BE10E` | FilterArray (melee `0043` node 63) |
  | `STU_0C3A5D53` | ForProbe (hero select gap) |
  | `STU_B5C87F3D` | SoundImportanceThreat (melee `0043` node 24; audio, likely inert on the server) |
  | `STU_431F3FCC` | ApplyAura |
  | `STU_441B50E4` | TrackStatTime |
  | `STU_3716E1D4` | AimSpeed |
  | `STU_96333BBE` | CameraLookAt |
  | `STU_AB09B113` | Reloader |
  | `STU_7B9E8909` | Bridge |
  | `STU_0E913A0B` | HeroSelect |
  | `STU_571CC72E` | EntryClientOnlySharedVars |
  | `STU_CEBDAF17` | ActionTrackStat |
  | `STU_100EF8C2` | ActionSendClientOnlyGameMessage |
  | `STU_C43F9944` | ActionSendClientGlobalGameMessage |
  | `STU_BD02E168` | ClientOnlyController |
  | `STU_CD46AF93` | ForCodeSynced |
  | `STU_9D7BF987` | ClientOnlySharedVars |
  | `STU_2F07848F` | ScaleTime (StEC) |

  Logical buttons: 11 jump, 12 crouch, 14 reload, 15–17 equip weapon 1–3, 20 fire, 21/22 next/previous weapon, 29 secondary fire, 30 interact, 31 quick weapon, 35–37 abilities 1–3. Modes: Practice Range `13C0` with controllers `13C1` (blue) and `0C90` (red); Assault, Escort and Hybrid `0F4A`; Control `0EC2`; Deathmatch `0F73`, `0F74`, `1058`.
- **Prometheus (beta 0.8, client trials):**
  - Loading a weapon or manager graph through the component's vtable slot `+0x60` instead of `+0x58` sets execution flag `0x8` on the instance; with it Lúcio's and Genji's weapons fired, Mercy's and Symmetra's did not. The author guesses the server sets it. A lead if some hero's weapon graph stalls on the client.
  - Weapon variables: instance variable `0x1C` is the weapon id (from 1 in `m_weapons` order in the beta; we number from 0); entity variable `0x1D` is a dictionary of valid weapon ids (key = value = id, plus 0; we send an array of booleans); entity variable `0xFD` = 1 "disables the spectator graph" (we send owner health there). Check numbering and the dictionary form on 104319.
  - Client object model: primitive type tags (byte 1, int 2, float 3, int64 4, string 5, vec3 6, intvec4 7, instance id 8, f16vec4 9, entity 0xB); notification kinds 0–8 (lifetime, timer, evaluate exits, traverse from, set var, set var array, stop instance, logical button, attribute dependency changed); per-component stack table of 7 priority lists; `entity_admin_scope` 0 owner entity, 1 game admin, 2 lobby admin, 3 embedded AI, 4 lobby map admin; variable selector GUIDs `0x0D8…03`–`06` (instance, entity, child, this), `…08`–`0A` (vec3 x/y/z), `…0B` (dictionary size); keyed array items are {key, value} pairs, and expression opcodes 32–34 and 37–39 look up by value, key or index.
  - The PvP game component (beta component 0x24) holds the current game state (a `0D8` asset), overtime left, a team id, the current round and a counter: look for the same in the 104319 mode component when modelling `0C90`.
  - The client stays in "World Replicating" until the server has placed every outstanding placeable (we already follow this).

## Lobby and login

### Logo2K's lobby protocol notes (rentry `3bswrgmw`, live)

The lobby here implements this; the notes are the reference for its details.

- **State-4 trailer**: bytes 256–291 of the 292-byte message are a peer id (type 5, any u16 in bytes 8–9), a channel id (type 13, the dialled port and IPv4) and a u32 1. With zeros the lobby never adopts the channel and state 6 times out (5 s tournament, 60 s retail). Ids must be fresh per connection. Nothing may follow the 292 bytes before the client's announcement.
- **Group indexes**: Base is index 0 on a type-13 channel (ack `00 02`, keepalive `00 03` every 5 s, echoed). Other groups take index i + 1 from the client's CRC announcement (86 groups, sorted by CRC); read them from the announcement. On 104319: ClientOutLobbyConnect 25, ClientInLobbyConnect 83, ClientInLobbyPlayerProgression 42, ClientOutLobbyPlayerProgression 45.
- **Login**: 0x5528 (name; empty on the retail route), 0x5531, then the server's 0x5014 hello, 0x5016 player record, 0x5018 content keys, 0x5019 four preload lists (heroes, unlocks, map headers, one more), 0x5EEC progression. 0x5015 is the login error (codes 4 and 9 have their own texts); 0x5017 is the disconnect. Two client messages may stay unanswered (group 0xEA60 offset 2 once, group 0xA668 every 11 s).
- **Loot boxes**: 0x5EEC lists boxes as {id16, type, i32 1}; types 0 standard, 1–5 seasonal, 6 Anniversary, 7 Golden, 9 Legendary Anniversary, 10 Wrecking Ball, 12 Legendary. Open: client 0x5E89 with the box id → 0x5EF1 items with the same box id (any other id frees the screen but leaves the box) → 0x5EF3 credits → client 0x5E8A. Every 0x5E89 must be answered or the screen locks ESC and BACK. The reveal shows the first four items sorted by order key. 0x5EEE appends boxes; with i32 4 and the bool set the client queues its own pop-up for the next 0x6724.
- **Routes**: tournament (`--tank_TournamentMode --lobbyServer=<host>:3724`, zero keys and client id, typed name) and retail (address, k0–k3 and client id from the Battle.net referral).
- **JAM encoding**: bools bit-packed; types 1–3 one byte, 4–5 two, 6/7/10 four (10 float), 8/9/11 eight (8 u64, 9 i64, 11 double), 12 string with NUL, 14 struct inline, 15 u32 length + bytes; arrays with count −1 are u32-counted. Test vector: the login's 0x5531 is `19 09 00`.
- **Handshake**: PANAMA stream cipher (key and IV both the 32-byte key, 32 pulls discarded); proof of work SHA-1(challenge ‖ counter) with trailing zero bits; state-4 block checks (RSA, HMAC over [0..176) ‖ [208..256), endpoint); lobby connection states 1–7 (DISCONNECTED to CONNECTED) and error codes 196810–196826.
- **Retail capture**: the published capture is client 1.68.0.0.92111. Its 0x9B78 float 417,689 is most likely the seconds left in the arcade week; 0x9B7A carries the arcade cards, wins rewards and odds.
- **Lobby maps** (rentry `wcwyv3r8`): 56 main menu lobby maps by header GUID (type `09F`), e.g. `0664` Hanamura, `0EEE` OWL Summer Showdown 2022, `0F14` Goodbye. `06C7` `BVid_Green_Room` has a lobby hero slot, untested.

### Boi-027's research document (`docs/PROJECT_STATE.txt` rev 31)

Client internals of the lobby connection (RVAs; add `0x7FF788F10000` for this repository's absolute addresses). Low value now that the lobby works, but useful when it breaks:

- Wire message id = `(channel << 8) | (message id − pool base)`, with channels assigned dynamically.
- Pool vtable slots: 3 CRC, 5 minimum id, 6 maximum id (exclusive), 7 allocate, 9 per-message operation. ClientInLobbyConnect descriptor `0x25FEC40` (ids 20500–20509).
- JAM router `0x560D90` (really the channel's send), lobby sequence machine `0x83FAF6` with per-state handlers, menu advance at `0x842DE3`.
- TACT keyring text format `key-<id reversed> = <value>`, loader `0x391DD0`; the 1.74 build config has 63 keys, 15 not in overtools (not `28B8EBB462B83EEA`).
- Battle.net pinning: a JSON blob at RVA `0x2D3B6B0`, cURL pin check `0x240DF65`.
- Its type table says 8 and 11 are doubles; the working lobby here uses 8 = u64, 9 = i64, 11 = f64. Keep ours.

### Plasmawatch login server (no licence)

Go Battle.net login: TLS on 1119, a web login form on 6969 (`LOGIN_FORM`, `authentication_state`), and a proxy that rewrote the client build to 114992. Its Connect reply carries a content handle (region US, depot URL) and `use_bindless_rpc=1`. The client here needs neither the form nor a ticket.

## Game rules from public sources

`game-rules.md` lists about 185 rules (phase timers, respawn, overtime, each mode's objectives, competitive scoring, leavers, Practice vs AI, combat formulas, ultimate charge, end of match, progression, matchmaking) compiled from ow1-preservation's `research_notes/`.
