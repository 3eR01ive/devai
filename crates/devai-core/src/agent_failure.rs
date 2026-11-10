//! Agent-failure classification (task 01a1b60c, REQ-AF-02/04).
//!
//! The platform records WHY an agent execution failed as DATA: a word from a
//! closed list, distinguishable by a human without parsing the driver's raw
//! output. The classification is a pure function of what the platform
//! observes at completion — the exit code, the captured text and, when a
//! driver reports one structurally, a hint. Nothing here guesses a cause the
//! evidence does not carry: a bare `exit 2` with no recognised marker is
//! `unclassified`, never `provider_model` (REQ-AF-02 AC5).
//!
//! Priority is the spec's: the exit CODE wins over words, then the scan goes
//! in the declared order (auth → model → rate → 5xx → network → config →
//! binary_missing). One input yields exactly one class (AC3).

/// The closed vocabulary of `agent.failed` classes (REQ-AF-02 AC1).
pub mod class {
    /// 401/403 — the key was not accepted.
    pub const PROVIDER_AUTH: &str = "provider_auth";
    /// 404 / `model_not_found` — the model vanished at the provider.
    pub const PROVIDER_MODEL: &str = "provider_model";
    /// 429 / quota / tempo — the provider throttled the call.
    pub const PROVIDER_RATE: &str = "provider_rate";
    /// 500–599 — the provider answered with a server error.
    pub const PROVIDER_5XX: &str = "provider_5xx";
    /// DNS / timeout / refused / reset — no usable network.
    pub const NETWORK: &str = "network";
    /// Unreadable configuration or an unknown provider.
    pub const CONFIG: &str = "config";
    /// The driver file is absent (or the shell said 127).
    pub const BINARY_MISSING: &str = "binary_missing";
    /// Not enough evidence — the honest default, including empty output.
    pub const UNCLASSIFIED: &str = "unclassified";

    /// The complete list, in the spec's scan order.
    pub const ALL: [&str; 8] = [
        PROVIDER_AUTH,
        PROVIDER_MODEL,
        PROVIDER_RATE,
        PROVIDER_5XX,
        NETWORK,
        CONFIG,
        BINARY_MISSING,
        UNCLASSIFIED,
    ];
}

use class::*;

/// The closed-list gate: an unknown token is not silently accepted as a
/// class (`None` — the caller reports «причина не классифицирована»).
pub fn parse(token: &str) -> Option<&'static str> {
    class::ALL.iter().copied().find(|c| *c == token)
}

/// One human reason phrase per class (REQ-AF-04 AC1). The eight phrases are
/// distinct on purpose: «модель исчезла» must never read as «нет сети».
pub fn word(class_token: &str) -> &'static str {
    match class_token {
        PROVIDER_AUTH => "ключ провайдера отвергнут",
        PROVIDER_MODEL => "модель исчезла у провайдера",
        PROVIDER_RATE => "провайдер ограничил темп или квоту",
        PROVIDER_5XX => "провайдер ответил ошибкой 5xx",
        NETWORK => "сеть недоступна",
        CONFIG => "конфигурация нечитаема",
        BINARY_MISSING => "файл драйвера не найден",
        UNCLASSIFIED => "причина не классифицирована",
        _ => "причина не распознана",
    }
}

/// The next action the human can take (REQ-AF-04 AC2). The platform itself
/// never applies one: no automatic model/provider/key switch (REQ-AF-09).
pub fn action(class_token: &str) -> &'static str {
    match class_token {
        PROVIDER_AUTH => "заменить ключ",
        PROVIDER_MODEL => "перевыпустить агента",
        PROVIDER_RATE => "снизить темп или повторить позже",
        PROVIDER_5XX => "повторить позже",
        NETWORK => "проверить сеть",
        CONFIG => "исправить конфигурацию",
        BINARY_MISSING => "установить файл",
        UNCLASSIFIED => "открыть сохранённый текст ошибки и журнал драйвера",
        _ => "открыть сохранённый текст ошибки",
    }
}

/// Classification result WITH its ground: the spec requires the basis to be
/// stored as data, so a reader can tell an exit-code verdict from a text
/// match (REQ-AF-02 AC5).
pub struct Classified {
    pub class: &'static str,
    pub basis: String,
}

/// The exit code rules, in order (REQ-AF-02 AC2/AC3). `None` = no mapped
/// code, fall through to the text scan.
fn class_of_code(code: i32) -> Option<&'static str> {
    match code {
        401 | 403 => Some(PROVIDER_AUTH),
        404 => Some(PROVIDER_MODEL),
        429 => Some(PROVIDER_RATE),
        127 => Some(BINARY_MISSING),
        500..=599 => Some(PROVIDER_5XX),
        _ => None,
    }
}

/// The textual markers, scanned in the declared order. Markers are
/// lower-case substrings; a marker is evidence, not a guess: `provider_model`
/// requires an explicit model phrase, so a plain "not found" never lands
/// there (REQ-AF-02 AC5).
fn class_of_text(text: &str) -> Option<(&'static str, &'static str)> {
    let t = text.to_lowercase();
    let has = |m: &str| t.contains(m);
    // 1) provider_auth — 401/403 and key phrases.
    if [
        "401",
        "403",
        "unauthorized",
        "authentication",
        "invalid api key",
        "invalid_api_key",
        "no_key",
        "ключ",
    ]
    .iter()
    .any(|m| has(m))
    {
        return Some((PROVIDER_AUTH, "401/403 or key marker"));
    }
    // 2) provider_model — explicit model phrases only.
    for m in [
        "model_not_found",
        "model not exist",
        "model not found",
        "model does not exist",
        "no such model",
        "unsupported model",
    ] {
        if has(m) {
            return Some((PROVIDER_MODEL, m));
        }
    }
    // 3) provider_rate — 429/quota/tempo.
    for m in [
        "429",
        "rate limit",
        "rate_limit",
        "quota",
        "too many requests",
        "темп",
    ] {
        if has(m) {
            return Some((PROVIDER_RATE, m));
        }
    }
    // 4) provider_5xx — server-side statuses.
    for m in [
        "500",
        "502",
        "503",
        "504",
        "internal server error",
        "bad gateway",
        "service unavailable",
        "gateway timeout",
        "5xx",
    ] {
        if has(m) {
            return Some((PROVIDER_5XX, m));
        }
    }
    // 5) network — DNS, timeout, refused/reset, connection errors.
    for m in [
        "dns",
        "name resolution",
        "timeout",
        "timed out",
        "connection refused",
        "connection reset",
        "apiconnectionerror",
        "stream stale",
        "unreachable",
        "network",
        "econnreset",
        "etimedout",
    ] {
        if has(m) {
            return Some((NETWORK, m));
        }
    }
    // 6) config — unreadable config or unknown provider.
    for m in [
        "yaml",
        "unknown provider",
        "unknown agent provider",
        "configuration",
        "config error",
        "конфигурац",
    ] {
        if has(m) {
            return Some((CONFIG, m));
        }
    }
    // 7) binary_missing — file not found / 127. The model phrases above
    // already claimed their own "not found" cases.
    for m in ["no such file", "not found", "command not found", "127"] {
        if has(m) {
            return Some((BINARY_MISSING, m));
        }
    }
    None
}

/// Classify one observed failure. `hint` is a structural driver report
/// (`"unknown_provider"`, `"launch_not_found"`) — it outranks the text
/// because the driver knows its own failure mode, but never outranks a
/// conclusive exit code.
pub fn classify(exit_code: Option<i32>, text: &str, hint: Option<&str>) -> Classified {
    if let Some(hint) = hint {
        match hint {
            "unknown_provider" => {
                return Classified {
                    class: CONFIG,
                    basis: "driver report: unknown provider".into(),
                };
            }
            "launch_not_found" => {
                return Classified {
                    class: BINARY_MISSING,
                    basis: "driver report: launch failed, file not found".into(),
                };
            }
            _ => {}
        }
    }
    if let Some(code) = exit_code
        && let Some(class) = class_of_code(code)
    {
        return Classified {
            class,
            basis: format!("exit_code:{code}"),
        };
    }
    if let Some((class, marker)) = class_of_text(text) {
        return Classified {
            class,
            basis: format!("text:{marker}"),
        };
    }
    Classified {
        class: UNCLASSIFIED,
        basis: if exit_code.is_some() {
            format!(
                "exit_code:{} (no recognised marker)",
                exit_code.unwrap_or_default()
            )
        } else {
            "no signals (no exit code, no recognised marker)".into()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_closed_and_in_spec_order() {
        assert_eq!(class::ALL.len(), 8);
        for c in class::ALL {
            assert_eq!(parse(c), Some(c));
        }
        assert_eq!(parse("something_else"), None);
    }

    /// The 07.11 incident: exit status 2, text carries `404 Model not exist`
    /// — the code alone says nothing, the text proves the model class.
    #[test]
    fn the_404_incident_is_provider_model_by_text() {
        let c = classify(
            Some(2),
            "`hermes` exited with exit status: 2: 404 Model not exist",
            None,
        );
        assert_eq!(c.class, PROVIDER_MODEL);
        assert!(c.basis.starts_with("text:"), "{}", c.basis);
    }

    /// A bare exit 2 without a 404 marker is never dressed as a model loss.
    #[test]
    fn a_bare_exit_two_stays_unclassified() {
        let c = classify(Some(2), "`hermes` exited with exit status: 2: ", None);
        assert_eq!(c.class, UNCLASSIFIED);
    }

    #[test]
    fn exit_codes_win_over_words() {
        assert_eq!(
            classify(Some(401), "network timeout", None).class,
            PROVIDER_AUTH
        );
        assert_eq!(
            classify(Some(429), "model not found", None).class,
            PROVIDER_RATE
        );
        assert_eq!(
            classify(Some(503), "unauthorized", None).class,
            PROVIDER_5XX
        );
        assert_eq!(classify(Some(127), "", None).class, BINARY_MISSING);
    }

    #[test]
    fn each_class_has_its_own_word_and_action() {
        let words: Vec<&str> = class::ALL.iter().map(|c| word(c)).collect();
        let mut sorted = words.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), words.len(), "the eight phrases are distinct");
        for c in class::ALL {
            assert!(!action(c).is_empty());
        }
        assert_eq!(word(UNCLASSIFIED), "причина не классифицирована");
    }

    #[test]
    fn network_and_config_and_binary_are_distinguished() {
        assert_eq!(
            classify(None, "APIConnectionError: Connection refused", None).class,
            NETWORK
        );
        assert_eq!(
            classify(None, "Stream stale for 120s, timed out", None).class,
            NETWORK
        );
        assert_eq!(
            classify(
                None,
                "failed to launch agent process: No such file or directory",
                None
            )
            .class,
            BINARY_MISSING
        );
        assert_eq!(
            classify(None, "broken YAML in agent config", None).class,
            CONFIG
        );
        assert_eq!(
            classify(None, "boom", Some("unknown_provider")).class,
            CONFIG
        );
    }
}
