//! AgentPath — hierarchical agent addressing (BLUE70 §3.1)
//!
//! Supports path formats:
//! - "root"                  → root agent
//! - "root/research"         → research child under root
//! - "root/research/coder"   → three-level nesting
//! - "."                     → current agent
//! - ".."                    → parent agent

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Hierarchical agent path: root/research/coder
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentPath {
    segments: Vec<String>,
}

impl AgentPath {
    /// Parse a "/" separated path string.
    ///
    /// Special cases:
    /// - "."  → empty segments (current agent)
    /// - ".." → single ".." segment (parent, resolved at runtime)
    pub fn parse(path: &str) -> Result<Self> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            bail!("AgentPath cannot be empty");
        }
        if trimmed == "." {
            return Ok(Self {
                segments: Vec::new(),
            });
        }
        if trimmed == ".." {
            return Ok(Self {
                segments: vec!["..".to_string()],
            });
        }
        let segments: Vec<String> = trimmed
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        if segments.is_empty() {
            bail!("AgentPath must contain at least one segment");
        }
        Ok(Self { segments })
    }

    /// Create from an existing segment vector (internal use).
    pub fn from_segments(segments: Vec<String>) -> Self {
        Self { segments }
    }

    /// Return the parent path, or None if already at root.
    pub fn parent(&self) -> Option<AgentPath> {
        if self.segments.is_empty() || self.segments.len() == 1 {
            None
        } else {
            Some(AgentPath {
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            })
        }
    }

    /// Append a child segment to this path.
    pub fn child(&self, name: &str) -> AgentPath {
        let mut segments = self.segments.clone();
        segments.push(name.to_string());
        AgentPath { segments }
    }

    /// Path depth (number of segments). Root is depth 0.
    pub fn depth(&self) -> usize {
        self.segments.len()
    }

    /// Whether this is the root path (empty segments).
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Simplified wildcard matching: supports only single-level `*`.
    ///
    /// Pattern format:
    /// - `root/*/coder` matches `root/research/coder` but not `root/a/b/coder`
    /// - `root/*` matches `root/research` but not `root/a/b`
    /// - `*` matches any single-segment path
    pub fn matches_simple(&self, pattern: &AgentPathPattern) -> bool {
        if pattern.prefix.is_empty() && pattern.suffix.is_empty() {
            return self.segments.len() == 1; // bare `*`
        }
        if self.segments.len() != pattern.prefix.len() + 1 + pattern.suffix.len() {
            return false;
        }
        // Check prefix match
        for (i, seg) in pattern.prefix.iter().enumerate() {
            if i >= self.segments.len() || self.segments[i] != *seg {
                return false;
            }
        }
        // Check suffix match
        let suffix_start = pattern.prefix.len() + 1;
        for (i, seg) in pattern.suffix.iter().enumerate() {
            let idx = suffix_start + i;
            if idx >= self.segments.len() || self.segments[idx] != *seg {
                return false;
            }
        }
        true
    }

    /// Return the segments as a slice.
    pub fn as_segments(&self) -> &[String] {
        &self.segments
    }

    /// Convert to display path string.
    pub fn to_path_string(&self) -> String {
        if self.segments.is_empty() {
            return ".".to_string();
        }
        self.segments.join("/")
    }
}

impl fmt::Display for AgentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_path_string())
    }
}

impl std::str::FromStr for AgentPath {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        AgentPath::parse(s)
    }
}

/// Simplified wildcard pattern: `root/*/coder` → prefix=["root"], suffix=["coder"]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentPathPattern {
    pub prefix: Vec<String>,
    pub suffix: Vec<String>,
}

impl AgentPathPattern {
    /// Parse a pattern string: `root/*/coder`
    pub fn parse(pattern: &str) -> Result<Self> {
        let parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
        let star_pos = parts.iter().position(|&p| p == "*");
        match star_pos {
            None => bail!("AgentPathPattern must contain exactly one '*'"),
            Some(pos) => Ok(AgentPathPattern {
                prefix: parts[..pos].iter().map(|s| s.to_string()).collect(),
                suffix: parts[pos + 1..].iter().map(|s| s.to_string()).collect(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_root() {
        let p = AgentPath::parse("root").unwrap();
        assert_eq!(p.depth(), 1);
        assert!(!p.is_root());
        assert_eq!(p.to_path_string(), "root");
    }

    #[test]
    fn test_parse_nested() {
        let p = AgentPath::parse("root/research/coder").unwrap();
        assert_eq!(p.depth(), 3);
        assert_eq!(p.to_path_string(), "root/research/coder");
    }

    #[test]
    fn test_parse_current() {
        let p = AgentPath::parse(".").unwrap();
        assert_eq!(p.depth(), 0);
        assert!(p.is_root());
    }

    #[test]
    fn test_parent() {
        let p = AgentPath::parse("root/research/coder").unwrap();
        let parent = p.parent().unwrap();
        assert_eq!(parent.to_path_string(), "root/research");
        let grandparent = parent.parent().unwrap();
        assert_eq!(grandparent.to_path_string(), "root");
        assert!(grandparent.parent().is_none());
    }

    #[test]
    fn test_child() {
        let p = AgentPath::parse("root").unwrap();
        let child = p.child("research");
        assert_eq!(child.to_path_string(), "root/research");
    }

    #[test]
    fn test_wildcard_match() {
        let p = AgentPath::parse("root/research/coder").unwrap();
        let pattern = AgentPathPattern::parse("root/*/coder").unwrap();
        assert!(p.matches_simple(&pattern));

        let no_match = AgentPath::parse("root/a/b/coder").unwrap();
        assert!(!no_match.matches_simple(&pattern));
    }

    #[test]
    fn test_wildcard_single_level() {
        let p = AgentPath::parse("root/research").unwrap();
        let pattern = AgentPathPattern::parse("root/*").unwrap();
        assert!(p.matches_simple(&pattern));

        let deep = AgentPath::parse("root/a/b").unwrap();
        assert!(!deep.matches_simple(&pattern));
    }

    #[test]
    fn test_empty_path_fails() {
        assert!(AgentPath::parse("").is_err());
    }

    #[test]
    fn test_from_str() {
        let p: AgentPath = "root/research".parse().unwrap();
        assert_eq!(p.depth(), 2);
    }

    #[test]
    fn test_pattern_no_star_fails() {
        assert!(AgentPathPattern::parse("root/coder").is_err());
    }
}
