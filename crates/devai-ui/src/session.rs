//! Session bootstrap for devai-ui (REQ-UI-01).
//!
//! Opening a home is NOT a registry operation — it is the bootstrap entry
//! `Api::open` (RATIFIED OQ-10; `crates/devai-api/src/lib.rs:57-68`). The UI
//! never touches the store, never calls init/seed: it opens the chosen home
//! through the API and holds the handle inside itself.
//!
//! Resolution chains are the operator-ratified GUI answers:
//!   * home  (OQ-UI-1):  env $DEVAI_HOME, else default ~/.devai.
//!                       No picker dialog / recent-homes in this iteration.
//!   * actor (OQ-UI-8):  the CLI chain — $DEVAI_USER, else the user's env
//!                       variable ($USER), else the default "ui". This
//!                       iteration the UI is read-only, so the actor marks
//!                       the session but no UI write ever records it.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use devai_api::Api;
use serde::Serialize;

/// A resolved home path for the GUI window (OQ-UI-1 chain). `lookup` is
/// injectable so the chain is testable without mutating the process env.
pub fn resolve_home(lookup: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(v) = lookup("DEVAI_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    // Default: ~/.devai (same fallback shape as the CLI's clap default).
    dirs::home_dir()
        .map(|h| h.join(".devai"))
        .unwrap_or_else(|| PathBuf::from(".devai"))
}

/// Actor identity for `Api::open` (OQ-UI-8 chain: DEVAI_USER > USER > "ui").
pub fn resolve_actor(lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let non_empty = |k: &str| {
        lookup(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    non_empty("DEVAI_USER")
        .or_else(|| non_empty("USER"))
        .unwrap_or_else(|| "ui".to_string())
}

/// The open session: API handle + the facts the status bar renders
/// (REQ-UI-01 AC3: the home path must be visible to the operator).
pub struct Session {
    pub api: Api,
    pub info: SessionInfo,
}

#[derive(Serialize, Clone, Debug)]
pub struct SessionInfo {
    pub home: String,
    pub actor: String,
    pub opened_at: DateTime<Utc>,
}

impl Session {
    /// Opens the session with the process environment (the chains above).
    pub fn open_env() -> Result<Session, String> {
        let lookup = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let home = resolve_home(&lookup);
        let actor = resolve_actor(&lookup);
        let api = Api::open(&home, &actor)
            .map_err(|e| format!("cannot open home {}: {e}", home.display()))?;
        let info = SessionInfo {
            home: api.home().display().to_string(),
            actor: api.actor().to_string(),
            opened_at: Utc::now(),
        };
        Ok(Session { api, info })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn home_prefers_devai_home_env() {
        // OQ-UI-1: $DEVAI_HOME wins when set.
        let h = resolve_home(&env(&[("DEVAI_HOME", "/srv/other.devai")]));
        assert_eq!(h, PathBuf::from("/srv/other.devai"));
    }

    #[test]
    fn home_blank_env_falls_back_to_default() {
        // A blank $DEVAI_HOME is not a chosen home — fall through.
        let h = resolve_home(&env(&[("DEVAI_HOME", "   ")]));
        let default = dirs::home_dir().unwrap().join(".devai");
        assert_eq!(h, default);
    }

    #[test]
    fn actor_chain_is_devai_user_then_user_then_ui() {
        // OQ-UI-8, ratified as the CLI chain + "ui" default.
        assert_eq!(
            resolve_actor(&env(&[("DEVAI_USER", "guerrero"), ("USER", "someone")])),
            "guerrero"
        );
        assert_eq!(resolve_actor(&env(&[("USER", "someone")])), "someone");
        assert_eq!(resolve_actor(&env(&[("DEVAI_USER", "")])), "ui");
        assert_eq!(resolve_actor(&env(&[])), "ui");
    }
}
