//! What may run, where bytes may land, and what a model is allowed to read.
//!
//! Every module here is pure: a string goes in, a verdict comes out, nothing is
//! read off a disk or a socket. That is what lets the whole layer be tested
//! against a table of cases, which matters more here than anywhere else in the
//! assistant -- a command judged wrongly in `auto` mode is a safety incident, not
//! a bug report.

pub mod command;
pub mod path;
pub mod redact;
pub mod risk;
