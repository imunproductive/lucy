//! The Workshop of the 1.74 client: its catalog, the rules in their text
//! and binary forms, and the rule engine the retail server ran natively
//! (`D:/Work/Coding/refs/reports/Workshop.md`).
//!
//! The client neither compiles nor runs rules; every action is a
//! statescript graph (`054/0155 m_graph`) that the server starts with the
//! evaluated parameters in `#18739`. So this crate decides when rules run
//! and evaluates values, and hands each action to the VM side through
//! [`host::Actions`], reading the match through [`host::World`] and the
//! entity variables through [`host::Variables`].

#![forbid(unsafe_code)]

pub mod binary;
pub mod catalog;
pub mod engine;
pub mod eval;
pub mod host;
pub mod logic;
pub mod text;
pub mod value;

pub use binary::decode;
pub use catalog::Catalog;
pub use engine::Engine;
pub use logic::Logic;
pub use text::parse;
