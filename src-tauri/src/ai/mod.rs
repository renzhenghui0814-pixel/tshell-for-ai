//! The assistant.
//!
//! The plan-act-observe loop and everything it needs: what may run, what the
//! reply meant, where the bytes go, and what is remembered afterwards.
//!
//! The one rule that shapes the layout is that [`session`] must be testable
//! without a machine to talk to. Everything it needs from the outside world
//! arrives through the six traits in [`host`], so the loop's awkward paths -- a
//! refused command, a declined confirmation, a timeout, running out of steps --
//! can be produced on demand rather than provoked out of a real server.

pub mod bridge;
pub mod cancel;
pub mod commands;
pub mod context;
pub mod files;
pub mod host;
pub mod link;
pub mod llm;
pub mod moves;
pub mod parse;
pub mod policy;
pub mod prompt;
pub mod session;
#[cfg(test)]
mod session_tests;
pub mod settings;
pub mod shell;
pub mod store;
pub mod tools;
pub mod types;
