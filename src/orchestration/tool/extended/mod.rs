//! Extended built-in tools for go-on
//!
//! Additional tool implementations beyond the original 6, providing
//! shell execution, HTTP requests, file operations, search, cargo integration,
//! image processing, data serialization, and compression utilities.

pub mod archive;
#[cfg(feature = "barcode-tools")]
pub mod barcode;
#[cfg(feature = "cad-utils")]
pub mod cad;
pub mod cargo;
pub mod compress;
#[cfg(feature = "data-export")]
pub mod csv_utils;
#[cfg(feature = "data-export")]
pub mod data_serialization;
#[cfg(feature = "document-docx")]
pub mod docx;
#[cfg(feature = "cad-dxf")]
pub mod dxf_tool;
#[cfg(feature = "document-email")]
pub mod email;
pub mod filesystem;
#[cfg(feature = "cad-geo")]
pub mod geo;
pub mod git;
#[cfg(feature = "cad-gltf")]
pub mod gltf;
pub mod http;
#[cfg(feature = "cad-iges")]
pub mod iges;
#[cfg(feature = "image-processing")]
pub mod image;
#[cfg(feature = "document-invoice")]
pub mod invoice;
#[cfg(feature = "cad-obj")]
pub mod obj;
pub mod office;
#[cfg(feature = "document-pdf")]
pub mod pdf;
#[cfg(feature = "cad-ply")]
pub mod ply;
pub mod search;
pub mod shell;
#[cfg(feature = "backend-sqlite")]
pub mod sqlite;
#[cfg(feature = "cad-step")]
pub mod step;
#[cfg(feature = "cad-stl")]
pub mod stl;
#[cfg(feature = "drawing-svg")]
pub mod svg;
#[cfg(feature = "document-html")]
pub mod web;

pub use archive::{ArchiveExtractTool, ArchiveInspectTool};
#[cfg(feature = "barcode-tools")]
pub use barcode::QrCodeTool;
#[cfg(feature = "cad-utils")]
pub use cad::CadConvertTool;
pub use cargo::{CargoCheckTool, CargoTestTool};
pub use compress::{CompressTool, DecompressTool};
#[cfg(feature = "data-export")]
pub use csv_utils::{CsvAnalyzeTool, CsvTransformTool};
#[cfg(feature = "data-export")]
pub use data_serialization::{
    CsvReadTool, CsvWriteTool, TomlReadTool, TomlWriteTool, YamlReadTool, YamlWriteTool,
};
#[cfg(feature = "document-docx")]
pub use docx::ReadDocxTool;
#[cfg(feature = "cad-dxf")]
pub use dxf_tool::DxfReadTool;
#[cfg(feature = "document-email")]
pub use email::EmailParseTool;
pub use filesystem::{FileDeleteTool, FileMoveTool, ListDirectoryTool};
#[cfg(feature = "cad-geo")]
pub use geo::GeoUtilTool;
pub use git::GitTool;
#[cfg(feature = "cad-gltf")]
pub use gltf::GltfReadTool;
pub use http::HttpRequestTool;
#[cfg(feature = "cad-iges")]
pub use iges::IgesReadTool;
#[cfg(feature = "image-processing")]
pub use image::{ImageAnalyzeTool, ImageConvertTool, ImageGenerateTool, ImageResizeTool};
#[cfg(feature = "document-invoice")]
pub use invoice::InvoiceParseTool;
#[cfg(feature = "cad-obj")]
pub use obj::ObjReadTool;
#[cfg(feature = "document-excel")]
pub use office::ReadExcelTool;
#[cfg(feature = "document-docx")]
pub use office::WriteDocxTool;
#[cfg(feature = "document-excel-write")]
pub use office::WriteExcelTool;
#[cfg(feature = "document-ppt")]
pub use office::{ReadPptTool, WritePptTool};
#[cfg(feature = "document-pdf")]
pub use pdf::{PdfMergeTool, PdfSplitTool, ReadPdfTool};
#[cfg(feature = "cad-ply")]
pub use ply::PlyReadTool;
pub use search::{FindFilesTool, GrepTool};
pub use shell::ShellExecTool;
#[cfg(feature = "backend-sqlite")]
pub use sqlite::SqliteQueryTool;
#[cfg(feature = "cad-step")]
pub use step::StepReadTool;
#[cfg(feature = "cad-stl")]
pub use stl::{StlGenerateTool, StlReadTool};
#[cfg(feature = "drawing-svg")]
pub use svg::{SvgExportTool, SvgGenerateTool, SvgReadTool};
#[cfg(feature = "document-html")]
pub use web::WebScrapeTool;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn shell_exec_runs_echo() {
        let tool = ShellExecTool;
        let mut last_output: Option<ToolOutput> = None;

        // This command is deterministic, but process spawning can be flaky
        // under heavy all-target test parallelism. Retry a few times.
        for _ in 0..3 {
            let input = tool_input(serde_json::json!({
                "command": "echo hello",
                "timeout_ms": 5000,
            }));
            let output = tool.run(&input).expect("shell_exec should run");
            if output.success {
                let result = output
                    .result
                    .as_ref()
                    .expect("successful shell_exec should include result");
                assert!(result["stdout"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("hello"));
                return;
            }
            last_output = Some(output);
        }

        let details = last_output
            .as_ref()
            .and_then(|o| o.result.as_ref())
            .map(|r| {
                format!(
                    "stdout='{}', stderr='{}', exit_code={:?}",
                    r["stdout"].as_str().unwrap_or_default(),
                    r["stderr"].as_str().unwrap_or_default(),
                    r["exit_code"]
                )
            })
            .unwrap_or_else(|| "no output details".to_string());
        panic!("shell_exec_runs_echo remained unsuccessful after retries: {details}");
    }

    #[test]
    fn find_files_finds_rs_files() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "text").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "find".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "pattern": "*.rs",
                "directory": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = FindFilesTool;
        let output = tool.run(&input).expect("find_files should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let files = result["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn list_directory_lists_entries() {
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "list".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": tmp.path().to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ListDirectoryTool;
        let output = tool.run(&input).expect("list_directory should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn file_move_renames_file() {
        let tmp = TempDir::new().expect("temp dir");
        let src = tmp.path().join("old.txt");
        let dst = tmp.path().join("new.txt");
        std::fs::write(&src, "content").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "move".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "source": src.to_string_lossy(),
                "destination": dst.to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = FileMoveTool;
        let output = tool.run(&input).expect("file_move should succeed");
        assert!(output.success);
        assert!(!src.exists());
        assert!(dst.exists());
    }

    #[test]
    fn file_delete_requires_confirmation() {
        let tmp = TempDir::new().expect("temp dir");
        let f = tmp.path().join("delete_me.txt");
        std::fs::write(&f, "bye").unwrap();

        let input_no_confirm = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "delete".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
                "confirm": false,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = FileDeleteTool;
        let output = tool.run(&input_no_confirm);
        assert!(output.is_err(), "file_delete without confirm should error");
        assert!(f.exists());

        let input_confirm = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "delete".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
                "confirm": true,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let output2 = tool
            .run(&input_confirm)
            .expect("file_delete with confirm should succeed");
        assert!(output2.success);
        assert!(!f.exists());
    }

    #[test]
    fn cargo_check_in_workspace() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "check".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "directory": workspace.to_string_lossy(),
            }),
            allowed_base_dir: Some(workspace),
        };

        let tool = CargoCheckTool;
        let output = tool.run(&input).expect("cargo_check should run");
        // Should succeed in this workspace
        assert!(output.success);
    }

    #[test]
    fn git_status_runs() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = ToolInput {
            task_id: "test-1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "subcommand": "status",
                "directory": workspace.to_string_lossy(),
            }),
            allowed_base_dir: Some(workspace),
        };
        let tool = GitTool;
        let output = tool.run(&input).expect("git status should run");
        assert!(output.success);
    }

    #[test]
    fn cargo_test_runs() {
        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "tester".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "directory": ".",
                "filter": "shell_exec_runs",
            }),
            allowed_base_dir: Some(PathBuf::from(".")),
        };

        let tool = CargoTestTool;
        let output = tool.run(&input).expect("cargo_test should run");
        // May or may not find the test
        assert!(output.result.is_some());
    }
}
