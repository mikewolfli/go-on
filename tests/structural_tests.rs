//! structural — Structural validation test suite for go-on.
//!
//! This file is the Cargo-discoverable entry point that pulls in the
//! structural test modules from `tests/structural/`. Each sub-module covers
//! invariants across multiple go-on subsystems.
//!
//! Tests use in-memory type construction and structural validation.
//! They do NOT require external infrastructure — `#[ignore]` is not needed.

mod structural;
