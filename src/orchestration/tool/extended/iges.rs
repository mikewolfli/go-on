//! IGES CAD file reading tools
//!
//! Provides `IgesReadTool` for reading IGES (Initial Graphics Exchange Specification)
//! files. IGES files are plain text with "S" (Start), "G" (Global), "D" (Directory),
//! and "P" (Parameter) section markers. Parsing is done natively without external
//! dependencies.
//! Only compiled when `feature = "cad-iges"` is enabled.

#[cfg(feature = "cad-iges")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-iges")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-iges")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-iges")]
use std::collections::BTreeMap;
#[cfg(feature = "cad-iges")]
use std::fs;
#[cfg(feature = "cad-iges")]
use tracing::info;

/// Parsed IGES summary.
#[cfg(feature = "cad-iges")]
struct IgesSummary {
    entity_count: usize,
    entity_types: BTreeMap<String, usize>,
    product_id: Option<String>,
    parameter_count: usize,
    start_section_lines: usize,
}

/// Parse an IGES file from its text content and return a summary.
#[cfg(feature = "cad-iges")]
fn parse_iges(content: &str) -> Result<IgesSummary> {
    let mut start_section_lines = 0usize;
    let mut global_section_lines = Vec::new();
    let mut entity_types: BTreeMap<String, usize> = BTreeMap::new();
    let mut entity_count = 0usize;
    let mut parameter_count = 0usize;

    for line in content.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }

        // IGES format: each line ends with a section identifier in column 73 (SE) or
        // column 73 is the section code (S, G, D, P, or T).
        // The section identifier is typically the last non-whitespace character.
        // More robust: look for a single capital letter after at least 72 characters,
        // or at the end of the line after spaces.
        let section_char = if let Some(ch) = trimmed.chars().last() {
            if ch == 'S' || ch == 'G' || ch == 'D' || ch == 'P' || ch == 'T' {
                // Also check that the character before is a digit (line number)
                let mut chars = trimmed.chars();
                chars.next_back();
                if let Some(prev) = chars.next_back() {
                    if prev.is_ascii_digit() {
                        ch
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            } else {
                continue;
            }
        } else {
            continue;
        };

        match section_char {
            'S' => {
                start_section_lines += 1;
            }
            'G' => {
                global_section_lines.push(trimmed);
            }
            'D' => {
                // Directory Entry lines come in pairs (2 lines per entity)
                // The DE line contains entity type number in columns 1-8
                if let Some(type_str) = trimmed.get(..8) {
                    let type_num = type_str.trim();
                    if !type_num.is_empty() && type_num.chars().all(|c| c.is_ascii_digit()) {
                        // Map common IGES entity type numbers to names
                        let type_name = iges_entity_name(type_num);
                        *entity_types.entry(type_name).or_insert(0) += 1;
                        entity_count += 1;
                    }
                }
            }
            'P' => {
                parameter_count += 1;
            }
            'T' => {}
            _ => {}
        }
    }

    // The global section contains product identification in a specific format.
    // Parameter Data section numbers are delimited by ',' or ';'.
    // Sample: "1H,,1H,,8HIGES file,"
    // We extract a product ID from the global section.
    let product_id = if !global_section_lines.is_empty() {
        // Take the first global section line and try to find product ID in
        // columns 1-8 or between delimiters
        let first = global_section_lines[0];
        // IGES Global section format: parameter delimiter, record delimiter,
        // product ID (as Hollerith string), etc.
        // Product ID is typically the 3rd field (delimited by the parameter delimiter)
        let param_delim = first.chars().next().unwrap_or(',');
        let parts: Vec<&str> = first.split(param_delim).collect();
        if parts.len() > 2 {
            let raw = parts[2].trim();
            // Strip Hollerith prefix (e.g., "8HProduct")
            let clean = if let Some(idx) = raw.find('H') {
                raw[idx + 1..].to_string()
            } else {
                raw.to_string()
            };
            if !clean.is_empty() {
                Some(clean)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Ok(IgesSummary {
        entity_count,
        entity_types,
        product_id,
        parameter_count,
        start_section_lines,
    })
}

/// Map IGES entity type number to a human-readable name.
/// Covers common IGES entity types.
#[cfg(feature = "cad-iges")]
fn iges_entity_name(type_num: &str) -> String {
    match type_num {
        "100" => "Circular Arc".to_string(),
        "102" => "Composite Curve".to_string(),
        "104" => "Conic Arc".to_string(),
        "106" => "Copious Data".to_string(),
        "108" => "Plane".to_string(),
        "110" => "Line".to_string(),
        "112" => "Parametric Spline Curve".to_string(),
        "114" => "Parametric Spline Surface".to_string(),
        "116" => "Point".to_string(),
        "118" => "Ruled Surface".to_string(),
        "120" => "Surface of Revolution".to_string(),
        "122" => "Tabulated Cylinder".to_string(),
        "124" => "Transformation Matrix".to_string(),
        "126" => "Rational B-Spline Curve".to_string(),
        "128" => "Rational B-Spline Surface".to_string(),
        "130" => "Offset Curve".to_string(),
        "132" => "Connect Point".to_string(),
        "134" => "Node".to_string(),
        "136" => "Finite Element".to_string(),
        "138" => "Nodal Displacement/Rotation".to_string(),
        "140" => "Offset Surface".to_string(),
        "142" => "Curve on Parametric Surface".to_string(),
        "144" => "Trimmed Parametric Surface".to_string(),
        "146" => "External Reference".to_string(),
        "148" => "External Reference File".to_string(),
        "150" => "Block".to_string(),
        "152" => "Right Angular Wedge".to_string(),
        "154" => "Right Circular Cylinder".to_string(),
        "156" => "Right Circular Cone Frustum".to_string(),
        "158" => "Sphere".to_string(),
        "160" => "Toroidal Surface".to_string(),
        "162" => "Solid of Revolution".to_string(),
        "164" => "Solid of Linear Extrusion".to_string(),
        "168" => "Ellipsoid".to_string(),
        "180" => "Boolean Tree".to_string(),
        "182" => "Selected Component".to_string(),
        "184" => "Solid Assembly".to_string(),
        "186" => "Manifold Solid B-Rep Object".to_string(),
        "190" => "Curve on Surface".to_string(),
        "192" => "Advanced Face".to_string(),
        "194" => "Advanced B-Rep Shell".to_string(),
        "196" => "Advanced B-Rep Solid".to_string(),
        "198" => "Advanced B-Rep Closed Shell".to_string(),
        "202" => "Angular Dimension".to_string(),
        "204" => "Curve Dimension".to_string(),
        "206" => "Diameter Dimension".to_string(),
        "208" => "Flag Note".to_string(),
        "210" => "General Label".to_string(),
        "212" => "General Note".to_string(),
        "214" => "Leader Line".to_string(),
        "216" => "Linear Dimension".to_string(),
        "218" => "Ordinate Dimension".to_string(),
        "220" => "Point Dimension".to_string(),
        "222" => "Radius Dimension".to_string(),
        "224" => "Sectioned Area".to_string(),
        "226" => "Sectioned Area (old)".to_string(),
        "228" => "General Symbol".to_string(),
        "230" => "Sectioned Area (new)".to_string(),
        "302" => "Associativity Definition".to_string(),
        "304" => "Line Font Definition".to_string(),
        "306" => "Macro Definition".to_string(),
        "308" => "Subfigure Definition".to_string(),
        "310" => "Text Font Definition".to_string(),
        "312" => "Text Display Template".to_string(),
        "314" => "Color Definition".to_string(),
        "316" => "Units Data".to_string(),
        "320" => "Attribute Table Definition".to_string(),
        "322" => "Attribute Table".to_string(),
        _ => format!("Type {}", type_num),
    }
}

#[cfg(feature = "cad-iges")]
pub struct IgesReadTool;

#[cfg(feature = "cad-iges")]
impl Tool for IgesReadTool {
    fn name(&self) -> &'static str {
        "iges_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read IGES: {}", validated.display()))?;

        let summary = parse_iges(&content)?;
        let byte_size = content.len();

        info!(
            path = %validated.display(),
            entities = summary.entity_count,
            "IGES file read"
        );

        let report = tool_execution_report("iges_read", Some("cad_read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "entity_count": summary.entity_count,
                "entity_types": summary.entity_types,
                "product_id": summary.product_id,
                "parameter_count": summary.parameter_count,
                "start_section_lines": summary.start_section_lines,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "iges_read: {} entities, {} parameter lines from {}",
                summary.entity_count,
                summary.parameter_count,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "cad-iges")]
mod tests {
    use super::*;

    fn test_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "iges-test".to_string(),
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
    fn parse_minimal_iges() {
        // Format: each line ends with a section code (S, G, D, P, T)
        // Column 73 is section identifier, preceded by line number
        let iges = r#"some start data                                               1S
,,10HIGES test,8HExample ,,16HGo-on CAD tool,               1G
    100     1     1     1     0     0       0               1D
    100     1     1     1     0     0       0               2D
    110     1     1     1     0     0       0               3D
    110     1     1     1     0     0       0               4D
    126     1     1     1     0     0       0               5D
    126     1     1     1     0     0       0               6D
     1                                                      1P
     1                                                      2P
"#;
        let summary = parse_iges(iges).expect("valid IGES");
        assert_eq!(summary.entity_count, 6);
        assert_eq!(summary.start_section_lines, 1);
        assert!(summary.product_id.is_some());
        // 6 D-lines: 2 per entity type (Circular Arc(100), Line(110), Rational B-Spline Curve(126))
        assert_eq!(*summary.entity_types.get("Circular Arc").unwrap_or(&0), 2);
        assert_eq!(*summary.entity_types.get("Line").unwrap_or(&0), 2);
        assert_eq!(
            *summary
                .entity_types
                .get("Rational B-Spline Curve")
                .unwrap_or(&0),
            2
        );
    }

    #[test]
    fn parse_empty_section() {
        let iges = r#"                                                                        S      1
                                                                                        G      1
                                                                                        D      1
                                                                                        P      1
"#;
        let summary = parse_iges(iges).expect("valid IGES");
        assert_eq!(summary.entity_count, 0);
        assert_eq!(summary.entity_types.len(), 0);
    }
}
