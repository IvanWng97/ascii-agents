//! The agent **scope** layer (Layer B) — the parent↔subagent tree and the
//! lifecycle rules that propagate along it.
//!
//! The reducer runs two stacked state machines: the per-agent FSM (Layer A —
//! `Idle / Active / Waiting` plus the exit + debounce lifecycle, in
//! [`super::reducer`]) and this **scope** layer over `AgentSlot.parent_id`. The
//! scope encodes one invariant — *a subagent's lifetime is contained in its
//! parent's* (structured concurrency / an OTP-style supervision tree) — and
//! expresses it as a few directional operations the reducer delegates to.
//!
//! Housing them here gives the containment invariant a single home: a new
//! lifecycle concern becomes a function in this module rather than yet another
//! bespoke `parent_id` walk bolted onto the reducer (which is exactly how this
//! logic accreted before — cascade, then liveness, then readiness, then
//! completion, each a separate reactive scan).
//!
//! - **exit flows DOWN** — [`cascade_exit`]: a node leaving takes its whole
//!   subtree. Used by `SessionEnd`, the stale-sweep, and subagent-completion.
//! - **liveness flows UP** — [`refresh_lineage`]: a working descendant keeps its
//!   ancestors alive, so a blocked-but-delegating parent isn't stale-swept.
//! - **readiness, queried UP** — [`has_waiting_ancestor`]: a node blocked under a
//!   `Waiting` ancestor is "not ready", not dead (liveness vs readiness, k8s-style).

use std::collections::{BTreeMap, HashSet};
use std::time::SystemTime;

use crate::state::{ActivityState, AgentSlot, SceneState};
use crate::AgentId;

/// Mark every not-yet-exiting descendant of `root` exiting, BFS over `parent_id`
/// links (exit flows DOWN). `root` is only the BFS seed and is never re-stamped
/// *by this function* — whether the caller marks `root` itself is caller-specific:
/// the `SessionEnd` arm and `sweep_stale` stamp it first (the whole subtree
/// leaves together), while subagent-completion does NOT (the parent keeps
/// running; only its subtree leaves). Idempotent: slots already exiting are
/// filtered out, so a leaf or a partly-exiting subtree is a safe no-op.
pub(crate) fn cascade_exit(scene: &mut SceneState, root: AgentId, now: SystemTime) {
    let mut visited: HashSet<AgentId> = HashSet::new();
    visited.insert(root);
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        let children: Vec<AgentId> = scene
            .agents
            .values()
            .filter(|s| s.parent_id == Some(parent) && s.exiting_at.is_none())
            .map(|s| s.agent_id)
            .collect();
        for cid in children {
            if visited.insert(cid) {
                if let Some(slot) = scene.agents.get_mut(&cid) {
                    slot.exiting_at = Some(now);
                }
                frontier.push(cid);
            }
        }
    }
}

/// Refresh `last_event_at` for `id` and every ancestor (liveness flows UP), so a
/// parent (and grandparent) isn't stale-swept while a descendant is still
/// emitting events — even if the parent's own hooks dropped or a subagent's hook
/// was misattributed to it. The mirror of [`cascade_exit`]. Cycle-guarded;
/// `last_event_at` only gates the stale-sweep, so this never alters an ancestor's
/// visible state/pose.
pub(crate) fn refresh_lineage(scene: &mut SceneState, id: AgentId, now: SystemTime) {
    let mut visited: HashSet<AgentId> = HashSet::new();
    let mut cur = Some(id);
    while let Some(aid) = cur {
        if !visited.insert(aid) {
            break;
        }
        match scene.agents.get_mut(&aid) {
            Some(slot) => {
                slot.last_event_at = now;
                cur = slot.parent_id;
            }
            None => break,
        }
    }
}

/// True if any ancestor of `id` (walking `parent_id`) is in `Waiting` state. A
/// subagent's permission `Notification` is attributed to the PARENT (the hook
/// `transcript_path` is the parent's), so the parent goes `Waiting` while the
/// blocked subagent stays `Active`. Such a subagent is paused on a human gate the
/// ancestor holds — "not ready", not dead — so `sweep_stale` exempts it from the
/// aggressive Active timer (liveness vs readiness). Cycle-guarded; the chain is
/// shallow in practice. Takes `&BTreeMap` rather than `&SceneState` (unlike its
/// siblings) so it can be called inside `sweep_stale`'s pass-1 closure while
/// `&scene.agents` is already borrowed immutably — `&SceneState` would conflict
/// with that live borrow.
pub(crate) fn has_waiting_ancestor(agents: &BTreeMap<AgentId, AgentSlot>, id: AgentId) -> bool {
    let mut visited: HashSet<AgentId> = HashSet::new();
    let mut cur = agents.get(&id).and_then(|s| s.parent_id);
    while let Some(pid) = cur {
        if !visited.insert(pid) {
            break;
        }
        match agents.get(&pid) {
            Some(p) if matches!(p.state, ActivityState::Waiting { .. }) => return true,
            Some(p) => cur = p.parent_id,
            None => break,
        }
    }
    false
}
