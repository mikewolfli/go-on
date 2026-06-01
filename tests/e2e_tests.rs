//! e2e — End-to-end integration test suite for go-on.
//!
//! This file is the Cargo-discoverable entry point that pulls in the e2e
//! test modules from `tests/e2e/`. Each sub-module covers a complete
//! end-to-end workflow across multiple go-on subsystems.
//!
//! All tests are annotated with `#[ignore]` because they require actual
//! infrastructure (services, databases, network peers) to run.

mod e2e;
