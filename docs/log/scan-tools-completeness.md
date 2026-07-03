# Tool System Completeness Scan

**Date**: 2026-07-03
**Scope**: Full audit of the tool registration, descriptor, pipeline, and implementation layers.

---

## 1. Tool Registration (`src/orchestration/tool/`)

### 1.1 Registry Implementation
**File**: `src/orchestration/tool/mod.rs`

The `ToolRegistry::new()` method (lines 184–1343) registers tools in the following order. Every registration has a `ToolCapabilityProfile` with capability, risk level, timeout, retry policy, and fallback chain.

#### Core Tools (always compiled, no feature gate)
| Tool struct | Register name (from `fn name()`) | Lines |
|---|---|---|
| `ReadFileTool` | `"read_file"` | 186–198 |
| `WriteFileTool` | `"write_file"` | 199–211 |
| `SearchFilesTool` | `"search_files"` | 212–224 |
| `ApplyPatchTool` | `"apply_patch"` | 225–237 |
| `RunTestsTool` | `"run_tests"` | 238–250 |
| `InspectGitDiffTool` | `"inspect_git_diff"` | 251–263 |
| `ShellExecTool` | `"shell_exec"` | 265–277 |
| `HttpRequestTool` | `"http_request"` | 278–290 |
| `GrepTool` | `"grep"` | 291–303 |
| `FindFilesTool` | `"find_files"` | 304–316 |
| `GitTool` | `"git"` | 317–329 |
| `ListDirectoryTool` | `"list_directory"` | 330–342 |
| `FileMoveTool` | `"file_move"` | 369–381 |
| `FileDeleteTool` | `"file_delete"` | 382–394 |
| `CargoCheckTool` | `"cargo_check"` | 343–355 |
| `CargoTestTool` | `"cargo_test"` | 356–368 |
| `CodeIndexTool` | `"code_index_search"` | 1314–1326 |
| `ArchiveInspectTool` | `"archive_inspect"` | 443–455 |
| `ArchiveExtractTool` | `"archive_extract"` | 456–468 |
| `CompressTool` | `"compress"` | 779–805 |
| `DecompressTool` | `"decompress"` | 792–804 |
| `RssReadTool` | `"rss_read"` | 1045–1057 |
| `JsonlReadTool` | `"jsonl_read"` | 1124–1136 |
| `JsonlWriteTool` | `"jsonl_write"` | 1139–1151 |
| `DnsLookupTool` | `"dns_lookup"` | 1154–1166 |
| `PingTool` | `"ping"` | 1167–1179 |
| `PortScanTool` | `"port_scan"` | 1180–1192 |
| `DateTimeTool` | `"date_time"` | 1195–1207 |
| `DiagnosticsTool` | `"diagnostics"` | 1210–1222 |
| `EnvironmentInfoTool` | `"environment_info"` | 1225–1237 |
| `SkillListTool` | `"skill_list"` | 1252–1264 |
| `SkillExecuteTool` | `"skill_execute"` | 1267–1279 |
| `SkillCreateTool` | `"skill_create"` | 1282–1294 |
| `SkillReloadTool` | `"skill_reload"` | 1297–1309 |

#### Feature-Gated Tools
| Feature flag | Tool struct | Registration name | Lines in mod.rs |
|---|---|---|---|
| `document-excel` | `ReadExcelTool` | `"read_excel"` | 397–410 |
| `document-ppt` | `ReadPptTool` | `"read_ppt"` | 412–425 |
| `document-excel-write` | `WriteExcelTool` | `"write_excel"` | 427–440 |
| `document-pdf` | `ReadPdfTool` | `"read_pdf"` | 471–484 |
| `document-pdf` | `PdfMergeTool` | `"pdf_merge"` | 486–513 |
| `document-pdf` | `PdfSplitTool` | `"pdf_split"` | 500–513 |
| `document-email` | `EmailParseTool` | `"email_parse"` | 515–528 |
| `document-docx` | `ReadDocxTool` | `"read_docx"` | 531–544 |
| `document-docx` | `WriteDocxTool` | `"write_docx"` | 545–558 |
| `document-ppt` | `WritePptTool` | `"write_ppt"` | 559–572 |
| `backend-sqlite` | `SqliteQueryTool` | `"sqlite_query"` | 575–588 |
| `document-html` | `WebScrapeTool` | `"web_scrape"` | 591–604 |
| `data-export` | `CsvReadTool` | `"csv_read"` | 607–620 |
| `data-export` | `CsvWriteTool` | `"csv_write"` | 621–634 |
| `data-export` | `CsvAnalyzeTool` | `"csv_analyze"` | 635–648 |
| `data-export` | `CsvTransformTool` | `"csv_transform"` | 649–662 |
| `data-export` | `TomlReadTool` | `"toml_read"` | 663–676 |
| `data-export` | `TomlWriteTool` | `"toml_write"` | 677–690 |
| `data-export` | `YamlReadTool` | `"yaml_read"` | 691–704 |
| `data-export` | `YamlWriteTool` | `"yaml_write"` | 705–718 |
| `image-processing` | `ImageResizeTool` | `"image_resize"` | 721–734 |
| `image-processing` | `ImageConvertTool` | `"image_convert"` | 735–748 |
| `image-processing` | `ImageAnalyzeTool` | `"image_analyze"` | 749–762 |
| `image-processing` | `ImageGenerateTool` | `"image_generate"` | 763–776 |
| `cad-dxf` | `DxfReadTool` | `"dxf_read"` | 807–820 |
| `drawing-svg` | `SvgReadTool` | `"svg_read"` | 823–836 |
| `drawing-svg` | `SvgGenerateTool` | `"svg_generate"` | 837–850 |
| `drawing-svg` | `SvgExportTool` | `"svg_export"` | 933–946 |
| `cad-stl` (+not model-3d) | `StlReadTool` | `"stl_read"` | 853–866 |
| `cad-obj` | `ObjReadTool` | `"obj_read"` | 869–882 |
| `cad-step` | `StepReadTool` | `"step_read"` | 885–898 |
| `cad-geo` | `GeoUtilTool` | `"geo_util"` | 901–914 |
| `cad-utils` | `CadConvertTool` | `"cad_convert"` | 917–930 |
| `cad-gltf` | `GltfReadTool` | `"gltf_read"` | 949–962 |
| `cad-iges` | `IgesReadTool` | `"iges_read"` | 965–978 |
| `cad-ply` | `PlyReadTool` | `"ply_read"` | 981–994 |
| `cad-stl` | `StlGenerateTool` | `"stl_generate"` | 997–1010 |
| `document-invoice` | `InvoiceParseTool` | `"invoice_parse"` | 1013–1026 |
| `barcode-tools` | `QrCodeTool` | `"qrcode_generate"` | 1029–1042 |
| `model-3d` | `StlReadTool` | `"stl_read"` | 1060–1073 |
| `cam-gcode` | `GcodeReadTool` | `"gcode_read"` | 1076–1089 |
| `gis-gpx` | `GpxReadTool` | `"gpx_read"` | 1092–1105 |
| `model-3d-extra` | `ObjModelReadTool` | `"obj_model_read"` | 1108–1121 |
| `game-*` (any) | `register_game_tools()` | various | 1240–1249 |

### ✅ Finding: All tool implementations are registered — no dangling references
Every struct that implements `Tool` and is publicly re-exported from `extended/mod.rs` has a corresponding `registry.register_with_profile(...)` call. There are no tools referenced in the registry that lack an implementation file.

### ⚠️ Finding: Alias registrations (lines 1332–1341)
These aliases map legacy names to canonical tools. The aliases are correctly handled by `get()` and `get_arc()` (lines 1373–1405). However, they have **no profiles** — `profile()` (line 1429) resolves aliases to canonical names, so this works at runtime.

| Alias | Canonical | Comment |
|---|---|---|
| `create_directory` | `write_file` | Different params — `create_directory` would need only `path`, but `write_file` also needs `content` |
| `delete_path` | `file_delete` | Reasonable mapping |
| `move_path` | `file_move` | Reasonable mapping |
| `copy_path` | `write_file` | Same issue as `create_directory` — different semantics |
| `execute_command` | `shell_exec` | Reasonable mapping |
| `terminal` | `shell_exec` | Reasonable mapping |
| `bash` | `shell_exec` | Reasonable mapping |
| `find_path` | `find_files` | Reasonable mapping |
| `semantic_search` | `code_index_search` | Reasonable mapping |

**Issue**: `create_directory → write_file` and `copy_path → write_file` are semantically misleading. These aliases will pass the payload directly to `write_file`, but neither `create_directory` nor `copy_path` would provide `"content"` — causing failures. These should have their own dedicated implementations or be remapped to proper tools.

---

## 2. Extended Tools (`src/orchestration/tool/extended/`)

### 2.1 Module Structure
**File**: `src/orchestration/tool/extended/mod.rs`

All 43 `.rs` files in the `extended/` directory are properly referenced in the module declarations and re-exported. The feature gates in `mod.rs` exactly match those in the registry's `new()` method.

### 2.2 Implementation Completeness

I examined every extended tool implementation. **None are stubs**. Every `run()` method contains a full implementation. Here are the verified tools and their implementation status:

#### Fully implemented tools (verified by reading source):
- `filesystem.rs`: `ListDirectoryTool` (lines 14–82), `FileMoveTool` (86–139), `FileDeleteTool` (143–195) — all with audit logs, PUA reports, and proper path sanitization
- `search.rs`: `GrepTool` (14–75), `FindFilesTool` (136–176) — recursive file scanning with glob support
- `shell.rs`: `ShellExecTool` (12–252) — full timeout handling with GNU timeout + Rust thread-based fallback
- `http.rs`: `HttpRequestTool` (25–168) — supports GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS, custom headers, bearer auth, query params
- `network.rs`: `DnsLookupTool` (15–86), `PingTool` (90–179), `PortScanTool` (183–289) — all with proper error handling
- `git.rs`: `GitTool` (12–89) — limited to 5 allowed subcommands (status, log, diff, show, stash)
- `cargo.rs`: `CargoCheckTool` (12–98), `CargoTestTool` (102–161) — structured JSON parsing for cargo output
- `compress.rs`: `CompressTool` (17–125), `DecompressTool` (129–220) — gzip via flate2
- `time.rs`: `DateTimeTool` (38–300) — 4 operations: now, format, diff, parse (stdlib-only date math)
- `diagnostics.rs`: `DiagnosticsTool` (16–86) — runs `cargo check --message-format=short`
- `environment_info.rs`: `EnvironmentInfoTool` (16–98) — OS info, project structure, tooling checks
- `jsonl.rs`: `JsonlReadTool` (13–74), `JsonlWriteTool` (76–126)
- `rss.rs`: `RssReadTool` (14–132) — inline XML parser for RSS 2.0 and Atom
- `archive.rs`: `ArchiveInspectTool` (L22–120), `ArchiveExtractTool` (L126–210) — zip/tar.gz/tar.bz2
- `code_index.rs`: `CodeIndexTool` (L867–984) — language-aware symbol extraction for Rust/Python/Go/TS/Java/C++
- `barcode.rs`: `QrCodeTool` (L17–57) — pure-Rust QR code generation to SVG
- `stl.rs`: `StlReadTool`, `StlGenerateTool` — ASCII/binary STL parsing and generation
- `obj.rs`: `ObjReadTool` — OBJ parsing with bounding box computation
- `web.rs`: `WebScrapeTool` — CSS selector based HTML scraping via `scraper`
- `data_serialization.rs`: CSV/TOML/YAML read/write tools — all with full implementations
- `image.rs`: Image resize/convert/analyze/generate — full implementations via `image` crate
- `svg.rs`: SVG read/generate/export — full implementations via `svg` crate

### 2.3 Tool Description Override Completeness

The `Tool` trait has a default `description()` method returning `""` (mod.rs line 116). Only tools with explicit `input_schema()` overrides in `native.rs` get meaningful descriptions in the provider schema.

**Tools that DO NOT override `description()`** (return `""`):
- `ArchiveInspectTool`, `ArchiveExtractTool`
- `RssReadTool`
- `JsonlReadTool`, `JsonlWriteTool`
- `ObjReadTool`, `StlReadTool`, `StlGenerateTool`
- `StepReadTool`, `PlyReadTool`, `IgesReadTool`, `GltfReadTool`
- `GpxReadTool`, `GcodeReadTool`
- `InvoiceParseTool`, `EmailParseTool`
- `SqliteQueryTool`
- `GeoUtilTool`, `CadConvertTool`
- `SvgReadTool`, `SvgGenerateTool`, `SvgExportTool`
- `DxfReadTool`
- `ObjModelReadTool`

These tools rely on the generic fallback in `native.rs` (line 648): `format!("Execute the {} tool", tool_name)` — which is functional but not descriptive.

### ⚠️ Finding: Duplicate `StlReadTool` implementations
**File**: `src/orchestration/tool/extended/mod.rs`, lines 152–157

The re-exports are complex:
- `#[cfg(all(feature = "cad-stl", feature = "model-3d"))]` → `stl::StlGenerateTool` only
- `#[cfg(all(feature = "cad-stl", not(feature = "model-3d")))]` → `stl::{StlGenerateTool, StlReadTool}`
- `#[cfg(feature = "model-3d")]` → `stl_tool::StlReadTool`

This means `StlReadTool` has **two separate implementations**: one in `stl.rs` with native STL parsing, and one in `stl_tool.rs` using the `stl_io` crate. When both `cad-stl` and `model-3d` are enabled, only the `stl_tool.rs` version is used. This is intentional (domain-specific vs general 3D model tooling) but creates a maintenance burden.

---

## 3. Tool Descriptor Layer (`src/shared/tool_descriptors.rs`)

### 3.1 `tool_descriptor()` Coverage
**Lines**: 16–443

This function returns structured `McpTool` descriptors for LLM-facing schemas. It has 30 explicit match arms and a fallthrough `other => "Registered MCP tool"` at line 438.

**Explicitly covered** (30 tools):
`read_file`, `write_file`, `search_files`, `apply_patch`, `run_tests`, `inspect_git_diff`, `workflow_execute`, `workflow_ask`, `workflow_generate`, `skill-creator`, `github_search_skills`, `import_skill`, `shell_exec`, `http_request`, `grep`, `find_files`, `git`, `list_directory`, `file_move`, `file_delete`, `cargo_check`, `cargo_test`, `compress`, `decompress`, `date_time`, `dns_lookup`, `ping`, `port_scan`, `skill_execute`, `skill_create`, `skill_reload`, `skill_list`

**Fall through to generic** (all others, ~40+ tools):
Tools like `archive_inspect`, `jsonl_read`, `csv_read`, `image_resize`, `read_docx`, `sqlite_query`, `rss_read`, etc. get the generic `"Registered MCP tool"` description with an empty `{}` input schema. This is **functional but poor UX** — LLMs won't know what these tools do or how to call them.

### 3.2 `KNOWN_TOOLS` Test List
**Lines**: 618–654

The test list is **incomplete**. It only covers 33 tools but the registry registers ~60+ tools (depending on feature flags). Missing from `KNOWN_TOOLS`:
- `skill_create`, `skill_reload`
- `archive_inspect`, `archive_extract`
- `jsonl_read`, `jsonl_write`
- `code_index_search`
- `csv_analyze`, `csv_transform`, `csv_read`, `csv_write`
- `toml_read`, `toml_write`, `yaml_read`, `yaml_write`
- `image_resize`, `image_convert`, `image_analyze`, `image_generate`
- All CAD tools: `dxf_read`, `stl_read`, `obj_read`, `step_read`, `ply_read`, `iges_read`, `gltf_read`
- All SVG tools: `svg_read`, `svg_generate`, `svg_export`
- All document tools: `read_docx`, `read_excel`, `read_pdf`, `read_ppt`, `write_docx`, `write_excel`, `write_ppt`
- `pdf_merge`, `pdf_split`
- `rss_read`, `web_scrape`
- `sqlite_query`, `qrcode_generate`
- `email_parse`, `invoice_parse`
- `gcode_read`, `gpx_read`, `geo_util`, `cad_convert`
- `obj_model_read`, `stl_generate`

---

## 4. Native Tool Bridge (`src/orchestration/tool/native.rs`)

### 4.1 Schema Coverage
**Lines**: 41–667

The `build_tool_schema()` function has explicit match arms for 29 + 30 game tools. The rest fall through to a dynamic schema built from the Tool trait's `description()` and `input_schema()` methods.

**Explicitly covered**: `read_file`, `write_file`, `search_files`, `apply_patch`, `run_tests`, `inspect_git_diff`, `shell_exec`, `http_request`, `grep`, `find_files`, `git`, `list_directory`, `cargo_check`, `cargo_test`, `file_move`, `file_delete`, plus 30 game tool schemas (lines 413–639).

### ⚠️ Finding: Triple-maintenance problem
Tool schemas are maintained in **three separate locations**:
1. `tool_descriptor()` in `shared/tool_descriptors.rs` (used by ACP/MCP handlers)
2. `build_tool_schema()` in `native.rs` (used by provider function calling)
3. The Tool trait's `description()` and `input_schema()` methods (the canonical source)

When adding a new tool, all three must be updated for full coverage, but there's no enforcement mechanism.

---

## 5. Tool Pipeline (`src/orchestration/tool/pipeline.rs`)

### 5.1 Governance Action Mapping
**Lines**: 132–237

The `pipeline_tool_to_action()` function maps tool names to governance actions (`read`, `search`, `write`, `shell`, `network`). The mapping is generally comprehensive but has issues:

### ⚠️ Finding: Pipeline references tools not in the registry
These tool names appear in the pipeline governance map but **do not exist as registered tools or aliases**:

| Tool name | Pipeline line | Status |
|---|---|---|
| `echo_skill` | 137 | Not registered |
| `builtin.echo` | 137 | Not registered |
| `goon_skill_version_list` | 137 | Not registered |
| `skill-finder` | 138 | Not registered |
| `chat.execute` | 138 | Not registered |
| `acp_trace_get` | 139 | Not registered |
| `acp_debug_panel_get` | 139 | Not registered |
| `goon_workflow_run_list` | 140 | Not registered |
| `goon_workflow_run_get` | 140 | Not registered |
| `goon_metrics_window_query` | 141 | Not registered |
| `goon_metrics_errors_summary` | 141 | Not registered |
| `goon_provider_capabilities` | 142 | Not registered |
| `prompts_list` | 142 | Not registered |
| `prompts_get` | 142 | Not registered |
| `goon_skill_update` | 180 | Not registered |
| `goon_skill_version_rollback` | 181 | Not registered |
| `goon_workflow_run_cancel` | 183 | Not registered |
| `goon_workflow_run_pause` | 184 | Not registered |
| `goon_workflow_run_resume` | 185 | Not registered |
| `goon_provider_test_completion` | 223 | Not registered |
| `goon_provider_test_connection` | 224 | Not registered |
| `game_auto_grind` | 198 | Not registered |
| `game_keyboard_input` | 199 | Not registered |
| `game_mouse_input` | 200 | Not registered |
| `game_state_modify` | 201 | Not registered |
| `game_monitor` | 221 | Not registered |
| `game_online_status` | 222 | Not registered |
| `game_replay_recorder` | 177 | Not registered |
| `game_save_manager` | 178 | Not registered |
| `game_screen_capture` | 179 | Not registered |
| `game_mod_install` | 176 | Not registered |
| `import_skill` | 144 | Registered as alias? Actually, `import_skill` is NOT in the registry either! |

**Wait** — `import_skill` has a descriptor in `tool_descriptors.rs` but I didn't see it registered as a Tool implementation. Let me check... Searching for `import_skill` in the registry... It's not there! The descriptor exists but the tool implementation doesn't.

Actually, looking more carefully at the descriptor `tool_descriptors.rs` lines 125–177, `github_search_skills` and `import_skill` have descriptors but they're NOT registered in `ToolRegistry::new()`. This means they're dead entries in the descriptor.

**These ~30+ tool names will log warnings at runtime** (pipeline.rs line 228): `"pipeline_tool_to_action: unknown tool '{name}', defaulting to 'read' action — audit needed"`. This flooding of warnings is a noise problem.

### 5.2 Budget Enforcement
**Lines**: 294–304

The pipeline enforces a hard cap of 256 tool calls per pipeline execution. This is reasonable.

---

## 6. Missing Tools — Gaps and Recommendations

### 6.1 Code Analysis Tools

| Missing tool | Rationale |
|---|---|
| `lsp_request` | Send LSP requests for real-time diagnostics, completions, hover info |
| `ast_parse` | Parse source code into AST for structural analysis |
| `format_code` | Run code formatter (rustfmt, prettier, etc.) |

### 6.2 File System Tools

| Missing tool | Rationale |
|---|---|
| `create_directory` | Currently aliased to `write_file` (semantically broken) — needs its own implementation |
| `copy_path` | Currently aliased to `write_file` (semantically broken) — needs its own implementation |
| `symlink` | Create symbolic links |
| `chmod` / `chown` | Change file permissions/ownership |
| `file_info` | Get detailed file metadata without reading content |

### 6.3 Git Integration Tools

| Missing tool | Rationale |
|---|---|
| `git_add` | Stage changes (current `GitTool` only allows read-only commands) |
| `git_commit` | Commit staged changes |
| `git_push` / `git_pull` | Sync with remote |
| `git_reset` / `git_checkout` | Reset/discard changes |
| `git_branch` | Manage branches |

### 6.4 Data Processing Tools

| Missing tool | Rationale |
|---|---|
| `json_format` | Validate/format/prettify JSON |
| `json_transform` | Transform JSON using jq-like expressions |
| `xml_parse` | Parse/extract from XML documents |
| `regex_test` | Test regular expressions against sample text |
| `hash` / `checksum` | Compute file hashes (MD5, SHA256) |
| `base64` | Encode/decode base64 |

### 6.5 Network Tools

| Missing tool | Rationale |
|---|---|
| `whois` | Domain WHOIS lookup |
| `cert_info` | Check SSL/TLS certificate info |
| `ip_geo` | IP address geolocation |

### 6.6 Developer Productivity Tools

| Missing tool | Rationale |
|---|---|
| `edit_file` | Targeted line-based file editing (apply changes to specific lines without full patches) |
| `diff_files` | Diff two arbitrary files (non-git) |
| `count_lines` | Count lines of code with language detection |
| `todo_scan` | Scan for TODO/FIXME/HACK comments |
| `dependency_graph` | Analyze dependency relationships (Cargo.toml, package.json, etc.) |
| `env_validate` | Validate required environment variables and tools are available |

---

## 7. Summary of All Issues Found

### Issue A: Semantically broken aliases (CRITICAL)
- **File**: `src/orchestration/tool/mod.rs`, lines 1332, 1335
- `create_directory → write_file` and `copy_path → write_file` will fail at runtime because the input payload won't contain a `"content"` field.
- **Recommendation**: Create dedicated `CreateDirectoryTool` and `CopyPathTool` implementations, or alias them to tools that accept the same argument shape.

### Issue B: Dead descriptor entries (MEDIUM)
- **File**: `src/shared/tool_descriptors.rs`, lines 125–177
- `github_search_skills` and `import_skill` have descriptors but no implementation in `ToolRegistry::new()`.
- **Recommendation**: Either implement these tools or remove their descriptors.

### Issue C: Pipeline references tools that don't exist (MEDIUM)
- **File**: `src/orchestration/tool/pipeline.rs`, lines 132–237
- ~30+ tool names are listed in `pipeline_tool_to_action()` but have no implementation. They'll log warnings at runtime.
- **Recommendation**: Audit and either implement, register as aliases, or remove from the pipeline mapping.

### Issue D: Triple-maintenance for tool schemas (MEDIUM)
- **Files**: `shared/tool_descriptors.rs`, `native.rs`, and individual tool `description()/input_schema()` methods
- Adding a tool requires updates in three places with no automated enforcement.
- **Recommendation**: Derive all LLM-facing schemas from a single source (e.g., the Tool trait methods) and remove the hardcoded match arms in `tool_descriptor()` and `build_tool_schema()`.

### Issue E: Incomplete KNOWN_TOOLS test list (LOW)
- **File**: `src/shared/tool_descriptors.rs`, lines 618–654
- Only 33 of ~60+ tools are in the test coverage list.
- **Recommendation**: Update `KNOWN_TOOLS` to include all registered (non-feature-gated) tools at minimum. Consider auto-generating this list from the registry.

### Issue F: Many tools lack `description()` overrides (LOW)
- ~20+ extended tools don't override `description()` on their `Tool` trait impl, returning `""`.
- **Recommendation**: Add meaningful `fn description()` to every tool that doesn't have one.

### Issue G: `StlReadTool` dual implementation (LOW)
- **File**: `src/orchestration/tool/extended/mod.rs`, lines 152–157
- Two implementations in `stl.rs` and `stl_tool.rs`, selected by feature flags.
- **Recommendation**: Consider consolidating or documenting the rationale for two implementations more clearly.

### Issue H: Missing critical developer tools (LOW)
- No `edit_file` (line-based editing), no `git_add`/`git_commit`/`git_push`, no `create_directory`/`copy_path` implementations.
- **Recommendation**: Prioritize `edit_file` and git write tools, as they are essential for an AI coding agent.
