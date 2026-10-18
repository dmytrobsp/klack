//! Core Slack client logic for slacktui: sessions, Web API, real-time events and the
//! data model. Contains no terminal code.

pub mod api;
pub mod format;
pub mod rtm;
pub mod session;
pub mod types;

pub use api::Client;
