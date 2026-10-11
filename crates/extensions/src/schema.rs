//! Deliberately bounded JSON Schema subset. Unsupported semantics fail closed.
use crate::{ExtensionError, Result};
use serde_json::Value;

const KEYS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minItems",
    "maxItems",
    "title",
    "description",
    "default",
    "examples",
    "$schema",
    "deprecated",
];
pub(crate) fn check(schema: &Value) -> Result<()> {
    if serde_json::to_vec(schema)
        .map_err(|_| ExtensionError::TooLarge)?
        .len()
        > 65536
    {
        return Err(ExtensionError::TooLarge);
    }
    check_inner(schema, 0)
}
fn check_inner(schema: &Value, depth: usize) -> Result<()> {
    if depth > 16 {
        return Err(ExtensionError::TooLarge);
    }
    let object = schema
        .as_object()
        .ok_or_else(|| ExtensionError::Invalid("MCP schema must be an object".into()))?;
    if object.keys().any(|key| !KEYS.contains(&key.as_str())) {
        return Err(ExtensionError::Invalid(
            "MCP schema uses unsupported constraints or references".into(),
        ));
    }
    if let Some(kind) = object.get("type")
        && !matches!(
            kind.as_str(),
            Some("object" | "array" | "string" | "boolean" | "integer" | "number" | "null")
        )
    {
        return Err(ExtensionError::Invalid(
            "unsupported MCP schema type".into(),
        ));
    }
    if let Some(properties) = object.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(|| ExtensionError::Invalid("schema properties must be an object".into()))?;
        if properties.len() > 256 {
            return Err(ExtensionError::TooLarge);
        }
        for property in properties.values() {
            check_inner(property, depth + 1)?
        }
    }
    if let Some(required) = object.get("required")
        && !required
            .as_array()
            .is_some_and(|a| a.len() <= 256 && a.iter().all(|v| v.as_str().is_some()))
    {
        return Err(ExtensionError::Invalid(
            "schema required must contain names".into(),
        ));
    }
    if let Some(extra) = object.get("additionalProperties")
        && !extra.is_boolean()
    {
        check_inner(extra, depth + 1)?
    }
    if let Some(items) = object.get("items") {
        check_inner(items, depth + 1)?
    }
    if let Some(values) = object.get("enum")
        && !values
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.len() <= 256)
    {
        return Err(ExtensionError::Invalid("invalid schema enum".into()));
    }
    for key in ["minLength", "maxLength", "minItems", "maxItems"] {
        if object.get(key).is_some_and(|v| v.as_u64().is_none()) {
            return Err(ExtensionError::Invalid(
                "invalid schema length bound".into(),
            ));
        }
    }
    for key in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
        if object.get(key).is_some_and(|v| v.as_f64().is_none()) {
            return Err(ExtensionError::Invalid(
                "invalid numeric schema bound".into(),
            ));
        }
    }
    Ok(())
}
pub(crate) fn validate(schema: &Value, value: &Value) -> Result<()> {
    check(schema)?;
    validate_inner(schema, value, 0)
}
fn invalid() -> ExtensionError {
    ExtensionError::Invalid("MCP arguments do not match discovered schema".into())
}
fn validate_inner(schema: &Value, value: &Value, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(ExtensionError::TooLarge);
    }
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let matches = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            _ => false,
        };
        if !matches {
            return Err(invalid());
        }
    }
    if schema.get("const").is_some_and(|v| v != value)
        || schema
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.contains(value))
    {
        return Err(invalid());
    }
    if let Some(object) = value.as_object() {
        if object.len() > 256 {
            return Err(ExtensionError::TooLarge);
        }
        if schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|keys| {
                keys.iter()
                    .any(|key| !object.contains_key(key.as_str().unwrap_or_default()))
            })
        {
            return Err(invalid());
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (name, v) in object {
            if let Some(s) = properties.and_then(|p| p.get(name)) {
                validate_inner(s, v, depth + 1)?
            } else {
                match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => return Err(invalid()),
                    Some(s) if s.is_object() => validate_inner(s, v, depth + 1)?,
                    _ => {}
                }
            }
        }
    }
    if let Some(array) = value.as_array() {
        if array.len() > 4096 {
            return Err(ExtensionError::TooLarge);
        }
        bounds(schema, "minItems", "maxItems", array.len() as u64)?;
        if let Some(items) = schema.get("items") {
            for item in array {
                validate_inner(items, item, depth + 1)?
            }
        }
    }
    if let Some(text) = value.as_str() {
        bounds(
            schema,
            "minLength",
            "maxLength",
            text.chars().count() as u64,
        )?
    }
    if let Some(n) = value.as_f64()
        && (schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|v| n < v)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|v| n > v)
            || schema
                .get("exclusiveMinimum")
                .and_then(Value::as_f64)
                .is_some_and(|v| n <= v)
            || schema
                .get("exclusiveMaximum")
                .and_then(Value::as_f64)
                .is_some_and(|v| n >= v))
    {
        return Err(invalid());
    }
    Ok(())
}
fn bounds(schema: &Value, min: &str, max: &str, length: u64) -> Result<()> {
    if schema
        .get(min)
        .and_then(Value::as_u64)
        .is_some_and(|v| length < v)
        || schema
            .get(max)
            .and_then(Value::as_u64)
            .is_some_and(|v| length > v)
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_nested_required_and_unknown_fields() {
        let s = json!({"type":"object","properties":{"items":{"type":"array","maxItems":2,"items":{"type":"integer","minimum":1}}},"required":["items"],"additionalProperties":false});
        assert!(validate(&s, &json!({"items":[1,2]})).is_ok());
        for v in [
            json!({}),
            json!({"items":[0]}),
            json!({"items":[1],"unexpected":true}),
        ] {
            assert!(validate(&s, &v).is_err())
        }
    }
    #[test]
    fn unsupported_semantics_are_rejected() {
        for s in [
            json!({"$ref":"#/defs/input"}),
            json!({"type":"string","pattern":"secret"}),
            json!({"oneOf":[{"type":"string"}]}),
        ] {
            assert!(check(&s).is_err())
        }
    }
}
