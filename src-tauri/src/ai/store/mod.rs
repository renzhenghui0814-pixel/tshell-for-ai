//! What survives a window being closed.
//!
//! Five stores, five separate files on disk, no shared format between them. That
//! is deliberate: every one of these is something the user is meant to be able to
//! open, read and correct in an editor, so each is written in whatever shape suits
//! it -- markdown for the facts, JSON for the conversations and the trust list, a
//! flat transcript for the log.

pub mod chat;
pub mod log;
pub mod memory;
pub mod skill;
pub mod trust;
