//! CAD utility tools
//!
//! Provides `CadConvertTool` for converting between CAD coordinate systems,
//! units (mm, inch, cm, m, ft), and angle formats.
//! Only compiled when `feature = "cad-utils"` is enabled.

#[cfg(feature = "cad-utils")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-utils")]
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-utils")]
use anyhow::Result;
#[cfg(feature = "cad-utils")]
use tracing::info;

/// Unit conversion factors relative to 1 millimeter.
#[cfg(feature = "cad-utils")]
const MM_TO_MM: f64 = 1.0;
#[cfg(feature = "cad-utils")]
const CM_TO_MM: f64 = 10.0;
#[cfg(feature = "cad-utils")]
const M_TO_MM: f64 = 1000.0;
#[cfg(feature = "cad-utils")]
const INCH_TO_MM: f64 = 25.4;
#[cfg(feature = "cad-utils")]
const FT_TO_MM: f64 = 304.8;

#[cfg(feature = "cad-utils")]
fn unit_factor(unit: &str) -> Option<f64> {
    match unit.to_lowercase().as_str() {
        "mm" | "millimeter" | "millimeters" => Some(MM_TO_MM),
        "cm" | "centimeter" | "centimeters" => Some(CM_TO_MM),
        "m" | "meter" | "meters" => Some(M_TO_MM),
        "in" | "inch" | "inches" | "\"" => Some(INCH_TO_MM),
        "ft" | "foot" | "feet" | "'" => Some(FT_TO_MM),
        _ => None,
    }
}

#[cfg(feature = "cad-utils")]
fn unit_name(unit: &str) -> &str {
    match unit.to_lowercase().as_str() {
        "mm" | "millimeter" | "millimeters" => "mm",
        "cm" | "centimeter" | "centimeters" => "cm",
        "m" | "meter" | "meters" => "m",
        "in" | "inch" | "inches" | "\"" => "in",
        "ft" | "foot" | "feet" | "'" => "ft",
        _ => unit,
    }
}

#[cfg(feature = "cad-utils")]
pub struct CadConvertTool;

#[cfg(feature = "cad-utils")]
impl Tool for CadConvertTool {
    fn name(&self) -> &'static str {
        "cad_convert"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // Determine operation: "convert" (unit conversion) or "rotate" (angle conversion)
        let operation = input.payload["operation"].as_str().unwrap_or("convert");

        match operation {
            "convert" => self.run_convert(input),
            "rotate" => self.run_rotate(input),
            other => Err(anyhow::anyhow!(
                "unknown operation '{other}'; expected 'convert' or 'rotate'"
            )),
        }
    }
}

#[cfg(feature = "cad-utils")]
impl CadConvertTool {
    fn run_convert(&self, input: &ToolInput) -> Result<ToolOutput> {
        let value = input.payload["value"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("missing or non-numeric 'value' in payload"))?;

        let from_unit = input.payload["from"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'from' unit in payload"))?;

        let to_unit = input.payload["to"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'to' unit in payload"))?;

        let from_factor =
            unit_factor(from_unit).ok_or_else(|| anyhow::anyhow!("unknown unit '{from_unit}'"))?;
        let to_factor =
            unit_factor(to_unit).ok_or_else(|| anyhow::anyhow!("unknown unit '{to_unit}'"))?;

        // Convert: value * (from_factor / to_factor)
        let converted = value * from_factor / to_factor;
        let from_name = unit_name(from_unit);
        let to_name = unit_name(to_unit);

        info!(
            value = value,
            from = from_name,
            to = to_name,
            result = converted,
            "CAD unit conversion"
        );

        let report = tool_execution_report("cad_convert", Some("convert"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "operation": "convert",
                "input_value": value,
                "input_unit": from_name,
                "output_value": converted,
                "output_unit": to_name,
                "conversion_factor": from_factor / to_factor,
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "cad_convert: {} {} → {} {}",
                value, from_name, converted, to_name
            )),
            pua_report: Some(report),
        })
    }

    fn run_rotate(&self, input: &ToolInput) -> Result<ToolOutput> {
        let value = input.payload["value"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("missing or non-numeric 'value' in payload"))?;

        let from_format = input.payload["from"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'from' format in payload"))?;

        let to_format = input.payload["to"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'to' format in payload"))?;

        let (converted, _desc) = match (
            from_format.to_lowercase().as_str(),
            to_format.to_lowercase().as_str(),
        ) {
            ("deg" | "degrees", "rad" | "radians") => (
                value.to_radians(),
                format!("{value}° → {} rad", value.to_radians()),
            ),
            ("rad" | "radians", "deg" | "degrees") => (
                value.to_degrees(),
                format!("{value} rad → {}°", value.to_degrees()),
            ),
            ("deg" | "degrees", "deg" | "degrees") => (value, format!("{value}° (unchanged)")),
            ("rad" | "radians", "rad" | "radians") => (value, format!("{value} rad (unchanged)")),
            _ => {
                return Err(anyhow::anyhow!(
                    "unsupported angle format conversion: '{from_format}' → '{to_format}'. Supported: deg, rad"
                ));
            }
        };

        info!(
            value = value,
            from = from_format,
            to = to_format,
            result = converted,
            "CAD angle conversion"
        );

        let report = tool_execution_report("cad_convert", Some("rotate"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "operation": "rotate",
                "input_value": value,
                "input_format": from_format,
                "output_value": converted,
                "output_format": to_format,
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "cad_convert: angle {} {} → {} {}",
                value, from_format, converted, to_format
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "cad-utils")]
mod tests {
    use super::*;
    use crate::orchestration::tool::{Tool, ToolInput};

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-cad".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn convert_mm_to_inch() {
        let tool = CadConvertTool;
        let input = tool_input(serde_json::json!({
            "operation": "convert",
            "value": 25.4,
            "from": "mm",
            "to": "in",
        }));
        let output = tool.run(&input).expect("conversion should succeed");
        let result = output.result.unwrap();
        let out_val = result["output_value"].as_f64().unwrap();
        assert!(
            (out_val - 1.0).abs() < 1e-10,
            "25.4 mm should be 1 inch, got {out_val}"
        );
    }

    #[test]
    fn convert_inch_to_mm() {
        let tool = CadConvertTool;
        let input = tool_input(serde_json::json!({
            "operation": "convert",
            "value": 1.0,
            "from": "in",
            "to": "mm",
        }));
        let output = tool.run(&input).expect("conversion should succeed");
        let result = output.result.unwrap();
        let out_val = result["output_value"].as_f64().unwrap();
        assert!(
            (out_val - 25.4).abs() < 1e-10,
            "1 inch should be 25.4 mm, got {out_val}"
        );
    }

    #[test]
    fn convert_ft_to_m() {
        let tool = CadConvertTool;
        let input = tool_input(serde_json::json!({
            "operation": "convert",
            "value": 1.0,
            "from": "ft",
            "to": "m",
        }));
        let output = tool.run(&input).expect("conversion should succeed");
        let result = output.result.unwrap();
        let out_val = result["output_value"].as_f64().unwrap();
        assert!(
            (out_val - 0.3048).abs() < 1e-10,
            "1 ft should be 0.3048 m, got {out_val}"
        );
    }

    #[test]
    fn rotate_deg_to_rad() {
        let tool = CadConvertTool;
        let input = tool_input(serde_json::json!({
            "operation": "rotate",
            "value": 180.0,
            "from": "deg",
            "to": "rad",
        }));
        let output = tool.run(&input).expect("rotation should succeed");
        let result = output.result.unwrap();
        let out_val = result["output_value"].as_f64().unwrap();
        assert!(
            (out_val - std::f64::consts::PI).abs() < 1e-10,
            "180° should be π rad, got {out_val}"
        );
    }

    #[test]
    fn rotate_rad_to_deg() {
        let tool = CadConvertTool;
        let input = tool_input(serde_json::json!({
            "operation": "rotate",
            "value": std::f64::consts::PI,
            "from": "rad",
            "to": "deg",
        }));
        let output = tool.run(&input).expect("rotation should succeed");
        let result = output.result.unwrap();
        let out_val = result["output_value"].as_f64().unwrap();
        assert!(
            (out_val - 180.0).abs() < 1e-10,
            "π rad should be 180°, got {out_val}"
        );
    }
}
