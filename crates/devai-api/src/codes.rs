//! Machine codes of the homes vocabulary — the ONE place the wire tokens
//! of the homes registry are spelled (R3 quality return: the same literals
//! were duplicated in `homes.rs`, `lib.rs` and the UI session bootstrap, so
//! a rename could drift silently between producer and consumer).
//!
//! Consumers branch on these tokens (UI `StateChip`, banner, failure
//! mapping); the frontend does NOT mirror them by hand — the TS module
//! `frontend/src/generated/home-codes.ts` is generated from this file by
//! `frontend/scripts/gen-home-codes.mjs` (R1 quality return), so a code
//! added here reaches the UI with no second edit. The tests below pin every
//! constant to the exact string the wire carries, and the frontend parity
//! test pins the generated module against a fresh render of this file.
//! Adding a code means changing this dictionary — not a literal at a call
//! site.

/// A registered home whose directory or store is no longer on disk.
pub const HOME_UNAVAILABLE: &str = "home_unavailable";
/// The path is a directory but has no initialised store (`devai.db`).
pub const HOME_NO_STORE: &str = "home_no_store";
/// The record exists but is switched off (registry inclusion = false).
pub const HOME_DISABLED: &str = "home_disabled";
/// The stored schema rank cannot be observed by this build.
pub const HOME_SCHEMA_VERSION: &str = "home_schema_version";
/// No active home: every registered home is switched off (ratified
/// OQ-HOME-8 — switching the active home off is a normal operator action
/// whose outcome is "no active home", never an error). Domain records are
/// unavailable until the operator enables or adds a home; the registry
/// keeps every record.
pub const HOME_NONE_ACTIVE: &str = "home_none_active";
/// The registry document of the control home is unparsable / of an unknown
/// format version (fails loud, never quietly discarded).
pub const HOMES_REGISTRY_CORRUPT: &str = "homes_registry_corrupt";
/// The registry document itself could not be read or written (I/O of the
/// control home: permissions, a vanished directory, a failed rename). The
/// document is not corrupt — the bytes were never trusted, because the act
/// of reading/writing them failed (R3 quality return: the bootstrap
/// boundary needs a registry-IO token distinct from a parse refusal).
pub const HOMES_REGISTRY_UNAVAILABLE: &str = "homes_registry_unavailable";

/// Classify the machine code of a failed OBSERVATION OPEN into the homes
/// vocabulary (R2 quality return, rel/error-context: the probe used to
/// flatten every cause into `Option<String>`, so a consumer could only see
/// "there is text" and never branch). Pure and shared: the API probe and
/// the UI bootstrap both classify through this one function, so the two
/// producers cannot drift.
///   * `not_found` — the directory is there but the store/schema is not:
///     `home_no_store` (initialise it or pick another path).
///   * `schema_version` — a rank this build refuses: `home_schema_version`
///     (bring a matching binary; the home is intact).
///   * `io` / `corrupt` — unreadable at all: `home_unavailable`. No version
///     token, because an unreadable store has no trustworthy rank.
///   * `_` — an unknown future variant fails closed to the widest honest
///     token, "this home cannot be observed".
pub fn home_failure_code(api_code: &str) -> &'static str {
    match api_code {
        "not_found" => HOME_NO_STORE,
        "schema_version" => HOME_SCHEMA_VERSION,
        "io" | "corrupt" => HOME_UNAVAILABLE,
        _ => HOME_UNAVAILABLE,
    }
}

/// The platform's own operator hint beside a home machine code (R2: the
/// wire carries `code + path + hint`, the UI branches on the code and the
/// text stays for the human). `None` for a code this build has no advice
/// for — an invented hint would be a lie.
pub fn home_hint(code: &str) -> Option<&'static str> {
    match code {
        HOME_UNAVAILABLE => {
            Some("put the path back on disk, or activate another home; the registry record is kept")
        }
        HOME_NO_STORE => Some("initialise the store (`devai init`) or add another path"),
        HOME_SCHEMA_VERSION => {
            Some("bring a binary matching the stored schema rank; the home itself is intact")
        }
        HOME_DISABLED => Some("enable the home first, then activate it"),
        HOME_NONE_ACTIVE => Some(
            "enable or add a home; the registry keeps every record and no home is chosen for you",
        ),
        HOMES_REGISTRY_CORRUPT => {
            Some("repair or remove the registry document of the control home")
        }
        HOMES_REGISTRY_UNAVAILABLE => Some("check the control home's permissions and free space"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire contract: these strings are what a consumer sees in
    /// `ApiError::code` / `HomeFailure::code`. Editing the constant without
    /// editing this test means the contract changed — deliberately.
    #[test]
    fn wire_tokens_are_pinned() {
        assert_eq!(HOME_UNAVAILABLE, "home_unavailable");
        assert_eq!(HOME_NO_STORE, "home_no_store");
        assert_eq!(HOME_DISABLED, "home_disabled");
        assert_eq!(HOME_SCHEMA_VERSION, "home_schema_version");
        assert_eq!(HOME_NONE_ACTIVE, "home_none_active");
        assert_eq!(HOMES_REGISTRY_CORRUPT, "homes_registry_corrupt");
        assert_eq!(HOMES_REGISTRY_UNAVAILABLE, "homes_registry_unavailable");
    }

    /// No two codes collapse into one token: the UI branches on identity.
    #[test]
    fn wire_tokens_are_distinct() {
        let all = [
            HOME_UNAVAILABLE,
            HOME_NO_STORE,
            HOME_DISABLED,
            HOME_SCHEMA_VERSION,
            HOME_NONE_ACTIVE,
            HOMES_REGISTRY_CORRUPT,
            HOMES_REGISTRY_UNAVAILABLE,
        ];
        let mut seen = std::collections::HashSet::new();
        for code in all {
            assert!(seen.insert(code), "duplicate machine code: {code}");
        }
    }

    /// R2 quality return (rel/error-context): the classifier is the ONE
    /// mapping from a domain error token to the homes vocabulary — pinned
    /// here so the API probe and the UI bootstrap cannot drift apart.
    #[test]
    fn observation_failures_classify_into_home_tokens() {
        assert_eq!(home_failure_code("not_found"), HOME_NO_STORE);
        assert_eq!(home_failure_code("schema_version"), HOME_SCHEMA_VERSION);
        assert_eq!(home_failure_code("io"), HOME_UNAVAILABLE);
        assert_eq!(home_failure_code("corrupt"), HOME_UNAVAILABLE);
        // Unknown future domain variants fail closed, never to success.
        assert_eq!(home_failure_code("some_future_code"), HOME_UNAVAILABLE);
    }

    /// Every home token a probe can emit carries operator advice; a token
    /// with no advice stays honestly `None` instead of inventing a hint.
    #[test]
    fn every_probe_token_has_a_hint() {
        for code in [
            HOME_UNAVAILABLE,
            HOME_NO_STORE,
            HOME_SCHEMA_VERSION,
            HOME_DISABLED,
            HOME_NONE_ACTIVE,
            HOMES_REGISTRY_CORRUPT,
            HOMES_REGISTRY_UNAVAILABLE,
        ] {
            let hint = home_hint(code);
            assert!(hint.is_some(), "{code}: a published token has no hint");
            assert!(
                !hint.unwrap().trim().is_empty(),
                "{code}: the hint is not a placeholder"
            );
        }
        assert_eq!(home_hint("not_a_home_code"), None);
    }
}
