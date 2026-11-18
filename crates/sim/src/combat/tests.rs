use super::*;
use crate::{instance::RETAIL_QUANTA, scene_assets::SceneAssets};
use orpheus_content::maps::GameMap;

const SHOOTER: Shooter = Shooter {
    source: 0xa000_0101,
    team: 0,
    bot: false,
};

#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn repeated_rifle_hits_retire_and_respawn_only_the_hit_bot() {
    let runtime = orpheus_testkit::runtime_dir();
    let mut scene = SceneAssets::load(runtime)
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let slot = scene
        .bots()
        .iter()
        .position(|bot| bot.spawn.team == 1)
        .unwrap();
    let bot = &scene.bots()[slot];
    let entity = bot.entity;
    let [x, y, z] = bot.position;
    let count = scene.bots().len();
    let mut combat = Combat::new(&scene);
    let mut context = ShotContext {
        quanta: RETAIL_QUANTA,
        tick: 1000,
        now: Duration::from_secs(4),
    };
    combat.record_poses(&scene, &[], context.tick);
    let mut shot = PosedHitscan {
        source: SHOOTER.source,
        // Level with the Training Bot's head: its idle pose centres the
        // head sphere 1.98 m up (from Soldier: 76's standing eye, combat.py
        // `EYE` 1.55 m, a level shot meets its chest).
        origin: [x - 8.0, y + 2.0, z],
        command: Command {
            frame: context.tick,
            yaw: 16384,
            ..Command::default()
        },
        // Along +x, as yaw 16384 aims.
        direction: [1.0, 0.0, 0.0],
        shot_frame: 999,
        reach: 180.0,
        hit: HitDefinition {
            amount: 20.0,
            loadout: None,
            critical: 2.0,
            falloff: None,
            tags: [0x0d80_0000_0000_09a2].into_iter().collect(),
            allies: AllyHits::None,
        },
        ray_profile: None,
    };
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| false)
            .unwrap()
            .is_none()
    );
    assert!(
        combat
            .hitscan(
                &mut scene,
                shot,
                Shooter { team: 1, ..SHOOTER },
                context,
                |_| { true }
            )
            .unwrap()
            .is_none()
    );
    for index in 0..5 {
        let result = combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .unwrap();
        let terminal = index == 4;
        assert_eq!(result.health.entity, entity);
        assert_eq!(
            result.notices[0],
            Some(CombatNotice::Hit {
                target: entity,
                dealt: 40.0,
                critical: true,
                killed: terminal,
                shot_frame: shot.shot_frame,
            })
        );
        assert_eq!(
            result.notices[1],
            terminal.then_some(CombatNotice::Elimination {
                killer: shot.source,
                victim: entity,
            })
        );
        assert_eq!(
            combat.targets[slot].vitals.total(),
            160.0 - f64::from(index) * 40.0
        );
        assert_eq!(result.remaining, 160.0 - f64::from(index) * 40.0);
        assert_eq!(
            result.death,
            terminal.then_some(BotDeath {
                bot: entity,
                killer: SHOOTER.source,
                critical: true,
                loadout: None,
                tags: shot.hit.tags,
                tick: context.tick,
            })
        );
        // A hit it survives carries its damage for the bot's script.
        assert_eq!(
            result.damage,
            (!terminal).then_some(BotDamage {
                source: SHOOTER.source,
                dealt: 40.0,
                tags: shot.hit.tags,
                tick: context.tick,
            })
        );
        assert_eq!(scene.bots()[slot].dead, terminal);
        // Its pools for every client; with the killing hit, the filter bits
        // of a dead body (`scene` module notes).
        let Some(Operation::Update(components)) = &result.health.operation else {
            panic!("the hit's health update");
        };
        let indices: Vec<u8> = components.iter().map(|component| component.index).collect();
        assert_eq!(indices, if terminal { vec![51, 26] } else { vec![51] });
        if terminal {
            assert_eq!(
                components[1],
                crate::scene::filter_component(
                    0x0100_0000 | crate::scene::BODY_KINDS | crate::scene::DEAD
                )
            );
        }
    }
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .is_none()
    );
    let due = context.now + scene.respawn_time();
    assert!(
        combat
            .respawn_due(&mut scene, 1187, due - Duration::from_millis(1))
            .unwrap()
            .is_empty()
    );
    let [replacement] = combat
        .respawn_due(&mut scene, 1188, due)
        .unwrap()
        .try_into()
        .ok()
        .unwrap();
    assert_eq!(replacement.retired, entity);
    let next = BOT_ENTITY + count as u32;
    assert_eq!(replacement.created.entity, next);
    assert!(matches!(
        replacement.created.operation,
        Some(Operation::Create(_))
    ));
    assert!(!scene.bots()[slot].dead);
    assert_eq!(combat.targets[slot].vitals.total(), 200.0);
    context.tick = 1188;
    context.now = due;
    combat.record_poses(&scene, &[], context.tick);
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .is_none()
    );
    shot.command.frame = context.tick;
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |visible| visible
                == next)
            .unwrap()
            .is_some()
    );
    assert_eq!(combat.targets[slot].vitals.total(), 160.0);
}

#[test]
fn heroes_respawn_after_the_mode_time() {
    let times = RespawnTimes::from_json(r#"{"0x0230000000000018": 3}"#).unwrap();
    assert_eq!(
        times.for_mode(0x0230_0000_0000_0018),
        Duration::from_secs(3)
    );
    assert_eq!(
        times.for_mode(0x0230_0000_0000_0001),
        Duration::from_secs(10)
    );
    assert_eq!(RESPAWN_RETRY, Duration::from_millis(250));
}

/// The kill feed line reads the killer, the credited body (00BD), the
/// tags (003D), the crit and the loadout from the "died" message.
#[test]
fn the_died_message_names_the_killer_the_crit_the_loadout_and_the_tags() {
    let melee: DamageTags = [0x0d80_0000_0000_0302].into_iter().collect();
    let (message, parameters) =
        death_message(0xa000_0201, true, Some(0x0b90_0000_0000_008d), melee);
    assert_eq!(message, DIED);
    assert_eq!(parameters[&KILLER], Value::Entity(0xa000_0201));
    assert_eq!(parameters[&CREDITED], Value::Entity(0xa000_0201));
    assert_eq!(
        parameters[&DAMAGE_TAGS],
        Value::Array(vec![Value::Asset(0x0d80_0000_0000_0302)])
    );
    assert_eq!(parameters[&CRIT], Value::Bool(true));
    assert_eq!(parameters[&LOADOUT], Value::Asset(0x0b90_0000_0000_008d));
    assert_eq!(parameters[&ASSISTS], Value::Array(Vec::new()));
    let plain = death_message(1, false, None, DamageTags::NONE).1;
    assert_eq!(plain[&LOADOUT], Value::Null);
    assert_eq!(plain[&DAMAGE_TAGS], Value::Array(Vec::new()));
}

/// A hit the body survives: its damage message carries the hit's tags, the
/// attacker, the damage dealt and the unit direction from the attacker to
/// the body; none (null) when the two meet.
#[test]
fn the_damage_message_carries_the_tags_the_attacker_and_the_way() {
    let splash: DamageTags = [0x0d80_0000_0000_078d].into_iter().collect();
    let direction = damage_direction([1.0, 0.0, 1.0], [1.0, 0.0, 5.0]);
    assert_eq!(direction, Some([0.0, 0.0, 1.0]));
    let (message, parameters) = damage_message(0x8000_8800, 37.5, splash, direction);
    assert_eq!(message, DAMAGED);
    assert_eq!(parameters[&DAMAGE_TAGS], splash.to_value());
    assert_eq!(parameters[&DAMAGE_SOURCE], Value::Entity(0x8000_8800));
    assert_eq!(parameters[&DAMAGE_AMOUNT], Value::Float(37.5));
    assert_eq!(
        parameters[&DAMAGE_DIRECTION],
        Value::Vector([0.0, 0.0, 1.0])
    );
    assert_eq!(damage_direction([2.0; 3], [2.0; 3]), None);
    assert_eq!(
        damage_message(1, 1.0, DamageTags::NONE, None).1[&DAMAGE_DIRECTION],
        Value::Null
    );
}

/// The messages a hit gives the body, in order: a hit it survives, its
/// damage message; a killing hit, its damage message with the kill's
/// fields (`1544` rid54 reads the credited body, crit, loadout and assists
/// from `0011.025`), then the death message. The `.hurt off` switch holds
/// back the damage message only.
#[test]
fn a_killing_hit_sends_its_damage_message_before_the_death_message() {
    let splash: DamageTags = [0x0d80_0000_0000_078d].into_iter().collect();
    let loadout = Some(0x0b90_0000_0000_008e);
    let hit = PlayerHit {
        body: 0xa000_0201,
        shooter: SHOOTER,
        amount: 120.0,
        critical: true,
        loadout,
        frame: 1000,
        world_tick: Some(1000),
        heal: false,
        tags: splash,
        direction: Some([0.0, 0.0, 1.0]),
    };
    let on = HurtFeedback::default();
    let damage = damage_message(SHOOTER.source, 75.0, splash, hit.direction);
    assert_eq!(
        hit_messages(&hit, hit.tags, 75.0, false, on),
        std::slice::from_ref(&damage)
    );

    let [(first, killing), died] = hit_messages(&hit, hit.tags, 75.0, true, on)
        .try_into()
        .unwrap();
    assert_eq!(first, DAMAGED);
    for (field, value) in &damage.1 {
        assert_eq!(&killing[field], value);
    }
    assert_eq!(killing[&CREDITED], Value::Entity(SHOOTER.source));
    assert_eq!(killing[&CRIT], Value::Bool(true));
    assert_eq!(killing[&LOADOUT], Value::Asset(0x0b90_0000_0000_008e));
    assert_eq!(killing[&ASSISTS], Value::Array(Vec::new()));
    assert_eq!(died, death_message(SHOOTER.source, true, loadout, splash));

    let off = HurtFeedback {
        message: false,
        ..on
    };
    assert!(hit_messages(&hit, hit.tags, 75.0, false, off).is_empty());
    assert_eq!(hit_messages(&hit, hit.tags, 75.0, true, off), [died]);
}

#[test]
fn shooters_hit_other_teams_and_everyone_in_a_free_for_all() {
    let target = |body, team, dead, hurt| PlayerSlot {
        target: PlayerTarget {
            body,
            team,
            pose: TargetPose {
                feet: [0.0; 3],
                height: 1.8,
                radius: 0.4,
                eye: 1.55,
                yaw: 0.0,
                shapes: None,
            },
            dead,
            hurt,
            tags: None,
            ai: None,
            dealt: Vec::new(),
        },
        history: TargetHistory::new(None),
    };
    let combat = Combat {
        targets: Vec::new(),
        players: vec![
            target(0xa000_0101, 0, false, true),
            target(0xa000_0201, 0, false, false),
            target(0xa000_0301, 1, false, false),
            target(0xa000_0401, 1, true, true),
            target(0xa000_0501, 0, false, true),
            target(0xa000_0601, 0, true, true),
        ],
        rockets: Vec::new(),
        player_hits: Vec::new(),
        player_pushes: Vec::new(),
        next_bot: BOT_ENTITY,
        seen_ids: SeenIds::new(),
        field_ids: FieldIds::default(),
        fields: SpawnedFields::new(
            Arc::new(
                orpheus_script::graph::GraphStore::from_json(
                    br#"{"version": 1, "graphs": [], "bodies": {}, "missing": []}"#,
                )
                .unwrap(),
            ),
            Vec::new(),
            0,
            0.0,
            None,
        ),
        next_rocket: 0,
        landed: Vec::new(),
        auras: Vec::new(),
    };
    let bodies = |free_for_all, allies| -> Vec<u32> {
        combat
            .player_candidates(SHOOTER, free_for_all, allies, |slot| {
                Some(slot.target.pose.clone())
            })
            .map(|(body, _)| body)
            .collect()
    };
    // Not himself, not an ally, not the dead.
    assert_eq!(bodies(false, AllyHits::None), [0xa000_0301]);
    assert_eq!(
        bodies(true, AllyHits::None),
        [0xa000_0201, 0xa000_0301, 0xa000_0501]
    );
    // A hit that strikes hurt allies (the Biotic Rifle's) passes through
    // the ally at full health, never strikes the shooter or the dead, and
    // changes nothing in a free-for-all.
    assert_eq!(bodies(false, AllyHits::Hurt), [0xa000_0301, 0xa000_0501]);
    assert_eq!(
        bodies(false, AllyHits::All),
        [0xa000_0201, 0xa000_0301, 0xa000_0501]
    );
    assert_eq!(
        bodies(true, AllyHits::Hurt),
        [0xa000_0201, 0xa000_0301, 0xa000_0501]
    );
}

/// The bodies the statescript's target searches find (`TrackTargets`):
/// the players' bodies as recorded this tick, then every bot, with their
/// teams, capsules and deaths.
#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn target_searches_see_the_players_then_the_bots() {
    let scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let mut combat = Combat::new(&scene);
    let pose = TargetPose {
        feet: [1.0, 2.0, 3.0],
        height: 1.8,
        radius: 0.4,
        eye: 1.55,
        yaw: 0.0,
        shapes: None,
    };
    let player = PlayerTarget {
        body: SHOOTER.source,
        team: 1,
        pose: pose.clone(),
        dead: true,
        hurt: false,
        tags: None,
        ai: None,
        dealt: Vec::new(),
    };
    combat.record_poses(&scene, &[player], 1000);
    let candidates = combat.target_candidates(&scene);
    assert_eq!(candidates.len(), 1 + scene.bots().len());
    assert_eq!(
        candidates[0],
        TargetCandidate {
            entity: SHOOTER.source,
            team: Some(1),
            dead: true,
            controller: Some(SHOOTER.source & !0xff),
            ai: false,
            ai_control: None,
            feet: pose.feet,
            height: pose.height,
            radius: pose.radius,
            yaw: pose.yaw,
            tags: None,
            spawned: Vec::new(),
        }
    );
    for (slot, (candidate, bot)) in candidates[1..].iter().zip(scene.bots()).enumerate() {
        let target = scene.bot_target(bot);
        assert_eq!(
            (candidate.entity, candidate.team, candidate.dead),
            (bot.entity, Some(bot.spawn.team), bot.dead)
        );
        assert_eq!(
            (candidate.controller, candidate.ai),
            (
                Some(crate::scene::bot_controller_entity(slot).unwrap()),
                true
            )
        );
        assert_eq!(
            (candidate.feet, candidate.height, candidate.radius),
            (target.feet, target.height, target.radius)
        );
    }
}

/// A player's shot on another player's body: the hit waits for the match
/// with what the victim's damage message carries (`damage_message`): the
/// shooter's body as the attacker, the weapon's damage tags and the way
/// from the shooter's feet to the victim's, as this tick placed them.
#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn a_players_shot_on_another_player_carries_its_tags_and_way() {
    const ENEMY: u32 = 0xa000_0201;
    let mut scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    // Open ground beside a bot, which stands behind the enemy.
    let [x, y, z] = scene.bots()[0].position;
    let mut combat = Combat::new(&scene);
    let context = ShotContext {
        quanta: RETAIL_QUANTA,
        tick: 1000,
        now: Duration::from_secs(4),
    };
    let player = |body, team, feet| PlayerTarget {
        body,
        team,
        pose: TargetPose {
            feet,
            height: 1.8,
            radius: 0.4,
            eye: 1.55,
            yaw: 0.0,
            shapes: None,
        },
        dead: false,
        hurt: false,
        tags: None,
        ai: None,
        dealt: Vec::new(),
    };
    combat.record_poses(
        &scene,
        &[
            player(SHOOTER.source, 0, [x - 8.0, y, z]),
            player(ENEMY, 1, [x - 4.0, y, z]),
        ],
        context.tick,
    );
    let tags: DamageTags = [0x0d80_0000_0000_03bf, 0x0d80_0000_0000_09a2]
        .into_iter()
        .collect();
    let shot = PosedHitscan {
        source: SHOOTER.source,
        origin: [x - 8.0, y + 1.2, z],
        command: Command {
            frame: context.tick,
            yaw: 16384,
            ..Command::default()
        },
        direction: [1.0, 0.0, 0.0],
        shot_frame: 999,
        reach: 100.0,
        hit: HitDefinition {
            amount: 19.0,
            loadout: None,
            critical: 2.0,
            falloff: None,
            tags,
            allies: AllyHits::None,
        },
        ray_profile: None,
    };
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .is_none(),
        "the enemy player stands in front of the bot"
    );
    let [hit] = combat.take_player_hits().try_into().unwrap();
    assert_eq!(
        (hit.body, hit.shooter, hit.amount, hit.heal, hit.critical),
        (ENEMY, SHOOTER, 19.0, false, false)
    );
    assert_eq!(hit.tags, tags);
    assert_eq!(hit.direction, Some([1.0, 0.0, 0.0]));
}

/// The Biotic Rifle's hit (`0B94` st44: 70, strikes hurt allies) on the
/// Practice Range, after leafall903/ORPHEUS (MIT) `test_game_ana.py`
/// (c239a84): a hurt Friendly Bot heals 70 and then the rest to full, with
/// no notice; at full health the shot passes through it to the Training
/// Bot behind, which takes 70 with the shooter's 25001; a hurt allied
/// player's body is healed by the match, an unhurt one is passed through;
/// the hip dart, a projectile, heals the same.
#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn the_biotic_rifle_heals_hurt_allies_and_damages_enemies() {
    use crate::projectile_flight::ProjectileFlight;

    const RIFLE: HitDefinition = HitDefinition {
        amount: 70.0,
        loadout: Some(0x0b90_0000_0000_00e3),
        critical: 1.0,
        falloff: None,
        allies: AllyHits::Hurt,
        tags: DamageTags::NONE,
    };
    let mut scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let team = |team| {
        scene
            .bots()
            .iter()
            .position(|bot| bot.spawn.team == team)
            .unwrap()
    };
    let (friendly, enemy) = (team(0), team(1));
    let [x, y, z] = scene.bots()[friendly].position;
    scene.bots_mut()[enemy].position = [x + 2.0, y, z];
    let (friendly_bot, enemy_bot) = (scene.bots()[friendly].entity, scene.bots()[enemy].entity);
    let full = scene.bots()[friendly].spawn.health;
    let mut combat = Combat::new(&scene);
    let context = ShotContext {
        quanta: RETAIL_QUANTA,
        tick: 1000,
        now: Duration::from_secs(4),
    };
    combat.record_poses(&scene, &[], context.tick);
    let shot = PosedHitscan {
        source: SHOOTER.source,
        origin: [x - 8.0, y + 1.2, z],
        command: Command {
            frame: context.tick,
            yaw: 16384,
            ..Command::default()
        },
        direction: [1.0, 0.0, 0.0],
        shot_frame: 999,
        reach: 100.0,
        hit: RIFLE,
        ray_profile: None,
    };
    let fire = |combat: &mut Combat, scene: &mut PreparedScene| {
        combat
            .hitscan(scene, shot, SHOOTER, context, |_| true)
            .unwrap()
    };
    let wound = Death {
        time: 0.0,
        frame: 0,
        killer: 0,
    };
    combat.targets[friendly].vitals.damage(100.0, wound);
    for (given, total) in [(70.0, full - 30.0), (30.0, full)] {
        let hit = fire(&mut combat, &mut scene).expect("the friendly bot heals");
        assert_eq!(
            (hit.bot, hit.healed, hit.remaining, hit.notices, hit.death),
            (friendly_bot, given, total, [None, None], None)
        );
        assert_eq!(hit.health.entity, friendly_bot);
    }
    let hit = fire(&mut combat, &mut scene).expect("the enemy behind takes the dart");
    assert_eq!(
        (hit.bot, hit.healed, hit.remaining),
        (enemy_bot, 0.0, 130.0)
    );
    assert_eq!(
        hit.notices[0],
        Some(CombatNotice::Hit {
            target: enemy_bot,
            dealt: 70.0,
            critical: false,
            killed: false,
            shot_frame: shot.shot_frame,
        })
    );
    assert_eq!(combat.targets[friendly].vitals.total(), full);

    // An allied player's body in front: struck while hurt, for the match
    // to heal; passed through at full health.
    let player = |hurt| PlayerTarget {
        body: 0xa000_0201,
        team: 0,
        pose: TargetPose {
            feet: [x - 4.0, y, z],
            height: 1.8,
            radius: 0.4,
            eye: 1.55,
            yaw: 0.0,
            shapes: None,
        },
        dead: false,
        hurt,
        tags: None,
        ai: None,
        dealt: Vec::new(),
    };
    combat.record_poses(&scene, &[player(true)], context.tick);
    assert!(fire(&mut combat, &mut scene).is_none());
    let [hit] = combat.take_player_hits().try_into().unwrap();
    assert_eq!(
        (hit.body, hit.amount, hit.heal, hit.critical),
        (0xa000_0201, 70.0, true, false)
    );
    combat.record_poses(&scene, &[player(false)], context.tick);
    assert_eq!(fire(&mut combat, &mut scene).unwrap().bot, enemy_bot);
    assert!(combat.take_player_hits().is_empty());

    // The hip dart flies (125 m/s) and heals the same.
    combat.record_poses(&scene, &[], context.tick);
    combat.targets[friendly].vitals.damage(50.0, wound);
    let dart = ProjectileBallistics {
        speed: 125.0,
        lifetime: 0.8,
        definition: None,
        direct: Some(RIFLE),
        blast: None,
        radius: 0.0,
        gravity: 0.0,
        fuse: None,
        unmodelled: Vec::new(),
        ray_profile: None,
    };
    let mut flight =
        ProjectileFlight::new(dart, shot.origin, shot.direction, Some(context.tick)).unwrap();
    let mut hits = Vec::new();
    for tick in context.tick + 1..context.tick + 10 {
        let context = ShotContext { tick, ..context };
        if combat
            .step_projectile(&mut scene, SHOOTER, &mut flight, context, &mut hits)
            .unwrap()
            .is_some()
        {
            break;
        }
    }
    let [hit] = hits.try_into().ok().unwrap();
    assert_eq!(
        (hit.bot, hit.healed, hit.remaining, hit.notices),
        (friendly_bot, 50.0, full, [None, None])
    );
}
