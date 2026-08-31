//! aaa-daemon library crate (the `aaa-daemon` binary is a thin shim).

pub mod agents;
pub mod api;
pub mod cache;
pub mod checkpoint;
pub mod claude_hooks;
pub mod config;
pub mod daemon;
pub mod events;
pub mod inbox;
pub mod messages;
pub mod namer;
pub mod ntfy;
pub mod pair;
pub mod paths;
pub mod perms;
pub mod pool;
pub mod ports;
pub mod registry;
pub mod service;
pub mod slug;
pub mod statemachine;
pub mod stores;
pub mod waiting;
