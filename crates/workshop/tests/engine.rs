//! The rule engine against a fake match: when rules run, how long waits
//! last in ticks, subroutines, loops, wait behaviours, player events and
//! graph actions, with rules written as Workshop text.

mod common;

use common::{ABILITY_2, INTERACT, Match, Player, catalog};
use orpheus_workshop::{
    Engine, Logic,
    host::{Context, GameEvent},
    logic::events,
    parse,
    value::Value,
};

/// The server tick the tests run at [assumption: Workshop rules run every
/// server tick, 0.016 s, the minimum Wait (`1370` clamps to 0.016)].
const TICK: f64 = 0.016;

fn logic(text: &str) -> Logic {
    parse(text, &catalog()).unwrap().logic
}

fn engine(text: &str) -> Engine {
    Engine::new(&logic(text), catalog(), TICK, 1)
}

fn run(engine: &mut Engine, game: &mut Match, ticks: usize) {
    for _ in 0..ticks {
        game.advance();
        engine.tick(game);
    }
    assert_eq!(engine.faults(), [] as [String; 0]);
}

fn number(value: Value) -> f64 {
    value.number()
}

/// C32S2's "CP Clear / Fail" rule as the script has it, with test
/// stand-ins for its subroutines: landing inside the next checkpoint's
/// radius clears it, landing anywhere else fails.
const CHECKPOINT: &str = r#"
variables
{
	global:
		0: Cleared
		1: Failed
	player:
		2: CPData
		6: Shift
		7: Roaming
		8: Grace
		9: AbilityDetect
		11: Local_Pos
		12: Local_Rad
		53: Effpos
		56: EffradD
		58: Effabi
		61: Effactual
		62: Effvisible
		72: TTtimer
		84: PM_Centerless
}

subroutines
{
	8: CPclearend
	36: CPfail
	37: CPclear
}

rule("CP Clear / Fail")
{
	event
	{
		Ongoing - Each Player;
		All;
		All;
	}

	conditions
	{
		Distance Between(Position Of(Event Player), Event Player.CPData[3]) > Event Player.CPData[4] + 0.100;
		"Rocket Punch Grace   |   Disable to turn OFF"
		(Is Firing Secondary(Event Player) || Event Player.Grace < 0.120) == True;
		Event Player.Roaming <= False;
		Event Player.Shift == False;
		Is On Ground(Event Player) == True;
	}

	actions
	{
		If(Event Player.Roaming == -1);
			If(Distance Between(Position Of(Event Player), Event Player.Effpos[Event Player.Effactual])
				< Event Player.EffradD[Event Player.Effactual]);
				Event Player.Shift = True;
				Disallow Button(Event Player, Button(Ability 2));
				If(Is Using Ability 2(Event Player) && !Is Button Held(Event Player, Button(Ability 1)));
					Cancel Primary Action(Event Player);
				End;
				Play Effect(Event Player, Buff Explosion Sound, Color(White), Event Player, 69);
				Event Player.Effvisible[Event Player.Effactual + 7] = True;
				Event Player.CPData[3] = Event Player.Effpos[Event Player.Effactual];
				Event Player.CPData[4] = Event Player.EffradD[Event Player.Effactual];
				Skip If(Event Player.PM_Centerless, True);
				Teleport(Event Player, Event Player.CPData[3]);
				Set Secondary Fire Enabled(Event Player, First Of(Event Player.Effabi[Event Player.Effactual]));
				Set Ability 1 Enabled(Event Player, Event Player.Effabi[Event Player.Effactual][True]);
				Set Ability 2 Enabled(Event Player, Last Of(Event Player.Effabi[Event Player.Effactual]));
				Call Subroutine(CPclearend);
			Else;
				Call Subroutine(CPfail);
			End;
		Else;
			Skip If(Event Player.TTtimer, True);
			Disallow Button(Event Player, Button(Ability 2));
			"Prepare Cancel Slam Animation"
			If(Is Using Ability 2(Event Player) || (Event Player.Grace && !Is Button Held(Event Player, Button(Secondary Fire))));
				Event Player.AbilityDetect = Event Player.Grace ? 2 : True;
			End;
			If(Is True For Any(Event Player.Local_Rad, Current Array Element && Distance Between(Position Of(Event Player),
				Event Player.Local_Pos[Current Array Index]) <= Current Array Element));
				Call Subroutine(CPclear);
			Else;
				Call Subroutine(CPfail);
			End;
		End;
		Loop If Condition Is True;
	}
}

rule("CP Clear sub (test)")
{
	event
	{
		Subroutine;
		CPclear;
	}

	actions
	{
		Global.Cleared += 1;
		Event Player.CPData[3] = First Of(Event Player.Local_Pos);
		Small Message(Event Player, Custom String("Checkpoint"));
	}
}

rule("CP Fail sub (test)")
{
	event
	{
		Subroutine;
		CPfail;
	}

	actions
	{
		Global.Failed += 1;
		Teleport(Event Player, Event Player.CPData[3]);
		Event Player.Shift = True;
	}
}
"#;

#[test]
fn checkpoint_clears_when_the_player_lands_inside_the_next_one() {
    let mut engine = engine(CHECKPOINT);
    let mut game = Match::new(catalog());
    game.players.push(Player::new(1, 0));
    // At the current checkpoint (origin, radius 2); the next one at x 10,
    // radius 1.5.
    let mut cp_data = vec![Value::Number(0.0); 5];
    cp_data[3] = Value::Vector([0.0; 3]);
    cp_data[4] = Value::Number(2.0);
    game.player_variables.insert((1, 2), Value::Array(cp_data));
    game.player_variables
        .insert((1, 11), Value::Array(vec![Value::Vector([10.0, 0.0, 0.0])]));
    game.player_variables
        .insert((1, 12), Value::Array(vec![Value::Number(1.5)]));
    game.player_mut(1).position = [1.0, 0.0, 0.0];

    run(&mut engine, &mut game, 3);
    assert_eq!(number(game.global(0)), 0.0, "inside the current checkpoint");
    assert!(game.take_started().is_empty());

    // In the air over the next checkpoint: nothing until the landing.
    game.player_mut(1).position = [10.0, 0.0, 1.0];
    game.player_mut(1).on_ground = false;
    run(&mut engine, &mut game, 2);
    assert!(game.take_started().is_empty());
    game.player_mut(1).on_ground = true;
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 1.0);
    assert_eq!(number(game.global(1)), 0.0);
    assert_eq!(game.started[0].params[1], Value::Button(ABILITY_2));
    assert_eq!(game.take_started(), ["Disallow Button", "Small Message"]);
    // The cleared checkpoint is the current one now: no loop, no rerun.
    run(&mut engine, &mut game, 5);
    assert_eq!(number(game.global(0)), 1.0);
    assert!(game.take_started().is_empty());

    // Landing short of the next one fails.
    game.player_mut(1).position = [20.0, 0.0, 0.0];
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(1)), 1.0);
    assert_eq!(game.take_started(), ["Disallow Button", "Teleport"]);
}

#[test]
fn a_wait_lasts_its_time_in_whole_ticks() {
    let mut engine = engine(
        r#"
variables { global: 0: A }
rule("Wait")
{
	event { Ongoing - Global; }
	actions
	{
		Global.A = 1;
		Wait(0.100, Ignore Condition);
		Global.A = 2;
		Wait(0, Ignore Condition);
		Global.A = 3;
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 1.0);
    // 0.1 s is 6.25 ticks of 0.016 s: the rule goes on in the 7th tick
    // after the one it waited in.
    run(&mut engine, &mut game, 6);
    assert_eq!(number(game.global(0)), 1.0);
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 2.0);
    // Wait(0) is clamped to 0.016 s: one tick.
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 3.0);
    assert_eq!(engine.running(), 0);
}

#[test]
fn subroutines_share_the_context_and_start_rule_runs_beside() {
    let mut engine = engine(
        r#"
variables
{
	global:
		0: Order
		1: Started
	player:
		0: Mark
}
subroutines
{
	0: Inline
	1: Beside
}
rule("Main")
{
	event { Ongoing - Each Player; All; All; }
	actions
	{
		Call Subroutine(Inline);
		Modify Global Variable(Order, Append To Array, 2);
		Start Rule(Beside, Do Nothing);
		Start Rule(Beside, Do Nothing);
		Modify Global Variable(Order, Append To Array, 3);
	}
}
rule("Inline")
{
	event { Subroutine; Inline; }
	actions
	{
		Event Player.Mark = 7;
		Modify Global Variable(Order, Append To Array, 1);
	}
}
rule("Beside")
{
	event { Subroutine; Beside; }
	actions
	{
		Global.Started += 1;
		Wait(0.050, Ignore Condition);
		Modify Global Variable(Order, Append To Array, 4);
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    game.globals.insert(0, Value::Array(Vec::new()));
    game.players.push(Player::new(5, 0));
    run(&mut engine, &mut game, 1);
    assert_eq!(game.player_variables[&(5, 0)], Value::Number(7.0));
    let order =
        |game: &Match| -> Vec<f64> { game.global(0).array().into_iter().map(number).collect() };
    assert_eq!(order(&game), [1.0, 2.0, 3.0]);
    // Started once: the second Start Rule found it running.
    assert_eq!(number(game.global(1)), 1.0);
    assert!(engine.is_running("Beside", Some(5)));
    // 0.05 s = 3.125 ticks: 4 ticks.
    run(&mut engine, &mut game, 4);
    assert_eq!(order(&game), [1.0, 2.0, 3.0, 4.0]);
}

/// Abort ends the action list it is in: in a called subroutine it returns
/// to the caller (C32S2's MLVLicons: a level's icon, then Abort, and
/// GenerateMap goes on), and Abort If does the same; in the rule itself it
/// ends the rule.
#[test]
fn abort_in_a_called_subroutine_returns_to_its_caller() {
    let mut engine = engine(
        r#"
variables
{
	global:
		0: Order
}
subroutines
{
	0: Icons
	1: Text
}
rule("Main")
{
	event { Ongoing - Global; }
	actions
	{
		Call Subroutine(Icons);
		Modify Global Variable(Order, Append To Array, 2);
		Call Subroutine(Text);
		Modify Global Variable(Order, Append To Array, 4);
		Abort;
		Modify Global Variable(Order, Append To Array, 99);
	}
}
rule("Icons")
{
	event { Subroutine; Icons; }
	actions
	{
		Modify Global Variable(Order, Append To Array, 1);
		Abort;
		Modify Global Variable(Order, Append To Array, 99);
	}
}
rule("Text")
{
	event { Subroutine; Text; }
	actions
	{
		Modify Global Variable(Order, Append To Array, 3);
		Abort If(True);
		Modify Global Variable(Order, Append To Array, 99);
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    game.globals.insert(0, Value::Array(Vec::new()));
    run(&mut engine, &mut game, 1);
    let order: Vec<f64> = game.global(0).array().into_iter().map(number).collect();
    assert_eq!(order, [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(engine.running(), 0);
}

#[test]
fn for_while_break_and_continue() {
    let mut engine = engine(
        r#"
variables { global: 0: Sum 1: I 2: Count 3: Down 4: J }
rule("Loops")
{
	event { Ongoing - Global; }
	actions
	{
		Global.Sum = 0;
		For Global Variable(I, 0, 10, 1);
			If(Global.I == 3);
				Continue;
			Else If(Global.I == 6);
				Break;
			End;
			Global.Sum += Global.I;
		End;
		Global.Count = 0;
		While(Global.Count < 4);
			Global.Count += 1;
		End;
		Global.Down = Empty Array;
		For Global Variable(J, 5, 0, -2);
			Modify Global Variable(Down, Append To Array, Global.J);
		End;
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 12.0, "0 + 1 + 2 + 4 + 5");
    assert_eq!(number(game.global(1)), 6.0);
    assert_eq!(number(game.global(2)), 4.0);
    let down: Vec<f64> = game.global(3).array().into_iter().map(number).collect();
    assert_eq!(down, [5.0, 3.0, 1.0]);
}

/// Each rising edge starts a run, but one that comes while the rule runs
/// is spent: the rule doesn't start again when that run ends (C32S2's
/// "Spectate" waits for its chord pressed again and must end there).
#[test]
fn ongoing_rules_run_on_each_rising_edge_of_their_conditions() {
    let mut engine = engine(
        r#"
variables { global: 0: Presses 1: Waited }
rule("Interact")
{
	event { Ongoing - Each Player; All; All; }
	conditions { Is Button Held(Event Player, Button(Interact)) == True; }
	actions { Global.Presses += 1; }
}
rule("Ability 2")
{
	event { Ongoing - Each Player; All; All; }
	conditions { Is Button Held(Event Player, Button(Ability 2)) == True; }
	actions
	{
		Wait Until(!Is Button Held(Event Player, Button(Ability 2)), 99999);
		Wait Until(Is Button Held(Event Player, Button(Ability 2)), 99999);
		Global.Waited += 1;
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    game.players.push(Player::new(1, 0));
    run(&mut engine, &mut game, 1);
    game.player_mut(1).buttons.insert(INTERACT);
    run(&mut engine, &mut game, 5);
    assert_eq!(number(game.global(0)), 1.0, "held: one run");
    game.player_mut(1).buttons.clear();
    run(&mut engine, &mut game, 1);
    game.player_mut(1).buttons.insert(INTERACT);
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 2.0);
    // Pressed, released, pressed again: the second press ends the run.
    game.player_mut(1).buttons.insert(ABILITY_2);
    run(&mut engine, &mut game, 3);
    game.player_mut(1).buttons.remove(&ABILITY_2);
    run(&mut engine, &mut game, 3);
    game.player_mut(1).buttons.insert(ABILITY_2);
    run(&mut engine, &mut game, 10);
    assert_eq!(number(game.global(1)), 1.0);
    // Still held after the run: no new one began (it would still wait).
    game.player_mut(1).buttons.remove(&ABILITY_2);
    run(&mut engine, &mut game, 3);
    game.player_mut(1).buttons.insert(ABILITY_2);
    run(&mut engine, &mut game, 10);
    assert_eq!(number(game.global(1)), 1.0, "a new run waits for the chord");
}

#[test]
fn wait_behaviours_abort_and_restart_with_the_conditions() {
    let mut engine = engine(
        r#"
variables { global: 0: Begun 1: Done 2: Gate 3: Runs 4: Finished }
rule("Abort When False")
{
	event { Ongoing - Each Player; All; All; }
	conditions { Is Button Held(Event Player, Button(Interact)) == True; }
	actions
	{
		Global.Begun += 1;
		Wait(1, Abort When False);
		Global.Done += 1;
	}
}
rule("Restart When True")
{
	event { Ongoing - Global; }
	conditions { Global.Gate == True; }
	actions
	{
		Global.Runs += 1;
		Wait(1, Restart When True);
		Global.Finished += 1;
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    game.players.push(Player::new(1, 0));
    game.player_mut(1).buttons.insert(INTERACT);
    game.globals.insert(2, Value::Bool(true));
    run(&mut engine, &mut game, 2);
    assert_eq!(number(game.global(0)), 1.0);
    assert_eq!(number(game.global(3)), 1.0);
    // Released mid-wait: aborted. The gate closes and opens again:
    // restarted from the top.
    game.player_mut(1).buttons.clear();
    game.globals.insert(2, Value::Bool(false));
    run(&mut engine, &mut game, 2);
    game.globals.insert(2, Value::Bool(true));
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(3)), 2.0);
    // 1 s = 62.5 ticks: 63 after the restart.
    run(&mut engine, &mut game, 62);
    assert_eq!(number(game.global(4)), 0.0);
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(4)), 1.0);
    assert_eq!(
        number(game.global(1)),
        0.0,
        "the aborted wait never finished"
    );
}

#[test]
fn player_events_run_for_their_player_and_filters() {
    let mut engine = engine(
        r#"
variables { global: 0: Deaths 1: Killer 2: TeamTwoDeaths }
rule("Died")
{
	event { Player Died; All; All; }
	actions
	{
		Global.Deaths += 1;
		Global.Killer = Attacker;
	}
}
rule("Team 2 died")
{
	event { Player Died; Team 2; All; }
	actions { Global.TeamTwoDeaths += 1; }
}
"#,
    );
    let mut game = Match::new(catalog());
    game.players.push(Player::new(1, 0));
    game.players.push(Player::new(2, 1));
    run(&mut engine, &mut game, 1);
    engine.fire(GameEvent {
        kind: events::PLAYER_DIED,
        context: Context {
            event_player: Some(2),
            victim: Some(2),
            attacker: Some(1),
            ..Context::default()
        },
    });
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 1.0);
    assert_eq!(game.global(1), Value::Entity(1));
    assert_eq!(number(game.global(2)), 0.0, "player 2 is in Team 1");
}

#[test]
fn a_rule_waits_for_its_action_graph_to_finish() {
    let mut engine = engine(
        r#"
variables { global: 0: After }
rule("Message")
{
	event { Ongoing - Global; }
	actions
	{
		Small Message(All Players(All Teams), Custom String("{0} / {1}", 1, 2));
		Global.After = 1;
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    game.players.push(Player::new(1, 0));
    game.durations.insert("Small Message".into(), 3);
    run(&mut engine, &mut game, 3);
    assert_eq!(number(game.global(0)), 0.0);
    let started = game.started.clone();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].params[0], Value::Array(vec![Value::Entity(1)]));
    let Value::Text(text) = &started[0].params[1] else {
        panic!("a text parameter");
    };
    assert_eq!(text.render(&catalog()), "1 / 2");
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 1.0);
}

#[test]
fn operators_follow_the_writers_precedence() {
    let mut engine = engine(
        r#"
variables { global: 0: X 1: Y 2: Z 3: W }
rule("Values")
{
	event { Ongoing - Global; }
	actions
	{
		Global.X = 1 + 2 * 3 ^ 2 - 4 / 2;
		Global.Y = True || False && False ? 5 : 6;
		Global.Z = Array(10, 20, 30)[1] + Count Of(Filtered Array(Array(1, 2, 3, 4), Current Array Element % 2 == 0));
		Global.W = -2 + Absolute Value(-3) * Distance Between(Vector(0, 0, 0), Vector(3, 4, 0));
	}
}
"#,
    );
    let mut game = Match::new(catalog());
    run(&mut engine, &mut game, 1);
    assert_eq!(number(game.global(0)), 17.0);
    assert_eq!(number(game.global(1)), 5.0);
    assert_eq!(number(game.global(2)), 22.0);
    assert_eq!(number(game.global(3)), 13.0);
}
