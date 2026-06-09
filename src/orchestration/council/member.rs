//! Member management methods for `OrchestrationCouncil`.
//!
//! Handles adding, removing, retrieving, and listing council members.

use super::council::OrchestrationCouncil;
use super::types::*;
use crate::i18n::runtime::tf;
use anyhow::{anyhow, Result};

impl OrchestrationCouncil {
    /// Add a new member to the council.
    ///
    /// Returns an error if a member with the same ID already exists.
    pub fn add_member(&self, member: CouncilMember) -> Result<()> {
        let mut members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        if members.contains_key(&member.id) {
            return Err(anyhow!(tf(
                "error.council.member_already_exists",
                &[("member_id", &member.id)]
            )));
        }

        members.insert(member.id.clone(), member);
        Ok(())
    }

    /// Remove a member from the council by ID.
    ///
    /// Returns `Ok(true)` if the member was removed, `Ok(false)` if the
    /// member did not exist (no-op).
    pub fn remove_member(&self, id: &str) -> Result<bool> {
        let mut members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        Ok(members.remove(id).is_some())
    }

    /// Get a member's details by ID.
    pub fn get_member(&self, id: &str) -> Result<CouncilMember> {
        let members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        members
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!(tf("error.council.member_not_found", &[("member_id", id)])))
    }

    /// List all registered council members.
    pub fn list_members(&self) -> Result<Vec<CouncilMember>> {
        let members = self
            .members
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on members: {}", e))?;

        let mut list: Vec<CouncilMember> = members.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(list)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::council::test_support::*;

    #[test]
    fn test_new_council_empty() {
        let council = default_council();
        let p = council.profile();
        assert_eq!(p.total_members, 0);
        assert_eq!(p.active_members, 0);
        assert_eq!(p.total_proposals, 0);
        assert_eq!(p.passed_count, 0);
        assert_eq!(p.rejected_count, 0);
        assert_eq!(p.pending_count, 0);
    }

    #[test]
    fn test_add_and_list_members() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 2))
            .unwrap();

        let members = council.list_members().unwrap();
        assert_eq!(members.len(), 2);

        let alice = council.get_member("alice").unwrap();
        assert_eq!(alice.name, "Alice");
        assert_eq!(alice.voting_power, 1);

        let bob = council.get_member("bob").unwrap();
        assert_eq!(bob.name, "Bob");
        assert_eq!(bob.voting_power, 2);
    }

    #[test]
    fn test_remove_member() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();

        let removed = council.remove_member("alice").unwrap();
        assert!(removed);
        assert!(council.get_member("alice").is_err());
        assert_eq!(council.list_members().unwrap().len(), 0);
    }

    #[test]
    fn test_remove_nonexistent_member_noop() {
        let council = default_council();
        let removed = council.remove_member("nonexistent").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_add_duplicate_member_fails() {
        let council = default_council();
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();

        let err = council
            .add_member(sample_member("alice", "Alice Again", "analyst", 2))
            .unwrap_err();
        assert!(
            err.to_string().contains("already exists")
                || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }
}
