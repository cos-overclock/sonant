use jsonschema::JSONSchema;
use serde_json::Value;

use crate::domain::{GenerationResult, LlmError};

pub const GENERATION_RESULT_JSON_SCHEMA: &str = r#"
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["request_id", "model", "candidates"],
  "properties": {
    "request_id": {
      "type": "string",
      "minLength": 1
    },
    "model": {
      "type": "object",
      "additionalProperties": false,
      "required": ["provider", "model"],
      "properties": {
        "provider": {
          "type": "string",
          "minLength": 1
        },
        "model": {
          "type": "string",
          "minLength": 1
        }
      }
    },
    "candidates": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["id", "bars", "notes"],
        "properties": {
          "id": {
            "type": "string",
            "minLength": 1
          },
          "bars": {
            "type": "integer",
            "minimum": 1
          },
          "score_hint": {
            "type": ["number", "null"],
            "minimum": 0.0,
            "maximum": 1.0
          },
          "notes": {
            "type": "array",
            "minItems": 1,
            "items": {
              "type": "object",
              "additionalProperties": false,
              "required": ["pitch", "start_tick", "duration_tick", "velocity"],
              "properties": {
                "pitch": {
                  "type": "integer",
                  "minimum": 0,
                  "maximum": 127
                },
                "start_tick": {
                  "type": "integer",
                  "minimum": 0
                },
                "duration_tick": {
                  "type": "integer",
                  "minimum": 1
                },
                "velocity": {
                  "type": "integer",
                  "minimum": 0,
                  "maximum": 127
                },
                "channel": {
                  "type": "integer",
                  "minimum": 1,
                  "maximum": 16
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

pub struct LlmResponseSchemaValidator {
    compiled_schema: JSONSchema,
}

impl LlmResponseSchemaValidator {
    pub fn new() -> Result<Self, LlmError> {
        let schema: Value = serde_json::from_str(GENERATION_RESULT_JSON_SCHEMA).map_err(|err| {
            LlmError::internal(format!("invalid built-in generation schema: {err}"))
        })?;
        let compiled_schema = JSONSchema::compile(&schema).map_err(|err| {
            LlmError::internal(format!("failed to compile generation schema: {err}"))
        })?;
        Ok(Self { compiled_schema })
    }

    pub fn validate_response_json(
        &self,
        response_json: &str,
    ) -> Result<GenerationResult, LlmError> {
        let json_value: Value = serde_json::from_str(response_json).map_err(|err| {
            LlmError::invalid_response(format!("response JSON decode failed: {err}"))
        })?;
        self.validate_response_value(json_value)
    }

    pub fn validate_response_value(&self, response: Value) -> Result<GenerationResult, LlmError> {
        self.compiled_schema
            .validate(&response)
            .map_err(schema_validation_error)?;

        let result: GenerationResult = serde_json::from_value(response).map_err(|err| {
            LlmError::invalid_response(format!(
                "response JSON did not match GenerationResult contract: {err}"
            ))
        })?;

        // Keep domain-level rules as a second gate so validation behavior is centralized.
        result.validate().map_err(|err| match err {
            LlmError::Validation { message } => LlmError::invalid_response(message),
            other => other,
        })?;

        Ok(result)
    }
}

fn schema_validation_error<'a, I>(errors: I) -> LlmError
where
    I: IntoIterator<Item = jsonschema::ValidationError<'a>>,
{
    let details = errors
        .into_iter()
        .map(|err| err.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    LlmError::invalid_response(format!("response schema validation failed: {details}"))
}

#[cfg(test)]
mod tests {
    use super::LlmResponseSchemaValidator;
    use crate::domain::LlmError;

    fn validator() -> LlmResponseSchemaValidator {
        LlmResponseSchemaValidator::new().expect("schema validator must compile")
    }

    #[test]
    fn validate_response_json_accepts_valid_payload() {
        let json = r#"{
          "request_id": "req-42",
          "model": {
            "provider": "anthropic",
            "model": "claude-3-5-sonnet"
          },
          "candidates": [
            {
              "id": "cand-1",
              "bars": 4,
              "score_hint": 0.82,
              "notes": [
                {
                  "pitch": 60,
                  "start_tick": 0,
                  "duration_tick": 240,
                  "velocity": 96,
                  "channel": 1
                }
              ]
            }
          ]
        }"#;

        let result = validator()
            .validate_response_json(json)
            .expect("valid response should pass");

        assert_eq!(result.request_id, "req-42");
        assert_eq!(result.candidates.len(), 1);
    }

    #[test]
    fn validate_response_json_rejects_invalid_json() {
        let json = "{ this is not valid json";
        let error = validator()
            .validate_response_json(json)
            .expect_err("invalid JSON must fail");

        assert!(matches!(error, LlmError::InvalidResponse { .. }));
    }

    #[test]
    fn validate_response_json_rejects_schema_violation_as_invalid_response() {
        let json = r#"{
          "request_id": "req-42",
          "model": {
            "provider": "anthropic",
            "model": "claude-3-5-sonnet"
          },
          "candidates": []
        }"#;

        let error = validator()
            .validate_response_json(json)
            .expect_err("schema violation must fail");

        assert!(matches!(error, LlmError::InvalidResponse { .. }));
    }

    #[test]
    fn validate_response_json_rejects_domain_violation_as_invalid_response() {
        let json = r#"{
          "request_id": "   ",
          "model": {
            "provider": "anthropic",
            "model": "claude-3-5-sonnet"
          },
          "candidates": [
            {
              "id": "cand-1",
              "bars": 4,
              "notes": [
                {
                  "pitch": 60,
                  "start_tick": 0,
                  "duration_tick": 240,
                  "velocity": 96
                }
              ]
            }
          ]
        }"#;

        let error = validator()
            .validate_response_json(json)
            .expect_err("domain violation must fail");

        assert!(matches!(
            error,
            LlmError::InvalidResponse { message } if message == "request_id must not be empty"
        ));
    }
}
