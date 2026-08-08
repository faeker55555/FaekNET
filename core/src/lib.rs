//! lan_mesh_core -- shared networking engine used by both the CLI
//! (`lan_mesh_cli`) and the native GUI (`lan_mesh_gui`). Everything here is
//! UI-agnostic: config/key management, the wire protocol, encryption,
//! gossip-based peer auto-discovery, STUN self-address-discovery, and the
//! mesh runtime itself (virtual adapter + UDP transport).

pub mod config;
pub mod crypto;
pub mod dns;
pub mod gossip;
pub mod hosts;
pub mod logsink;
pub mod mesh;
pub mod peer;
pub mod proto;
pub mod share;
pub mod stun;
