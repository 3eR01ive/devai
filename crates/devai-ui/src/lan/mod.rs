//! LAN access from the phone (task 01a1be98, REQ-LAN-01…10) — module root.
//!
//! The window is a Tauri app whose data door is the command plane in
//! `commands.rs`; there is no other way to the store. The phone therefore
//! does NOT get a second data path: this module puts an embeddable HTTP
//! server in front of the SAME `#[tauri::command]` functions, and a browser
//! page marks itself as the phone transport (the frontend calls `rpc`
//! instead of IPC when it is not inside the Tauri webview). One command
//! plane, two transports.
//!
//! Responsibilities are split into single-purpose modules (quality return
//! R4 — the first revision kept four of them in one 1600-line file):
//!   * [`state`] — the access state machine, invitations and sessions. It is
//!     the ONLY place that names the `off`/`awaiting`/`connected` states;
//!   * [`net`] — the machine's interface list, the LAN address choice and the
//!     network-change watchdog;
//!   * [`server`] — the listener lifecycle (bind, serve, stop, restore);
//!   * [`http`] — the HTTP boundary (routes, asset path normalization, the
//!     RPC envelope) over a [`http::Surface`];
//!   * [`commands`] — the phone's command catalogue: ONE table that is both
//!     the reachable name list and the dispatcher (R10);
//!   * [`instance`] — the single-instance handoff (ratified Q4: the access
//!     outlives the window).
//!
//! Boundaries that are deliberate, not incidental:
//!   * the listener binds to THIS MACHINE'S LAN address, never `0.0.0.0` and
//!     never a loopback bridge, so the surface does not exist outside the home
//!     segment (REQ-LAN-03);
//!   * one-time invitation token → one device session; the token exists only
//!     while the access is on, is shown once, and is never written to logs or
//!     to disk (REQ-LAN-04);
//!   * the phone's writes call the same `Api::open` write handle the window and
//!     the CLI use, and carry a device marker in the actor string
//!     (`<operator> · телефон: <name> (<addr>)`) so history says where a
//!     decision came from without changing WHO signed it (REQ-LAN-06);
//!   * a watchdog re-reads the machine's interface list: when the bound address
//!     disappears the access is turned OFF with a typed reason and is NOT
//!     silently rebound (REQ-LAN-08).
//!
//! Ratified decisions of this task that shape the lifecycle:
//!   * Q1 «должен» — an access that was on before the last shutdown comes back
//!     on the next launch (the flag is persisted, never the token);
//!   * Q2 «может» — several phones may be connected; a new invitation retires
//!     the PREVIOUS invitation, not the sessions already paired;
//!   * Q4 «должен жить» — closing the window does not kill the access: the
//!     close is intercepted while the access is on (the process keeps serving,
//!     a second launch re-shows the window through the instance socket).

pub mod commands;
pub mod deeplink;
pub mod http;
pub mod instance;
pub mod net;
pub mod pair;
pub mod server;
pub mod state;

// The names the window itself (lib.rs) and `commands.rs` use. Everything
// else is reached through its owning module.
pub use instance::InstanceAcquire;
pub use server::restore_at_startup;
pub use state::LanAccess;
