//! e2e — End-to-end integration test suite for go-on.
//!
//! This file is the Cargo-discoverable entry point that pulls in the e2e
//! test modules from `tests/e2e/`. Each sub-module covers a complete
//! end-to-end workflow across multiple go-on subsystems.
//!
//! Tests use in-memory type construction and structural validation.
//! They do NOT require external infrastructure — `#[ignore]` is not needed.

mod e2e;
