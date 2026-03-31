
# TRAE AI PROJECT – RUST CODE MANDATORY RULES

## 🔴 Forbidden Patterns (Zero Tolerance)
- No `todo!()`, `unimplemented!()`, empty blocks `{}`, or any placeholder/fake implementations.
- No `simple_impl`, `stub_impl`, `dummy`, or similar patterns.
- No placeholder returns like `Ok(())` or `Ok(Default::default())`.
- No incomplete logic hidden in any branch or loop.
- No repeated retries for the same error (e.g., crate feature issues).
- Never guess or use functions from disabled crates or features.
- Do not delete or modify unrelated code.
- Never leave unclosed `{}`, `()`, `[]`, or `<>` after edits.
- No tool usage that skips syntax validation or symbol pairing checks.
- No partial deletions that break code structure.
- No blind auto-fixes—always review and validate syntax.
- No simplifying code for tests when a method/struct/module is missing; instead, fully implement the missing method/struct/module.

## 🟢 Mandatory Workflow & Validation
1. **Full Implementation**: All code paths, branches, and match arms must be fully implemented—no placeholders at any level.
2. **Compilation**: Code must compile as-is, without manual fixes.
3. **Error Handling**: Use proper `Result`-based error handling; avoid panics.
4. **WASM Compatibility**: Ensure code compiles to WASM.
5. **Architecture Compliance**: Follow project structure, types, and naming conventions.
6. **Cargo Feature Check**:
  - Before coding, verify crate existence and enabled features in `Cargo.toml`.
  - If a feature or function is missing, output the exact `Cargo.toml` fix first, then generate code.
  - If unavailable, explain clearly once—do not retry or guess.
7. **Self-Check**: Scan for forbidden patterns before output; fix all issues.
8. **Output Only**: Generate real, complete, production-ready Rust code.
9. **Symbol Pair Validation**: Always ensure `{}`, `()`, `[]`, `<>` are balanced after any edit.
10. **Tool Usage**: Use tools only after manual syntax review; never skip symbol checks.
11. **Automated Checks**: Integrate CI and linting to enforce these rules automatically.
12. **Code Review**: Require peer review for all merges, focusing on these standards.

## 📝 Task Generation & Execution (For TRAE)
- List all tasks in full detail, preserving original order.
- Execute tasks strictly one by one, without skipping or merging.
- Every task must be visible, numbered, and fully executed.
- Use clear, concise English comments in Rust style.
- New modules must be in separate files and imported properly.
- New functions, structs, and modules must fit existing structure and naming conventions.
- Fix all new errors and warnings immediately.
- Never use stubs or placeholders to pass tests.
- Before creating any function, struct, enum, trait, or impl, check if it already exists—reuse if possible.
- Avoid heap pollution by fixing current errors and warnings.
- new code must with english comments and new solutions of errors/warnings/issues/bugs/optimizations are several, must use the best one, not the simplest one.

## 🚫 Duplication Rules
- Never create duplicate functions, structs, enums, traits, or impls.
- Always check for existing items before adding new ones.
- Only create new items if they do not already exist.
- If unsure, search and verify first.

---

**Best Practice Suggestions:**
- Use automated linting and CI checks to enforce these rules.
- Require code reviews focused on these standards.
- Document any exceptions in a separate section for clarity.
- Encourage continuous improvement—review and update these rules as the project evolves.