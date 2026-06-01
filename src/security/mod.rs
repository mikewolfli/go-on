#![allow(dead_code)]

//! Security Module (GAP-B52)
//!
//! Provides request signing, mTLS configuration, prompt injection detection,
//! secret rotation, audit integrity with hash chains, and content safety
//! checking for the go-on runtime.

pub mod audit_integrity;
pub mod content_safety;
pub mod mtls;
pub mod prompt_injection;
pub mod request_signing;
pub mod secret_rotation;
pub mod security_advisor;
pub mod vulnerability_scan;
