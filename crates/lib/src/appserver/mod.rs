//! JSON-RPC 2.0 transport over stdio for the codex-app-server protocol subset.
//!
//! Only the symmetric transport (`rpc`) remains here: kessel drives a backend
//! agent as an ACP *client* (see [`crate::acp_client`]), reusing this same
//! `Connection` + `serve` to send `initialize`/`thread/start`/`turn/start` and
//! handle the backend's inbound `item/tool/call` and approval requests. The
//! in-process server (the old `kessel-cli app-server`) was removed when the
//! agent core moved to the standalone backend — see docs/REFACTOR.md.

pub mod rpc;
