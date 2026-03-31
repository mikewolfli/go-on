

# Project Code Review Mandatory Standards

**This document defines the mandatory standards for code review, automation, and AI code generation in this project. All developers and AI assistants must strictly follow these rules to ensure code quality and consistency.**



## 1. Empty Implementations and Placeholders
- **Empty functions, methods, or branches containing only `TODO` / `FIXME` are strictly forbidden.**
- If an empty implementation is detected, prompt the developer to complete the logic; if enough context exists, generate a reasonable implementation directly.
**Example:**
```rust
// Incorrect
fn foo() { /* TODO: implement */ }
// Correct
fn foo() { println!("Hello"); }
```



## 2. Infinite Loops and Logical Errors
- Check all `for`/`while` loops to ensure there is a reachable exit condition.
- Recursive functions must have a termination condition.
- Review conditional branches to avoid always-true/false or contradictory logic.
**Example:**
```rust
// Incorrect
while true { /* ... */ }
// Correct
while !done { /* ... */ }
```



## 3. Cross‑Calls and Circular Dependencies
- Check call relationships between modules, classes, and functions; flag any risk of circular dependencies.
- If A calls B and B calls back to A, analyze and suggest refactoring.
**Recommended practice:**
Prefer decoupling with interfaces/traits, or split modules if necessary.



## 4. Unused Code
- Mark all unused functions, variables, classes, or imports.
- Recommend deleting unused code or require justification for keeping it.
**Example:**
```rust
// Incorrect
let unused = 42;
// Correct
// Remove unused variable, or add a comment explaining its purpose
```



## 5. Function Completeness and Limitations
- Strictly check function parameters, logic, and return values.
- **Edge cases:** Handle null, out-of-bounds, and unexpected types.
- **Error handling:** Add error handling for potential exceptions (e.g., file/network errors).
- **Feature completeness:** If a function name is `saveUser` but does not log the action, complete the implementation.
- **Limitations:** If only a specific format is supported, point it out and suggest generalization.
**Recommended practice:**
Prefer using `Result`/`Option`, and validate all input types and ranges.



# GitHub Copilot Component Instructions
# STRICTLY ENFORCED FOR ALL CODE EDITS, GENERATION, AND REFACTORING



## FORBIDDEN ACTIONS
- **Strictly forbidden** to delete code without validating all symbol pairs: `{}`, `()`, `[]`, `<>`
- Do not leave unclosed braces, parentheses, brackets, or angle brackets
- Do not perform bulk deletions that break syntax structure
- Do not use auto-fix tools without checking symbol integrity
- Do not generate partial, placeholder, or incomplete implementations
- Do not use: `todo!()`, `unimplemented!()`, empty blocks `{}`, `simple_impl`, `stub`, `placeholder`
- Do not guess crate features or functions that do not exist
- Do not modify or delete unrelated code



## MANDATORY BEHAVIOR
1. **Validate all symbol pairs before and after every edit.**
2. Ensure all code blocks are closed and structurally complete.
3. Only generate complete, compilable Rust code.
4. Check for syntax errors before outputting any change.
5. Unless explicitly instructed, preserve the original structure and logic.
6. When modifying functions or code blocks, maintain full structure.
7. If unbalanced symbols or syntax issues are found, **fix them first**.

---
**Recommended practice:**
- Run `cargo check` locally before committing code.
- Prioritize edge cases and error handling during review.
- Fully leverage Rust's type system and error handling mechanisms.