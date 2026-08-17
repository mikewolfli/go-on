//! Shared write-path safety checks.
//!
//! Single canonical source for the protected-directory and sensitive-extension
//! blocklists used by both write-path enforcement points:
//!
//! - the tool-layer sandbox `orchestration::tool::builtin_tools::enforce_write_sandbox`
//! - the governance quick gate `governance::status::check_write`
//!
//! Previously each kept its own independent list and they drifted apart
//! (e.g. `/root` and `/var/run` were only protected in the gate). Matching is
//! purely lexical — no filesystem access — so it also works for paths that do
//! not exist yet.

use std::path::Path;

/// Directories that write operations must never touch. Union of the former
/// tool-sandbox list (`/etc`, `/sys`, `/proc`, `/dev`, `/boot`, `/var/log`,
/// `/var/db`, `/usr/lib`, `/usr/bin`) and the governance-gate list (`/etc`,
/// `/boot`, `/sys`, `/proc`, `/dev`, `/root`, `/var/run`, `/run`,
/// `/tmp/.X11-unix`). Matched case-insensitively.
const PROTECTED_DIRS: &[&str] = &[
    "/etc",
    "/sys",
    "/proc",
    "/dev",
    "/boot",
    "/var/log",
    "/var/db",
    "/usr/lib",
    "/usr/bin",
    "/root",
    "/var/run",
    "/run",
    "/tmp/.x11-unix",
];

/// Windows system directories, matched via lowercase substring on the raw
/// path (backslash paths are never normalized). Mirrors the former tool
/// sandbox entries verbatim.
const WINDOWS_PROTECTED_DIRS: &[&str] = &["c:\\windows\\", "c:\\program files\\"];

/// File extensions that indicate sensitive configuration or credential data.
/// Writes to files with these extensions are flagged.
const SENSITIVE_EXTENSIONS: &[&str] = &[
    ".pem",
    ".key",
    ".crt",
    ".cer",
    ".der",
    ".p12",
    ".pfx",
    ".gpg",
    ".asc",
    ".envrc",
    ".env",
    ".htpasswd",
    ".htaccess",
    ".shadow",
    ".passwd",
];

/// Lexically normalize a path string: collapse duplicate slashes and resolve
/// `.` / `..` segments (no filesystem access, so it works for paths that do
/// not exist yet). Leading `..` segments are preserved so relative traversal
/// escapes (`../../etc/cron.d/x`) stay recognizable instead of collapsing to
/// a bare relative name.
fn normalize_path_string(path: &str) -> String {
    let is_abs = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.is_empty() {
                    // Preserve a leading escape so `../../etc/x` does not
                    // collapse into `etc/x` (which would miss the check).
                    out.push("..");
                } else {
                    out.pop();
                }
            }
            s => out.push(s),
        }
    }
    let mut joined = out.join("/");
    if is_abs {
        joined.insert(0, '/');
    }
    joined
}

/// Return the matched protected directory when `path` touches any directory
/// that writes must never reach, or `None` when the write is allowed.
///
/// Matching is a strict superset of the two former implementations:
///
/// - the lexically normalized path equals a protected dir or starts with
///   `<dir>/` (governance-gate behavior, catches `..` / duplicate-slash
///   traversal on paths that may not exist yet);
/// - the lowercased raw path contains `<dir>/` (tool-sandbox substring
///   behavior, catches relative paths such as `my/etc/x`);
/// - Windows entries match via lowercase substring, as the tool sandbox did.
pub fn protected_write_path(path: &Path) -> Option<&'static str> {
    let raw = path.to_string_lossy();
    let raw_lower = raw.to_lowercase();

    for entry in WINDOWS_PROTECTED_DIRS {
        if raw_lower.contains(entry) {
            return Some(entry);
        }
    }

    let normalized = normalize_path_string(raw.as_ref()).to_lowercase();
    for &dir in PROTECTED_DIRS {
        if normalized == dir || normalized.starts_with(&format!("{dir}/")) {
            return Some(dir);
        }
        if raw_lower.contains(&format!("{dir}/")) {
            return Some(dir);
        }
    }
    None
}

/// Return the matched sensitive extension when `path` names a file whose
/// extension indicates sensitive configuration or credential data, or `None`
/// otherwise. Case-insensitive suffix match, as in the governance gate.
pub fn sensitive_file_extension(path: &Path) -> Option<&'static str> {
    let lower = path.to_string_lossy().to_lowercase();
    SENSITIVE_EXTENSIONS
        .iter()
        .find(|ext| lower.ends_with(**ext))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_write_path_blocks_protected_dirs() {
        for path in [
            "/etc/passwd",
            "/etc",
            "my/etc/x",
            "/var/log/app.log",
            "/usr/bin/ls",
            "/root/.bashrc",
            "C:\\Windows\\system32\\x",
            "/tmp/.X11-unix/X0",
            "/sys/kernel/debug",
            "/proc/self/mem",
            "/dev/sda",
            "/boot/grub/grub.cfg",
            "/var/db/something",
            "/usr/lib/x86_64-linux-gnu/libc.so",
            // Traversal / normalization variants
            "../../etc/cron.d/x",
            "sub//etc/x",
            "/ETC/passwd",
        ] {
            assert!(
                protected_write_path(Path::new(path)).is_some(),
                "expected {path:?} to be blocked"
            );
        }
    }

    #[test]
    fn protected_write_path_allows_safe_paths() {
        for path in [
            "/home/user/file.txt",
            "/workspace/project/readme.md",
            "relative/file.txt",
            "/tmp/somefile.log",
            "/var/app/run-scripts/start.sh",
            // A relative path merely ENDING in a protected dir name is not
            // a protected-dir touch (matches both prior implementations).
            "my/etc",
        ] {
            assert!(
                protected_write_path(Path::new(path)).is_none(),
                "expected {path:?} to be allowed"
            );
        }
    }

    #[test]
    fn sensitive_file_extension_matches() {
        for (path, ext) in [
            ("/home/user/id_rsa.pem", ".pem"),
            ("config/.env", ".env"),
            ("creds/CA.crt", ".crt"),
            ("x.gpg", ".gpg"),
        ] {
            assert_eq!(
                sensitive_file_extension(Path::new(path)),
                Some(ext),
                "{path:?}"
            );
        }
        for path in ["/etc/hosts", "readme.md", "/home/user/file.txt"] {
            assert_eq!(sensitive_file_extension(Path::new(path)), None, "{path:?}");
        }
    }
}
