use super::*;

const T0: Timestamp = 1_700_000_000_000;

fn hop(layer: Layer, status: HopStatus) -> HopSnapshot {
    HopSnapshot {
        layer,
        status,
        transitioned_at: None,
        error: None,
    }
}

fn run(status: RunStatus) -> RunSnapshot {
    RunSnapshot {
        run_id: "run-1".into(),
        status,
        hops: vec![
            hop(Layer::Torii, HopStatus::Ok),
            hop(Layer::Fujin, HopStatus::Ok),
        ],
        approval: None,
        incidents: vec![],
        updated_at: T0,
        error: None,
        gate_verdict: None,
    }
}

fn approval(verdict: Option<ApprovalVerdict>, stale: bool) -> Option<ApprovalSnapshot> {
    Some(ApprovalSnapshot {
        verdict,
        stale,
        consumed: false,
    })
}

fn incident(status: IncidentStatus) -> IncidentSnapshot {
    IncidentSnapshot {
        id: "inc-1".into(),
        domain: IncidentDomain::Workspace,
        status,
        kind: "pipeline_run_abandoned".into(),
    }
}

fn one(snapshot: RunSnapshot, now: Timestamp) -> ProcessionEntry {
    let mut entries = project(&[snapshot], now, &Thresholds::default());
    assert_eq!(entries.len(), 1);
    entries.remove(0)
}

fn stale_after() -> Timestamp {
    Thresholds::default().stale_after as Timestamp
}

#[test]
fn stale_boundary_exactly_at_threshold_is_not_stale() {
    let entry = one(run(RunStatus::Running), T0 + stale_after());
    assert_eq!(entry.stale_for, None);
    assert_eq!(entry.blocker, None);
    assert_eq!(entry.responsible_layer, None);
}

#[test]
fn stale_boundary_one_past_threshold_is_stale() {
    let entry = one(run(RunStatus::Running), T0 + stale_after() + 1);
    assert_eq!(entry.stale_for, Some(Thresholds::default().stale_after + 1));
    assert_eq!(entry.blocker, Some(Blocker::Stale));
    assert_eq!(entry.responsible_layer, Some(Layer::Fujin));
}

#[test]
fn stale_uses_latest_of_hop_transition_and_updated_at() {
    let mut snapshot = run(RunStatus::Running);
    snapshot.hops[1].transitioned_at = Some(T0 + 10);
    let entry = one(snapshot.clone(), T0 + stale_after() + 10);
    assert_eq!(entry.last_transition, Some(T0 + 10));
    assert_eq!(entry.stale_for, None);

    snapshot.hops[1].transitioned_at = Some(T0 - 10);
    let entry = one(snapshot, T0 + stale_after() + 1);
    assert_eq!(entry.stale_for, Some(Thresholds::default().stale_after + 1));
}

#[test]
fn future_timestamps_have_zero_age() {
    let entry = one(run(RunStatus::Running), T0 - 1);
    assert_eq!(entry.stale_for, None);
    let zero = Thresholds { stale_after: 0 };
    let entries = project(&[run(RunStatus::Running)], T0 - 1, &zero);
    assert_eq!(entries[0].stale_for, None);
}

#[test]
fn failed_runs_have_no_stale_age() {
    let entry = one(run(RunStatus::Failed), T0 + stale_after() * 10);
    assert_eq!(entry.stale_for, None);
    assert_eq!(entry.blocker, Some(Blocker::Failed { reason: None }));
}

#[test]
fn completed_reports_idle_age_without_stale_blocker() {
    let now = T0 + stale_after() + 1;
    let entry = one(run(RunStatus::Completed), now);
    assert_eq!(entry.stale_for, Some(Thresholds::default().stale_after + 1));
    assert_eq!(entry.blocker, None);

    let mut snapshot = run(RunStatus::Completed);
    snapshot.approval = approval(None, false);
    let entry = one(snapshot, now);
    assert_eq!(entry.stale_for, Some(Thresholds::default().stale_after + 1));
    assert_eq!(entry.blocker, Some(Blocker::AwaitingApproval));
}

#[test]
fn completed_without_approval_or_incidents_is_ready() {
    let entry = one(run(RunStatus::Completed), T0);
    assert_eq!(entry.approval_state, ApprovalState::NotRequired);
    assert_eq!(entry.blocker, None);
    assert_eq!(entry.responsible_layer, None);
}

#[test]
fn completed_with_open_incident_blocks() {
    let mut snapshot = run(RunStatus::Completed);
    snapshot.incidents = vec![incident(IncidentStatus::Open)];
    let entry = one(snapshot, T0);
    assert!(matches!(entry.blocker, Some(Blocker::OpenIncident { .. })));
    assert_eq!(entry.responsible_layer, Some(Layer::Fujin));
}

#[test]
fn completed_without_verdict_awaits_approval() {
    let mut snapshot = run(RunStatus::Completed);
    snapshot.approval = approval(None, false);
    let entry = one(snapshot, T0);
    assert_eq!(entry.approval_state, ApprovalState::Pending);
    assert_eq!(entry.blocker, Some(Blocker::AwaitingApproval));
    // The owner holds the run, not a layer.
    assert_eq!(entry.responsible_layer, None);
}

#[test]
fn approval_non_approve_verdicts_block() {
    for (verdict, state, blocker) in [
        (
            ApprovalVerdict::RequestChanges,
            ApprovalState::ChangesRequested,
            Blocker::ChangesRequested,
        ),
        (
            ApprovalVerdict::Reject,
            ApprovalState::Rejected,
            Blocker::Rejected,
        ),
    ] {
        let mut snapshot = run(RunStatus::Completed);
        // A changed fence does not mask a non-approve verdict (host fence order).
        snapshot.approval = approval(Some(verdict), true);
        let entry = one(snapshot, T0);
        assert_eq!(entry.approval_state, state);
        assert_eq!(entry.blocker, Some(blocker));
        assert_eq!(entry.responsible_layer, None);
    }
}

#[test]
fn stale_approve_blocks() {
    let mut snapshot = run(RunStatus::Completed);
    snapshot.approval = approval(Some(ApprovalVerdict::Approve), true);
    let entry = one(snapshot, T0);
    assert_eq!(entry.approval_state, ApprovalState::Stale);
    assert_eq!(entry.blocker, Some(Blocker::ApprovalStale));
}

#[test]
fn fresh_approve_does_not_block() {
    let mut snapshot = run(RunStatus::Completed);
    snapshot.approval = approval(Some(ApprovalVerdict::Approve), false);
    let entry = one(snapshot, T0);
    assert_eq!(entry.approval_state, ApprovalState::Approved);
    assert_eq!(entry.blocker, None);
}

#[test]
fn consumed_approval_is_reported_but_not_a_blocker() {
    let mut snapshot = run(RunStatus::Completed);
    snapshot.approval = Some(ApprovalSnapshot {
        verdict: Some(ApprovalVerdict::Approve),
        stale: true,
        consumed: true,
    });
    let entry = one(snapshot, T0);
    assert_eq!(entry.approval_state, ApprovalState::Consumed);
    assert_eq!(entry.blocker, None);
}

#[test]
fn no_approval_snapshot_means_not_required() {
    assert_eq!(
        one(run(RunStatus::Completed), T0).approval_state,
        ApprovalState::NotRequired
    );
}

#[test]
fn loop_detected_names_responsible_layer() {
    let mut snapshot = run(RunStatus::Failed);
    snapshot.error = Some("loop_detected:satori".into());
    snapshot.approval = approval(None, false);
    snapshot.incidents = vec![incident(IncidentStatus::Open)];
    let entry = one(snapshot, T0 + stale_after() * 10);
    assert_eq!(
        entry.blocker,
        Some(Blocker::LoopDetected {
            layer: Layer::Satori
        })
    );
    assert_eq!(entry.responsible_layer, Some(Layer::Satori));
    assert_eq!(entry.current_hop, Some(Layer::Fujin));
}

#[test]
fn unknown_loop_layer_falls_back_to_failed() {
    let mut snapshot = run(RunStatus::Failed);
    snapshot.error = Some("loop_detected:unknown".into());
    let entry = one(snapshot, T0);
    assert_eq!(
        entry.blocker,
        Some(Blocker::Failed {
            reason: Some("loop_detected:unknown".into())
        })
    );
}

#[test]
fn terminal_statuses_clear_blocker_and_stale() {
    for status in [RunStatus::HandedOff, RunStatus::Superseded] {
        let mut snapshot = run(status);
        snapshot.error = Some("loop_detected:fujin".into());
        snapshot.approval = approval(None, false);
        snapshot.gate_verdict = Some(GateVerdict::NotReady {
            missing: vec!["goal".into()],
        });
        snapshot.incidents = vec![incident(IncidentStatus::Open)];
        let entry = one(snapshot, T0 + stale_after() * 10);
        assert_eq!(entry.blocker, None, "{status:?}");
        assert_eq!(entry.stale_for, None, "{status:?}");
        assert_eq!(entry.responsible_layer, None, "{status:?}");
    }
}

#[test]
fn gate_not_ready_precedes_approval() {
    let mut snapshot = run(RunStatus::Completed);
    snapshot.approval = approval(None, false);
    snapshot.gate_verdict = Some(GateVerdict::NotReady {
        missing: vec!["goal".into()],
    });
    let entry = one(snapshot, T0);
    assert_eq!(entry.blocker, Some(Blocker::GateFailed));
    assert_eq!(entry.responsible_layer, Some(Layer::Fujin));
    assert_eq!(entry.approval_state, ApprovalState::Pending);
    assert!(matches!(
        entry.gate_verdict,
        Some(GateVerdict::NotReady { .. })
    ));
}

#[test]
fn unresolved_incident_blocks_in_input_order() {
    let mut snapshot = run(RunStatus::Running);
    let mut claimed = incident(IncidentStatus::Claimed);
    claimed.id = "inc-2".into();
    snapshot.incidents = vec![incident(IncidentStatus::Resolved), claimed];
    let entry = one(snapshot, T0 + stale_after() + 1);
    assert_eq!(
        entry.blocker,
        Some(Blocker::OpenIncident {
            incident_id: "inc-2".into(),
            domain: IncidentDomain::Workspace,
            status: IncidentStatus::Claimed,
            incident_kind: "pipeline_run_abandoned".into(),
        })
    );
}

#[test]
fn resolved_incidents_do_not_block() {
    let mut snapshot = run(RunStatus::Running);
    snapshot.incidents = vec![incident(IncidentStatus::Resolved)];
    assert_eq!(one(snapshot, T0).blocker, None);
}

#[test]
fn failed_reason_falls_back_to_last_error_hop() {
    let mut snapshot = run(RunStatus::Failed);
    let mut failed = hop(Layer::Enma, HopStatus::Error);
    failed.error = Some("enma timeout".into());
    snapshot.hops.push(failed);
    let entry = one(snapshot, T0);
    assert_eq!(
        entry.blocker,
        Some(Blocker::Failed {
            reason: Some("enma timeout".into())
        })
    );
    assert_eq!(entry.responsible_layer, Some(Layer::Enma));
}

#[test]
fn run_without_hops_has_no_current_hop() {
    let mut snapshot = run(RunStatus::Running);
    snapshot.hops.clear();
    let entry = one(snapshot, T0 + stale_after() + 1);
    assert_eq!(entry.current_hop, None);
    assert_eq!(entry.last_transition, None);
    assert_eq!(entry.blocker, Some(Blocker::Stale));
    assert_eq!(entry.responsible_layer, None);
}

#[test]
fn projection_preserves_order_and_input() {
    let mut second = run(RunStatus::HandedOff);
    second.run_id = "run-2".into();
    let runs = vec![run(RunStatus::Running), second];
    let before = runs.clone();
    let entries = project(&runs, T0, &Thresholds::default());
    assert_eq!(runs, before);
    assert_eq!(
        entries
            .iter()
            .map(|e| e.run_id.as_str())
            .collect::<Vec<_>>(),
        ["run-1", "run-2"]
    );
    assert_eq!(entries, project(&runs, T0, &Thresholds::default()));
}

#[test]
fn run_snapshot_serde_roundtrip() {
    let mut snapshot = run(RunStatus::Failed);
    snapshot.hops[0].transitioned_at = Some(T0);
    snapshot.hops[1].status = HopStatus::Retry;
    snapshot.hops.push(HopSnapshot {
        layer: Layer::Daruma,
        status: HopStatus::Blocked,
        transitioned_at: None,
        error: Some("boom".into()),
    });
    snapshot.approval = Some(ApprovalSnapshot {
        verdict: Some(ApprovalVerdict::RequestChanges),
        stale: true,
        consumed: false,
    });
    snapshot.incidents = vec![incident(IncidentStatus::Open)];
    snapshot.error = Some("loop_detected:yatagarasu".into());
    snapshot.gate_verdict = Some(GateVerdict::NotReady {
        missing: vec!["goal".into()],
    });
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["status"], "failed");
    assert_eq!(json["hops"][1]["status"], "retry");
    assert_eq!(json["approval"]["verdict"], "request_changes");
    assert_eq!(json["gate_verdict"]["verdict"], "not_ready");
    let back: RunSnapshot = serde_json::from_value(json).unwrap();
    assert_eq!(back, snapshot);

    let entry = one(snapshot, T0);
    let json = serde_json::to_string(&entry).unwrap();
    assert_eq!(
        serde_json::from_str::<ProcessionEntry>(&json).unwrap(),
        entry
    );
}

// ---------------------------------------------------------------------------
// v1: task snapshots
// ---------------------------------------------------------------------------

fn task(id: &str, status: TaskStatus, age: Milliseconds) -> TaskSnapshot {
    TaskSnapshot {
        task_id: id.into(),
        status,
        status_since: T0 - age as Timestamp,
        claim: None,
        updated_at: T0,
    }
}

fn signals(tasks: &[TaskSnapshot]) -> Vec<TaskSignal> {
    project_tasks(tasks, T0, &TaskThresholds::default())
}

#[test]
fn task_thresholds_default_to_owner_values() {
    let t = TaskThresholds::default();
    assert_eq!(
        (t.in_progress, t.in_review, t.other_open),
        (24 * HOUR, 168 * HOUR, 168 * HOUR)
    );
}

#[test]
fn each_status_stalls_strictly_after_its_threshold() {
    let t = TaskThresholds::default();
    for (status, threshold, reason) in [
        (
            TaskStatus::InProgress,
            t.in_progress,
            TaskStall::InProgressTooLong,
        ),
        (
            TaskStatus::InReview,
            t.in_review,
            TaskStall::InReviewTooLong,
        ),
        (TaskStatus::Todo, t.other_open, TaskStall::OpenTooLong),
    ] {
        assert!(
            signals(&[task("t", status, threshold)]).is_empty(),
            "{status:?} exactly at the threshold"
        );
        assert_eq!(
            signals(&[task("t", status, threshold + 1)]),
            vec![TaskSignal {
                task_id: "t".into(),
                status,
                stalled_for: threshold + 1,
                reason,
            }],
            "{status:?} one ms past it"
        );
    }
}

#[test]
fn terminal_blocked_and_inbox_tasks_never_signal() {
    for status in [
        TaskStatus::Done,
        TaskStatus::Cancelled,
        TaskStatus::Blocked,
        TaskStatus::Inbox,
    ] {
        let mut stale = task("t", status, 1_000 * HOUR);
        stale.claim = Some(ClaimSnapshot {
            agent_id: "a".into(),
            expires_at: T0 - 1,
        });
        assert!(signals(&[stale]).is_empty(), "{status:?}");
    }
}

#[test]
fn expired_claim_wins_over_age_and_ignores_the_threshold() {
    let claim = |expires_at| {
        Some(ClaimSnapshot {
            agent_id: "agent".into(),
            expires_at,
        })
    };
    // Young task, agent gone: signals anyway.
    let mut young = task("young", TaskStatus::InProgress, HOUR);
    young.claim = claim(T0 - 1);
    // Old task, agent gone: ClaimExpired, not InProgressTooLong.
    let mut old = task("old", TaskStatus::InProgress, 48 * HOUR);
    old.claim = claim(T0 - 1);
    // Claim expiring exactly now is still live.
    let mut live = task("live", TaskStatus::InProgress, HOUR);
    live.claim = claim(T0);
    let out = signals(&[young, old, live]);
    assert_eq!(
        out.iter()
            .map(|s| (s.task_id.as_str(), s.reason))
            .collect::<Vec<_>>(),
        [
            ("old", TaskStall::ClaimExpired),
            ("young", TaskStall::ClaimExpired)
        ]
    );
    // An expired claim on a task in review does not matter.
    let mut review = task("r", TaskStatus::InReview, HOUR);
    review.claim = claim(T0 - 1);
    assert!(signals(&[review]).is_empty());
}

#[test]
fn future_status_since_never_signals() {
    let mut skewed = task("t", TaskStatus::InProgress, 0);
    skewed.status_since = T0 + 1;
    skewed.claim = Some(ClaimSnapshot {
        agent_id: "a".into(),
        expires_at: T0 - 1,
    });
    assert!(signals(&[skewed]).is_empty());
}

#[test]
fn signals_sort_by_age_then_id_and_empty_input_is_empty() {
    let out = signals(&[
        task("b", TaskStatus::Todo, 200 * HOUR),
        task("c", TaskStatus::InProgress, 30 * HOUR),
        task("a", TaskStatus::Todo, 200 * HOUR),
        task("d", TaskStatus::Done, 900 * HOUR),
    ]);
    assert_eq!(
        out.iter().map(|s| s.task_id.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert!(signals(&[]).is_empty());
}

#[test]
fn task_signal_serializes_snake_case() {
    let json =
        serde_json::to_value(&signals(&[task("t", TaskStatus::InReview, 169 * HOUR)])[0]).unwrap();
    assert_eq!(json["status"], "in_review");
    assert_eq!(json["reason"], "in_review_too_long");
}
