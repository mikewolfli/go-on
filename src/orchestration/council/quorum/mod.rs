//! Deliberation and quorum-related methods for `OrchestrationCouncil`.
//!
//! Handles multi-round deliberation, statements, round voting, profile
//! snapshots, and auto-ejection of low performers.
//!
//! ## Sub-modules
//!
//! * [`proposal`] — Starting deliberations, routing proposals through voting paths
//! * [`voting`] — Submitting statements and casting votes within deliberation rounds
//! * [`consensus`] — Round conclusion, tallying, unanimity checks, multi-round orchestration

mod consensus;
mod proposal;
mod voting;

#[cfg(test)]
mod tests {
    use super::super::council::test_support::*;
    use super::super::council::OrchestrationCouncil;
    use super::super::types::*;

    #[test]
    fn test_start_deliberation() {
        let council = sample_deliberation_council();

        let delib_id = council.start_deliberation("prop-1").unwrap();
        assert_eq!(delib_id.0, "delib-prop-1");

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.proposal_id, "prop-1");
        assert_eq!(delib.max_rounds, 3);
        assert!(!delib.consensus_reached);
        assert!(delib.final_decision.is_none());
        assert_eq!(delib.rounds.len(), 1);
        assert_eq!(delib.rounds[0].round_number, 1);
        assert!(!delib.rounds[0].concluded);
    }

    #[test]
    fn test_start_deliberation_nonexistent_proposal_fails() {
        let council = sample_deliberation_council();
        let err = council.start_deliberation("nonexistent").unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("error.council."),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_start_deliberation_duplicate_fails() {
        let council = sample_deliberation_council();
        council.start_deliberation("prop-1").unwrap();
        let err = council.start_deliberation("prop-1").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_submit_statement_in_deliberation() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        let statement = DeliberationStatement {
            member_id: "alice".to_string(),
            position: CouncilPosition::Support,
            reasoning: "This is a good proposal.".to_string(),
            amendments: vec![],
            submitted_at: now_epoch_ms(),
        };

        council.submit_statement(&delib_id, statement).unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].statements.len(), 1);
        assert_eq!(delib.rounds[0].statements[0].member_id, "alice");
        assert_eq!(
            delib.rounds[0].statements[0].position,
            CouncilPosition::Support
        );
    }

    #[test]
    fn test_submit_statement_replaces_existing_in_round() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Alice submits initial statement.
        council
            .submit_statement(
                &delib_id,
                DeliberationStatement {
                    member_id: "alice".to_string(),
                    position: CouncilPosition::Support,
                    reasoning: "I support this.".to_string(),
                    amendments: vec![],
                    submitted_at: now_epoch_ms(),
                },
            )
            .unwrap();

        // Alice changes her mind (still in round 1).
        council
            .submit_statement(
                &delib_id,
                DeliberationStatement {
                    member_id: "alice".to_string(),
                    position: CouncilPosition::Oppose,
                    reasoning: "Actually, I oppose this now.".to_string(),
                    amendments: vec![],
                    submitted_at: now_epoch_ms(),
                },
            )
            .unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].statements.len(), 1);
        assert_eq!(
            delib.rounds[0].statements[0].position,
            CouncilPosition::Oppose
        );
    }

    #[test]
    fn test_vote_in_round() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].votes.len(), 2);
    }

    #[test]
    fn test_vote_in_round_replaces_previous_vote() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Alice votes support.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        // Alice changes vote to oppose (still in round 1).
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds[0].votes.len(), 1);
        assert_eq!(delib.rounds[0].votes[0].selected_option, "oppose");
    }

    #[test]
    fn test_inactive_member_cannot_vote_in_round() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Deactivate Alice.
        {
            let mut members = council.members.lock().unwrap();
            if let Some(m) = members.get_mut("alice") {
                m.is_active = false;
            }
        }

        let err = council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("inactive"));
    }

    #[test]
    fn test_get_active_deliberations() {
        let council = sample_deliberation_council();

        // Submit a second proposal.
        let mut p2 = sample_proposal("prop-2", "Another Test", "bob");
        p2.options = vec!["support".to_string(), "oppose".to_string()];
        p2.status = ProposalStatus::Active;
        council.submit_proposal(p2).unwrap();

        let d1 = council.start_deliberation("prop-1").unwrap();
        let d2 = council.start_deliberation("prop-2").unwrap();

        let active = council.get_active_deliberations();
        assert_eq!(active.len(), 2);
        assert!(active.contains(&d1));
        assert!(active.contains(&d2));
    }

    #[test]
    fn test_multi_round_deliberation_unanimous_round_one() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // All three members vote support in round 1.
        for member in &["alice", "bob", "carol"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "support".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Conclude round 1 — should reach consensus.
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert!(delib.consensus_reached);
        assert!(delib.final_decision.is_some());
        assert_eq!(
            delib.final_decision.unwrap().position,
            CouncilPosition::Support
        );

        // No active deliberations after conclusion.
        let active = council.get_active_deliberations();
        assert!(active.is_empty());
    }

    #[test]
    fn test_multi_round_deliberation_advances_to_round_two() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Split vote: alice supports, bob opposes, carol supports.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "carol".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        // Round 1 not unanimous — advance to round 2.
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(!concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert_eq!(delib.rounds.len(), 2);
        assert_eq!(delib.rounds[0].round_number, 1);
        assert!(delib.rounds[0].concluded);
        assert_eq!(delib.rounds[1].round_number, 2);
        assert!(!delib.rounds[1].concluded);
    }

    #[test]
    fn test_multi_round_deliberation_position_changes_between_rounds() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Round 1: alice and carol support, bob opposes.
        for (member, vote) in &[
            ("alice", "support"),
            ("bob", "oppose"),
            ("carol", "support"),
        ] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: vote.to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Conclude round 1 — not unanimous, advances to round 2.
        council.conclude_round(&delib_id).unwrap();

        // Round 2: bob changes position from oppose to support.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        // Alice and carol also vote again (same positions).
        for member in &["alice", "carol"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "support".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Conclude round 2 — now unanimous, should conclude.
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert!(delib.consensus_reached);
        assert_eq!(
            delib.final_decision.unwrap().position,
            CouncilPosition::Support
        );
    }

    #[test]
    fn test_multi_round_deliberation_forced_conclusion_at_max_rounds() {
        let council = OrchestrationCouncil::new_with_deliberation_config(
            CouncilConfig {
                name: "Forced Conclusion Test".to_string(),
                min_members_for_quorum: 2,
                voting_duration_ms: 86_400_000,
                max_proposals: 100,
                enable_reputation: false,
                reputation_warmup_rounds: 0,
                ..Default::default()
            },
            DeliberationConfig {
                max_rounds: 2, // Use 2 for faster test
                require_consensus: false,
                debate_timeout_secs: 60,
            },
        );
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Forced Test", "alice"))
            .unwrap();

        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Round 1: split vote.
        for (member, vote) in &[("alice", "support"), ("bob", "oppose")] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: vote.to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Round 1 not unanimous — advances to round 2 (max).
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(!concluded);

        // Round 2: still split.
        for (member, vote) in &[("alice", "support"), ("bob", "oppose")] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: vote.to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        // Round 2 is last — forced conclusion (support wins by tiebreak / first max).
        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(concluded);

        let delib = council.get_deliberation(&delib_id).unwrap();
        assert!(delib.final_decision.is_some());
        let decision = delib.final_decision.unwrap();
        assert_eq!(decision.decided_at_round, 2);
        // After forced conclusion on a tie (1 support, 1 oppose), either is valid.
        // The tiebreaker is deterministic based on the tally map's iteration order.
        assert!(
            decision.position == CouncilPosition::Support
                || decision.position == CouncilPosition::Oppose,
            "Expected Support or Oppose (tie at 1-1), got {:?}",
            decision.position
        );
    }

    #[test]
    fn test_deliberation_submit_statement_after_conclusion_fails() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // All vote unanimously.
        for member in &["alice", "bob", "carol"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "support".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }

        council.conclude_round(&delib_id).unwrap();

        // Try to submit statement after conclusion.
        let err = council
            .submit_statement(
                &delib_id,
                DeliberationStatement {
                    member_id: "alice".to_string(),
                    position: CouncilPosition::Amend,
                    reasoning: "Too late!".to_string(),
                    amendments: vec![],
                    submitted_at: now_epoch_ms(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("concluded"));
    }

    #[test]
    fn test_deliberation_vote_in_concluded_round_fails() {
        // Use max_rounds=1 so that a split vote in round 1 forces conclusion.
        let council = OrchestrationCouncil::new_with_deliberation_config(
            CouncilConfig {
                name: "Test Council".to_string(),
                min_members_for_quorum: 2,
                voting_duration_ms: 86_400_000,
                max_proposals: 100,
                enable_reputation: false,
                reputation_warmup_rounds: 0,
                ..Default::default()
            },
            DeliberationConfig {
                max_rounds: 1,
                require_consensus: false,
                debate_timeout_secs: 60,
            },
        );
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 1))
            .unwrap();
        council
            .add_member(sample_member("carol", "Carol", "overseer", 1))
            .unwrap();
        council
            .submit_proposal(sample_proposal("prop-1", "Test Proposal", "alice"))
            .unwrap();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Round 1 votes split, conclude round 1.
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "alice".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();
        council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "bob".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "oppose".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap();

        let concluded = council.conclude_round(&delib_id).unwrap();
        assert!(
            concluded,
            "Deliberation should have concluded with max_rounds=1"
        );

        // With max_rounds=1 and a split vote, the deliberation has concluded.
        // Attempting to vote should fail with "already concluded".
        let err = council
            .vote_in_round(
                &delib_id,
                CouncilVote {
                    member_id: "carol".to_string(),
                    proposal_id: "prop-1".to_string(),
                    selected_option: "support".to_string(),
                    weight: 1,
                    vote_ms: now_epoch_ms(),
                    rationale: None,
                },
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("concluded"),
            "Expected error containing 'concluded', got: {}",
            err
        );
    }

    #[test]
    fn test_conclude_round_twice_fails() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        // Vote and conclude round 1.
        for member in &["alice", "bob"] {
            council
                .vote_in_round(
                    &delib_id,
                    CouncilVote {
                        member_id: member.to_string(),
                        proposal_id: "prop-1".to_string(),
                        selected_option: "oppose".to_string(),
                        weight: 1,
                        vote_ms: now_epoch_ms(),
                        rationale: None,
                    },
                )
                .unwrap();
        }
        council.conclude_round(&delib_id).unwrap();

        // Second conclude on the same round should fail.
        let err = council.conclude_round(&delib_id).unwrap_err();
        assert!(err.to_string().contains("concluded"));
    }

    #[test]
    fn test_deliberation_statement_with_amendments() {
        let council = sample_deliberation_council();
        let delib_id = council.start_deliberation("prop-1").unwrap();

        let statement = DeliberationStatement {
            member_id: "alice".to_string(),
            position: CouncilPosition::Amend,
            reasoning: "We should increase the memory limit.".to_string(),
            amendments: vec![
                "Increase memory limit from 512MB to 1GB".to_string(),
                "Add monitoring alerts".to_string(),
            ],
            submitted_at: now_epoch_ms(),
        };

        council.submit_statement(&delib_id, statement).unwrap();

        let delib = council.get_deliberation(&delib_id).unwrap();
        let stmt = &delib.rounds[0].statements[0];
        assert_eq!(stmt.position, CouncilPosition::Amend);
        assert_eq!(stmt.amendments.len(), 2);
        assert!(stmt.amendments[0].contains("1GB"));
    }

    #[test]
    fn test_deliberation_config_default_values() {
        let config = DeliberationConfig::default();
        assert_eq!(config.max_rounds, 3);
        assert_eq!(config.debate_timeout_secs, 60);
        assert!(!config.require_consensus);
    }

    #[test]
    fn test_profile_reflects_state() {
        let council = default_council();

        // Initial profile.
        let p = council.profile();
        assert_eq!(p.total_members, 0);

        // Add members.
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();
        council
            .add_member(sample_member("bob", "Bob", "analyst", 2))
            .unwrap();

        let p = council.profile();
        assert_eq!(p.total_members, 2);
        assert_eq!(p.active_members, 2);

        // Submit some proposals.
        let mut p1 = sample_proposal("prop-1", "Proposal A", "alice");
        p1.status = ProposalStatus::Pending;
        council.submit_proposal(p1).unwrap();

        let mut p2 = sample_proposal("prop-2", "Proposal B", "bob");
        p2.status = ProposalStatus::Active;
        council.submit_proposal(p2).unwrap();

        let p = council.profile();
        assert_eq!(p.total_proposals, 2);
        assert_eq!(p.pending_count, 2); // Both are pending or active
        assert_eq!(p.passed_count, 0);
        assert_eq!(p.rejected_count, 0);
        assert_eq!(p.tied_count, 0);

        // Manually alter a proposal's status to test counts.
        {
            let mut proposals = council.proposals.lock().unwrap();
            if let Some(p) = proposals.get_mut("prop-1") {
                p.status = ProposalStatus::Passed;
            }
        }

        let p = council.profile();
        assert_eq!(p.passed_count, 1);
        assert_eq!(p.pending_count, 1); // Only prop-2 is still active
    }

    #[test]
    fn test_count_valid_proposals_in_profile() {
        let council = OrchestrationCouncil::new(CouncilConfig {
            name: "Profile Tester".to_string(),
            min_members_for_quorum: 1,
            voting_duration_ms: 1,
            max_proposals: 100,
            enable_reputation: false,
            reputation_warmup_rounds: 0,
            ..Default::default()
        });

        // Add a member.
        council
            .add_member(sample_member("alice", "Alice", "strategist", 1))
            .unwrap();

        // Submit a proposal and make it pass.
        council
            .submit_proposal(sample_proposal("p1", "Pass me", "alice"))
            .unwrap();
        let mut p2 = sample_proposal("p2", "Reject me", "alice");
        p2.status = ProposalStatus::Rejected;
        council.submit_proposal(p2).unwrap();
        let mut p3 = sample_proposal("p3", "Expire me", "alice");
        p3.status = ProposalStatus::Expired;
        council.submit_proposal(p3).unwrap();
        let mut p4 = sample_proposal("p4", "Tie me", "alice");
        p4.status = ProposalStatus::Tied;
        council.submit_proposal(p4).unwrap();

        {
            let proposals = council.proposals.lock().unwrap();
            let mut p1c = proposals.get("p1").cloned().unwrap();
            p1c.status = ProposalStatus::Passed;
            drop(proposals);
            let mut proposals = council.proposals.lock().unwrap();
            proposals.insert("p1".to_string(), p1c);
        }

        let p = council.profile();
        assert_eq!(p.total_proposals, 4);
        assert_eq!(p.passed_count, 1);
        assert_eq!(p.rejected_count, 1);
        assert_eq!(p.tied_count, 1);
    }

    #[test]
    fn test_auto_eject_low_performers() {
        let mut council = OrchestrationCouncil::new(CouncilConfig {
            name: "Ejection Test".to_string(),
            min_members_for_quorum: 2,
            voting_duration_ms: 86_400_000,
            max_proposals: 100,
            enable_reputation: true,
            reputation_warmup_rounds: 0,
            ejection_threshold: Some(0.3),
            ejection_window: Some(20),
            ejection_warmup_rounds: Some(0),
            ..Default::default()
        });

        // Add a member with poor recent accuracy
        let member_id = "poor-performer".to_string();
        council
            .add_member(sample_member(&member_id, "Poor Performer", "analyst", 1))
            .unwrap();

        {
            let mut rep = council.reputation.lock().unwrap();
            let record = rep
                .entry(member_id.clone())
                .or_insert_with(|| ReputationRecord::new(&member_id, 0));
            record.warmup_remaining = 0;
            record.recent_window = vec![false; 25]; // 25 consecutive wrong votes
        }

        let ejected = council.auto_eject_low_performers();
        assert!(
            ejected.contains(&member_id),
            "low performer should be ejected"
        );
    }
}
