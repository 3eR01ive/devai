use devai_core::types::RoleDefinition;

/// Role system prompt. The role text (instructions, rules, criteria) is
/// compiled from the declarative config into the stored RoleDefinition —
/// this function only frames it. No hidden chain-of-thought is requested
/// or stored: only the verifiable result matters.
pub fn build_system_prompt(role: &RoleDefinition) -> String {
    format!(
        "You are the Devai `{role}` agent ({name}).\n\
         Responsibility: {purpose}\n\
         Instructions:\n{instructions}\n\
         Done criteria: {done}",
        role = role.slug,
        name = role.name,
        purpose = role.purpose,
        instructions = role.instructions,
        done = role.done_criteria,
    )
}

/// User prompt: the workflow inputs for this execution plus the contract.
pub fn build_user_prompt(
    title: &str,
    business: &str,
    inputs: &[(String, String)],
    output_contract: Option<(&str, u32)>,
    contract_schema: Option<&str>,
) -> String {
    let mut s = String::new();
    s.push_str(&format!("TASK: {title}\nBUSINESS DESCRIPTION:\n{business}\n\n"));
    if !inputs.is_empty() {
        s.push_str("INPUT ARTIFACTS:\n");
        for (label, content) in inputs {
            s.push_str(&format!("\n--- {label} ---\n{content}\n"));
        }
        s.push('\n');
    }
    if let Some((name, ver)) = output_contract {
        s.push_str(&format!("OUTPUT CONTRACT: {name}@{ver}\n"));
    }
    if let Some(schema) = contract_schema {
        s.push_str(&format!("OUTPUT JSON SCHEMA:\n{schema}\n\nReturn only the JSON object.\n"));
    }
    s
}

/// Best-effort extraction of the first balanced top-level JSON object from an
/// agent reply, tolerating code fences and surrounding prose.
pub fn extract_json_object(text: &str) -> Option<String> {
    let s = strip_fences(text);
    let start = s.find('{')?;
    let bytes: Vec<char> = s[start..].chars().collect();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut out = String::new();
    for c in bytes {
        out.push(c);
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(out);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_fences(text: &str) -> String {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(end) = rest.rfind("```") {
            return rest[..end].to_string();
        }
        return rest.to_string();
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_and_fenced_json() {
        assert_eq!(extract_json_object(r#"{"a":1}"#).as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(extract_json_object("```json\n{\"a\":1}\n```").as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(extract_json_object("here you go: {\"k\":{\"n\":2}} trailing").as_deref(), Some(r#"{"k":{"n":2}}"#));
        assert_eq!(extract_json_object("no object here"), None);
        // braces inside strings must not affect depth
        assert_eq!(extract_json_object(r#"{"s":"{not a brace}"}"#).as_deref(), Some(r#"{"s":"{not a brace}"}"#));
    }
}
