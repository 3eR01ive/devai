//! The mechanical name detector of the human task name (task 01a1ad1d,
//! REQ-TN-05; ratified by the operator's decision: «Дисциплина объёма/шаблон
//! имени — декларативный конфиг, по образцу шаблона очеловечивания H1» —
//! the same idiom as [`crate::humanize`], applied to the task's signboard).
//!
//! Scope is fixed BY CONSTRUCTION, not by a code branch over artifact kinds:
//! the detector fires ONLY on contracts whose stored schema requires the
//! configured name field (`task_name:` policy) as a top-level field — today
//! `requirements-specification@4`. Artifacts published under @1..@3 have no
//! such requirement in their stored schema, so they are exempt exactly as
//! REQ-TN-02 AC3 ratifies (the obligation is not retroactive — the
//! humanize/@3 precedent, seed.rs). The platform-assembled bodies of other
//! contracts carry no name field in their schemas: zero checks by
//! construction.
//!
//! What the detector checks, all from `config/workflow/task-name.yaml`
//! (REQ-TN-05 — never code literals, AC2 "его правка не требует ребилда"):
//!  * the volume window `min_chars`..`max_chars` (a blank/whitespace name
//!    is rejected: `required` already owns absence, the window owns
//!    emptiness and size — AC1 «пустое, пробельное, длиннее лимита»);
//!  * `single_line`: no embedded newline — a name must fit one list row /
//!    graph edge label (REQ-TN-05 statement).
//!
//! Same input ⇒ same verdict, always: pure functions over text, no time,
//! no randomness, no network, no model judgment (AC3).

use devai_store::config::TaskNamePolicy;
use serde_json::Value;

/// True when the stored schema REQUIRES the name field at top level — the
/// scope predicate of the detector (the `schema_requires_paragraph` twin).
pub fn schema_requires_name(schema: &Value, policy: &TaskNamePolicy) -> bool {
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

/// The detector: violations for the name of an already shape-valid body.
/// An ABSENT (or non-string) name is not reported here — the schema
/// `required` gate owns presence (one repair text per one defect, the
/// humanize discipline); this half owns EDIBILITY of the value.
pub fn name_violations(value: &Value, schema: &Value, policy: &TaskNamePolicy) -> Vec<String> {
    let mut errs = Vec::new();
    if !schema_requires_name(schema, policy) {
        return errs;
    }
    let Some(name) = value.get(&policy.field).and_then(|v| v.as_str()) else {
        return errs;
    };
    let path = format!("$.{}", policy.field);

    // «витрина не пуста»: a blank or whitespace-only value is not a name.
    let trimmed = name.trim();
    if trimmed.is_empty() {
        errs.push(format!(
            "{path}: the human task name is blank — write the short signboard of the task (a name is required to be a name, not whitespace)"
        ));
        return errs;
    }

    // Volume window from the config (REQ-TN-05; REQ-TN-01 "короткое").
    let chars = trimmed.chars().count();
    if chars < policy.min_chars {
        errs.push(format!(
            "{path}: name is {chars} chars, below the configured minimum {} — too short to say what the task is",
            policy.min_chars
        ));
    }
    if chars > policy.max_chars {
        errs.push(format!(
            "{path}: name is {chars} chars, above the configured maximum {} — the signboard must fit a list row and a graph label",
            policy.max_chars
        ));
    }

    // One-line rule: no embedded newline inside the value (the window is
    // over the trimmed text; a newline is never "surrounding whitespace").
    if policy.single_line && trimmed.chars().any(|c| c == '\n' || c == '\r') {
        errs.push(format!(
            "{path}: the name carries a newline — it must fit ONE line of a list or a graph (single_line is declared in config/workflow/task-name.yaml)"
        ));
    }
    errs
}

/// Validate a HUMAN-supplied name (the `task.rename` operation, ratified
/// OQ-TN-1) against the SAME window the publication gate uses — one rule,
/// two doors, no door with a weaker frame. Returns the trimmed name when
/// legal, the violation list when not.
pub fn validate_human_name(name: &str, policy: &TaskNamePolicy) -> Result<String, Vec<String>> {
    let mut errs = Vec::new();
    let trimmed = name.trim();
    if trimmed.is_empty() {
        errs.push("name is blank — a rename to nothing is not a signboard".to_string());
        return Err(errs);
    }
    let chars = trimmed.chars().count();
    if chars < policy.min_chars {
        errs.push(format!(
            "name is {chars} chars, below the configured minimum {} — too short to say what the task is",
            policy.min_chars
        ));
    }
    if chars > policy.max_chars {
        errs.push(format!(
            "name is {chars} chars, above the configured maximum {} — the signboard must fit a list row and a graph label",
            policy.max_chars
        ));
    }
    if policy.single_line && trimmed.chars().any(|c| c == '\n' || c == '\r') {
        errs.push(
            "the name carries a newline — it must fit ONE line of a list or a graph"
                .to_string(),
        );
    }
    if errs.is_empty() {
        Ok(trimmed.to_string())
    } else {
        Err(errs)
    }
}

/// The declarative template rendered into the ANALYST prompt (REQ-TN-05:
/// the volume discipline is config and this is its second delivery — the
/// gate itself never re-reads the prompt; the twin of
/// [`crate::humanize::template_section`]). Fires only when the stored
/// contract REQUIRES the name field; the honest three branches of the
/// humanize half are kept: no schema / no requirement ⇒ None (the duty is
/// stated exactly where the gate enforces it), unparseable schema ⇒ a loud
/// corruption marker, never silence.
pub fn template_section(
    policy: &TaskNamePolicy,
    contract_schema: Option<&str>,
    language: &str,
) -> Option<String> {
    let raw = contract_schema?;
    let schema_v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return Some(format!(
                concat!(
                    "\n=== TASK NAME: СХЕМА КОНТРАКТА ПОВРЕЖДЕНА ===\n",
                    "JSON выходного контракта не разобран: {err}. Требование поля `{field}` ",
                    "из такой схемы не читается — это повреждение контракта, а не «поле не ",
                    "требуется». Смысл не додумывай: зафиксируй повреждение вопросом ",
                    "существующим путём (`devai question ask`) и не публикуй имя наугад.\n"
                ),
                err = e,
                field = policy.field,
            ));
        }
    };
    if !schema_requires_name(&schema_v, policy) {
        return None;
    }
    Some(format!(
        concat!(
            "\n=== TASK NAME (часть протокола) ===\n",
            "Ответ обязан нести top-level поле `{field}` — короткое человеческое имя ",
            "САМОЙ задачи по смыслу текущей версии спека: им платформа подпишет задачу в ",
            "очередях, графе и UI.\n",
            "- Объём: {min}–{max} символов в одну строку (механический детектор платформы).\n",
            "- Язык: {lang} (язык бизнес-описания задачи).\n",
            "- Имя переиздаётся ЦЕЛИКОМ в каждой новой версии спека: пиши его заново по ",
            "смыслу текущей версии, а не правь старое.\n",
            "- Формальный заголовок задачи не переписывается и в ответ не входит: имя — ",
            "отдельное поле, заголовок остаётся текстом создавшего.\n"
        ),
        field = policy.field,
        min = policy.min_chars,
        max = policy.max_chars,
        lang = language,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_store::config::TaskNamePolicy;
    use serde_json::json;

    fn policy() -> TaskNamePolicy {
        TaskNamePolicy::default()
    }

    fn spec_schema() -> Value {
        json!({
            "type": "object",
            "required": ["version", "requirements", "unknowns", "assumptions", "human_paragraph", "task_name"],
            "properties": {
                "version": { "type": "integer" },
                "task_name": { "type": "string" },
                "human_paragraph": { "type": "string" },
                "requirements": { "type": "array" }
            }
        })
    }

    fn body(name: &str) -> Value {
        json!({
            "version": 1,
            "task_name": name,
            "human_paragraph": "…",
            "requirements": [],
            "unknowns": [],
            "assumptions": []
        })
    }

    #[test]
    fn good_name_passes() {
        let errs = name_violations(&body("Понятные имена задач в списках"), &spec_schema(), &policy());
        assert_eq!(errs, Vec::<String>::new(), "{errs:?}");
    }

    #[test]
    fn blank_name_is_rejected() {
        let errs = name_violations(&body("   "), &spec_schema(), &policy());
        assert!(
            errs.iter().any(|e| e.contains("blank")),
            "whitespace-only name must be rejected: {errs:?}"
        );
    }

    #[test]
    fn over_long_name_is_rejected() {
        let long = "а".repeat(61);
        let errs = name_violations(&body(&long), &spec_schema(), &policy());
        assert!(
            errs.iter().any(|e| e.contains("above the configured maximum")),
            "{errs:?}"
        );
    }

    #[test]
    fn newline_name_is_rejected() {
        let errs = name_violations(&body("две\nстроки"), &spec_schema(), &policy());
        assert!(errs.iter().any(|e| e.contains("newline")), "{errs:?}");
    }

    #[test]
    fn window_and_scope_come_from_config_not_code() {
        // A project that lowers the window accepts what the factory rejects…
        let mut lenient = policy();
        lenient.min_chars = 2;
        lenient.max_chars = 5;
        let errs = name_violations(&body("имя"), &spec_schema(), &lenient);
        assert_eq!(errs, Vec::<String>::new(), "{errs:?}");
        // …and one that lowers the max rejects one char over.
        let errs = name_violations(&body("имя456"), &spec_schema(), &lenient);
        assert!(
            errs.iter().any(|e| e.contains("above the configured maximum")),
            "{errs:?}"
        );
        // @3-style schema: no name requirement ⇒ zero checks (non-retroactive).
        let legacy = json!({
            "type": "object",
            "required": ["version", "requirements", "unknowns", "assumptions", "human_paragraph"],
            "properties": { "version": { "type": "integer" } }
        });
        let errs = name_violations(&body(""), &legacy, &policy());
        assert!(errs.is_empty(), "no obligation under @3: {errs:?}");
    }

    #[test]
    fn human_rename_shares_the_window() {
        assert!(validate_human_name("Понятные имена задач", &policy()).is_ok());
        assert!(validate_human_name(" ab ", &policy()).is_err());
        assert!(validate_human_name(&"x".repeat(61), &policy()).is_err());
        assert!(validate_human_name("две\nстроки", &policy()).is_err());
        // trimmed result is what gets stored
        assert_eq!(
            validate_human_name("  нормальное имя  ", &policy()).unwrap(),
            "нормальное имя"
        );
    }

    #[test]
    fn template_fires_only_on_contracts_that_require_the_field() {
        let p = policy();
        let s = template_section(&p, Some(&spec_schema().to_string()), "ru");
        assert!(s.is_some_and(|t| t.contains("task_name")));
        let qr = json!({"type":"object","required":["report_text"],"properties":{"report_text":{"type":"string"}}}).to_string();
        assert!(template_section(&p, Some(&qr), "ru").is_none());
        assert!(template_section(&p, None, "ru").is_none());
    }
}
