

# ZED AI STRICT INSTRUCTIONS – RUST DEVELOPMENT

**This document defines strict requirements for AI code generation and automation in the ZED editor for all Rust development. All rules are mandatory.**

---
## 1. FORBIDDEN
- Do not use `todo!()`, `unimplemented!()`, or any variants
- Do not use empty code blocks `{ }`, empty functions, or empty implementations
- Do not use `simple_impl`, `stub`, `placeholder`, or fake logic
- Do not use `Ok(())` or `Ok(Default::default())` as placeholders
- Do not hide incomplete logic inside if/else/match/loop branches
- Do not truncate, merge, simplify, reorder, or skip tasks
- Do not perform bulk edits that break symbol pairs: { } ( ) [ ] < >
- Do not leave unclosed braces, parentheses, brackets, or angle brackets
- Do not guess crate features or functions that do not exist
- Do not delete or modify unrelated code
- Do not blindly rely on tool output; always manually validate syntax

**Common mistakes:**
```rust
fn foo() { todo!() }
fn bar() {}
let x = Ok(());
```

---
## 2. MANDATORY BEHAVIOR
1. All tasks must be listed **completely, in detail, with no truncation**
2. Tasks must be executed **strictly in order** (1,2,3...)
3. Each task must be **fully completed** before proceeding to the next
4. **Validate all symbol pairs** before and after every edit: { } ( ) [ ] < >
5. Code must be **fully implemented and compilable**
6. Error handling must use Rust `Result`
7. WASM compilation must be supported
8. Follow project architecture and naming conventions
9. Check Cargo.toml features before generating code
10. Self-check for all forbidden patterns and fix before output

**Recommended practice:**
- Fully leverage Rust's type system and error handling
- Run `cargo check` locally after code generation to verify
- Immediately fix any unclosed symbols, placeholders, or fake implementations

---
**Purpose:**
This document applies to all Rust projects in the ZED editor, ensuring AI-generated code is safe, standardized, and maintainable.