# Office Rest & Socialize — design

**Date:** 2026-05-29
**Status:** approved (brainstorm), pre-plan
**Branch:** `worktree-office-rest-socialize`

## Problem

Idle agents read as *pacing* — they spend most of a wander cycle walking or
loitering, and when they do reach a rest spot they leave almost immediately. A
directed visit dwells only ~3–5 s (a fraction of the 7–13 s cycle), and a sofa
lounge looks identical in length to a vending-machine grab. The meeting room's
sofas are pure decor — nobody sits, nobody talks there.

User intent (verbatim distillation): people should **rest longer** at real
spots (desk / pantry / sofa), the **meeting room should be alive** with people
sitting and talking, sofas should hold **multiple characters facing each other**,
and **standing people should be able to talk** too.

## Goals

1. Per-spot dwell: a sofa lounge feels long; a vending grab feels quick.
2. Meeting room becomes a live, emergent group-talk venue (sit + stand).
3. Generalize chitchat from pairwise to N-way group conversation.
4. Net effect: less aimless pacing, more time at purposeful rest spots.

## Non-goals (YAGNI / deferred)

- **Summoned / scheduled meetings** — no coordinator that gathers N agents on a
  joint timeline. That is the coordinated-motion class that produced the
  teleport/replay regressions fixed in #62. The venue + chitchat seams are left
  clean so a v2 scheduler can layer on without rework.
- **New furniture art** — reuse `meeting_sofa`, the back-couch sprite, and the
  existing character sprites. No new `.sprite` assets.
- **Behavior change to existing pairwise waypoints** (pantry / couch / vending /
  printer) beyond the dwell retune.

## Architecture invariant respected

`pixtuoid-core` stays terminal-free (invariant #1). Dwell + slot geometry live
in core; render/facing/chitchat live in the tui crate.

---

## Component 1 — Per-spot dwell (`core::pose`)

Add `pub fn dwell_ms(kind: WaypointKind, agent_id: AgentId) -> u64`, returning a
per-spot base with per-agent jitter. Jitter draws from a splitmix64 bit-range
**disjoint** from the existing `speed_mult` (24..34), `pause_ms` (40..52), and
`cycle_ms` (>>16) ranges, so personality dimensions stay independent.

| Spot | Base dwell |
|---|---|
| Desk (between-trip Seated phase) | 15–30 s |
| Couch / MeetingSofa / MeetingStand | 20–40 s |
| Pantry | 10–18 s |
| PhoneBooth / StandingDesk | 8–30 s (task-length) |
| Vending / Printer | 4–8 s (grab) |

**Timeline change.** Today the wander timeline phases are fractions of
`cycle_ms` (`PHASE_SEATED_FRAC`, `PHASE_AT_WAYPOINT_FRAC`). The AtWaypoint and
Seated phases switch to **absolute** `dwell_ms`. Walk legs remain physics-driven
(`physics::walk_profile`). `wander_cycle_n` still increments once per completed
cycle, so deterministic destination selection (`takes_trip` / `is_aimless_cycle`
/ `waypoint_index_for_cycle`) is **unchanged**.

**Dual-timeline coordination (load-bearing).** The dwell change must be applied
in **both**:
- `tui::motion::advance_wander` — the rendering authority (stateful elastic
  timeline).
- `core::pose::idle_pose` — the stateless timeline that drives the occupancy
  overlay (`pixel_painter` builds the overlay from `core::pose::derive`).

If the two disagree on dwell, the overlay marks the agent as an obstacle at a
different time than the sprite is actually there → a new leg can re-route onto a
different shape → the re-route flash class fixed in #62. Both consume the same
`dwell_ms` to stay coherent. `PHASE_SEATED_FRAC` / `PHASE_AT_WAYPOINT_FRAC` are
removed or demoted accordingly; `PHASE_WALK_OUT_FRAC` is no longer meaningful
(walk legs are already physics-timed) and is removed if unused after the switch.

---

## Component 2 — Meeting room as a live venue (`core::layout`)

Two new `WaypointKind` variants in `layout::decor`:
- `MeetingSofa` — seated on a meeting-room sofa.
- `MeetingStand` — standing beside the meeting table.

`layout::compute` pushes meeting **slots** into `layout.waypoints` (the existing
`Vec<Waypoint>`):
- Per meeting sofa: 1–2 seat slots, offset along the sofa's 16 px width so two
  sitters don't overlap (a single sofa on a narrow floor may yield one slot).
- Per meeting table: 2 standing slots flanking it (east/west of center).

Each slot carries posture (seated/standing) via its kind and **facing** derived
from its position relative to the room center: north-side → faces south, etc.
Because slots are ordinary `Waypoint`s, the existing
`waypoint_index_for_cycle` naturally spreads idle agents across them — **emergent
group formation, no coordinator**. Slot choice is deterministic per
`(agent, cycle)`; rare brief collisions (two agents pick the same slot) are
accepted — they resolve as agents leave on their own clocks.

Slots only exist when a meeting room exists (gated on `layout.meeting_room`,
already conditional on floor size). Dense floors with a second meeting room get
a second slot set, same as the existing decor sofas.

---

## Component 3 — Facing + render (`tui::pixel_painter`)

`anchors.rs` and `drawable.rs` gain `Pose::AtWaypoint { kind: MeetingSofa | MeetingStand }`
arms:
- **MeetingSofa**: seat anchor on the sofa sprite (reuse the back-couch sprite
  for north-side slots so the back faces the table; front-view seated for
  south-side). The sofa itself is already painted as decor — sitters y-sort on
  top.
- **MeetingStand**: standing anchor beside the table; `flip_x` toward table
  center so the agent faces inward.

Facing for left/right is the existing `flip_x`; north/south is sprite choice
(back-couch vs front-seated). No new art.

---

## Component 4 — Group chitchat (`tui::chitchat`)

Generalize `ActiveChitchat` from a fixed `(agent_a, agent_b)` pair to a
**venue-keyed group**:
- Replace the two agent fields with `participants: Vec<AgentId>` (stable-sorted
  by raw id for deterministic speaker order).
- Round-robin speaker: `speaker = participants[(elapsed / TURN_MS) % n]`.
- `venue_key`: for meeting-room slots, the **room id** (all slots in one room
  share it → one group conversation per room). For existing single-point social
  waypoints, the waypoint point (physically ~1–2 agents → stays effectively
  pairwise; behavior unchanged).
- A conversation lives while ≥2 participants are present at the venue;
  participants are recomputed each frame as agents arrive/leave on their own
  clocks (join/leave mid-conversation is supported).

`supports_chitchat` extended to include `MeetingSofa` and `MeetingStand`.
Standing slots share the room venue, so standing agents talk too — satisfying
"people standing can also talk."

Bubbles anchor over the **current speaker's** rendered anchor each frame.

---

## Component 5 — Motion safety (emergent guarantees)

No joint timeline, no coordinator. Each agent routes independently using the
existing frozen-leg-path snapshot + stale-resume protections from #62 —
unchanged.

**Long-dwell × stale-resume interaction (verified safe):** stale-resume in
`advance_wander` fires only when `now - last_advanced_at > cycle_ms_for(id)`.
On-screen, `advance_wander` runs every frame (~33 ms), so the gap stays ~33 ms
regardless of how long a dwell is — a 40 s lounge never trips it. Stale-resume
still trips only for **off-screen** floors (frozen `now`), where the existing
desk-reset behavior applies and is acceptable. No conflict.

---

## Testing (TDD)

**core** (`crates/pixtuoid-core/tests/` + `pose.rs` unit):
- `dwell_ms` returns values in the documented per-kind ranges; jitter bit-range
  is disjoint from speed/pause/cycle (no aliasing).
- `idle_pose` holds an agent `AtWaypoint` for the full `dwell_ms` window, then
  transitions to walk-back.
- `wander_cycle_n` / destination selection is byte-identical before/after the
  dwell change (dwell does not perturb which cycle visits which spot).
- Meeting slots appear in `layout.waypoints` across all office geometries that
  have a meeting room; absent when no meeting room; doubled for dual-meeting
  floors.

**motion** (`tui::motion` unit + `tui::pose` continuity):
- AtWaypoint phase duration equals `dwell_ms(kind)` (not a cycle fraction).
- On-screen long dwell (40 s simulated at 33 ms frames) never triggers
  stale-resume.
- Meeting-slot arrival is continuous (frozen-path sweep, ≤20 px/frame).
- Multiple agents converging on one room don't teleport (multi-agent overlay
  sweep, 20 px threshold) — mutation-verify by reverting the freeze.

**chitchat** (`tui::chitchat` unit):
- N=3 and N=4 round-robin cycles through all participants in id order.
- Participant join mid-conversation extends the rotation; leave (down to 1)
  ends the conversation.
- Venue grouping: two meeting slots in the same room merge into one
  conversation; two distinct single-point waypoints do not merge.
- `supports_chitchat` true for the two new kinds.

**harness** (`tui::tui_renderer` headless):
- A populated meeting room renders sitters + standers + at least one bubble.
- Facing: north-side slot paints the back-couch silhouette, south-side paints
  front-seated (assert via distinguishing pixels).

## Risks

- **Dual-timeline drift** (Component 1) is the highest-risk area — guarded by the
  shared `dwell_ms` and the continuity sweeps. This is the same failure class as
  #62 Bug 1/3; the test methodology carries over directly.
- **Slot collision** (two agents pick the same slot) is visually minor and
  self-resolving; not worth stateful free-slot allocation in v1.
- **Sofa width vs sitter width** — 16 px sofa, ~12 px sitter; two offset slots
  may slightly overlap. Verify visually via snapshot; fall back to one slot per
  sofa + standing slots if it reads poorly.

## Out of scope / future (v2 seams)

- Summoned/scheduled meetings layered on the venue + group-chitchat seams.
- Per-conversation topic threading (current bubbles stay the existing dev-humor
  one-liner pool).
