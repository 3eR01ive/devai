//! The `ALL` vocabularies must be COMPLETE against the const blocks they
//! enumerate — the rule `event_kind::ALL` / `json_field::ALL` state in their
//! doc comments ("a new const above MUST be added here too") is mechanical,
//! not aspirational: the REWORK delivery policy validates user-authored
//! event/field selectors against these lists at load (2026-11-02 QA return,
//! block 2: an omitted kind means a trigger that silently matches nothing,
//! the exact disease the loader exists to prevent). A drifting list is
//! caught here, by reading the source of the module itself.

use std::collections::BTreeSet;

fn types_src() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/types.rs")).expect("types.rs is readable from the test crate")
}

/// The full text of one `pub mod <name> { ... }` block, brace-matched. The
/// two vocabulary modules carry no braces inside their doc comments, so a
/// plain depth scan over the source is exact here.
fn module_block(src: &str, module: &str) -> String {
    let header = format!("pub mod {module} {{");
    let start = src.find(&header).unwrap_or_else(|| panic!("module `{module}` not found in types.rs"));
    let body = &src[start..];
    let mut depth = 0usize;
    for (idx, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return body[..=idx].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("module `{module}` is never closed");
}

/// (declared const names, names listed inside `pub const ALL = &[...]`).
fn parse_vocab(block: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut declared = BTreeSet::new();
    let mut all_members = BTreeSet::new();
    let mut in_all = false;
    for line in block.lines() {
        let t = line.trim();
        if t.starts_with("pub const ALL") {
            in_all = true;
            continue;
        }
        if in_all {
            if t.starts_with(']') {
                in_all = false;
                continue;
            }
            let name = t.trim_end_matches(',').trim();
            // A vocabulary const identifier: SCREAMING case, may carry
            // digits (`SHA256`), starts with a letter.
            if !name.is_empty()
                && name.as_bytes()[0].is_ascii_uppercase()
                && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                all_members.insert(name.to_string());
            }
            continue;
        }
        // `pub const NAME: &str = "value";` — a vocabulary entry.
        if let Some(rest) = t.strip_prefix("pub const ") {
            if let Some((name, _)) = rest.split_once(": &str") {
                declared.insert(name.trim().to_string());
            }
        }
    }
    (declared, all_members)
}

fn assert_complete(module: &str) {
    let (declared, all) = parse_vocab(&module_block(&types_src(), module));
    assert!(!declared.is_empty(), "{module} declares consts");
    let missing: Vec<_> = declared.difference(&all).cloned().collect();
    let extra: Vec<_> = all.difference(&declared).cloned().collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{module}::ALL drifted: missing {missing:?}, unknown entries {extra:?}"
    );
}

#[test]
fn event_kind_all_enumerates_every_declared_kind() {
    assert_complete("event_kind");
}

#[test]
fn json_field_all_enumerates_every_declared_field() {
    assert_complete("json_field");
}
