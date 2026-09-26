//! MCP's bounded primitive forms projected into the existing question UI.

use serde_json::{Map, Value, json};

use crate::ports::agent_session::AgentSessionError;

pub(super) fn questions(schema: &Value) -> Result<Vec<Value>, AgentSessionError> {
    if schema["type"] != "object" || !schema.is_object() {
        return Err(AgentSessionError::Rejected);
    }
    let empty = Map::new();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    if properties.len() > 32
        || schema.as_object().is_some_and(|fields| {
            fields.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "type"
                        | "properties"
                        | "required"
                        | "title"
                        | "description"
                        | "additionalProperties"
                )
            })
        })
    {
        return Err(AgentSessionError::Rejected);
    }
    if let Some(required) = schema.get("required") {
        let required = required.as_array().ok_or(AgentSessionError::Rejected)?;
        if required
            .iter()
            .any(|key| key.as_str().is_none_or(|key| !properties.contains_key(key)))
        {
            return Err(AgentSessionError::Rejected);
        }
    }
    properties
        .iter()
        .map(|(key, property)| {
            if key.is_empty()
                || key.len() > 128
                || key.chars().any(char::is_control)
                || !supported(property)
            {
                return Err(AgentSessionError::Rejected);
            }
            let options = if property["type"] == "boolean" {
                vec![json!({"label":"true"}), json!({"label":"false"})]
            } else {
                let choice = if property["type"] == "array" {
                    &property["items"]
                } else {
                    property
                };
                choice["enum"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|value| json!({"label":value.as_str().unwrap_or("")}))
                    .collect()
            };
            Ok(
                json!({"id":key,"header":key,"question":property["title"].as_str().unwrap_or(key),
            "description":property["description"].as_str(),"options":options,
            "multiSelect":property["type"] == "array","allowOther":property.get("enum").is_none()}),
            )
        })
        .collect()
}

fn supported(property: &Value) -> bool {
    let Some(fields) = property.as_object() else {
        return false;
    };
    if fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type"
                | "title"
                | "description"
                | "default"
                | "enum"
                | "enumNames"
                | "minLength"
                | "maxLength"
                | "minimum"
                | "maximum"
                | "minItems"
                | "maxItems"
                | "items"
        )
    }) {
        return false;
    }
    let enumerated = property.get("enum").is_none_or(|values| {
        values
            .as_array()
            .is_some_and(|values| values.len() <= 128 && values.iter().all(Value::is_string))
    });
    enumerated
        && match property["type"].as_str() {
            Some("string" | "boolean" | "integer" | "number") => true,
            Some("array") => property["items"]["type"] == "string" && supported(&property["items"]),
            _ => false,
        }
}

pub(super) fn content(schema: &Value, updated: &Value) -> Result<Value, AgentSessionError> {
    questions(schema)?;
    let empty = Map::new();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let answers = updated
        .get("content")
        .or_else(|| updated.get("answers"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    if answers.keys().any(|key| !properties.contains_key(key)) {
        return Err(AgentSessionError::Rejected);
    }
    let raw = updated.get("content").is_some();
    let mut content = Map::new();
    for (key, property) in properties {
        let required = schema["required"]
            .as_array()
            .is_some_and(|keys| keys.iter().any(|value| value == key));
        let Some(answer) = answers.get(key).or_else(|| property.get("default")) else {
            if required {
                return Err(AgentSessionError::Rejected);
            }
            continue;
        };
        if !required && answer == "" {
            continue;
        }
        let value = if raw {
            answer.clone()
        } else {
            parse_answer(property, answer)?
        };
        validate_value(property, &value)?;
        content.insert(key.clone(), value);
    }
    Ok(Value::Object(content))
}

fn parse_answer(property: &Value, answer: &Value) -> Result<Value, AgentSessionError> {
    let Some(text) = answer.as_str() else {
        return Ok(answer.clone());
    };
    match property["type"].as_str() {
        Some("string") => Ok(json!(text)),
        Some("array") => Ok(json!(
            text.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        )),
        Some("boolean" | "integer" | "number") => {
            serde_json::from_str(text).map_err(|_| AgentSessionError::Rejected)
        }
        _ => Err(AgentSessionError::Rejected),
    }
}

fn validate_value(property: &Value, value: &Value) -> Result<(), AgentSessionError> {
    let valid = match property["type"].as_str() {
        Some("string") => value.as_str().is_some_and(|text| {
            text.len() <= 4096
                && !text.contains('\0')
                && count_bounds(property, text.chars().count(), "minLength", "maxLength")
                && property["enum"]
                    .as_array()
                    .is_none_or(|choices| choices.contains(value))
        }),
        Some("boolean") => value.is_boolean(),
        Some("number" | "integer") => value.as_f64().is_some_and(|number| {
            number.is_finite()
                && (property["type"] != "integer" || number.fract() == 0.0)
                && bounds(property, number, "minimum", "maximum")
        }),
        Some("array") => value.as_array().is_some_and(|values| {
            values.len() <= 128
                && count_bounds(property, values.len(), "minItems", "maxItems")
                && values
                    .iter()
                    .all(|value| validate_value(&property["items"], value).is_ok())
        }),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(AgentSessionError::Rejected)
    }
}

fn bounds(property: &Value, value: f64, minimum: &str, maximum: &str) -> bool {
    property
        .get(minimum)
        .is_none_or(|minimum| minimum.as_f64().is_some_and(|minimum| value >= minimum))
        && property
            .get(maximum)
            .is_none_or(|maximum| maximum.as_f64().is_some_and(|maximum| value <= maximum))
}

fn count_bounds(property: &Value, count: usize, minimum: &str, maximum: &str) -> bool {
    u32::try_from(count).is_ok_and(|count| bounds(property, f64::from(count), minimum, maximum))
}

#[cfg(test)]
mod tests;
