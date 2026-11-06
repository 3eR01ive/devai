//! The ONE vocabulary of the quality-evaluation pipeline (rework R3/R5/R8/R4).
//!
//! The check states the evaluator emits and the item kinds the stored human
//! form carries used to be written out as string literals in four places —
//! the producer (`evaluation.rs`), the parser (`human_view.rs`), the stored
//! JSON-schema enum/description (`seed.rs`) and the frontend contract
//! (`types.ts`) — with no machine check tying them together, so a state the
//! producer emitted could silently fall into a catch-all (MECHANICAL_PASS did).
//!
//! Rework R4 took the last step: the catalog no longer lives in hand-written
//! code at all. `crates/devai-core/vocab.json` is the single source; this
//! module INCLUDES the enums and their wire tables generated from it by
//! `build.rs`, and the other surfaces quote the same generated constants:
//!
//! * the seeded `quality-report` schema takes its `checks.state` enum from
//!   [`CheckState::JSON_ENUM`] and its `kind` description from
//!   [`ItemKind::LIST`] (placeholders filled in `seed::contracts`);
//! * `crates/devai-ui/frontend/src/vocab.generated.ts` is generated from the
//!   same JSON by the UI build script; `types.ts` re-exports its unions.
//!
//! Parity tests fail loudly when a surface is hand-edited instead of
//! regenerated (`devai-store` schema tests, `devai-ui`
//! `ts_types_match_the_rust_vocabulary`, and the round-trip test below).
//!
//! Unknown input is NEVER guessed into a known branch: [`CheckState::parse`]
//! returns `None` and the reader presents it as an explicitly named unknown.

include!(concat!(env!("OUT_DIR"), "/vocab_generated.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_vocabularies_are_total_and_round_trip() {
        for s in CheckState::ALL {
            assert_eq!(CheckState::parse(s.as_str()), Some(s));
        }
        assert_eq!(CheckState::parse("SOMETHING_NEW"), None);
        let mut kinds: Vec<&str> = ItemKind::ALL.iter().map(|k| k.as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), ItemKind::ALL.len(), "kinds are distinct");
    }

    /// The generated fragments the other surfaces quote say exactly what the
    /// enums say — a stale hand-edit of either is caught here.
    #[test]
    fn the_generated_json_and_ts_fragments_track_the_enums() {
        let quoted: Vec<String> = CheckState::JSON_ENUM
            .trim_matches(['[', ']'])
            .split(", ")
            .map(|t| t.trim_matches('"').to_string())
            .collect();
        let expected: Vec<String> = CheckState::ALL
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();
        assert_eq!(quoted, expected);

        let ts: Vec<String> = CheckState::TS_UNION
            .split(" | ")
            .map(|t| t.trim_matches('"').to_string())
            .collect();
        assert_eq!(ts, expected);

        let listed: Vec<String> = ItemKind::LIST.split(" | ").map(str::to_string).collect();
        let kinds: Vec<String> = ItemKind::ALL
            .iter()
            .map(|k| k.as_str().to_string())
            .collect();
        assert_eq!(listed, kinds);
        assert!(ItemKind::TS_UNION.contains("\"gate-unperformed\""));
    }
}
