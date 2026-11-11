//! The match simulation for retail build 104319: bodies, bots, combat, projectiles and replication.

#![forbid(unsafe_code)]

pub mod ai;
pub mod body;
pub mod body_script;
pub mod bot_animation;
pub mod bot_script;
pub mod bots;
pub mod combat;
pub mod controller;
pub mod egress;
pub mod hitboxes;
pub mod hitscan;
pub mod instance;
pub mod match_flow;
pub mod mode_graph_values;
pub mod mode_rules_data;
pub mod mode_script;
pub mod notices;
pub mod objective_data;
pub mod objectives;
pub mod others;
pub mod pools;
pub mod projectile_flight;
pub mod projectile_wire;
pub mod projectiles;
pub mod replication;
pub mod scene;
pub mod scene_assets;
pub mod voice;
pub mod weapon_spread;
