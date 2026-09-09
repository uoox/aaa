//! aaa-daemon library crate (the `aaa-daemon` binary is a thin shim).

/// 协议语义版本（`GET /health` 的 `schema`，PROTOCOL「版本兼容」）。**只在语义搬家时 +1**
/// ——某个判定从客户端搬进 daemon、某个字段改口径；纯新增可选字段不动它。客户端拿它当
/// 唯一的兼容闸门：够新就直接用 daemon 算好的，太旧就明说「daemon 版本过旧」，
/// **不许悄悄退回自己那套**——留一套影子实现，就等于把刚删掉的分歧又养回来。
///
/// - 2（v1.22）：`/projects` 每行带 `status` / `title` / `session_id` / `updated_at`；
///   会话对象带 `asking_seq` / `checklist`。
pub const SCHEMA: u32 = 2;

pub mod agents;
pub mod answer;
pub mod api;
pub mod cache;
pub mod config;
pub mod daemon;
pub mod events;
pub mod feed;
pub mod history;
pub mod hooks;
pub mod inbox;
pub mod messages;
pub mod migrate;
pub mod namer;
pub mod pair;
pub mod paths;
pub mod perms;
pub mod pool;
pub mod quota;
pub mod registry;
pub mod rootcheck;
pub mod screen;
pub mod service;
pub mod slug;
pub mod stores;
pub mod summary;
pub mod trust;
