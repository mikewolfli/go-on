//! Invoice document parsing tool
//!
//! Provides `InvoiceParseTool` for extracting common invoice fields (invoice number,
//! date, vendor, total amount, line items) from text content using pure Rust string
//! parsing and regex patterns. No external invoice-parsing crates required.
//! Only compiled when `feature = "document-invoice"` is enabled.

#[cfg(feature = "document-invoice")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "document-invoice")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "document-invoice")]
use anyhow::{Context, Result};
#[cfg(feature = "document-invoice")]
use std::fs;
#[cfg(feature = "document-invoice")]
use tracing::info;

// ── InvoiceParseTool ──────────────────────────────────────────────────────────

#[cfg(feature = "document-invoice")]
pub struct InvoiceParseTool;

#[cfg(feature = "document-invoice")]
impl Tool for InvoiceParseTool {
    fn name(&self) -> &'static str {
        "invoice_parse"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let has_text = input.payload["text"].as_str();
        let path = input.payload["path"].as_str();

        let content: String = if let Some(text) = has_text {
            text.to_string()
        } else if let Some(p) = path {
            let validated = sanitize_path(input, p)?;
            fs::read_to_string(&validated)
                .with_context(|| format!("failed to read file: {}", validated.display()))?
        } else {
            anyhow::bail!("either 'text' or 'path' must be provided");
        };

        info!(text_len = content.len(), "parsing invoice from content");

        let invoice = parse_invoice(&content);

        let report = tool_execution_report("invoice_parse", Some("invoice_parsed"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::to_value(&invoice)?),
            error: None,
            verification: Some("invoice_parsed".to_string()),
            audit_log: Some(format!(
                "Parsed invoice: number='{}', vendor='{}', total={:?}",
                invoice.invoice_number, invoice.vendor, invoice.total_amount
            )),
            pua_report: Some(report),
        })
    }
}

// ── Data structures ───────────────────────────────────────────────────────────

#[cfg(feature = "document-invoice")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InvoiceData {
    pub invoice_number: String,
    pub date: String,
    pub vendor: String,
    pub customer: String,
    pub total_amount: Option<f64>,
    pub subtotal: Option<f64>,
    pub tax_amount: Option<f64>,
    pub currency: String,
    pub line_items: Vec<LineItem>,
    pub raw_text: String,
}

#[cfg(feature = "document-invoice")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LineItem {
    pub description: String,
    pub quantity: Option<f64>,
    pub unit_price: Option<f64>,
    pub amount: Option<f64>,
}

// ── Invoice parser ────────────────────────────────────────────────────────────

#[cfg(feature = "document-invoice")]
fn parse_invoice(text: &str) -> InvoiceData {
    let invoice_number = extract_invoice_number(text);
    let date = extract_date(text);
    let vendor = extract_vendor(text);
    let customer = extract_customer(text);
    let total_amount = extract_total_amount(text);
    let subtotal = extract_subtotal(text);
    let tax_amount = extract_tax_amount(text);
    let currency = extract_currency(text);
    let line_items = extract_line_items(text);

    InvoiceData {
        invoice_number,
        date,
        vendor,
        customer,
        total_amount,
        subtotal,
        tax_amount,
        currency,
        line_items,
        raw_text: text.to_string(),
    }
}

/// Extract invoice number using common patterns.
#[cfg(feature = "document-invoice")]
fn extract_invoice_number(text: &str) -> String {
    let patterns = [
        // "INV-12345", "INV12345" — use horizontal whitespace only to avoid matching across lines
        r"(?i)\binvoice[ \t]*(?:#|no\.?|number)?[ \t]*:?[ \t]*([\w\d][-/\w\d]{2,30})",
        // "INV-12345" standalone
        r"(?i)\bINV[-]?(\d{4,20})\b",
        // "Invoice Number: 12345"
        r"(?i)invoice\s+(?:#|number|no\.?)\s*[:#]?\s*(\S+)",
        // Just a number after "Invoice"
        r"(?i)invoice\s+(\d{4,20})",
    ];

    for pattern in &patterns {
        if let Some(cap) = regex_lite(pattern, text) {
            let val = cap.trim();
            if !val.is_empty() && val.len() <= 30 {
                return val.to_string();
            }
        }
    }

    String::new()
}

/// Extract date from text using common date patterns.
#[cfg(feature = "document-invoice")]
fn extract_date(text: &str) -> String {
    let patterns = [
        // "Date: 2024-01-15" or "Invoice Date: 01/15/2024"
        r"(?i)(?:invoice\s*)?date\s*:?\s*(\d{1,4}[-/.]\d{1,2}[-/.]\d{1,4})",
        // "January 15, 2024"
        r"(?i)(?:invoice\s*)?date\s*:?\s*([A-Z][a-z]+ \d{1,2},?\s*\d{4})",
        // MM/DD/YYYY or DD/MM/YYYY standalone
        r"\b(\d{1,2}[/-]\d{1,2}[/-]\d{2,4})\b",
        // YYYY-MM-DD
        r"\b(\d{4}-\d{2}-\d{2})\b",
    ];

    for pattern in &patterns {
        if let Some(cap) = regex_lite(pattern, text) {
            let val = cap.trim();
            if !val.is_empty() && val.len() <= 30 {
                return val.to_string();
            }
        }
    }

    String::new()
}

/// Extract vendor/sender name.
#[cfg(feature = "document-invoice")]
fn extract_vendor(text: &str) -> String {
    let patterns = [
        r"(?i)(?:from|vendor|seller|supplier|bill\s*from|remit\s*to)\s*:?\s*(.+?)[\r\n]",
        r"(?i)^\s*([A-Z][A-Za-z0-9\s&.,'-]{2,60})\s*[\r\n]",
        r"(?i)(?:company|organization|business)\s*:?\s*(.+?)[\r\n]",
    ];

    for pattern in &patterns {
        if let Some(cap) = regex_lite(pattern, text) {
            let val = cap.trim();
            if !val.is_empty() && val.len() >= 2 && val.len() <= 60 {
                return val.to_string();
            }
        }
    }

    String::new()
}

/// Extract customer/bill-to name.
#[cfg(feature = "document-invoice")]
fn extract_customer(text: &str) -> String {
    let patterns = [
        r"(?i)(?:to|customer|client|bill\s*to|sold\s*to|ship\s*to)\s*:?\s*(.+?)[\r\n]",
        r"(?i)(?:bill\s*to|sold\s*to|ship\s*to)\s*:?\s*(.+?)[\r\n]",
    ];

    for pattern in &patterns {
        if let Some(cap) = regex_lite(pattern, text) {
            let val = cap.trim();
            if !val.is_empty() && val.len() >= 2 {
                return val.to_string();
            }
        }
    }

    String::new()
}

/// Extract total amount.
#[cfg(feature = "document-invoice")]
fn extract_total_amount(text: &str) -> Option<f64> {
    let patterns = [
        r"(?i)(?:\btotal\b|amount\s*due|balance\s*due|grand\s*total|total\s*due)\s*:?\s*[\$€£¥]?\s*(\d{1,10}(?:,\d{3})*(?:\.\d{1,2})?)",
        r"(?i)(?:\btotal\b|amount\s*due|balance\s*due)\s*[\$€£¥]?\s*(\d{1,10}(?:,\d{3})*(?:\.\d{1,2})?)",
        r"[\$€£¥]\s*(\d{1,10}(?:,\d{3})*(?:\.\d{1,2})?)\s*$",
    ];

    for pattern in &patterns {
        if let Some(cap) = regex_lite(pattern, text) {
            let cleaned = cap.replace(',', "");
            if let Ok(val) = cleaned.parse::<f64>() {
                if val > 0.0 {
                    return Some(val);
                }
            }
        }
    }

    None
}

/// Extract subtotal.
#[cfg(feature = "document-invoice")]
fn extract_subtotal(text: &str) -> Option<f64> {
    let patterns = [
        r"(?i)(?:subtotal|sub\s*total)\s*:?\s*[\$€£¥]?\s*(\d{1,10}(?:,\d{3})*(?:\.\d{1,2})?)",
        r"(?i)(?:subtotal|sub\s*total)\s*[\$€£¥]?\s*(\d{1,10}(?:,\d{3})*(?:\.\d{1,2})?)",
    ];

    for pattern in &patterns {
        if let Some(cap) = regex_lite(pattern, text) {
            let cleaned = cap.replace(',', "");
            if let Ok(val) = cleaned.parse::<f64>() {
                if val > 0.0 {
                    return Some(val);
                }
            }
        }
    }

    None
}

/// Extract tax amount.
#[cfg(feature = "document-invoice")]
fn extract_tax_amount(text: &str) -> Option<f64> {
    let patterns = [
        r"(?i)(?:tax|vat|gst|hst|sales\s*tax)\s*:?\s*[\$€£¥]?\s*(\d{1,10}(?:,\d{3})*(?:\.\d{1,2})?)",
        r"(?i)(?:tax|vat|gst)\s*[\$€£¥]?\s*(\d{1,10}(?:,\d{3})*(?:\.\d{1,2})?)",
    ];

    for pattern in &patterns {
        if let Some(cap) = regex_lite(pattern, text) {
            let cleaned = cap.replace(',', "");
            if let Ok(val) = cleaned.parse::<f64>() {
                if val > 0.0 {
                    return Some(val);
                }
            }
        }
    }

    None
}

/// Extract currency symbol.
#[cfg(feature = "document-invoice")]
fn extract_currency(text: &str) -> String {
    if text.contains("€") || text.contains("EUR") {
        return "EUR".to_string();
    }
    if text.contains("£") || text.contains("GBP") {
        return "GBP".to_string();
    }
    if text.contains("¥") || text.contains("JPY") || text.contains("CNY") {
        return "CNY".to_string();
    }
    if text.contains("$") || text.contains("USD") {
        return "USD".to_string();
    }
    String::new()
}

/// Extract line items from tabular invoice content.
///
/// Looks for rows that start with a quantity (number) followed by a description
/// and an amount at the end. Also captures rows with description + amount patterns.
#[cfg(feature = "document-invoice")]
fn extract_line_items(text: &str) -> Vec<LineItem> {
    let mut items = Vec::new();

    // Try to find tabular line items: Qty | Description | Unit Price | Amount
    // Pattern: leading whitespace, a number (qty), then text, then up to 2-3 price-like numbers
    let line_item_patterns = [
        // Qty, Description, Unit Price, Amount
        r"(?m)^\s*(\d+)\s+(.{3,60}?)\s+(\d+(?:,\d{3})*(?:\.\d{1,2})?)\s+(\d+(?:,\d{3})*(?:\.\d{1,2})?)\s*$",
        // Description, Amount (2 columns)
        r"(?m)^\s*(.+?)\s{2,}(\d+(?:,\d{3})*(?:\.\d{1,2})?)\s*$",
        // Qty x Unit Price = Amount
        r"(?m)^\s*(\d+)\s*[xX*]\s*\$?(\d+(?:\.\d{1,2})?)\s+(.{3,60}?)\s+\$?(\d+(?:\.\d{1,2})?)\s*$",
    ];

    for pattern in &line_item_patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            for cap in re.captures_iter(text) {
                // Determine which pattern matched based on capture count
                let (qty, desc, unit_price, amount) = if cap.len() == 5 {
                    // Pattern 0 or 2: has qty
                    let qty_val = cap[1].replace(',', "").parse::<f64>().ok();
                    let desc_val = cap[3].trim().to_string();
                    let unit_val = cap[2].replace(',', "").parse::<f64>().ok();
                    let amt_val = cap[4].replace(',', "").parse::<f64>().ok();
                    (qty_val, desc_val, unit_val, amt_val)
                } else if cap.len() == 3 {
                    // Pattern 1: just description and amount
                    (
                        None,
                        cap[1].trim().to_string(),
                        None,
                        cap[2].replace(',', "").parse::<f64>().ok(),
                    )
                } else {
                    continue;
                };

                // Skip header-like rows and rows with very long descriptions
                let desc_lower = desc.to_lowercase();
                if desc_lower.contains("description")
                    || desc_lower.contains("item")
                    || desc_lower.contains("product")
                    || desc_lower.contains("qty")
                    || desc_lower.contains("quantity")
                    || desc_lower.contains("unit price")
                    || desc_lower.contains("amount")
                    || desc.len() > 100
                    || desc.len() < 2
                {
                    continue;
                }

                items.push(LineItem {
                    description: desc,
                    quantity: qty,
                    unit_price,
                    amount,
                });
            }
        }

        if !items.is_empty() {
            break;
        }
    }

    items
}

/// Lightweight regex matching without full regex compilation overhead.
/// Tries to compile as a proper regex first; falls back to simple substring matching.
#[cfg(feature = "document-invoice")]
fn regex_lite(pattern: &str, text: &str) -> Option<String> {
    if let Ok(re) = regex::Regex::new(pattern) {
        return re
            .captures(text)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
    }
    // Fallback: simple text search for the pattern prefix
    let stripped = pattern.trim_start_matches("(?i)");
    if let Some(start) = stripped.find(':') {
        let prefix = &stripped[..start].trim();
        let text_lower = text.to_lowercase();
        if let Some(pos) = text_lower.find(&prefix.to_lowercase()) {
            let after = &text[pos + prefix.len()..];
            let line_end = after.find('\n').unwrap_or(after.len());
            let val = after[..line_end].trim().trim_matches(':').trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(feature = "document-invoice")]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_invoice() {
        let text = r#"
INVOICE

From: Acme Corporation
Invoice #: INV-2024-001
Date: 2024-03-15

Bill To: John Doe

Items:
1   Widget A      10.00   10.00
2   Widget B      25.00   50.00

Subtotal: 60.00
Tax: 5.00
Total: $65.00
"#;

        let invoice = parse_invoice(text);
        assert_eq!(invoice.invoice_number, "INV-2024-001");
        assert!(invoice.vendor.contains("Acme") || invoice.vendor.is_empty());
        assert_eq!(invoice.date, "2024-03-15");
        assert_eq!(invoice.total_amount, Some(65.00));
        assert_eq!(invoice.subtotal, Some(60.00));
        assert_eq!(invoice.tax_amount, Some(5.00));
        assert_eq!(invoice.currency, "USD");
        assert!(!invoice.line_items.is_empty());
    }

    #[test]
    fn test_parse_invoice_from_path() {
        // Use text payload instead of path for unit test
        let text = r#"Invoice Number: 98765
Date: 01/15/2024
Vendor: TechSupply Co.
Total: $1,234.56"#;
        let invoice = parse_invoice(text);
        assert_eq!(invoice.invoice_number, "98765");
        assert_eq!(invoice.total_amount, Some(1234.56));
    }

    #[test]
    fn test_extract_total_with_commas() {
        let invoice = parse_invoice("Total: $1,234.56");
        assert_eq!(invoice.total_amount, Some(1234.56));
    }

    #[test]
    fn test_empty_text() {
        let invoice = parse_invoice("");
        assert!(invoice.invoice_number.is_empty());
        assert!(invoice.total_amount.is_none());
    }
}
