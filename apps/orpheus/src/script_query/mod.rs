//! `orpheus script query`: the port of `tools/ssq.py`. Queries the 1.74
//! statescript graphs the server has (the derived graph bundle): which graphs
//! a hero or a mode runs, what a graph's nodes do, who reads and writes a
//! variable, where a node class is used.
//!
//! Each command returns its lines, as `run` in `tools/ssq.py` does.

mod commands;
mod data;
mod index;
mod json;
mod render;
mod show;
mod views;

#[cfg(test)]
mod tests;

use std::{
    io::{self, Write},
    path::Path,
};

use clap::{Args, Subcommand};

use data::{Bundle, Hero, Names};
use index::Index;
use json::{Int, J};

pub const ABOUT: &str = "\
Query the 1.74 statescript graphs the server has (the graph bundle derived from the install):
which graphs a hero or a mode runs, what a graph's nodes do, who reads and writes a variable,
where a node class is used.

    orpheus script query graphs --hero soldier
    orpheus script query show 0255 --state-index 1 --depth 1
    orpheus script query var 699 --graph 0255
    orpheus script query class MovementMod
    orpheus script query find \"hero select\"

Notation: #699 is instance variable 699 and #7644e entity variable 7644 (as the VM writes them);
rid is a node's m_nodes index (what owner frames name), st its m_states index, remote its
m_remoteSyncNodes index. Graphs are hex (0255), variables decimal (699), GUIDs index.type (02BB.01C).

Roles: body (a hero body's initial graph; ability when it has an Ability state), weapon (the weapon
component's manager and weapon scripts), mode and controller (the game modes' scripts), UI (hero select
and the HUD). A graph that other graphs name (a SubScript or PlayScript target, a graph value) has the
roles of the graphs that name it.

Variable uses: write = the node stores into it (a field the STU types declare as a variable, or the
first variable of an expression in such a field); read = it is evaluated; parent = an expression reads it
in a parent instance; id = an expression uses the identifier itself as a value or key; param = a game
message parameter; list = it is named in a variable list (ClientOnlySharedVars).

Names (class aliases, modes, buttons, variables) come from the embedded statescript_names.json.";

#[derive(Args)]
pub struct QueryArgs {
    /// TankLib/STU/Types of an OWLib checkout, for each field's declared type
    /// and offset.
    #[arg(long, env = "OWLIB_TYPES", value_name = "DIR")]
    pub owlib: Option<std::path::PathBuf>,
    #[command(flatten)]
    pub server: crate::settings::ServerArgs,
    #[command(subcommand)]
    pub command: QueryCommand,
}

#[derive(Clone, Debug, Subcommand)]
pub enum QueryCommand {
    /// Graphs with their role and size.
    Graphs {
        /// A hero's body graphs and the graphs they name.
        #[arg(long)]
        hero: Option<String>,
        /// A game mode's graphs (name or GUID).
        #[arg(long)]
        mode: Option<String>,
        /// A body entity (003 GUID or index).
        #[arg(long)]
        entity: Option<String>,
    },
    /// A graph as a tree.
    Show {
        /// Graph index (hex) or GUID.
        graph: String,
        /// Start at this m_states index (ssq.py `--state`; here `--state`
        /// is the state folder).
        #[arg(long, allow_negative_numbers = true)]
        state_index: Option<i64>,
        /// Follow links this deep.
        #[arg(long, allow_negative_numbers = true)]
        depth: Option<i64>,
        /// Print field values whole (default: cut at 160 characters).
        #[arg(long)]
        full: bool,
    },
    /// Every use of a variable.
    Var {
        /// 699, #699, #7644e (entity scope only), 0x2BB, 02BB.01C or a name.
        var: String,
        /// Only the uses in this graph.
        #[arg(long)]
        graph: Option<String>,
    },
    /// Where a class is used and its fields.
    Class {
        /// Alias, class name or hash.
        name: String,
    },
    /// Names, aliases, hashes, GUIDs.
    Find {
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        /// Lines per kind.
        #[arg(long, default_value_t = 40, allow_negative_numbers = true)]
        limit: i64,
    },
    /// A hero's body, graphs, classes and shared vars.
    Hero { name: String },
}

/// The graphs, the derived hero and map tables, the names catalog and the
/// index over them.
pub struct Query {
    bundle: Bundle,
    names: Names,
    index: Index,
    heroes: Vec<Hero>,
    maps: std::collections::BTreeMap<Int, J>,
}

impl Query {
    /// Reads the derived game data in `data` (the folder that holds
    /// `data/game_graphs_174.json.gz`), and OWLib's STU types when given.
    pub fn load(data: &Path, owlib: Option<&Path>) -> io::Result<Self> {
        let bundle = Bundle::load(data)?;
        let names = Names::embedded();
        let index = Index::build(&bundle, &names, owlib)?;
        Ok(Self {
            heroes: data::heroes(data)?,
            maps: data::maps(data)?,
            bundle,
            names,
            index,
        })
    }

    /// The lines a command prints.
    pub fn run(&self, command: &QueryCommand) -> Vec<String> {
        match command {
            QueryCommand::Graphs { hero, mode, entity } => {
                self.graphs(hero.as_deref(), mode.as_deref(), entity.as_deref())
            }
            QueryCommand::Show {
                graph,
                state_index,
                depth,
                full,
            } => self.show(graph, *state_index, *depth, *full),
            QueryCommand::Var { var, graph } => self.var(var, graph.as_deref()),
            QueryCommand::Class { name } => self.class(name),
            QueryCommand::Find { text, limit } => self.find(text, *limit),
            QueryCommand::Hero { name } => self.hero(name),
        }
    }
}

/// Runs a query on the derived data and prints its lines.
pub fn run(args: &QueryArgs, data: &Path) -> io::Result<()> {
    let query = Query::load(data, args.owlib.as_deref())?;
    let mut out = io::stdout().lock();
    for line in query.run(&args.command) {
        writeln!(out, "{line}")?;
    }
    out.flush()
}
