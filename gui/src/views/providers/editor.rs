//! Provider editor sub-module.
//!
//! Handles the provider editing form: add/update/delete provider keys, models, and labels.
//! Extracted from the monolithic providers.rs for better organization.
//!
//! TODO (BLUE65): Migrate the editing logic from mod.rs into this module.

use crate::i18n::I18n;

/// Check whether a provider name requires a dual-auth secret key (e.g. wenxin, qianfan).
#[allow(dead_code)]
pub fn provider_requires_secret(provider: &str) -> bool {
    matches!(provider.to_lowercase().as_str(), "wenxin" | "qianfan")
}

/// Format a localized provider label for display.
#[allow(dead_code)]
pub fn provider_label(i18n: &I18n, provider: &str) -> String {
    let key = format!("provider.{}", provider.to_lowercase());
    let label = i18n.t(&key);
    if label.as_ref() == key {
        provider.to_string()
    } else {
        label.into_owned()
    }
}
