//! Issue #2798 — Layer B: the dashboard reader-seam regression for the
//! always-empty "Creative Ideas" tab.
//!
//! Root cause (D1): the dashboard resolved its state root with a private copy of
//! the resolver that took `$SIMARD_STATE_ROOT` verbatim (and hardcoded a
//! `/home/azureuser` fallback), while the OODA daemon — which registers the
//! in-process writer and persists ideas — resolves via the canonical
//! `crate::state_root::simard_state_root()`, which validates the env (empty /
//! relative / NUL fall back to `~/.simard`). When they disagree,
//! `open_reader_client` tier-0 (`lookup_in_process_writer`) never matches the
//! daemon's key, so the dashboard reads a different store and the tab stays
//! empty. These tests pin the resolvers equal for every `SIMARD_STATE_ROOT`
//! input class; the empty/relative cases fail RED pre-fix. Layer A (engine
//! read-after-write) and Layer C (durability) live in
//! `crate::cognitive_memory::tests_library_parity`.
//!
//! Isolation: these mutate the process-global `SIMARD_STATE_ROOT` and register
//! the in-process writer, so they run serial under `cognitive_memory` (and share
//! `simard_state_root_env` with the `state_root` env tests) and use
//! `HermeticState`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use super::routes::resolve_state_root;
use crate::cognitive_memory::creative_idea::{
    CreativeIdea, CreativeIdeaStore, IdeaContext, ProspectiveCreativeIdeaStore,
};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::memory_ipc::{
    clear_in_process_writer, default_state_root, open_reader_client, register_in_process_writer,
};
use crate::state_root::{STATE_ROOT_ENV, simard_state_root};
use crate::test_support::HermeticState;

/// Scoped `SIMARD_STATE_ROOT` override that restores the prior value on drop.
/// Env access is process-global; every test using it is in the
/// `simard_state_root_env` + `cognitive_memory` serial groups so the mutation
/// cannot race a parallel reader.
struct StateRootEnvGuard {
    prev: Option<OsString>,
}

impl StateRootEnvGuard {
    fn set(value: &str) -> Self {
        let prev = std::env::var_os(STATE_ROOT_ENV);
        // SAFETY: serialised via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe {
            std::env::set_var(STATE_ROOT_ENV, value);
        }
        Self { prev }
    }
}

impl Drop for StateRootEnvGuard {
    fn drop(&mut self) {
        // SAFETY: serialised via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(STATE_ROOT_ENV, v),
                None => std::env::remove_var(STATE_ROOT_ENV),
            }
        }
    }
}

/// The core invariant: the dashboard reader and the daemon writer MUST resolve
/// the identical state root, otherwise tier-0 misses and the tab is empty.
/// `default_state_root()` delegates to `simard_state_root()`, so both are pinned.
fn assert_resolvers_agree(context: &str) {
    let dashboard = resolve_state_root();
    let daemon = simard_state_root();
    assert_eq!(
        dashboard, daemon,
        "dashboard resolve_state_root() must equal daemon simard_state_root() \
         ({context}); a divergence is the read-after-write miss behind the \
         always-empty Creative Ideas tab (#2798) — dashboard={dashboard:?}, \
         daemon={daemon:?}"
    );
    assert_eq!(
        dashboard,
        default_state_root(),
        "dashboard resolver must also equal memory_ipc::default_state_root() \
         ({context}), the key the daemon registers its in-process writer under"
    );
}

/// An **absolute** `SIMARD_STATE_ROOT` is honored identically by both resolvers.
/// (Control: this class already agreed before the fix.)
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn resolvers_agree_on_absolute_env() {
    let _g = StateRootEnvGuard::set("/tmp/simard-2798-abs-state-root");
    assert_eq!(
        resolve_state_root(),
        PathBuf::from("/tmp/simard-2798-abs-state-root"),
        "an absolute SIMARD_STATE_ROOT is used verbatim by both resolvers"
    );
    assert_resolvers_agree("absolute SIMARD_STATE_ROOT");
}

/// An **empty** `SIMARD_STATE_ROOT` must fall back to `~/.simard` in BOTH
/// resolvers. RED on the unpatched dashboard, which returned `PathBuf::from("")`
/// while the daemon fell back to `~/.simard` — the exact divergence that emptied
/// the tab.
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn resolvers_agree_on_empty_env() {
    let _g = StateRootEnvGuard::set("");
    let dashboard = resolve_state_root();
    assert_ne!(
        dashboard.as_os_str(),
        "",
        "an empty SIMARD_STATE_ROOT must NOT resolve to the empty path — it must \
         fall back to ~/.simard exactly as the daemon does (#2798 D1)"
    );
    assert_resolvers_agree("empty SIMARD_STATE_ROOT");
}

/// A **relative** `SIMARD_STATE_ROOT` must be rejected and fall back to
/// `~/.simard` in BOTH resolvers. RED on the unpatched dashboard, which returned
/// the relative path verbatim while the daemon fell back to `~/.simard`.
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn resolvers_agree_on_relative_env() {
    let _g = StateRootEnvGuard::set("relative/state/root");
    let dashboard = resolve_state_root();
    assert!(
        dashboard.is_absolute(),
        "a relative SIMARD_STATE_ROOT must be rejected and fall back to an \
         absolute ~/.simard, not used verbatim (#2798 D1); got {dashboard:?}"
    );
    assert!(
        !dashboard.ends_with("relative/state/root"),
        "the relative value must not leak through the dashboard resolver; got {dashboard:?}"
    );
    assert_resolvers_agree("relative SIMARD_STATE_ROOT");
}

/// With `SIMARD_STATE_ROOT` **unset**, both resolvers fall back to `~/.simard`.
/// (Control: this class already agreed before the fix when `HOME` is set.)
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn resolvers_agree_on_unset_env() {
    let prev = std::env::var_os(STATE_ROOT_ENV);
    // SAFETY: serialised via the group keys above.
    unsafe {
        std::env::remove_var(STATE_ROOT_ENV);
    }
    let result = std::panic::catch_unwind(|| {
        let dashboard = resolve_state_root();
        assert!(
            dashboard.ends_with(".simard"),
            "unset SIMARD_STATE_ROOT must fall back to ~/.simard; got {dashboard:?}"
        );
        assert_resolvers_agree("unset SIMARD_STATE_ROOT");
    });
    // Restore before propagating any panic.
    unsafe {
        match prev {
            Some(v) => std::env::set_var(STATE_ROOT_ENV, v),
            None => std::env::remove_var(STATE_ROOT_ENV),
        }
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

// ---------------------------------------------------------------------------
// Reader-seam read-after-write (deliverable 1a).
// ---------------------------------------------------------------------------

/// RAII: register an in-process writer at `state_root` (as the daemon does at
/// startup) and clear it on drop.
struct WriterReg {
    writer: Arc<dyn CognitiveMemoryOps>,
}

impl WriterReg {
    fn register(state: &HermeticState) -> Self {
        let writer: Arc<dyn CognitiveMemoryOps> =
            Arc::new(LibraryCognitiveMemory::open(state.state_root()).expect("open store"));
        register_in_process_writer(state.state_root().to_path_buf(), Arc::clone(&writer));
        Self { writer }
    }

    fn ops(&self) -> &dyn CognitiveMemoryOps {
        self.writer.as_ref()
    }
}

impl Drop for WriterReg {
    fn drop(&mut self) {
        clear_in_process_writer();
    }
}

fn seed_idea(ops: &dyn CognitiveMemoryOps, text: &str) {
    let store = ProspectiveCreativeIdeaStore::new(ops);
    let idea = CreativeIdea::new(
        text,
        IdeaContext {
            source: "creative-ideas-thread".to_string(),
            goals_snapshot: vec![],
            observation_digest: "digest".to_string(),
            rationale: "recall precision plateaued".to_string(),
        },
        1,
    );
    store.store(&idea).expect("store creative idea");
}

/// **Reader-seam read-after-write (deliverable 1a).** A creative idea persisted
/// through the registered in-process writer must be visible to a *fresh*
/// `open_reader_client(resolve_state_root())` — the exact path the dashboard's
/// `load_ideas` takes. Because the dashboard resolver now equals the daemon's
/// registration key, tier-0 shares the live writer handle and the read-after-write
/// holds end-to-end. A regression that re-diverges the resolver (D1) or drops the
/// tier-0 share would empty this list.
#[test]
#[serial_test::serial(cognitive_memory)]
fn creative_idea_read_after_write_through_reader_seam() {
    let state = HermeticState::new();
    let reg = WriterReg::register(&state);

    seed_idea(reg.ops(), "improve recall ranking");
    seed_idea(reg.ops(), "auto-delete stale worktrees");

    // Read exactly as the dashboard does: resolve the state root, open a fresh
    // reader client, list via the same store seam.
    let reader = open_reader_client(&resolve_state_root()).expect("open reader client");
    let ideas = ProspectiveCreativeIdeaStore::new(reader.ops())
        .list(u32::MAX)
        .expect("list creative ideas through reader seam");

    assert_eq!(
        ideas.len(),
        2,
        "both persisted creative ideas must be visible to a fresh reader opened \
         via resolve_state_root() (read-after-write across the reader seam, #2798)"
    );
    assert!(
        ideas.iter().any(|i| i.idea == "improve recall ranking"),
        "the persisted idea text must round-trip through the reader seam"
    );
}
