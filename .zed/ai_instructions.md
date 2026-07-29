

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

---
## 2. MANDATORY BEHAVIOR
1. All tasks must be listed **completely, in detail, with no truncation**
2. Tasks must be executed **strictly in order** (1,2,3...)
3. Each task must be **fully completed** before proceeding to the next
4. **Validate all symbol pairs** before and after every edit: { } ( ) [ ] < >
5. Code must be **fully implemented and compilable**
6. Error handling must use Rust `Result`
7. WASM compilation: not currently targeted; this project compiles for native (macOS/Linux/Windows).
8. Follow project architecture and naming conventions
9. Check Cargo.toml features before generating code
10. Self-check for all forbidden patterns and fix before output

**Recommended practice:**
- Fully leverage Rust's type system and error handling
- Run `cargo check` locally after code generation to verify
- Immediately fix any unclosed symbols, placeholders, or fake implementations

---
## 3. SKILL SYSTEM

See `skills/README.md` for skill system usage.

---
## 4. ZED AGENT SERVER INTEGRATION

go-on is configured as a Zed Agent Server in `.zed/settings.json`. It communicates with Zed
via the Agent Communication Protocol (ACP) over stdio, providing 60+ tools for autonomous
code execution, file manipulation, web browsing, and more.

For detailed setup instructions, configuration options, troubleshooting, and protocol details,
see the **[go-on Zed Integration Guide](../docs/zed-integration.md)**.

### Key points
- The agent server is registered in `.zed/settings.json` under `agent_servers.go-on`.
- Use the AI panel (`Ctrl+Enter`) and select "go-on" from the agent dropdown.
- ACP stdio is the required protocol mode; HTTP (`-b`) is optional for the GUI.
- Environment variables for API keys are set in the `env` block of the settings.

---
**Purpose:**
This document applies to all Rust projects in the ZED editor, ensuring AI-generated code is safe, standardized, and maintainable. The skill system enables AI agents to discover and execute reusable capabilities.