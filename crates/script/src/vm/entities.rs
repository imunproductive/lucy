//! Server-only actions on other entities: a team (`STU_0F647E55`), a
//! removal (`STU_523EE38D`), the dummy bots (`STU_FC3921B1`,
//! `STU_C1561031`) and a body's move (`STU_CFCE55DC`); the scene tag
//! queries of a state (`STU_160F2578`) and an action (`STU_42578342`); the
//! spawn picker (`STU_5E805B5A`); the shuffled picker (`STU_37072672`).
//!
//! Client research (1.74, `python tools/stu.py`): `STU_0F647E55`,
//! `STU_523EE38D`, `STU_FC3921B1` and `STU_C1561031` register no action of their own. The client looks up the
//! base action (record #165, `execute` 0x7FF789C44A80 = `mov al, 1; ret`),
//! so it never runs them: they are the server's rules, read from their
//! fields and uses. The others are the client's code (addresses below).

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    /// `STU_0F647E55`: give each entity of `m_targets` the team index
    /// `m_67465E75` (-1 none, 0 to 4; leafall903/ORPHEUS (MIT)
    /// `assign_entity_team` in `nodes.py`). The Practice Range's spawn
    /// markers `01CE` set their own team from #1016 (the team `0827` and
    /// `0724` name); Symmetra's teleporter (`0622`) and `05FE` set their
    /// entities' teams. A world that keeps no entity teams leaves it, as an
    /// assumption.
    pub(super) fn assign_team(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(team) = self.evaluate(id, node, field(&node.fields, "m_67465E75"), None) else {
            return Ok(false);
        };
        let team = expr::to_int(&team, i64::MIN);
        if !(-1..=4).contains(&team) {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("entity team index"),
            );
            return Ok(false);
        }
        let Some(targets) = self.evaluate(id, node, field(&node.fields, "m_targets"), None) else {
            return Ok(false);
        };
        let site = self.site(id, node);
        for entity in entities(&targets) {
            if !self.world.set_entity_team(site, entity, team) {
                self.assume(site, &node.class, "the world keeps no entity team");
            }
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_523EE38D`: remove the entity `m_entity` from the match (the
    /// Practice Range pad `26F8` removes itself when ruleset 29450 is
    /// false; `01C7` removes the weapon's temporary entity #669, which may
    /// not exist yet; leafall903/ORPHEUS (MIT) `retire_junkenstein_corpse`
    /// in `nodes.py`). No entity: nothing to remove. A world that can't
    /// remove it leaves it, as an assumption.
    pub(super) fn destroy_entity(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(target) = self.evaluate(id, node, field(&node.fields, "m_entity"), None) else {
            return Ok(false);
        };
        let site = self.site(id, node);
        for entity in entities(&target) {
            if !self.world.destroy_entity(site, entity) {
                self.assume(site, &node.class, "the world removes no entity");
            }
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STUStatescriptActionCreateEntity` (`STU_72C9C62F`, execute
    /// `Action_STU_72C9C62F` 0x7FF7897FA4C0): make an entity of definition
    /// `m_entityDef` (an `STU_8556841E`) at `m_position` and write it to
    /// `m_E95B313B`. The client makes it itself only for an instance it
    /// runs on its own (`instance+0x318` bit 3 clear) or a client-only
    /// node (`sub_7FF789809ED0`), or predicts it with `m_294B652A`
    /// (`sub_7FF78980A1C0`); an instance the server's frames made leaves
    /// it to the server, which makes it for every client. The world makes
    /// it ([`World::create_entity`]). Read: the definition, the position
    /// and the output, all the Workshop's Create Effect `1CE8` and Create
    /// Icon `1AB1` set (`m_51AB9D21` is 5 and the three flags 0 in all 17
    /// uses of the bundle); any other field (a team, a facing: `0D44`,
    /// `1061`, `116F`, `15AE`, `23B0`) or the prediction flag is refused.
    pub(super) fn create_entity(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        /// The action's other config fields (`python tools/stu.py
        /// STUStatescriptActionCreateEntity`).
        const UNREAD: [&str; 18] = [
            "m_F7621CAB",
            "m_40AF7E2D",
            "m_1C558638",
            "m_EC1E473E",
            "m_3E3E203A",
            "m_888BCD8B",
            "m_3EBF988B",
            "m_AA26E7DD",
            "m_A1A715A6",
            "m_D7F85181",
            "m_246C47AF",
            "m_F8E56B94",
            "m_A3A50DF2",
            "m_DE3B4F01",
            "m_373DBF32",
            "m_E2BF1226",
            "m_0FDD45BB",
            "m_481C68E7",
        ];
        let site = self.site(id, node);
        let unread = UNREAD
            .iter()
            .any(|name| !field(&node.fields, name).is_null())
            || field(&node.fields, "m_294B652A").as_i64() != Some(0);
        let definition = field(&node.fields, "m_entityDef");
        let definition = (definition.get("$").and_then(Json::as_str) == Some("STU_8556841E"))
            .then(|| definition.get("m_entityDef").and_then(asset_guid))
            .flatten();
        let (false, Some(definition)) = (unread, definition) else {
            self.report(
                site,
                Issue::UnsupportedField("STUStatescriptActionCreateEntity fields"),
            );
            return Ok(false);
        };
        let position = match field(&node.fields, "m_position") {
            Json::Null => None,
            config => match self.evaluate(id, node, config, None) {
                Some(Value::Vector(position)) => Some(position),
                Some(_) => {
                    self.report(
                        site,
                        Issue::UnsupportedField("STUStatescriptActionCreateEntity position"),
                    );
                    return Ok(false);
                }
                None => return Ok(false),
            },
        };
        let target = match expr::lvalue(field(&node.fields, "m_E95B313B")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(false);
            }
        };
        let Some(entity) = self.world.create_entity(
            site,
            EntityCreation {
                definition,
                position,
            },
        ) else {
            self.report(site, Issue::UnsupportedAction(node.class.clone()));
            return Ok(false);
        };
        if let Some(target) = target {
            self.write(target, Value::Entity(entity))?;
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STUStatescriptStateCreateEntity` (`STU_BB1054F4`) begins (slot 23
    /// `0x7FF7897FAC30`): as the action, the client makes the entity itself
    /// only for an instance it runs on its own or a client-only node
    /// (`sub_7FF789809ED0`), so a networked instance's is the server's.
    /// The client reads `m_entityDef`, `m_position`, the scale
    /// `m_40AF7E2D` and `m_1C558638` (evaluated, its value dropped), and
    /// writes the entity to `m_E95B313B`; with `m_294B652A` it predicts it
    /// (slot 56). A position is a vector or an array's first one
    /// (`sub_7FF789806DE0` → `sub_7FF789E75E70`, type 6 only); anything
    /// else (the health pack `0046` st15's `owner()`) leaves it to the
    /// creator's default, here none ([`EntityCreation::position`]). A set
    /// scale or prediction is refused. Its end (slot 24 `0x7FF7897FACF0`)
    /// destroys the entity with its children ([`Self::end_create_entity`]).
    pub(super) fn begin_create_entity(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        if !field(&node.fields, "m_40AF7E2D").is_null()
            || field(&node.fields, "m_294B652A").as_i64() != Some(0)
        {
            self.report(
                site,
                Issue::UnsupportedField("StateCreateEntity scale or prediction"),
            );
            return Ok(());
        }
        let Some(definition) = self.evaluate(id, node, field(&node.fields, "m_entityDef"), None)
        else {
            return Ok(());
        };
        let Value::Asset(definition) = definition else {
            self.report(
                site,
                Issue::UnsupportedField("StateCreateEntity definition"),
            );
            return Ok(());
        };
        let position = match field(&node.fields, "m_position") {
            Json::Null => None,
            config => match self.evaluate(id, node, config, None) {
                Some(value) => first_vector(&value),
                None => return Ok(()),
            },
        };
        let target = match expr::lvalue(field(&node.fields, "m_E95B313B")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(());
            }
        };
        let Some(entity) = self.world.create_entity(
            site,
            EntityCreation {
                definition,
                position,
            },
        ) else {
            self.report(site, Issue::UnsupportedState(node.class.clone()));
            return Ok(());
        };
        if let Some(state) = self.state_mut(key) {
            state.kind = StateKind::CreateEntity {
                entity: Some(entity),
            };
        }
        if let Some(target) = target {
            self.write(target, Value::Entity(entity))?;
        }
        Ok(())
    }

    /// The entity a `StateCreateEntity` made goes as the state ends.
    pub(super) fn end_create_entity(&mut self, key: StateKey, node: &Node, entity: u32) {
        let site = self.site(key.instance, node);
        if !self.world.destroy_entity(site, entity) {
            self.assume(site, &node.class, "the world removes no entity");
        }
    }

    /// `STU_FC3921B1` (server-only: the base action's run on the client):
    /// ask the world for a dummy bot of hero `m_heroGUID` in team
    /// `m_67465E75`'s slot `m_slot`, standing at `m_06B5A4F1` and facing
    /// `m_9211FBC7`, and write the entity it gets to `m_4AEEE6A1`. The
    /// Practice Range console `265F` makes its Training Partner so (rid4,
    /// rid35). A world that makes none leaves the output, as an assumption.
    pub(super) fn create_dummy_bot(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let target = match expr::lvalue(field(&node.fields, "m_4AEEE6A1")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(false);
            }
        };
        let mut read = |name| self.evaluate(id, node, field(&node.fields, name), None);
        let (Some(hero), Some(team), Some(slot), Some(position), Some(facing)) = (
            read("m_heroGUID"),
            read("m_67465E75"),
            read("m_slot"),
            read("m_06B5A4F1"),
            read("m_9211FBC7"),
        ) else {
            return Ok(false);
        };
        let (Value::Asset(hero), Value::Vector(position), Value::Vector(facing)) =
            (hero, position, facing)
        else {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("dummy bot hero, position or facing"),
            );
            return Ok(false);
        };
        let dummy = DummyBot {
            hero,
            team: expr::to_int(&team, -1),
            slot: expr::to_int(&slot, -1),
            position,
            facing,
        };
        let site = self.site(id, node);
        match self.world.create_dummy_bot(site, dummy) {
            Some(entity) => {
                if let Some(target) = target {
                    self.write(target, Value::Entity(entity))?;
                }
            }
            None => self.assume(site, &node.class, "the world makes no dummy bot"),
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_C1561031` (server-only, as `STU_FC3921B1`): remove the dummy
    /// bot in team `m_67465E75`'s slot `m_slot`, if there is one; the
    /// console `265F` does so before each `STU_FC3921B1`.
    pub(super) fn remove_dummy_bot(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let mut read = |name| self.evaluate(id, node, field(&node.fields, name), None);
        let (Some(team), Some(slot)) = (read("m_67465E75"), read("m_slot")) else {
            return Ok(false);
        };
        let site = self.site(id, node);
        if !self
            .world
            .remove_dummy_bot(site, expr::to_int(&team, -1), expr::to_int(&slot, -1))
        {
            self.assume(site, &node.class, "the world removes no dummy bot");
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_CFCE55DC` (server-only: no registration of its own,
    /// ControllerWorld.md): put the body `m_target` at `m_AF774F7D` facing
    /// `m_E3779CD0` (strong guess from its uses: Reinhardt's Charge `007D`
    /// carries its pinned enemy #536 so, rid94 and rid115; the controller
    /// `0C90` rid181 its body to the spawn point #8370; the Workshop's
    /// Teleport `1D45` the target player's body to the parameter
    /// `#18739[1]`, facing `STU_C8C4C20D` of the body, its own facing).
    /// With no target it moves nothing: `007D`'s entry rid17 runs it at
    /// every start with #536 unset. The world moves the body
    /// ([`World::move_body`]); where it doesn't, or the place isn't a
    /// vector, a target is refused, or left where it is in a release build
    /// (an assumption).
    pub(super) fn move_body(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(target) = self.evaluate(id, node, field(&node.fields, "m_target"), None) else {
            return Ok(false);
        };
        let bodies = entities(&target);
        if !bodies.is_empty() {
            let site = self.site(id, node);
            let position = match self.evaluate_quietly(id, node, field(&node.fields, "m_AF774F7D"))
            {
                Some(Value::Vector(position)) => Some(position),
                _ => None,
            };
            let facing = match self.evaluate_quietly(id, node, field(&node.fields, "m_E3779CD0")) {
                Some(Value::Vector(facing)) => Some(facing),
                _ => None,
            };
            let mut moved = position.is_some();
            for &body in &bodies {
                moved = moved
                    && position.is_some_and(|position| {
                        self.world.move_body(
                            site,
                            BodyMove {
                                body,
                                position,
                                facing,
                            },
                        )
                    });
            }
            if !moved {
                if !self.tolerates_unknowns() {
                    self.report(site, Issue::UnsupportedAction(node.class.clone()));
                    return Ok(false);
                }
                self.assume(site, &node.class, "moves no body");
            }
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_545AB3B8`, Set Environment Credit Player (server-only: no
    /// registration of its own, the client follows `m_outPlug`; the name
    /// from `refs/reports/Workshop.md`): an environment kill of each body
    /// of `m_target` credits the entity `m_0B897113` (none: nobody), for
    /// the ability `m_loadout`. 11 nodes in 10 graphs, every `m_target`
    /// the owner: the Rocket
    /// Punch victim `0D07` rid46 credits the puncher (#40) before its
    /// wall slam; `1DBC`, `01C9`, `0B1F`, `0E7D`, `224E`, `0083`, `00F8`,
    /// `0261`, `0CCE`. The world keeps it as the body's knock credit
    /// ([`World::credit_environment`]); one that doesn't credits nothing
    /// (an assumption). The loadout isn't read.
    pub(super) fn credit_environment(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(target) = self.evaluate(id, node, field(&node.fields, "m_target"), None) else {
            return Ok(false);
        };
        let by = self
            .evaluate_quietly(id, node, field(&node.fields, "m_0B897113"))
            .as_ref()
            .and_then(expr::to_entity);
        let site = self.site(id, node);
        for body in entities(&target) {
            if !self
                .world
                .credit_environment(site, EnvironmentCredit { body, by })
            {
                self.assume(site, &node.class, "credits no environment kill");
            }
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_A1DF3B2D` (server-only: no registration of its own; the death
    /// layer `01CB` rid14 and rid58, Echo's `19EB`): from the entity
    /// `m_4F64C1D3` (the body's controller) it writes an entity list to
    /// `m_9E580984` (#769), which `01CB` sends `0077` to, filters and
    /// removes (`STU_523EE38D`). What the list holds is not known (a guess:
    /// the entities that entity made); no world tracks it, so the list is
    /// empty, as an assumption.
    pub(super) fn related_entities(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let target = match expr::lvalue(field(&node.fields, "m_9E580984")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(false);
            }
        };
        self.assume(self.site(id, node), &node.class, "an empty entity list");
        if let Some(target) = target {
            self.write(target, Value::Array(Vec::new()))?;
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_A3DC6A25` at its begin (`sub_7FF78990C1B0`, its only own slot):
    /// every entity of the scene made from an entity placeable (type 9)
    /// whose identifier 2 (the placeable's body `+16`) is the identifier
    /// `m_44E07E6E` goes (`sub_7FF78A73C890`, with its children). `0CBB`
    /// st34 so removes the health packs (`2AB0.01C`) when the ruleset's
    /// Spawn Health Packs (#9542e) says so. The client runs the state too
    /// and removes its own placeables so (the packs' pads). False: the
    /// identifier failed to evaluate (reported).
    pub(super) fn remove_placeables(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let Some(identifier) = self.evaluate(id, node, field(&node.fields, "m_44E07E6E"), None)
        else {
            return Ok(false);
        };
        let site = self.site(id, node);
        let removed = match identifier {
            Value::Asset(identifier) => self.world.remove_placeables(site, identifier),
            // No identifier: no placeable has identifier 2 of 0 to remove.
            Value::Null => true,
            _ => false,
        };
        if !removed {
            self.assume(site, &node.class, "the world removes no placeable");
        }
        Ok(true)
    }

    /// The entities of the scene tag query that a state (`STU_160F2578`,
    /// once after its begin, 0x7FF789C4FE60) or an action
    /// (`STU_42578342`, 0x7FF789CB8EC0; both through 0x7FF789CC3850) makes
    /// of its fields, into `m_4AEEE6A1` ([`expr`]'s `tag_query`). `0C8F`
    /// lists each team's spawn points so (#8365, #8366: tag `#8364e[team]`,
    /// all of #11016). False: the query can't run here.
    pub(super) fn write_tag_query(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let target = match expr::lvalue(field(&node.fields, "m_4AEEE6A1")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(false);
            }
        };
        let mut query = serde_json::Map::new();
        query.insert("$".into(), Json::String(node.class.clone()));
        for name in ["m_9C7BD8B8", "m_B486603E", "m_8B2A4DF3", "m_filter"] {
            query.insert(name.into(), field(&node.fields, name).clone());
        }
        let Some(entities) = self.evaluate(id, node, &Json::Object(query), None) else {
            return Ok(false);
        };
        if let Some(target) = target {
            self.write(target, entities)?;
        }
        Ok(true)
    }

    /// An input into a spawn picker (`STU_5E805B5A`, `m_F4DD996D`; the
    /// client's 0x7FF789C53010 consumes it, and runs 0x7FF789C5F130 when
    /// the state is active): pick one of the candidates `m_814F8EC3` that
    /// none of the last `m_82317B5F` seconds' picks took (none: null),
    /// write it to `m_C51AF683` and follow `m_1405CDCB`. The picks so far
    /// live in the state's [`State::tracked`] (the candidate's slot and
    /// when), which a begin clears (0x7FF789C53000). `0C8F` picks a team's
    /// spawn point so on `0685` and sends it in `0687`.
    pub(super) fn pick_spawn(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let target = match expr::lvalue(field(&node.fields, "m_C51AF683")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        let lifetime = field(&node.fields, "m_82317B5F");
        let lifetime = if lifetime.is_null() {
            0.0
        } else {
            let Some(lifetime) = self.evaluate(id, node, lifetime, None) else {
                return Ok(());
            };
            expr::to_float(&lifetime)
        };
        // The seconds as an f32, in milliseconds rounded half away from 0.
        let lifetime_ms = (f64::from(lifetime as f32) * 1000.0).round() as i64;
        let Some(candidates) = self.evaluate(id, node, field(&node.fields, "m_814F8EC3"), None)
        else {
            return Ok(());
        };
        let candidates = match candidates {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            value => vec![value],
        };
        let now = self.machine.now_ms;
        let age = |since: u64| i64::try_from(now.saturating_sub(since)).unwrap_or(i64::MAX);
        let mut history = std::mem::take(&mut self.state_mut(key).expect("active picker").tracked);
        // From the newest: drop the picks older than the lifetime (each
        // swapped with the last), and find the oldest kept one.
        let mut oldest = 0;
        let mut oldest_age = 0;
        for index in (0..history.len()).rev() {
            let entry_age = age(history[index].since_ms);
            if entry_age > lifetime_ms {
                history.swap_remove(index);
            } else if entry_age >= oldest_age {
                oldest = index;
                oldest_age = entry_age;
            }
        }
        let picked = if candidates.is_empty() {
            Value::Null
        } else {
            let count = candidates.len();
            if history.len() >= count && oldest < history.len() {
                history.swap_remove(oldest);
            }
            let first = self.random_u32() as usize % count;
            let mut slot = first;
            while history.iter().any(|entry| entry.entity as usize == slot) {
                slot = (slot + 1) % count;
                if slot == first {
                    break;
                }
            }
            if lifetime_ms > 0 {
                match history
                    .iter_mut()
                    .find(|entry| entry.entity as usize == slot)
                {
                    Some(entry) => entry.since_ms = now,
                    None => history.push(TrackedTarget {
                        entity: slot as u32,
                        since_ms: now,
                    }),
                }
            }
            candidates[slot].clone()
        };
        self.state_mut(key).expect("active picker").tracked = history;
        if let Some(target) = target {
            self.write(target, picked)?;
        }
        self.follow(id, node, "m_1405CDCB", Some(key), depth)?;
        Ok(())
    }

    /// The shuffled picker `STU_37072672` (after-begin 0x7FF789C51AE0,
    /// begin 0x7FF789C51A80): `m_E6DD2A93` (default 1, at least 0) entries
    /// of the list `m_814F8EC3`, in a shuffled order, into `m_3DC6FAAA`
    /// (cleared first). Its begin empties the order, so each begin shuffles
    /// the whole list afresh (0x7FF789C601F0 over `[0, n)`) and its window
    /// starts at the running index `m_index` (default 0; the state never
    /// writes it). The k-th pick is `order[index + k − window]`; past the
    /// window's `n` entries it reshuffles and the window starts there: with
    /// `m_A07336C2` set, `[0, 2n/3)` and then `[max(min(n − 1, 1), n/3), n)`,
    /// which keeps the last picks out of the next ones; else the whole
    /// order. Elimination's random heroes (`0D0B` st11: from the hero list
    /// #8449e, as many as the larger team, into #8684e) and `0F2F` st2. The
    /// client draws from its thread's random numbers; the server from the
    /// component's stream. False: a field can't be evaluated here.
    pub(super) fn pick_shuffled(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let target = match expr::lvalue(field(&node.fields, "m_3DC6FAAA")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(false);
            }
        };
        let Some(list) = self.evaluate(id, node, field(&node.fields, "m_814F8EC3"), None) else {
            return Ok(false);
        };
        let mut integer = |name: &str, default: i64| -> Option<i64> {
            let value = field(&node.fields, name);
            if value.is_null() {
                return Some(default);
            }
            Some(expr::to_int(
                &self.evaluate(id, node, value, None)?,
                default,
            ))
        };
        let Some(index) = integer("m_index", 0) else {
            return Ok(false);
        };
        let Some(wanted) = integer("m_E6DD2A93", 1) else {
            return Ok(false);
        };
        let apart = field(&node.fields, "m_A07336C2")
            .as_i64()
            .is_some_and(|flag| flag != 0);
        let picked = match list {
            Value::Map(entries) => Value::Map(self.shuffled_picks(&entries, index, wanted, apart)?),
            Value::Pairs(entries) => {
                Value::Pairs(self.shuffled_picks(&entries, index, wanted, apart)?)
            }
            Value::Array(items) => Value::Array(self.shuffled_picks(&items, index, wanted, apart)?),
            Value::Null => Value::Array(Vec::new()),
            value => Value::Array(self.shuffled_picks(&[value], index, wanted, apart)?),
        };
        if let Some(target) = target {
            self.write(target, picked)?;
        }
        Ok(true)
    }

    fn shuffled_picks<T: Clone>(
        &mut self,
        items: &[T],
        index: i64,
        wanted: i64,
        apart: bool,
    ) -> Result<Vec<T>, InitFault> {
        let count = items.len();
        if count == 0 || wanted <= 0 {
            return Ok(Vec::new());
        }
        // One unit of work a pick: the client loops the count it is given.
        let wanted = usize::try_from(wanted).unwrap_or(usize::MAX);
        self.machine.work_left =
            self.machine
                .work_left
                .checked_sub(wanted)
                .ok_or(InitFault::WorkBudget {
                    limit: self.machine.max_work,
                    operation: "shuffled picks",
                })?;
        let mut order: Vec<usize> = (0..count).collect();
        self.shuffle(&mut order, 0, count);
        let count = count as i64;
        let mut window = index;
        let mut picks = Vec::with_capacity(wanted);
        for next in (index..).take(wanted) {
            if !(window..window + count).contains(&next) {
                let n = count as usize;
                if apart {
                    self.shuffle(&mut order, 0, 2 * n / 3);
                    self.shuffle(&mut order, (n - 1).min(1).max(n / 3), n);
                } else {
                    self.shuffle(&mut order, 0, n);
                }
                window = next;
            }
            picks.push(items[order[(next - window) as usize]].clone());
        }
        Ok(picks)
    }

    /// 0x7FF789C601F0: each entry of `order[from..to)` in turn goes to a
    /// random free slot of the range (a random slot, then the next free
    /// one, wrapping).
    fn shuffle(&mut self, order: &mut [usize], from: usize, to: usize) {
        if to <= from {
            return;
        }
        let len = to - from;
        let mut slots: Vec<Option<usize>> = vec![None; len];
        for &entry in &order[from..to] {
            let mut slot = self.random_u32() as usize % len;
            while slots[slot].is_some() {
                slot = (slot + 1) % len;
            }
            slots[slot] = Some(entry);
        }
        for (target, entry) in order[from..to].iter_mut().zip(slots) {
            *target = entry.expect("every slot filled");
        }
    }

    /// A random number from the component's stream, as the client's
    /// thread's random numbers (the spawn picker's first candidate).
    fn random_u32(&mut self) -> u32 {
        (super::context::next_random(&self.machine.random_state) >> 32) as u32
    }
}

/// The valid entities of a value (one entity or an array of them), each
/// once, in order.
pub(super) fn entities(value: &Value) -> Vec<u32> {
    let mut found = Vec::new();
    let mut add = |value: &Value| {
        if let Value::Entity(entity) = value {
            if expr::valid_entity(*entity) && !found.contains(entity) {
                found.push(*entity);
            }
        }
    };
    match value {
        Value::Array(items) => items.iter().for_each(&mut add),
        value => add(value),
    }
    found
}

/// A value as the client's position read takes it (`sub_7FF789E75E70`):
/// a vector, or an array's first item followed down; none for anything
/// else.
fn first_vector(value: &Value) -> Option<[f64; 3]> {
    match value {
        Value::Vector(position) => Some(*position),
        Value::Array(items) => first_vector(items.first()?),
        _ => None,
    }
}
