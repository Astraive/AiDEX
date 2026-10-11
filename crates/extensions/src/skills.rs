use crate::{
    ExtensionError, Result, SkillSummary,
    storage::{MAX_SKILL, digest, valid_id},
};
use std::collections::BTreeMap;

pub(crate) fn parse(content: &str) -> Result<(String, String, bool, bool)> {
    if content.len() > MAX_SKILL {
        return Err(ExtensionError::TooLarge);
    }
    if content.contains('\0') {
        return Err(ExtensionError::Invalid("skill contains NUL".into()));
    }
    let mut lines = content.lines();
    if lines.next() != Some("---") {
        return Err(ExtensionError::Invalid(
            "skill requires scalar YAML frontmatter".into(),
        ));
    }
    let mut fields = BTreeMap::new();
    let mut closed = false;
    for line in lines.take(128) {
        if line == "---" {
            closed = true;
            break;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| ExtensionError::Invalid("unsupported YAML frontmatter".into()))?;
        if key.trim() != key || key.is_empty() {
            return Err(ExtensionError::Invalid(
                "nested YAML frontmatter is unsupported".into(),
            ));
        }
        let value = value.trim();
        if matches!(
            value.chars().next(),
            Some('|' | '>' | '[' | '{' | '&' | '*' | '!')
        ) {
            return Err(ExtensionError::Invalid(
                "frontmatter supports scalar values only".into(),
            ));
        }
        let value = if value.starts_with('"') {
            serde_json::from_str::<String>(value)
                .map_err(|_| ExtensionError::Invalid("invalid quoted frontmatter".into()))?
        } else if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
            value[1..value.len() - 1].replace("''", "'")
        } else {
            value
                .split(" #")
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        if fields.insert(key.to_owned(), value).is_some() {
            return Err(ExtensionError::Invalid(
                "duplicate frontmatter field".into(),
            ));
        }
    }
    if !closed {
        return Err(ExtensionError::Invalid("unterminated frontmatter".into()));
    }
    let name = fields
        .remove("name")
        .ok_or_else(|| ExtensionError::Invalid("skill name is required".into()))?;
    valid_id(&name)?;
    let description = fields
        .remove("description")
        .ok_or_else(|| ExtensionError::Invalid("skill description is required".into()))?;
    if description.trim().is_empty() || description.len() > 4096 {
        return Err(ExtensionError::Invalid("invalid skill description".into()));
    }
    fn flag(value: Option<String>, default: bool) -> Result<bool> {
        match value.as_deref() {
            None => Ok(default),
            Some("true" | "yes" | "on" | "1") => Ok(true),
            Some("false" | "no" | "off" | "0") => Ok(false),
            _ => Err(ExtensionError::Invalid("invalid invocation flag".into())),
        }
    }
    let user = flag(fields.remove("user-invocable"), true)?;
    let model = !flag(fields.remove("disable-model-invocation"), false)?;
    Ok((name, description, user, model))
}
pub(crate) fn refresh(mut summary: SkillSummary, content: &str) -> Result<SkillSummary> {
    let (name, description, user, model) = parse(content)?;
    summary.name = name;
    summary.description = description;
    summary.user_invocable = user;
    summary.model_invocable = model;
    summary.digest = digest(content.as_bytes());
    Ok(summary)
}
