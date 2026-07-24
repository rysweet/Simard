//! Concurrency tests for `dispatch_actions` / `dispatch_actions_bounded`.
//!
//! These prove the core guarantees of the concurrent `AdvanceGoal` dispatch:
//! (a) multiple spawn-path actions run their slow `run_turn` calls in parallel
//! (wall-time far below the serialized sum), (b) the same goal is claimed by
//! exactly one thread (no double-spawn), (c) the AIMD `max_concurrency` cap is
//! never exceeded, and (d) one failing action surfaces its own error outcome
//! without aborting the others.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::base_types::{BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput};
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{GoalBoard, GoalProgress, add_active_goal};
use crate::ooda_actions::dispatch_actions_bounded;
use crate::ooda_actions::test_helpers::{active_goal, test_memories};
use crate::ooda_loop::{ActionKind, OodaState, OrchestratorSessionFactory, PlannedAction};

/// Shared instrumentation across every session a factory mints, so tests can
/// observe concurrency (peak simultaneous `run_turn`s) and total invocations.
#[derive(Default)]
struct Instrumentation {
    run_count: AtomicUsize,
    live: AtomicUsize,
    peak: AtomicUsize,
}

/// A fake [`BaseTypeSession`] whose `run_turn` sleeps (to create observable
/// concurrency), records peak overlap, and returns a configurable response —
/// or an error when the objective contains `fail_substring`.
struct FakeSession {
    instr: Arc<Instrumentation>,
    sleep: Duration,
    response: String,
    fail_substring: Option<String>,
}

// FakeSession is `Send` automatically (Arc<atomics> + owned fields); the
// trait only requires `Send`.
impl BaseTypeSession for FakeSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        unimplemented!("descriptor is not used by the advance-goal dispatch path")
    }

    fn open(&mut self) -> SimardResult<()> {
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        self.instr.run_count.fetch_add(1, Ordering::SeqCst);
        let now = self.instr.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.instr.peak.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.sleep);
        self.instr.live.fetch_sub(1, Ordering::SeqCst);

        if let Some(sub) = &self.fail_substring
            && input.objective.contains(sub.as_str())
        {
            return Err(SimardError::RpcTransportError {
                endpoint: "fake-session".to_string(),
                reason: format!("injected failure for objective containing '{sub}'"),
            });
        }
        Ok(BaseTypeOutcome {
            plan: String::new(),
            execution_summary: self.response.clone(),
            evidence: vec![],
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        Ok(())
    }
}

/// Mints independent [`FakeSession`]s, all sharing one [`Instrumentation`].
struct FakeFactory {
    instr: Arc<Instrumentation>,
    sleep: Duration,
    response: String,
    fail_substring: Option<String>,
}

impl OrchestratorSessionFactory for FakeFactory {
    fn open_session(&self) -> SimardResult<Box<dyn BaseTypeSession>> {
        Ok(Box::new(FakeSession {
            instr: Arc::clone(&self.instr),
            sleep: self.sleep,
            response: self.response.clone(),
            fail_substring: self.fail_substring.clone(),
        }))
    }
}

fn board_with_unassigned_goals(ids: &[&str]) -> GoalBoard {
    let mut board = GoalBoard::new();
    for id in ids {
        let mut g = active_goal(id);
        g.status = GoalProgress::NotStarted;
        g.assigned_to = None;
        add_active_goal(&mut board, g).unwrap();
    }
    board
}

fn advance_action(goal_id: &str) -> PlannedAction {
    PlannedAction {
        kind: ActionKind::AdvanceGoal,
        goal_id: Some(goal_id.to_string()),
        description: format!("advance {goal_id}"),
    }
}

// ── (a) + (c): parallelism and the AIMD cap ─────────────────────────────────

#[test]
fn concurrent_dispatch_parallelizes_and_respects_cap() {
    let ids = ["adv-t-0", "adv-t-1", "adv-t-2", "adv-t-3"];
    let actions: Vec<PlannedAction> = ids.iter().map(|id| advance_action(id)).collect();
    let sleep = Duration::from_millis(200);

    let instr = Arc::new(Instrumentation::default());
    let mut memories = test_memories();
    memories.session_factory = Some(Arc::new(FakeFactory {
        instr: Arc::clone(&instr),
        sleep,
        response: "NO ACTION\nREASON: concurrency test no-op".to_string(),
        fail_substring: None,
    }));
    let mut state = OodaState::new(board_with_unassigned_goals(&ids));

    // Run 1: cap = N → all dispatch concurrently.
    let t0 = Instant::now();
    let outcomes =
        dispatch_actions_bounded(&actions, &mut memories, &mut state, ids.len()).unwrap();
    let parallel_elapsed = t0.elapsed();

    assert_eq!(outcomes.len(), ids.len());
    for o in &outcomes {
        assert!(o.success, "NO ACTION dispatch should succeed: {}", o.detail);
    }
    assert_eq!(
        instr.run_count.load(Ordering::SeqCst),
        ids.len(),
        "each goal's run_turn must be invoked exactly once"
    );
    let peak_parallel = instr.peak.load(Ordering::SeqCst);
    assert!(
        peak_parallel >= 2,
        "with cap=N the slow run_turn calls must overlap; peak={peak_parallel}"
    );

    // NO ACTION leaves goals NotStarted + unassigned, so the same state is
    // reusable for a serialized run.
    instr.run_count.store(0, Ordering::SeqCst);
    instr.peak.store(0, Ordering::SeqCst);
    instr.live.store(0, Ordering::SeqCst);

    // Run 2: cap = 1 → dispatch serialized (peak must never exceed 1).
    let t1 = Instant::now();
    let _ = dispatch_actions_bounded(&actions, &mut memories, &mut state, 1).unwrap();
    let serial_elapsed = t1.elapsed();

    let peak_serial = instr.peak.load(Ordering::SeqCst);
    assert!(
        peak_serial <= 1,
        "cap=1 must serialize dispatch; peak={peak_serial}"
    );

    // Deflake #4560: the real concurrency guarantee is carried by the
    // deterministic logical assertions above (peak_parallel >= 2, peak_serial
    // <= 1). The wall-clock comparison is kept only as a weak, oversubscription-
    // tolerant sanity bound: under full-parallel canary CPU oversubscription
    // (observed load avg 73–158) a strict >=2x speedup flakes, so we merely
    // require the parallel run not to be slower than the serialized run.
    assert!(
        parallel_elapsed <= serial_elapsed,
        "concurrent dispatch must not be slower than serialized: parallel={parallel_elapsed:?}, serial={serial_elapsed:?}"
    );
}

// ── (b): atomic claim — the same goal is run/claimed exactly once ───────────

#[test]
fn same_goal_claimed_once_no_double_spawn() {
    // Two AdvanceGoal actions targeting the SAME unassigned goal.
    let actions = vec![advance_action("dup-goal"), advance_action("dup-goal")];

    let instr = Arc::new(Instrumentation::default());
    let mut memories = test_memories();
    memories.session_factory = Some(Arc::new(FakeFactory {
        instr: Arc::clone(&instr),
        sleep: Duration::from_millis(150),
        response: "NO ACTION\nREASON: duplicate-claim test no-op".to_string(),
        fail_substring: None,
    }));
    let mut state = OodaState::new(board_with_unassigned_goals(&["dup-goal"]));

    let outcomes =
        dispatch_actions_bounded(&actions, &mut memories, &mut state, actions.len()).unwrap();

    assert_eq!(outcomes.len(), 2);
    // Exactly one thread won the claim and ran the turn; the other skipped
    // before opening a session — so run_turn fired exactly once. This is the
    // guarantee that prevents double-spawning the same goal.
    assert_eq!(
        instr.run_count.load(Ordering::SeqCst),
        1,
        "the claim must let exactly one dispatch run the goal-action turn"
    );
    // Exactly one outcome reports the concurrent-claim skip.
    let skips = outcomes
        .iter()
        .filter(|o| {
            o.detail
                .contains("already claimed by a concurrent dispatch")
        })
        .count();
    assert_eq!(
        skips, 1,
        "exactly one action must observe the goal as claimed"
    );
}

// ── (d): one failing action does not abort the others ───────────────────────

#[test]
fn one_failing_advance_does_not_abort_others() {
    let ids = ["adv-ok-a", "adv-fail-b", "adv-ok-c"];
    let actions: Vec<PlannedAction> = ids.iter().map(|id| advance_action(id)).collect();

    let instr = Arc::new(Instrumentation::default());
    let mut memories = test_memories();
    memories.session_factory = Some(Arc::new(FakeFactory {
        instr: Arc::clone(&instr),
        sleep: Duration::from_millis(50),
        response: "NO ACTION\nREASON: failure-isolation test no-op".to_string(),
        // Only the "adv-fail-b" goal's objective contains "fail-b".
        fail_substring: Some("fail-b".to_string()),
    }));
    let mut state = OodaState::new(board_with_unassigned_goals(&ids));

    let outcomes =
        dispatch_actions_bounded(&actions, &mut memories, &mut state, ids.len()).unwrap();

    assert_eq!(outcomes.len(), 3, "one outcome per input action, in order");
    assert!(
        outcomes[0].success,
        "ok-a should succeed: {}",
        outcomes[0].detail
    );
    assert!(
        !outcomes[1].success,
        "fail-b should fail with its own error outcome: {}",
        outcomes[1].detail
    );
    assert!(
        outcomes[1].detail.contains("run_turn failed"),
        "failure detail should describe the run_turn error: {}",
        outcomes[1].detail
    );
    assert!(
        outcomes[2].success,
        "ok-c should succeed: {}",
        outcomes[2].detail
    );
    // Order is preserved: each outcome maps back to its input action's goal.
    assert_eq!(outcomes[0].action.goal_id.as_deref(), Some("adv-ok-a"));
    assert_eq!(outcomes[1].action.goal_id.as_deref(), Some("adv-fail-b"));
    assert_eq!(outcomes[2].action.goal_id.as_deref(), Some("adv-ok-c"));
}

// ── Deflake #4560a: oversubscription-tolerant logical concurrency contract ───
//
// The self-deploy canary gate flaked because `concurrent_dispatch_parallelizes_
// and_respects_cap` asserted a wall-clock speedup ratio (`parallel*2 <= serial`)
// that does not hold when full-parallel canaries oversubscribe the CPU (observed
// load avg 73–158). The REAL guarantee dispatch must provide is LOGICAL, not a
// wall-clock ratio: with cap=N the slow `run_turn`s overlap (peak >= 2), with
// cap=1 they serialize (peak <= 1), and each goal's `run_turn` fires exactly
// once. This test pins that logical contract while actively inducing CPU
// oversubscription, so it stays deterministic regardless of scheduler pressure.

/// Spins up CPU-hog threads to simulate the full-parallel canary
/// oversubscription under which wall-clock assertions flake. The hogs stop and
/// join when the guard is dropped.
struct CpuHogs {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl CpuHogs {
    fn spawn() -> Self {
        let count = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handles = (0..count)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Self { stop, handles }
    }
}

impl Drop for CpuHogs {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

#[test]
fn concurrent_dispatch_logical_guarantees_hold_under_oversubscription() {
    let _hogs = CpuHogs::spawn();

    let ids = ["over-0", "over-1", "over-2", "over-3"];
    let actions: Vec<PlannedAction> = ids.iter().map(|id| advance_action(id)).collect();

    let instr = Arc::new(Instrumentation::default());
    let mut memories = test_memories();
    memories.session_factory = Some(Arc::new(FakeFactory {
        instr: Arc::clone(&instr),
        sleep: Duration::from_millis(200),
        response: "NO ACTION\nREASON: oversubscription logical test no-op".to_string(),
        fail_substring: None,
    }));
    let mut state = OodaState::new(board_with_unassigned_goals(&ids));

    // cap = N → the slow run_turns must overlap (logical parallelism), even
    // when every core is already saturated by the hog threads.
    let outcomes =
        dispatch_actions_bounded(&actions, &mut memories, &mut state, ids.len()).unwrap();
    assert_eq!(outcomes.len(), ids.len());
    for o in &outcomes {
        assert!(o.success, "NO ACTION dispatch should succeed: {}", o.detail);
    }
    assert_eq!(
        instr.run_count.load(Ordering::SeqCst),
        ids.len(),
        "each goal's run_turn must be invoked exactly once under cap=N"
    );
    let peak_parallel = instr.peak.load(Ordering::SeqCst);
    assert!(
        peak_parallel >= 2,
        "cap=N must overlap slow run_turns even under oversubscription; peak={peak_parallel}"
    );

    // NO ACTION leaves goals NotStarted + unassigned, so the same state is
    // reusable for a serialized run.
    instr.run_count.store(0, Ordering::SeqCst);
    instr.peak.store(0, Ordering::SeqCst);
    instr.live.store(0, Ordering::SeqCst);

    // cap = 1 → dispatch must serialize regardless of load (peak never > 1).
    let _ = dispatch_actions_bounded(&actions, &mut memories, &mut state, 1).unwrap();
    let peak_serial = instr.peak.load(Ordering::SeqCst);
    assert!(
        peak_serial <= 1,
        "cap=1 must serialize dispatch regardless of load; peak={peak_serial}"
    );
    assert_eq!(
        instr.run_count.load(Ordering::SeqCst),
        ids.len(),
        "cap=1 still runs every goal exactly once"
    );
}
