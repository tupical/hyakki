//! Read-only coordination over host-supplied pipeline and task snapshots.
//!
//! No clock, storage, routing, maturity assessment or side effects.
//! All times are Unix milliseconds; durations are unsigned milliseconds.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch (UTC).
pub type Timestamp = i64;
/// Nonnegative duration in milliseconds.
pub type Milliseconds = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Torii,
    Satori,
    Enma,
    Yatagarasu,
    Fujin,
    Daruma,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Superseded,
    HandedOff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HopStatus {
    Ok,
    Retry,
    Blocked,
    Error,
}

/// An observed attempt, including unsuccessful attempts. Hosts preserve append order.
///
/// [`Layer`] is closed: the host mapper builds `HopSnapshot`s itself and skips
/// hops of unknown layers instead of deserializing host hop records directly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HopSnapshot {
    pub layer: Layer,
    pub status: HopStatus,
    /// Wall-clock transition time, if the host records one (MCPBox hops do not);
    /// never an elapsed execution duration.
    pub transitioned_at: Option<Timestamp>,
    pub error: Option<String>,
}

/// A verdict supplied by the gate owner; Hyakki never assesses maturity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum GateVerdict {
    Ready,
    NotReady { missing: Vec<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalVerdict {
    Approve,
    RequestChanges,
    Reject,
}

/// Present when owner approval applies. A missing verdict means pending approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalSnapshot {
    pub verdict: Option<ApprovalVerdict>,
    /// Supplied by the host after comparing the saved and current approval fences.
    /// Only meaningful for an `Approve` verdict, as in the host handoff fence.
    pub stale: bool,
    /// The host has already used this approval for a handoff.
    pub consumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    NotRequired,
    Pending,
    Approved,
    ChangesRequested,
    Rejected,
    Stale,
    Consumed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentDomain {
    Platform,
    Workspace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    Open,
    Claimed,
    Resolved,
}

/// An incident the host has explicitly associated with this run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentSnapshot {
    pub id: String,
    pub domain: IncidentDomain,
    pub status: IncidentStatus,
    /// Host-defined open vocabulary, preserved verbatim.
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub run_id: String,
    pub status: RunStatus,
    pub hops: Vec<HopSnapshot>,
    /// None means approval is not required, not that the verdict is missing.
    pub approval: Option<ApprovalSnapshot>,
    pub incidents: Vec<IncidentSnapshot>,
    pub updated_at: Timestamp,
    /// Preserve the host error, including the canonical `loop_detected:<layer>`.
    pub error: Option<String>,
    /// None means no observed verdict; an existing packet does not imply Ready.
    /// None is the normal case for a host that does not persist the fujin
    /// verdict (MCPBox currently does not).
    pub gate_verdict: Option<GateVerdict>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thresholds {
    /// A run becomes stale strictly after this duration. Zero is a valid threshold.
    /// Hosts should pass a value below their own abandoned-run sweeper TTL
    /// (MCPBox: `MCPBOX_PIPELINE_RUN_TTL_MINUTES`, 30 min), otherwise the run
    /// is failed by the host before Hyakki ever reports it as stale.
    pub stale_after: Milliseconds,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            stale_after: 10 * 60 * 1_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Blocker {
    LoopDetected {
        layer: Layer,
    },
    AwaitingApproval,
    ApprovalStale,
    ChangesRequested,
    Rejected,
    GateFailed,
    OpenIncident {
        incident_id: String,
        domain: IncidentDomain,
        status: IncidentStatus,
        incident_kind: String,
    },
    Stale,
    Failed {
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessionEntry {
    pub run_id: String,
    pub current_hop: Option<Layer>,
    pub last_transition: Option<Timestamp>,
    pub gate_verdict: Option<GateVerdict>,
    pub approval_state: ApprovalState,
    /// Full idle duration, not just the excess over the threshold. Running and completed runs only.
    pub stale_for: Option<Milliseconds>,
    pub blocker: Option<Blocker>,
    pub responsible_layer: Option<Layer>,
}

/// Project in input order without mutating snapshots or reading the clock.
///
/// The last appended hop is current, even on failure. Its timestamp is the last
/// transition; if absent, `updated_at` is used only for idle age, not fabricated
/// as a transition. Future timestamps have zero age.
///
/// Idle age is measured from the later of the last hop transition and
/// `updated_at`.
///
/// `stale_for` is reported for `running` and `completed` runs (waiting for the
/// owner is idle time too); the `Stale` blocker applies to `running` only.
///
/// `completed` is not terminal: the chain finished and the run awaits handoff
/// (owner verdicts are only accepted on completed runs). Only `handed_off` and
/// `superseded` are terminal.
///
/// Blocker priority (first match wins): terminal handed_off/superseded clears
/// all blockers; otherwise loop, gate not ready, approval (pending, changes
/// requested, rejected, stale), unresolved incident, failed run, stale running
/// run. A completed run that passes loop/gate/approval/incident is ready for
/// handoff (None). Gate precedes approval because the host handoff checks it
/// first. Incident ties use input order. Approval state follows the host fence
/// order: consumed, missing verdict, non-approve verdict, stale approve.
/// A consumed approval is not a blocker: the handoff already happened.
///
/// `responsible_layer` is the loop's layer; None for approval blockers (the
/// owner holds the run, not a layer) and when unblocked; otherwise the current
/// hop. No deeper ownership is inferred.
pub fn project(
    runs: &[RunSnapshot],
    now: Timestamp,
    thresholds: &Thresholds,
) -> Vec<ProcessionEntry> {
    runs.iter()
        .map(|run| {
            let current_hop = run.hops.last().map(|hop| hop.layer);
            let last_transition = run.hops.last().and_then(|hop| hop.transitioned_at);
            let reference = last_transition.map_or(run.updated_at, |t| t.max(run.updated_at));
            let idle = now.max(reference).abs_diff(reference);
            let stale_for = (matches!(run.status, RunStatus::Running | RunStatus::Completed)
                && idle > thresholds.stale_after)
                .then_some(idle);
            let approval_state = approval_state(run.approval.as_ref());
            let blocker = blocker(run, approval_state, stale_for);
            let responsible_layer = match blocker.as_ref() {
                Some(Blocker::LoopDetected { layer }) => Some(*layer),
                Some(
                    Blocker::AwaitingApproval
                    | Blocker::ApprovalStale
                    | Blocker::ChangesRequested
                    | Blocker::Rejected,
                )
                | None => None,
                Some(_) => current_hop,
            };
            ProcessionEntry {
                run_id: run.run_id.clone(),
                current_hop,
                last_transition,
                gate_verdict: run.gate_verdict.clone(),
                approval_state,
                stale_for,
                blocker,
                responsible_layer,
            }
        })
        .collect()
}

fn approval_state(approval: Option<&ApprovalSnapshot>) -> ApprovalState {
    match approval {
        None => ApprovalState::NotRequired,
        Some(approval) if approval.consumed => ApprovalState::Consumed,
        Some(approval) => match approval.verdict {
            None => ApprovalState::Pending,
            Some(ApprovalVerdict::Approve) if approval.stale => ApprovalState::Stale,
            Some(ApprovalVerdict::Approve) => ApprovalState::Approved,
            Some(ApprovalVerdict::RequestChanges) => ApprovalState::ChangesRequested,
            Some(ApprovalVerdict::Reject) => ApprovalState::Rejected,
        },
    }
}

fn blocker(
    run: &RunSnapshot,
    approval: ApprovalState,
    stale_for: Option<Milliseconds>,
) -> Option<Blocker> {
    if matches!(run.status, RunStatus::HandedOff | RunStatus::Superseded) {
        return None;
    }
    if let Some(layer) = run.error.as_deref().and_then(loop_layer) {
        return Some(Blocker::LoopDetected { layer });
    }
    if matches!(run.gate_verdict, Some(GateVerdict::NotReady { .. })) {
        return Some(Blocker::GateFailed);
    }
    match approval {
        ApprovalState::Pending => return Some(Blocker::AwaitingApproval),
        ApprovalState::Stale => return Some(Blocker::ApprovalStale),
        ApprovalState::ChangesRequested => return Some(Blocker::ChangesRequested),
        ApprovalState::Rejected => return Some(Blocker::Rejected),
        ApprovalState::NotRequired | ApprovalState::Approved | ApprovalState::Consumed => {}
    }
    if let Some(incident) = run
        .incidents
        .iter()
        .find(|incident| incident.status != IncidentStatus::Resolved)
    {
        return Some(Blocker::OpenIncident {
            incident_id: incident.id.clone(),
            domain: incident.domain,
            status: incident.status,
            incident_kind: incident.kind.clone(),
        });
    }
    if run.status == RunStatus::Failed {
        return Some(Blocker::Failed {
            reason: run.error.clone().or_else(|| {
                run.hops
                    .last()
                    .filter(|hop| hop.status == HopStatus::Error)
                    .and_then(|hop| hop.error.clone())
            }),
        });
    }
    stale_for
        .filter(|_| run.status == RunStatus::Running)
        .map(|_| Blocker::Stale)
}

fn loop_layer(error: &str) -> Option<Layer> {
    match error.strip_prefix("loop_detected:")? {
        "torii" => Some(Layer::Torii),
        "satori" => Some(Layer::Satori),
        "enma" => Some(Layer::Enma),
        "yatagarasu" => Some(Layer::Yatagarasu),
        "fujin" => Some(Layer::Fujin),
        "daruma" => Some(Layer::Daruma),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// v1: task snapshots
// ---------------------------------------------------------------------------

/// Task status as the stall rules need it. The host maps its own statuses;
/// `Blocked` is for a task the host knows is blocked (daruma has no such
/// status — a blocking relation, say). Neither `Blocked` nor `Inbox` (the
/// triage queue) counts as a stall.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Inbox,
    Todo,
    InProgress,
    InReview,
    Blocked,
    Done,
    Cancelled,
}

/// A live claim on a task by an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimSnapshot {
    pub agent_id: String,
    pub expires_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub status: TaskStatus,
    /// When the task entered `status`; stall age is measured from here.
    pub status_since: Timestamp,
    pub claim: Option<ClaimSnapshot>,
    pub updated_at: Timestamp,
}

/// A task stalls strictly after these durations in its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskThresholds {
    pub in_progress: Milliseconds,
    pub in_review: Milliseconds,
    /// `todo`.
    pub other_open: Milliseconds,
}

const HOUR: Milliseconds = 60 * 60 * 1_000;

impl Default for TaskThresholds {
    /// 24 h in progress, a week in review and in todo.
    fn default() -> Self {
        Self {
            in_progress: 24 * HOUR,
            in_review: 168 * HOUR,
            other_open: 168 * HOUR,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStall {
    InProgressTooLong,
    InReviewTooLong,
    OpenTooLong,
    /// In progress while the claiming agent's claim has expired: the agent is gone.
    ClaimExpired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSignal {
    pub task_id: String,
    pub status: TaskStatus,
    /// Full time in the status, not just the excess over the threshold.
    pub stalled_for: Milliseconds,
    pub reason: TaskStall,
}

/// Stall signals for the tasks that need attention, without reading the clock.
///
/// Done, cancelled, blocked and inbox tasks never signal (blocked has its
/// reason, inbox is the triage queue).
/// A status is stalled strictly after its threshold. An in-progress task whose
/// claim expired before `now` signals `ClaimExpired` whatever its age, taking
/// precedence over `InProgressTooLong`. A `status_since` in the future (host
/// clock skew) never signals. Output: longest `stalled_for` first, then
/// `task_id`.
pub fn project_tasks(
    tasks: &[TaskSnapshot],
    now: Timestamp,
    thresholds: &TaskThresholds,
) -> Vec<TaskSignal> {
    let mut signals: Vec<TaskSignal> = tasks
        .iter()
        .filter_map(|task| {
            if task.status_since > now {
                return None;
            }
            let age = now.abs_diff(task.status_since);
            let reason = match task.status {
                TaskStatus::InProgress
                    if task.claim.as_ref().is_some_and(|c| c.expires_at < now) =>
                {
                    TaskStall::ClaimExpired
                }
                TaskStatus::InProgress if age > thresholds.in_progress => {
                    TaskStall::InProgressTooLong
                }
                TaskStatus::InReview if age > thresholds.in_review => TaskStall::InReviewTooLong,
                TaskStatus::Todo if age > thresholds.other_open => TaskStall::OpenTooLong,
                _ => return None,
            };
            Some(TaskSignal {
                task_id: task.task_id.clone(),
                status: task.status,
                stalled_for: age,
                reason,
            })
        })
        .collect();
    signals.sort_by(|a, b| {
        b.stalled_for
            .cmp(&a.stalled_for)
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
    signals
}

#[cfg(test)]
mod tests;
