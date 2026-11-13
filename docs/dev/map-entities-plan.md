# Map entities: plan

The research behind this plan is in `D:/Work/Coding/refs/reports/MapEntities.md`.

## Why things are missing on the maps

A 1.74 map has static geometry and placeables. Each placeable has a definition (model, script and other components), a transform, up to two identifiers and a client id.

- **Client id not 0:** the client builds the placeable itself (fences, crates, clip boxes). These all show.
- **Client id 0:** the placeable is the server's. The client builds it only when the server creates it.

Today the server creates only two kinds of server-owned placeables: the loading list (spawn points and required entities) and the spawn door barriers that run graph `00F6`. The rest, 100 to 330 per map, are listed in `game_maps_174.json`'s `mirrored` list, and no code reads it. That is why spawn doors, doorway frames and team walls, capture point and payload models, and very likely objects like Volskaya's bus and Rialto's boats are missing.

Creating a placeable is not enough for anything that moves or reacts: the client gives it an empty script, and its graphs only run once the server starts them and streams their frames (upstream checked this on Rialto (Retribution)).

## Stages

| Stage | What | Effort | What it shows |
|---|---|---|---|
| A | Create every server-owned placeable of the map and mode, in its default state | 1–3 days | Static objects appear; scripted ones sit in their default state |
| B | Run the placeables' scripts on the server and stream their frames | weeks | Animated and scripted objects work (boats move, doors animate) |
| C | Connect them to the match | 1–2 weeks after B | Doors open and close with the objective (`072F` from the match flow), team walls block enemies, health packs heal and respawn, the payload entity follows the track, the Training Partner console |
| D | Moving objects in physics | medium to large | Standing on boats or the payload: dynamic shapes in the mover, the movement record's parent entity |

### Stage A

1. **Derivation:** the `mirrored` list keeps flat indexes only, without the identifiers the mode filter needs. Store each server-owned placeable with its identifiers (and its definition, for logs and tests), the way `door_barriers` already does; merge the door barriers into this list instead of keeping two lists.
2. **Filter** by the mode's identifiers (`m_A43573F4`), as the door barriers are filtered (`map_areas.rs` `in_mode`).
3. **Create** them in `initial_world` after the loading placeables, the player and the mode, like the door barriers (putting them before the player broke the first world frame). Respect the create budget: Rialto has about 270.
4. **Late joiners** get the same creates.
5. **Tests:** count the placeable creates per map and mode (the Practice Range, Rialto, Volskaya in Assault), and keep the prepared-world tests' order exact.

Risks: a scripted placeable shows whatever its default is (for example what `0D73` shows before its first `072F`, an IDA question); a wall can look open or closed when it shouldn't.

### Stage B

A `MapEntityWorld` beside the body and bot worlds: one script component per placed entity, started in the client's order (definition graphs, then the instance's overrides on the first matching graph, then the instance's extra graphs), streamed to each client after its create arrived, with target searches over the players. Bundle every graph the map's placeables name. Fill the VM's missing classes one at a time (`STU_33A46033`, `STU_B1FA9C2A`, `STU_7D71FDF3`, `STU_0F647E55`, `STU_49FE0019`, `STU_550D0C5C`, the console natives), keeping unknown classes refused in debug builds.

First slice done: the world, the derivation, the bundle and the delivery exist, and `STU_33A46033` and `STU_B1FA9C2A` run (the Range's barriers and doors send frames). Left: the other classes, Rialto's gaps, spawned entities (see `MapEntities.md`, Stage B).

### Stage C and D

See the table. Stage C needs the mode graph's `072F` parameters (from `0F4A` / `0EC2`) sent by `match_flow.rs`, since the mode graph itself doesn't run.
