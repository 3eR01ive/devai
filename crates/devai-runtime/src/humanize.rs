//! The mechanical edibility detector of the human paragraph (task 01a1acfe,
//! REQ-HP-03; ratified OQ-HP-2: strictly deterministic, no provider call).
//!
//! Scope is fixed BY CONSTRUCTION, not by a code branch over artifact kinds:
//! the detector fires ONLY on contracts whose stored schema requires the
//! configured paragraph field as a top-level field — the role-borne gate
//! artifacts (requirements-specification@3, review-report@2,
//! implementation-report@3). The platform-assembled `report_text` of
//! quality-report carries no such field in its schema, so it is exempt here
//! exactly as the spec ratifies: an overlap check on a §2.6 rendering would
//! reject the render by construction, and the taxonomy forbids replaying a
//! platform-class form onto a role (docs/response-form-loop.md:19).
//!
//! Two failure modes, both thresholds/slots/register from
//! `config/workflow/humanize.yaml` (REQ-HP-04 — never code literals):
//!  * «restatement of formalism» — the fraction of the paragraph's word
//!    n-grams already present in the artifact's formal body (echo fields
//!    from `overlap_ignore` excluded) above `overlap_max_ratio`;
//!  * «empty chatter» — below `min_chars` (or above `max_chars`, the
//!    REQ-HP-08 size discipline), or any declared explanation slot
//!    (что происходит / что изменится) with no marker phrase present, or a
//!    forbidden formal-protocol marker in plain words.
//!
//! Same input ⇒ same verdict, always: pure functions over text, no time,
//! no randomness, no network.

use devai_store::config::HumanizePolicy;
use serde_json::Value;

/// Lowercase word tokens (alphanumeric runs, unicode-aware); deterministic.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

/// The set of distinct word n-grams (`n` from the policy; a text shorter
/// than `n` words degrades to whole-token grams so short inputs still
/// produce a defined, stable set).
fn grams(tokens: &[String], n: usize) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    if tokens.is_empty() {
        return out;
    }
    let n = n.min(tokens.len()).max(1);
    for w in tokens.windows(n) {
        out.insert(w.join(" "));
    }
    out
}

/// True when the stored schema REQUIRES the paragraph field at top level —
/// the scope predicate of the detector (role-borne contracts only;
/// absence of the requirement on @1/@2/quality-report is what makes
/// REQ-HP-06 non-retroactivity and the report_text exemption mechanical).
pub fn schema_requires_paragraph(schema: &Value, policy: &HumanizePolicy) -> bool {
    let required_in_schema = schema
        .get("required")
        .and_then(|r| r.as_array())
        .is_some_and(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .any(|k| k == policy.field)
        });
    let declared = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .is_some_and(|p| p.contains_key(&policy.field));
    required_in_schema && declared
}

/// The detector: violations for the paragraph of an already shape-valid
/// body. An ABSENT (or non-string) paragraph is not reported here — the
/// schema `required` gate owns presence; double-reporting the same fact
/// would burn two repair texts on one defect (REQ-HP-03 fires on
/// «не съедобно», не на «не заполнено»).
pub fn paragraph_violations(value: &Value, schema: &Value, policy: &HumanizePolicy) -> Vec<String> {
    let mut errs = Vec::new();
    if !schema_requires_paragraph(schema, policy) {
        return errs;
    }
    let Some(paragraph) = value.get(&policy.field).and_then(|v| v.as_str()) else {
        return errs;
    };
    let path = format!("$.{}", policy.field);
    let chars = paragraph.chars().count();
    let lower = paragraph.to_lowercase();

    // «пустая болтовня», half 1: volume window from the config (REQ-HP-08
    // keeps the max inside the physical reply limit together with the full
    // schema skeleton).
    if chars < policy.min_chars {
        errs.push(format!(
            "{path}: paragraph is {chars} chars, below the configured minimum {} — write in plain words what happens now and what will change (empty filler is rejected)",
            policy.min_chars
        ));
    }
    if chars > policy.max_chars {
        errs.push(format!(
            "{path}: paragraph is {chars} chars, above the configured maximum {} — the reply must fit the response limit together with the formal body",
            policy.max_chars
        ));
    }

    // Register: plain words must not carry the formal protocol's own marks.
    for m in &policy.forbidden_markers {
        if !m.trim().is_empty() && lower.contains(&m.to_lowercase()) {
            errs.push(format!(
                "{path}: contains the formal-protocol marker `{m}` — the paragraph is for a human, not a copy of the schema's vocabulary"
            ));
        }
    }

    // «пустая болтовня», half 2: every declared explanation slot must be
    // filled by at least one of its marker phrases (case-insensitive).
    for slot in &policy.slots {
        if slot.markers.is_empty() {
            continue;
        }
        if !slot
            .markers
            .iter()
            .any(|m| !m.trim().is_empty() && lower.contains(&m.to_lowercase()))
        {
            errs.push(format!(
                "{path}: explanation slot «{name}» is not filled — none of the configured marker phrases appears",
                name = slot.name
            ));
        }
    }

    // «пересказ формализма»: overlap of the paragraph's n-grams with the
    // formal body of the SAME artifact (echo fields excluded by config).
    if let Some(body) = formal_body(value, policy) {
        let p_grams = grams(&tokenize(paragraph), policy.ngram);
        if !p_grams.is_empty() {
            let b_grams = grams(&tokenize(&body), policy.ngram);
            let hits = p_grams.intersection(&b_grams).count();
            let ratio = hits as f64 / p_grams.len() as f64;
            if ratio > policy.overlap_max_ratio {
                errs.push(format!(
                    "{path}: {:.0}% of the paragraph's {n}-grams repeat the formal body of the artifact — a restatement of the formalism, not a human explanation (write what it means in plain words)",
                    ratio * 100.0,
                    n = policy.ngram
                ));
            }
        }
    }
    errs
}

/// The formal body as the overlap corpus: the whole serialized artifact
/// minus the paragraph field itself and the configured echo fields — the
/// paragraph legitimately mirrors human prose, not the formal structure.
fn formal_body(value: &Value, policy: &HumanizePolicy) -> Option<String> {
    let obj = value.as_object()?;
    let mut kept = serde_json::Map::new();
    for (k, v) in obj {
        if k == &policy.field || policy.overlap_ignore.iter().any(|ig| ig == k) {
            continue;
        }
        kept.insert(k.clone(), v.clone());
    }
    Some(Value::Object(kept).to_string())
}

/// Ratified OQ-HP-3: the paragraph language follows the language of the
/// task's business description; the policy value is the fallback for
/// statements with no expressed language. Deterministic script count —
/// no model, no false precision: >50% Cyrillic letters => `ru`,
/// >50% Latin => `en`, otherwise the configured default.
pub fn paragraph_language(policy: &HumanizePolicy, business: &str) -> String {
    let (mut cyr, mut lat) = (0usize, 0usize);
    for c in business.chars() {
        if c.is_alphabetic() {
            if ('а'..='я').contains(&c.to_lowercase().next().unwrap_or(c)) || c == 'ё' {
                cyr += 1;
            } else if c.is_ascii_alphabetic() {
                lat += 1;
            }
        }
    }
    let total = cyr + lat;
    if total == 0 {
        return policy.language.clone();
    }
    if cyr * 2 > total {
        "ru".into()
    } else if lat * 2 > total {
        "en".into()
    } else {
        policy.language.clone()
    }
}

/// The declarative template rendered into the role's prompt (REQ-HP-04:
/// the «очеловечивание» requirements are config, and this is their only
/// second home — the gate itself never re-reads the prompt). Three honest
/// branches, never collapsed: None schema or a schema that does not
/// require the field => None (template shown exactly where the gate
/// enforces it); an unparseable schema => Some with an explicit corruption
/// marker — a broken contract is distinguishable from "requirement not
/// declared" and the role is told not to invent meaning (rework C5 R3).
pub fn template_section(
    policy: &HumanizePolicy,
    contract_schema: Option<&str>,
    business: &str,
) -> Option<String> {
    let raw = contract_schema?;
    let schema_v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return Some(format!(
                concat!(
                    "\n=== HUMAN PARAGRAPH: СХЕМА КОНТРАКТА ПОВРЕЖДЕНА ===\n",
                    "JSON выходного контракта не разобран: {err}. Требование поля `{field}` ",
                    "из такой схемы не читается — это повреждение контракта, а не «поле не ",
                    "требуется». Смысл не додумывай: зафиксируй повреждение вопросом ",
                    "существующим путём (`devai question ask`) и не публикуй абзац наугад.\n"
                ),
                err = e,
                field = policy.field,
            ));
        }
    };
    if !schema_requires_paragraph(&schema_v, policy) {
        return None;
    }
    let lang = paragraph_language(policy, business);
    let slots: Vec<String> = policy
        .slots
        .iter()
        .map(|s| format!("«{name}»", name = s.name))
        .collect();
    let markers: Vec<String> = policy
        .forbidden_markers
        .iter()
        .filter(|m| !m.trim().is_empty())
        .map(|m| format!("`{m}`"))
        .collect();
    Some(format!(
        concat!(
            "\n=== HUMAN PARAGRAPH (часть протокола, ратифицировано) ===\n",
            "Ответ обязан нести top-level поле `{field}` — человеческий абзац артефакта, ",
            "который оператор читает на гейте ПЕРВЫМ, до всякой формальной структуры.\n",
            "- Объём: {min}–{max} символов (механический детектор платформы, гейт без него не проходит).\n",
            "- Язык: {lang} (язык бизнес-описания задачи; значение шаблона — фолбэк).\n",
            "- Структура объяснения (обязательные слоты): {slots} — простыми словами, что происходит сейчас и что изменится.\n",
            "- Не пересказывай формальное тело: копирование стейтментов и ключей схемы отвергается детектором overlap; не используй маркеры формального протокола: {markers}.\n",
            "- Поле заполняется как строка в том же JSON-объекте; текст не переписывается платформой после публикации.\n",
            "- На rework (версии 2..N) абзац пишется заново целиком — он объясняет текущее состояние, а не дельту версий.\n"
        ),
        field = policy.field,
        min = policy.min_chars,
        max = policy.max_chars,
        lang = lang,
        slots = slots.join(", "),
        markers = markers.join(", "),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_store::config::HumanizePolicy;
    use serde_json::json;

    fn policy() -> HumanizePolicy {
        HumanizePolicy::default()
    }

    fn spec_schema() -> Value {
        json!({
            "type": "object",
            "required": ["version", "requirements", "unknowns", "assumptions", "human_paragraph"],
            "properties": {
                "version": { "type": "integer" },
                "human_paragraph": { "type": "string" },
                "business_description": { "type": ["string", "null"] },
                "requirements": { "type": "array" }
            }
        })
    }

    /// A real human paragraph of the factory volume window, slots filled.
    fn good_paragraph() -> String {
        "Простыми словами: сейчас все выводы ролей выглядят как формальный протокол, и человеку приходится переводить их на обычный язык самостоятельно по контексту запуска. Этот артефакт несёт понятное объяснение первым блоком: что происходит сегодня и что изменится после принятия решения. Платформа начнёт механически требовать такой абзац у каждой роли на гейте и показывать его оператору до формальной структуры, поэтому смысл задачи будет читаем без ручного разбора JSON.".to_string()
    }

    fn body(paragraph: &str) -> Value {
        json!({
            "version": 1,
            "business_description": "Выводы ролей стали формальными, но нечитаемыми для человека.",
            "requirements": [
                {"id": "R1", "statement": "Платформа проверяет наличие абзаца механически."}
            ],
            "unknowns": [],
            "assumptions": [],
            "human_paragraph": paragraph
        })
    }

    #[test]
    fn real_human_paragraph_passes() {
        let errs = paragraph_violations(&body(&good_paragraph()), &spec_schema(), &policy());
        assert_eq!(errs, Vec::<String>::new(), "{errs:?}");
    }

    #[test]
    fn copy_pasted_requirement_statement_is_rejected() {
        // The «пересказ формализма» fixture: the paragraph is the formal
        // statement, copy-pasted verbatim twice — nearly all its n-grams
        // already live in the body.
        let statement = "Единый контракт гейт платформы проверяет наличие человеческого абзаца на всех путях публикации без обходных дверей отклоняя артефакт который не проходит";
        let formal_body = json!({
            "version": 1,
            "business_description": "Выводы ролей стали формальными.",
            "requirements": [{ "id": "R1", "statement": statement }],
            "unknowns": [],
            "assumptions": [],
            "human_paragraph": format!("{statement}. {statement}.")
        });
        let errs = paragraph_violations(&formal_body, &spec_schema(), &policy());
        assert!(
            errs.iter().any(|e| e.contains("repeat the formal body")),
            "copy-retelling must be rejected: {errs:?}"
        );
    }

    #[test]
    fn empty_chatter_is_rejected() {
        let errs = paragraph_violations(
            &body("всё хорошо, работа выполнена"),
            &spec_schema(),
            &policy(),
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("below the configured minimum")),
            "short filler must be rejected: {errs:?}"
        );
    }

    #[test]
    fn unfilled_slot_is_rejected() {
        // Long enough, no formal markers, but only narrates the present —
        // «что изменится» never appears.
        let p = "Сегодня все выводы ролей выглядят как сухой формальный протокол, и оператору приходится самому переводить их на человеческий язык, опираясь на контекст запуска и память о предыдущих обсуждениях; так происходит на гейтахapproval, на гейтах приёмки, и каждый раз человек тратит время на расшифровку того, что модель уже знала и могла сказать просто.".to_string();
        let errs = paragraph_violations(&body(&p), &spec_schema(), &policy());
        assert!(
            errs.iter().any(|e| e.contains("explanation slot")),
            "unfilled slot must be rejected: {errs:?}"
        );
    }

    #[test]
    fn forbidden_register_marker_is_rejected() {
        let mut p = good_paragraph();
        p.push_str(" Основанием служит REQ-HP-03.");
        let errs = paragraph_violations(&body(&p), &spec_schema(), &policy());
        assert!(
            errs.iter().any(|e| e.contains("formal-protocol marker")),
            "formal ids in the paragraph must be rejected: {errs:?}"
        );
    }

    #[test]
    fn detector_is_deterministic() {
        let v = body(&good_paragraph());
        let a = paragraph_violations(&v, &spec_schema(), &policy());
        let b = paragraph_violations(&v, &spec_schema(), &policy());
        assert_eq!(a, b);
    }

    #[test]
    fn volume_thresholds_come_from_config_not_code() {
        // The same 50-char paragraph: rejected at the factory min, accepted
        // when the project lowers the window — config is the source of truth.
        let small = "а".repeat(50);
        let strict = paragraph_violations(&body(&small), &spec_schema(), &policy());
        assert!(!strict.iter().any(|e| e.is_empty()));
        assert!(
            strict
                .iter()
                .any(|e| e.contains("below the configured minimum")),
            "{strict:?}"
        );
        let mut lenient = policy();
        lenient.min_chars = 10;
        lenient.max_chars = 60;
        lenient.slots.clear();
        lenient.forbidden_markers.clear();
        let errs = paragraph_violations(&body(&small), &spec_schema(), &lenient);
        assert_eq!(
            errs,
            Vec::<String>::new(),
            "a config-lowered window must accept what the factory rejects"
        );
        // …and the max is config too: one char over the new max fails.
        let over = "а".repeat(61);
        let errs = paragraph_violations(&body(&over), &spec_schema(), &lenient);
        assert!(
            errs.iter()
                .any(|e| e.contains("above the configured maximum")),
            "{errs:?}"
        );
    }

    #[test]
    fn quality_report_render_is_exempt_by_construction() {
        // REQ-HP-03 AC5: the platform-assembled report_text never meets the
        // detector — its schema does not require the paragraph field, so
        // publish (any path) runs zero paragraph checks on it.
        let qr_schema = json!({
            "type": "object",
            "required": ["verdict", "recommendation_basis", "report_text", "profile", "evaluation"],
            "properties": { "report_text": { "type": "string" } }
        });
        let qr = json!({
            "verdict": "ACCEPT",
            "recommendation_basis": "all gates passed",
            "report_text": "verdict ACCEPT gates PASS checks all mechanical",
            "profile": { "project": "devai", "version": 12 },
            "evaluation": {}
        });
        let errs = paragraph_violations(&qr, &qr_schema, &policy());
        assert!(errs.is_empty(), "report_text must be exempt: {errs:?}");
        assert!(!schema_requires_paragraph(&qr_schema, &policy()));
    }

    #[test]
    fn old_contract_versions_are_not_retroactive() {
        // REQ-HP-06: @2-style schema without the paragraph requirement ⇒
        // zero paragraph violations, even for a body carrying no paragraph.
        let legacy = json!({
            "type": "object",
            "required": ["version", "requirements", "unknowns", "assumptions"],
            "properties": { "version": { "type": "integer" } }
        });
        let errs = paragraph_violations(&body(""), &legacy, &policy());
        assert!(
            errs.is_empty(),
            "no obligation under old contracts: {errs:?}"
        );
    }

    #[test]
    fn absent_paragraph_is_left_to_the_schema_gate() {
        // Presence belongs to `required` — the detector stays silent so the
        // repair prompt carries ONE text for ONE defect.
        let mut v = body("x");
        v.as_object_mut().unwrap().remove("human_paragraph");
        let errs = paragraph_violations(&v, &spec_schema(), &policy());
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn language_follows_the_business_statement_with_a_fallback() {
        let p = policy();
        assert_eq!(paragraph_language(&p, "Задача про русские тексты."), "ru");
        assert_eq!(
            paragraph_language(&p, "The task is written in English."),
            "en"
        );
        // No expressed language — the template value is the fallback (OQ-HP-3).
        assert_eq!(paragraph_language(&p, "1234 5678 ..."), "ru");
        let mut en = p.clone();
        en.language = "en".into();
        assert_eq!(paragraph_language(&en, "1234 5678 ..."), "en");
    }

    #[test]
    fn template_fires_only_on_contracts_that_require_the_field() {
        let p = policy();
        let business = "Требования стали формальными, но не читаемыми.";
        let s = template_section(&p, Some(&spec_schema().to_string()), business);
        assert!(s.is_some_and(|t| t.contains("human_paragraph")));
        // quality-report schema: no template (no obligation, no door).
        let qr = json!({"type":"object","required":["report_text"],"properties":{"report_text":{"type":"string"}}}).to_string();
        assert!(template_section(&p, Some(&qr), business).is_none());
        assert!(template_section(&p, None, business).is_none());
    }

    /// Rework C5 R3: an unparseable contract schema must NOT collapse into
    /// the honest None — the corruption prints an explicit marker naming
    /// the parse error, while "not declared" stays silent.
    #[test]
    fn corrupted_contract_schema_is_distinguishable_from_absent() {
        let p = policy();
        let business = "Требования стали формальными, но не читаемыми.";
        let broken = r#"{ "type": "object", "required": ["human_paragraph""#;
        let s = template_section(&p, Some(broken), business)
            .expect("corrupt schema yields an explicit marker, not silence");
        assert!(s.contains("СХЕМА КОНТРАКТА ПОВРЕЖДЕНА"), "{s}");
        let err = serde_json::from_str::<Value>(broken)
            .unwrap_err()
            .to_string();
        assert!(s.contains(&err), "the parse error is named verbatim: {s}");
        assert!(
            !s.contains("Обязательные слоты") && !s.contains("Структура объяснения"),
            "the corruption branch never renders the normal template: {s}"
        );
        // The honest branches are untouched by the repair.
        assert!(template_section(&p, None, business).is_none());
        let qr = json!({"type":"object","required":["report_text"],"properties":{"report_text":{"type":"string"}}}).to_string();
        assert!(template_section(&p, Some(&qr), business).is_none());
    }
}
