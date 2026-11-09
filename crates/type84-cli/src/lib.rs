//! Library part of `type84-cli`: full-state reads, the versioned JSON
//! backup format and presets, shared with the GUI. Writes go only through
//! `type84_hid::WriteSession`; [`write`] holds the pre-write backup and diff
//! helpers, [`plan`] turns presets into write plans, [`anim`] holds the
//! state handling of `type84 anim`, [`apply`] the guarded write pipeline
//! shared with the GUI (lock, restore steps, run of a plan).

pub mod anim;
pub mod apply;
pub mod backup;
pub mod lighting_state;
pub mod plan;
pub mod preset;
pub mod snapshot;
pub mod write;
