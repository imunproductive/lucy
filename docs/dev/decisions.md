# Decisions and history

The fork was rebuilt from a Python server with a Rust port into one Rust workspace (the refactor plan, finished and deleted on 2026-11-11). This page keeps what that plan decided and still applies. The Python server is reachable at the local git tag `python-reference`.

## Conventions

- **Module names** drop the old prefixes: `game_bots` becomes `sim::bots`, `contact_gjk` becomes `physics::contact::gjk`, `jam_codec` becomes `wire::jam::codec`.
- **File size:** aim for under 1,500 lines. Split by concern and keep tests in separate `tests.rs` or `tests/` modules, not at the bottom of a 7,000-line file.
- **Errors:** library crates keep their typed or `io::Error` errors. The app reports `io::Error`s.
- **Logging:** `tracing`, with a file appender in the state folder. No `println!` outside the CLI.
- **CLI parsing:** `clap` with the derive API.
- **Dashboard HTTP:** a small HTTP/1.1 server of our own (`server/dashboard/http.rs`) on the shared TCP worker model (`wire::tcp`), synchronous like the rest. It replaced `tiny_http` (2026-11-22), whose detached threads, missing timeouts and missing connection limit did not fit the connection hygiene of `server-architecture-plan.md`. The JavaScript files are embedded into the binary.
- **Developer tools:** stable Rust with the MSVC toolchain, Node.js (only for the dashboard test) and [`just`](https://github.com/casey/just). The `justfile` uses PowerShell on Windows. Every recipe is a short list of plain `cargo` or `node` commands, so each can also be run by hand.
- **Network:** the program opens only its own listeners and the relay pipe. It never downloads anything.
- **Target build:** `.build.info` is checked at startup, and any build other than 104319 is refused with a clear message.
- **Tests:** see [testing.md](testing.md).
- **Public repo data policy:** no client binaries, dumps, captures, keys or prepared data unless decision D1 explicitly allows a fixed-data item.

## Fixed data

These inputs can't be read from the install while the server runs. They are fixed for build 104319 and committed in `crates/fixed/data` (decision D1) and embedded in the binary; [data-sources.md](data-sources.md) records their source and SHA-256, and `D:/Work/Coding/refs/reports/RepoData.md` the audit of every committed data file (2026-11-23).

| Item | File | Why it isn't derived |
| --- | --- | --- |
| Statescript payload codecs | `statescript_codecs.json` | Our description of what the client's reader code reads per state class, generated with IDA from a client memory image. The program's code is encrypted on disk, so the server can't read it there. |
| Captured server templates | `retail_templates.json` | Server-side data, decoded from 1.68 reference traffic; never in the client. Being replaced by messages our code builds (agent TemplatesGone). |
| Server-set queue and Arcade card fields | Parts of `game_queues_174.json` and `arcade_cards_174.json` | The `game_queues` derivation reads the captured templates. Arcade card duration and permanence are set by the server, not the card asset. |
| Rules from client code used by derivations | Constants in the derivations, for example the placeable type flag table in `crates/assets/src/game_maps.rs` | Facts about compiled code. |
| Research names | `statescript_names.json` | Research-only naming catalog (class aliases, the STU fields that hold a variable, logical buttons, the hero select and HUD graphs, each mode's scripts, the server's variable names), used by `lucy script query`. Upstream research (LizRollin's `statescript_names_174.json`), not read from the client. |

Derived since 2026-11-23 (`lucy-assets` derivations `client` and `resource_keys`): the lobby and game link message layouts, the Battle.net descriptors and the lobby state-blob key from the client program's data sections (stored plain on disk), and the event resource keys from the install's `090` assets (D2).

## Decisions

### Decided

| Decision | Result |
| --- | --- |
| CI and CD | Out of scope for now; the inherited workflow was deleted. |
| Binaries | Users build from source. The launcher injects only the DLL from the same build. |
| Relay | Ported to Rust; the C++ version was deleted after the live login test passed. |
| Game data | Read from CASC at run time, except the fixed data above |
| Tools | Ported (`fake_client`, `ssq`), kept as research (the IDA script) or deleted |
| Dashboard | The JavaScript front end stays; the backend is Rust. No WebAssembly. |
| Runtime | No async runtime. `mio` and worker threads stay. |
| Target | Build 104319 only |
| D1 Fixed data | Committed to `crates/fixed/data` and embedded in the binary (decided 2026-11-10). The repository owns the files, so names and formats may change. Only what the server can't derive from the install at run time stays (2026-11-23). |
| D2 Resource keys | Derived from the install's `090` assets (2026-11-23): all 189 are there, with the shipped bytes. |

### Open

None.

## Upstream ports

What was taken from upstream ORPHEUS after the fork's sync (the plan: `D:/Work/Coding/refs/reports/UpstreamPort-2026-11-17.md`), and how it was fitted. The credits are in [CREDITS.md](../../CREDITS.md).

| Item | Upstream | Result |
| --- | --- | --- |
| Lobby defaults and safety | kjs `1819125b`, main `8a464026`, `fix/chat-reply-traceback` `212d85d0` | Tips Display on by default. The `+0x130` entries are kept in the wire's form (value u64, key u32, as the lobby schema types them), not upstream's signed form: our encoder refuses a negative value for an unsigned field. A failing chat command logs one warning; the bot's answers are cut to 200 characters. |
| Tab board | kjs `254fa913`, `827ef2d7` | Component 64's stat GUIDs go as ids. The mode entity's hand-built frame lists the mode graph with its `06DF.025` poster state on and `0CB6` under it, for the modes upstream listed (`scene::BOARD_POSTERS`); a graph driven by the VM still sends its own frames. Upstream swapped the names of 0x1D/0x1E and 0x07/0x09; the mode data (`0C5` `m_teams`) says 0x1D is Team Deathmatch (`0F60`), 0x1E Deathmatch (`0F73`), 0x07 Capture the Flag (`0CE8`), 0x09 Elimination (`0D0B`). Our names were right; the graph per mode id is the same in both. |
| Hitscan pellets and spread seed | kjs `1e2e10c3`, `1dc5b995`; main `9ed63eed` | A hitscan volley with pellet rings fires a ray per pellet (the same ring layout as projectiles, any sync). The volley spread's seed takes the state's `m_states` index, not its node position. |
| Training Bot rebuild and the `#7082` gate | kjs `a667ef27`, kjs-port `a9096f5d` | A bot made again at its spawner gets `0013.025` on its `0BA8` instances before its first frame. A kill between two bots that both hold `#7082` sends no kill feed pulse. |
| PvE start markers | kjs `08561fe4` | `game_maps` lists an entity whose only team tag is 0x310 as a spawn (derivations version 28). |

## Python test classification

Each of the 114 Python test files (at the tag `python-reference`): 78 covered by an existing Rust test, 10 ported to new Rust tests, 26 dropped. Covered tests marked "ignored" run only in the game tier (`just test-game`, `just test-assets`). The dashboard's JavaScript test stays, as `web/tests/test_dashboard_client.mjs`; `tests/golden/messages.json` moved to `crates/lobby/tests/golden/`.

The last differing test, `TakeOutTests::test_a_player_taken_out_before_his_game_connects...` in `test_leaving.py`, is ported too: the Rust server now behaves as Python did.

| Python test | Class | Rust test or reason |
| --- | --- | --- |
| `test_accounts.py` | covered | lobby `accounts.rs`: `accounts_keep_nicknames_and_identity_across_restarts`, `starter_boxes_and_ids_match_the_python_profile` |
| `test_arcade.py` | covered | lobby `arcade.rs`: `rotation_counts_groups_instead_of_cards_and_deduplicates_events`, `arcade_and_pass_wire_bytes_match_python_across_events_seasons_and_dates` (ignored) |
| `test_battle_tag_query.py` | covered | lobby `battle_tag_query.rs::gets_the_host_nickname_and_leaves_a_game_hello_unread`; `session/tests.rs::answers_battle_tag_queries_without_creating_unknown_accounts`; app `join.rs::the_battle_tag_comes_from_the_server_and_an_old_server_is_named`, `an_unreachable_server_is_named` |
| `test_bnet_auth.py` | covered | bnet `rpc.rs::connect_logon_and_referral_match_the_python_rpc_frames`; `session_key.rs::session_key_keeps_the_reference_marker_and_utf8_bytes` |
| `test_body_probe_cli.py` | dropped | Tests `tools/probe_body_script.py` (research probe over `research_runtime`); the tool is deleted in phase 8 |
| `test_body_vm_aliases.py` | dropped | Python VM snapshot alias identity (format BODY_VM_SNAPSHOT_V1, deleted); no Rust reader |
| `test_body_vm_snapshot.py` | dropped | Same Python-only body VM snapshot research format |
| `test_bot_chat.py` | ported | server `lobby/social/tests/bot_chat.rs`: `the_bot_answers_only_when_the_switch_is_on` |
| `test_bots.py` | covered | sim `bots/tests.rs`: `backward_walk_brake_and_next_leg_match_python_wire_vectors`, `catalogs_only_unconditional_spawners...`, `fifteen_practice_bots_match_python_roster_and_patrol_frames` (ignored) |
| `test_casc_bundle.py` | covered | casc `tests/storage.rs::reads_stored_and_bundled_assets_by_guid`, `damaged_bytes_fail_their_content_key`; `bundle.rs::reads_each_offset_width` |
| `test_cli.py` | covered | app `ask.rs::the_mode_menu_takes_a_number_and_enter_is_retail`, `answers_are_remembered_and_enter_reuses_them`, `addresses_and_names_are_checked_like_the_reference`; `main.rs::bad_join_options_are_refused_before_anything_starts`; `settings.rs` tests |
| `test_collision_fingerprint.py` | dropped | Python world-fingerprint/registry for replay tapes; research only |
| `test_collision_prop_catalog.py` | covered | assets `tests/maps.rs::collision_props_matches_the_pack`, `collision_matches_the_pack` (ignored) |
| `test_collision_ray.py` | covered | physics `collision/index.rs::ray_preserves_masks_winding_ties_and_inclusive_grid_boundary`, `invalid_inputs_and_extreme_traversal_are_bounded` |
| `test_collision_ray_cache.py` | covered | physics `collision/cache.rs::only_expected_ray_provenance_can_load_a_sidecar` |
| `test_collision_tape.py` | dropped | COLLISION_READ_TAPE_V1 research format (deleted) |
| `test_content_golden.py` | ported | lobby `menu/content_golden.rs::messages_are_unchanged` (golden data in `crates/lobby/tests/golden/messages.json`), `canonical_json_is_written_like_python` |
| `test_dashboard.py` | covered | server `dashboard/api/tests.rs` (14 tests, e.g. `a_rejected_field_leaves_the_saved_profile_unchanged`, `wins_cannot_exceed_matches_in_the_same_queue`); `dashboard.rs::pictures_stay_inside_their_folders` |
| `test_event_announcements.py` | covered | lobby `celebrations.rs::greeting_gives_each_new_event_its_box_and_items_once`, `challenge_rewards_come_once_each_tier_is_won`, `login_messages_match_python_for_events_challenges...` (ignored) |
| `test_events.py` | covered | lobby `celebrations.rs::every_listed_event_preset_is_known`, `prioritizes_event_then_challenge_keys...`; `clock.rs` tests; `templates.rs::preload_appends_only_unlisted_skins_in_input_order` |
| `test_extract_game_graphs.py` | dropped | Tests `tools/extract_game_graphs.py` (deleted); replaced by assets `tests/graphs.rs::hero_bundle_matches_the_pack`, `bot_bundle_matches_the_pack` |
| `test_extract_game_stats.py` | dropped | Tests the deleted extractor; assets `tests/heroes.rs::game_stats_matches_the_pack` is the oracle |
| `test_extract_mode_variables.py` | dropped | Tests the `--mode-xml` extractor (deleted); the Rust `assets::graphs::mode_variables` is checked by the graph pack oracle |
| `test_extract_raycast_metadata.py` | dropped | Tests `tools/extract_raycast_metadata.py`; not a pack file, nothing reads it |
| `test_friends.py` | covered | lobby `social/tests.rs::a_request_waits_then_a_request_back_means_yes_and_is_saved`, `a_declined_request_only_leaves_the_list`, `the_friends_list_has_presence_for_every_friend...`; `presence.rs` tests |
| `test_game_body_recovery.py` | ported | sim `replication/tests/recovery.rs`: `a_same_hero_pick_after_a_fault_replaces_the_body_and_waits_for_fresh_acks`, `healthy_and_scriptless_same_hero_picks_keep_the_body` |
| `test_game_body_script.py` | covered | script `stream/tests.rs::body_full_waits_for_create_and_the_first_owner_delta_retries_unchanged` |
| `test_game_bodytrace.py` | dropped | Python bodytrace research capture files |
| `test_game_bot_notices.py` | covered | sim `replication/tests.rs`: `possession_and_retirement_wait_when_budget_splits_the_prerequisites`, `a_late_ack_or_loss_cannot_resurrect_a_retired_or_reused_body`, `ring_reuses_only_acknowledged_destroy...`, `bot_health_replaces_stale_packets...` |
| `test_game_bot_statescript.py` | covered | script `bot.rs` tests; sim `bot_script/tests.rs` (3, ignored); script vm `ai_fire_holds_its_slot_button_until_it_ends`; snapshot `kill_feed_pulse_waits_for_the_world...` |
| `test_game_collision_ready.py` | ported | server `game_service/tests/collision_ready.rs`: `a_failed_collision_sends_connected_games_home_once_and_drops_the_others`, `seats_wait_for_their_scene_while_another_match_plays`, `a_cache_only_server_fails_a_missing_map_and_a_builder_still_builds_it`; physics `tests/collision_ready.rs::ready_movement_cannot_cross_the_wall` |
| `test_game_combat.py` | covered | sim `hitscan.rs` (3 tests), `combat/tests.rs`, `projectile_flight.rs`; script `vitals.rs::shield_armour_health_order...`; server `a_player_kills_another_who_respawns...` (ignored) |
| `test_game_command_epochs.py` | covered | wire `commands.rs::queue_admits_only_forward_commands_and_resets_only_explicitly`, `queue_limits_use_the_body_anchor_and_leave_wraps_for_later`; sim `replication/tests.rs::pending_queue_closes_after_the_prefix...` |
| `test_game_command_handoff.py` | covered | sim `replication/tests.rs::native_command_window_keeps_duplicates...`; server `ingress.rs::a_body_change_is_visible_when_the_returned_commands_are_applied` |
| `test_game_commands.py` | covered | wire `commands.rs::matches_the_existing_forward_and_turn_fixture` (cites this file), `matches_a_python_record_with_wrap_deltas_and_clicks` |
| `test_game_contacts.py` | covered | physics `contact/gjk.rs`, `solver.rs::clips_against_one_two_and_many_planes_in_order`, `gjk/sweep.rs` (4), `cache.rs::body_casts_match_separated_and_shrinking_overlap_fractions` |
| `test_game_correction.py` | covered | physics `mover/correction.rs::selects_one_newest_eligible_pose_after_create_ack`, `preserves_the_python_correction_bytes_for_all_input_frame_encodings` |
| `test_game_entity_lifecycle.py` | covered | sim `body.rs::owner_create_captures_pose_golden_skin_and_lease_in_python_order`, `observer_create_freezes_damaged_pools...`; `replication/tests.rs::body_script_retries_frozen_initial_frames...` |
| `test_game_environment.py` | dropped | ENVIRONMENT_READ_TAPE_V1 injection (deleted format); the handoff timing is covered separately by server `connection.rs` tests |
| `test_game_environment_tape.py` | dropped | Same research tape format |
| `test_game_global_clock.py` | covered | server `clock.rs::drains_one_tick_at_a_time_and_waits_for_the_last_report`, `preserves_tick_alignment_across_a_stall`; the journal-replay parts are Python-only |
| `test_game_graph_analysis.py` | dropped | Tests `ow174/game/script/analysis.py` for `tools/analyze_game_graphs.py` (deleted) |
| `test_game_health.py` | covered | wire `entity/tests.rs::sparse_pool_update...`, `mixed_frame_matches_python...`; sim `pools.rs` tests |
| `test_game_hit_timing.py` | covered | sim `hitscan.rs::uses_click_aim_and_interpolates_view_time_across_wrap`; `body_script/tests.rs::posed_volley_keeps_the_authored_shot_frame...` |
| `test_game_link.py` | covered | wire `link.rs::matches_python_datagrams_in_both_directions`, `rejects_tampering_and_wrong_directions`, `acknowledges_arrivals_but_drops_late_packets` |
| `test_game_match_screens.py` | covered | content `stats.rs::medals_exclude_flagged_stats...`, `owner_scoreboard_writes_full_slots...`; sim `others.rs::another_player_comes_switches_and_leaves_in_match_order` |
| `test_game_move_to.py` | covered | physics `mover.rs::world_target_moves_arrive_or_report_blocking`; script vm `move_to_exposes_only_a_fixed_target_while_active`, `finished_movement_ends_the_move_to_state_at_the_next_advance` |
| `test_game_move_to_replay.py` | covered | Same MoveTo tests as above |
| `test_game_movelog.py` | covered | Command-ack part: physics `correction.rs::caps_ack_while_waiting_for_the_first_pose_or_create`; the movelog file writer is a Python debug log |
| `test_game_mover.py` | ported | physics `mover/ported_tests.rs` (box worlds: run, stop in 5 ticks, slides, corners, steps, slopes, ceilings, falls, sprint and crouch timing); assets `collision/ported_tests.rs`, `tests/mover_spawns.rs`; sim `scene_assets/ported_tests.rs` |
| `test_game_notices.py` | covered | sim `notices.rs::cards_carry_the_python_flags`, `only_the_0c90_switch_modes_post_hero_switches`; `others.rs::teammates_take_each_hero_switch_once` |
| `test_game_onfire.py` | covered | content `stats.rs::on_fire_updates_match_python_and_leave_other_fields_unwritten` (asserts `meter` too) |
| `test_game_owner_delta_loss.py` | covered | script `stream/tests.rs::older_positive_ack_keeps_newer_lost_changes...`, `a_superseded_owner_report_still_resolves...` |
| `test_game_packet_budget.py` | covered | wire `entity/tests.rs::budget_reuses_trimmed_space...`, `record_cap_keeps_fifo...`; sim `egress/tests.rs` (8), `replication/tests.rs::owner_correction_respects_create_budget...` |
| `test_game_projectiles.py` | covered | sim `projectiles.rs` (9), `projectile_wire.rs` (4); server `another_client_sees_a_players_helix_rocket` (ignored) |
| `test_game_repeated_lifecycles.py` | covered | sim `replication/tests.rs::ring_reuses_only_acknowledged_destroy...`; server `prepared_world_waits_for_ownership_and_retries_baselines_before_scripts` (ignored) |
| `test_game_replay_manifest.py` | dropped | LOCAL_REPLAY_MANIFEST_V1 research format |
| `test_game_script_faults.py` | ported | sim `body_script/fault_tests.rs::an_exhausted_weapon_stops_the_body_before_combat_consumes_its_shots` |
| `test_game_script_replication.py` | dropped | Tests the `research_runtime` replication path (body.py diagnostic/experimental modes); the observer death is covered by server `a_player_kills_another...` |
| `test_game_script_runtime.py` | dropped | Tests `ow174/game/script/research_runtime.py`; Rust uses `lucy_script::vm` |
| `test_game_server.py` | covered | server `game_service/tests.rs`: `worker_serves_a_match_seat...`, `players_join_a_running_match_in_the_next_slots`, `two_players_in_one_practice_range_see_each_other` (ignored) |
| `test_game_simulation.py` | dropped | CommandTimeline/replay over `data/replays/basic.json`, which phase 8 deletes |
| `test_game_statescript.py` | covered | script `wire.rs::full_owner_and_ack_match_python...`; content `parameters.rs` pick tests (cite this file); sim `scene.rs::mode_ui_preserves_its_shared_variable_state...` (ignored) |
| `test_game_team_list.py` | covered | sim `controller.rs::team_entry_exact_wire_bytes_with_and_without_pick`, `others.rs::new_players_wait_for_a_card_slot_and_cards_go_again`, `instance.rs` tests |
| `test_game_tick.py` | covered | server `clock.rs::preserves_tick_alignment_across_a_stall`, `sends_once_for_every_update_that_crosses_a_world_boundary` |
| `test_game_to_game.py` | covered | server `connection.rs::authenticated_syn_opens_on_ack_and_fin_retires_after_reply`, `only_fresh_authenticated_packets_rebind...`; `matchmaker/tests.rs::one_player_to_start_pops_the_queue...` (ignored) |
| `test_game_voicelines.py` | dropped | The spawn bark (the body's first networked VoiceStimulus on for 4 s) is not in the data; removed 2026-11-17 for `01CA`'s respawn line (script `player.rs::the_hero_says_his_elimination_and_respawn_lines`) |
| `test_game_world.py` | covered | wire `entity/tests.rs::placeable_create_and_pose_match_python` (cites this file), `movement_timing_sentinels_and_signed_fields_match_python` |
| `test_group_finder.py` | covered | lobby `social/tests.rs::a_listed_group_is_found_until_it_is_full`, `search_filters_pick_the_groups`, `group_limits_hide...`, `the_bot_group_takes_the_first_slot...`, `watchers_hear_a_group_until_it_closes` |
| `test_launcher.py` | covered | launcher `game.rs::resolves_the_game_from_its_folder_or_the_official_launcher`, `finds_the_given_game_then_the_remembered_one...`, `prefers_the_system_language...` (gap: `close_running_copy` is untested) |
| `test_launcher_inject.py` | covered | launcher `tests/inject.rs::the_relay_loads_once_and_uses_the_given_port` (ignored); tasklist parsing is Python-only (Rust uses Toolhelp) |
| `test_leaderboard.py` | covered | lobby `leaderboard/tests.rs` (2); `mode_rules.rs::preserves_played_hero_order_and_adds_role_wins...`; `player_messages.rs::hello_uses_account_session_nickname_and_region` |
| `test_leaving.py` | ported | server `lobby/matchmaker/ported_tests.rs` (e.g. `the_leader_takes_the_party_that_is_in_his_match_and_it_stays_a_party`, `leaving_the_game_after_the_server_sent_it_home_keeps_the_party`). and `a_player_taken_out_before_his_game_connects_leaves_at_once_and_only_gets_20304` (also in `game_service::tests`) |
| `test_lineup.py` | covered | lobby `lineup.rs::roster_slots_count_within_each_team`, `the_messages_encode_with_the_lobby_schemas`; sim `instance.rs::instance_preserves_ranked_type...` |
| `test_lobby_matches.py` | covered | lobby `session/tests.rs::negotiates_pings_and_accepts_practice_messages_after_login`; server `practice.rs::practice_request_hands_off...` |
| `test_lobby_purchase.py` | covered | lobby `collection.rs` (asserts the 24306 unlock); server `server.rs::lobby_services_answer_the_game...` (ignored); dashboard `a_purchase_charges_its_own_currency...` |
| `test_lobby_smoke.py` | covered | lobby `session/tests.rs::prepared_subset_matches_the_python_login_in_wire_order` (ignored); server `serial_native_server_logs_in_saved_profile...` (ignored) |
| `test_lootbox.py` | covered | lobby `lootbox/tests.rs` (6, e.g. `an_event_box_draws_its_first_slot_from_the_event...`, `the_box_messages_match_python`) |
| `test_matchmaker.py` | covered | lobby `matchmaker.rs` (12, e.g. `a_party_stays_on_one_team...`, `flex_players_take_the_role_that_is_missing`); `handoff.rs::uses_the_reached_address...` |
| `test_menu_npc.py` | covered | lobby `menu.rs::player_portrait_and_event_model_keep_their_separate_choices` |
| `test_native_replay.py` | dropped | NATIVE_BODY_REPLAY_V2 and `tools/replay_native_body.py` |
| `test_ownership.py` | covered | lobby `collection.rs::new_hero_items_keep_catalog_order_and_stay_out_of_account_ownership`, `equips_account_icons...` |
| `test_party.py` | covered | lobby `social/tests.rs::an_unanswered_invite_times_out...`, `a_party_has_one_leader...`, `friends_see_the_chosen_status...`; `party.rs::pending_tiles_use_a_timer...` |
| `test_physics_snapshot.py` | dropped | PHYSICS_SNAPSHOT_V1 research format |
| `test_practice_probe.py` | dropped | Tests `tools/probe_practice.py` and the `inject.jsonl` research watcher (not ported, per parity.md) |
| `test_prop_entity_model_refs.py` | covered | ENTITY-chunk model refs, checked through assets `tests/maps.rs::collision_props_matches_the_pack` (ignored) |
| `test_queue.py` | ported | lobby `tests/queue.rs::the_entry_carries_the_request_key`; server `lobby/matchmaker/ported_tests.rs` (`a_role_queue_starts_with_a_role_check`, `a_group_waits_for_everyone`, `decline_takes_the_party_out_of_the_queue`, `a_priority_pass_is_held_by_the_search_and_given_back`, `cancel_takes_the_others_out_too`, ...) |
| `test_range_model.py` | dropped | `ow174/game/range_model.py` is not imported by any server code (research model) |
| `test_ranked.py` | covered | lobby `ranked.rs::queue_selection_depends_on_season_role_and_event_card`, `placements_use_saved_skill...`, `state_card_and_party_rating_messages_match_python_wire_vectors`; `clock.rs` STU dates |
| `test_region.py` | covered | content `catalog.rs::rewrites_nested_store_currency_prices_and_country_without_mutating_capture`; lobby `templates.rs::real_templates_and_localized_login_match_python_wire_hashes` |
| `test_replay_values.py` | dropped | Replay value codec used only by the journal/tape formats |
| `test_research_injection.py` | dropped | Research packet injection; parity.md marks it not ported |
| `test_script_abilities.py` | covered | script vm `isolated_soldier_sprint_ability...`, `authored_helix_uses_its_six_second_cooldown...` (ignored), `ultimate_fills_over_its_seconds...`; sim `the_ultimate_charges_on_command_frames...` |
| `test_script_driver.py` | covered | script `wire.rs::tagged_values_match_the_python_writer`, `full_owner_and_ack_match_python...`; snapshot `larger_masks_preserve...`, `actual_shared_ui_graph_baselines_match_python` (ignored) |
| `test_script_event_sequence.py` | covered | script `timeline.rs::preserves_due_time_fifo_and_instance_turns`, `bounded_pending_events_and_cancellation_reclaim_slots` |
| `test_script_expr.py` | covered | script `expr/tests.rs` (13, e.g. `short_circuit_preserves_the_deciding_operand...`, `authored_controller_hud_and_soldier_config_values_match_python`); `rulesets.rs::the_mode_schemas_answer_before_the_heroes` |
| `test_script_expr_budget.py` | covered | script `expr/tests.rs::executes_only_the_selected_path_and_bounds_nested_expressions` (truncated opcode, jump leaves stream) |
| `test_script_membership.py` | ported | script `vm/membership_tests.rs`: `config_matches_members_and_singletons_with_vm_equality`, `keyed_config_leaves_its_operands_to_the_existing_native_query` and the authored 0254/01C7 condition tests |
| `test_script_message_targets.py` | covered | script vm `messages_to_other_entities_go_through_the_world...`; sim `replication/tests.rs::a_body_message_runs_the_named_bodys_entries`, `body_messages_to_unknown_entities...` (gap: the 064A visor award is untested) |
| `test_script_mode_variables.py` | covered | script `graph.rs::reads_gzip_and_rejects...` (asserts typed `mode_variables`) (gap: the Assault 0BA4 gate is untested) |
| `test_script_pulser.py` | covered | script vm `pulser_counts_every_begin_and_marks_pulses_after_the_first_frame_fresh`; snapshot `kill_feed_pulse_waits_for_the_world...` |
| `test_script_raw_payloads.py` | dropped | `ow174/game/script/raw_payloads.py` is unused by the server (the test asserts it registers no writer); Rust never sends 9935FBC8/861D9015 |
| `test_script_raycast.py` | covered | script vm `keyed_set_var_replaces_equal_keys_and_an_uncastable_ray_only_follows_its_plug` (the no-provider path only) |
| `test_script_runtime.py` | covered | script vm `authored_soldier_rifle_holds_releases_and_reloads_on_the_reference_clock` (ignored); `timeline.rs::command_edges_preserve_aliases...`; sim `prepared_soldier_starts_the_reference_body...` (ignored) |
| `test_script_runtime_budget.py` | covered | script vm `unsupported_branches_are_reported_and_cyclic_actions_are_bounded`, `startup_queues_states_rechecks_scoped_writes...` |
| `test_server_journal.py` | dropped | CONTROLLED_SERVER_JOURNAL_V1/V2 research format |
| `test_server_snapshot.py` | dropped | SERVER_STORED_STATE_V1 research format |
| `test_shop.py` | covered | lobby `shop/tests.rs` (4: currencies, team partner, refusals, missing partners) |
| `test_ssq.py` | ported | app `script_query/tests.rs` (game tier, e.g. `presence_bits_in_order`, `who_starts_the_hero_select_graphs`, `each_mode_names_its_controller`) |
| `test_static_ray_round.py` | covered | physics `static_ray.rs` (5); `collision/scene.rs::expanded_scene_checks_provenance...` |
| `test_statescript_codecs.py` | covered | script `codecs.rs` (4, `samples()` cites this file), including `actual_payloads_match_python_in_owner_and_both_observer_modes`; the coverage report belongs to `codec_coverage.py` |
| `test_two_games.py` | covered | server `platform.rs` (2); `server.rs::battle_net_logs_a_second_game_in_as_its_account...`; lobby `login.rs::login_uses_tournament_name_or_retail_session_key` (gap: the `SECOND_GAME_ARGUMENTS` and bnet-port arguments are untested) |
| `test_ux_states.py` | covered | lobby `saved_settings/tests.rs::ux_states_and_season_intros_save_only_changes`; `player_messages.rs::saved_ux_values_replace_defaults...` |
| `test_wire_vectors.py` | covered | wire `jam/cipher.rs::matches_the_python_cipher_vector` (cites this file), `handshake.rs::state_blob_matches_reference...`, `codec.rs::lobby_wire_vectors_match_python`, `bools_cross_struct_and_array_edges...` |
