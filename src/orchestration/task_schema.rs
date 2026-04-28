//! F-GAP-07: Workflow Task Schema Specification
//!
//! Defines typed input/output schemas for agent roles,
//! with versioning support and validation.
//!
//! Each role (Planner, Coder, Reviewer, Tester, etc.) declares
//! what fields it expects as input and what fields it produces
//! as output.  The SchemaRegistry lets CapabilityBus validate
//! task envelopes before routing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A typed field definition within a role schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaField {
    pub name: String,
    /// One of "string", "number", "boolean", "array", "object"
    pub field_type: String,
    pub required: bool,
    pub description: String,
    pub default_value: Option<serde_json::Value>,
}

/// A complete schema for an agent role's input or output contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleSchema {
    pub role: String,
    pub version: String,
    pub input_fields: Vec<SchemaField>,
    pub output_fields: Vec<SchemaField>,
}

impl RoleSchema {
    /// Validate an input value against this schema's input fields.
    /// Returns `Ok` with a (possibly empty) list of warnings, or
    /// `Err` with a summary of missing required fields.
    pub fn validate_input(&self, input: &serde_json::Value) -> Result<Vec<String>, String> {
        let mut errors: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        for field in &self.input_fields {
            let present = input.get(&field.name).is_some();
            if field.required && !present {
                errors.push(format!(
                    "missing required field '{}' ({})",
                    field.name, field.description
                ));
            }
            if let Some(val) = input.get(&field.name) {
                if !type_matches(val, &field.field_type) {
                    warnings.push(format!(
                        "field '{}' expected type '{}' but got '{}'",
                        field.name,
                        field.field_type,
                        type_name(val)
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors.join("; "))
        }
    }

    /// Validate an output value against this schema's output fields.
    pub fn validate_output(&self, output: &serde_json::Value) -> Result<Vec<String>, String> {
        let mut errors: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        for field in &self.output_fields {
            let present = output.get(&field.name).is_some();
            if field.required && !present {
                errors.push(format!(
                    "missing required output field '{}' ({})",
                    field.name, field.description
                ));
            }
            if let Some(val) = output.get(&field.name) {
                if !type_matches(val, &field.field_type) {
                    warnings.push(format!(
                        "output field '{}' expected type '{}' but got '{}'",
                        field.name,
                        field.field_type,
                        type_name(val)
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors.join("; "))
        }
    }
}

fn type_matches(val: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "string" => val.is_string(),
        "number" => val.is_number(),
        "boolean" => val.is_boolean(),
        "array" => val.is_array(),
        "object" => val.is_object(),
        "any" => true,
        _ => true, // unknown types are permissive
    }
}

fn type_name(val: &serde_json::Value) -> &'static str {
    match val {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Registry of role schemas — used by CapabilityBus to validate
/// task envelopes before routing to an agent.
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    schemas: HashMap<String, RoleSchema>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, schema: RoleSchema) {
        self.schemas.insert(schema.role.clone(), schema);
    }

    pub fn get(&self, role: &str) -> Option<&RoleSchema> {
        self.schemas.get(role)
    }

    pub fn all(&self) -> Vec<&RoleSchema> {
        let mut v: Vec<_> = self.schemas.values().collect();
        v.sort_by_key(|s| s.role.as_str());
        v
    }

    /// Register the default schemas for the five built-in roles.
    pub fn register_defaults(&mut self) {
        self.register(RoleSchema {
            role: "planner".to_string(),
            version: "1.0".to_string(),
            input_fields: vec![
                SchemaField {
                    name: "objective".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    description: "High-level goal to achieve".to_string(),
                    default_value: None,
                },
                SchemaField {
                    name: "constraints".to_string(),
                    field_type: "string".to_string(),
                    required: false,
                    description: "Constraints or limitations".to_string(),
                    default_value: None,
                },
            ],
            output_fields: vec![
                SchemaField {
                    name: "plan".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    description: "Decomposed execution plan".to_string(),
                    default_value: None,
                },
                SchemaField {
                    name: "estimated_steps".to_string(),
                    field_type: "number".to_string(),
                    required: false,
                    description: "Number of steps in the plan".to_string(),
                    default_value: None,
                },
            ],
        });
        self.register(RoleSchema {
            role: "coder".to_string(),
            version: "1.0".to_string(),
            input_fields: vec![
                SchemaField {
                    name: "specification".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    description: "Code specification or description".to_string(),
                    default_value: None,
                },
                SchemaField {
                    name: "language".to_string(),
                    field_type: "string".to_string(),
                    required: false,
                    description: "Target programming language".to_string(),
                    default_value: Some(serde_json::json!("rust")),
                },
            ],
            output_fields: vec![
                SchemaField {
                    name: "code".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    description: "Generated source code".to_string(),
                    default_value: None,
                },
                SchemaField {
                    name: "tests".to_string(),
                    field_type: "string".to_string(),
                    required: false,
                    description: "Test code if applicable".to_string(),
                    default_value: None,
                },
            ],
        });
        self.register(RoleSchema {
            role: "reviewer".to_string(),
            version: "1.0".to_string(),
            input_fields: vec![
                SchemaField {
                    name: "code".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    description: "Source code to review".to_string(),
                    default_value: None,
                },
                SchemaField {
                    name: "standards".to_string(),
                    field_type: "string".to_string(),
                    required: false,
                    description: "Coding standards to check against".to_string(),
                    default_value: None,
                },
            ],
            output_fields: vec![
                SchemaField {
                    name: "verdict".to_string(),
                    field_type: "string".to_string(),
                    required: true,
                    description: "Review verdict (approved/changes/blocked)".to_string(),
                    default_value: None,
                },
                SchemaField {
                    name: "issues".to_string(),
                    field_type: "array".to_string(),
                    required: false,
                    description: "List of issues found".to_string(),
                    default_value: None,
                },
            ],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_get() {
        let mut reg = SchemaRegistry::new();
        reg.register_defaults();
        let schema = reg.get("coder");
        assert!(schema.is_some());
        assert_eq!(schema.unwrap().version, "1.0");
    }

    #[test]
    fn test_validate_input_pass() {
        let mut reg = SchemaRegistry::new();
        reg.register_defaults();
        let schema = reg.get("coder").unwrap();
        let input = serde_json::json!({"specification": "write a function"});
        let result = schema.validate_input(&input);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_input_fail_missing_required() {
        let mut reg = SchemaRegistry::new();
        reg.register_defaults();
        let schema = reg.get("coder").unwrap();
        let input = serde_json::json!({"language": "python"});
        let result = schema.validate_input(&input);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("specification"));
    }

    #[test]
    fn test_validate_output_pass() {
        let mut reg = SchemaRegistry::new();
        reg.register_defaults();
        let schema = reg.get("reviewer").unwrap();
        let output = serde_json::json!({"verdict": "approved", "issues": []});
        let result = schema.validate_output(&output);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_output_fail_missing_required() {
        let mut reg = SchemaRegistry::new();
        reg.register_defaults();
        let schema = reg.get("reviewer").unwrap();
        let output = serde_json::json!({"issues": ["bug"]});
        let result = schema.validate_output(&output);
        assert!(result.is_err());
    }

    #[test]
    fn test_type_mismatch_warning() {
        let schema = RoleSchema {
            role: "test".to_string(),
            version: "1.0".to_string(),
            input_fields: vec![SchemaField {
                name: "count".to_string(),
                field_type: "number".to_string(),
                required: true,
                description: "A count".to_string(),
                default_value: None,
            }],
            output_fields: vec![],
        };
        let input = serde_json::json!({"count": "not_a_number"});
        let result = schema.validate_input(&input);
        assert!(result.is_ok());
        let warnings = result.unwrap();
        assert!(warnings
            .iter()
            .any(|w| w.contains("expected type 'number'")));
    }

    #[test]
    fn test_register_defaults_contains_all_roles() {
        let mut reg = SchemaRegistry::new();
        reg.register_defaults();
        let all = reg.all();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|s| s.role.as_str()).collect();
        assert!(names.contains(&"coder"));
        assert!(names.contains(&"planner"));
        assert!(names.contains(&"reviewer"));
    }
}
