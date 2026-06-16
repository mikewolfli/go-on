//! Agent vendor categorization and organization
//!
//! This module provides organized access to agents by vendor/category.

/// Vendor categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VendorCategory {
    /// OpenAI and compatible vendors
    OpenAIFamily,
    /// Chinese AI vendors
    ChineseVendors,
    /// Other vendors
    OtherVendors,
}
