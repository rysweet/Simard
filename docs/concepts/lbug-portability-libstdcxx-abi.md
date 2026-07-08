---
title: lbug portability across libstdc++ ABIs — the std::format SIGSEGV and its fix
description: Root cause and fix for the cognitive-memory SIGSEGV seen on newer Linux hosts — a prebuilt lbug (LadybugDB) binary compiled against an older libstdc++ std::format ABI crashes in std::vformat during Database::initBufferManager when linked into a binary running on a host with a newer libstdc++. The fix forces lbug to build from source against the host toolchain, lands in amplihack-memory-lib, and is verified by the installer's ABI probe.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ./platform-installer.md
  - ../howto/run-the-installer-preflight-doctor.md
  - ../howto/install-a-simard-family-agent.md
  - ../reference/platform-installer-cli.md
  - ../memory.md
---

# lbug portability across libstdc++ ABIs — the std::format SIGSEGV and its fix

## Summary

On host `dev` (Ubuntu 26.04 LTS, g++ 15.2.0, libstdc++ exposing
`GLIBCXX_3.4.35`), the Simard/Crocutus binary **segfaulted during
cognitive-memory initialization**. The same binary source runs cleanly on host
`ia2` (Ubuntu 25.10, libstdc++ 3.4.34). The crash is a **C++ standard-library
ABI mismatch**: a *prebuilt* lbug (LadybugDB) artifact compiled against an older
libstdc++'s `std::format` machinery is loaded into a process running on a host
with a **newer** libstdc++, and the two disagree about the `std::format` ABI.
The fix is to stop consuming the prebuilt on such hosts and **build lbug from
source against the host's own toolchain**, so there is exactly one libstdc++ ABI
in play. The fix lands in **`amplihack-memory-lib`** (the engine library),
Simard consumes it by bumping its pin, and the
[platform installer](./platform-installer.md) verifies it with an ABI probe.

## The observed crash

Simard's persistent cognitive memory is the embedded **lbug** engine (LadybugDB,
the `lbug = "=0.17.1"` crate), compiled and statically linked into the binary
through `amplihack-memory-lib`'s `persistent` feature. Opening the store walks
this path:

```
crocutus::main
  └─ simard::…run_ooda_daemon
       └─ LibraryCognitiveMemory::open
            └─ amplihack_memory::graph::lbug_store::open_database
                 └─ lbug::main::Database::initBufferManager
                      └─ std::vformat / std::__format::__formatter_str   ← SIGSEGV
```

The fault is inside `std::vformat` — C++20 `std::format` — during buffer-manager
initialization. It is deterministic on 26.04 and absent on 25.10.

## Root cause: two libstdc++ ABIs in one process

`std::format` was added in C++20 and its libstdc++ implementation details
(the `std::__format` internals, formatter symbols, and the layout of the types
passed through `std::vformat`) **changed across libstdc++ releases**. libstdc++
tags these with `GLIBCXX_3.4.NN` version nodes; Ubuntu 25.10 ships `3.4.34` and
26.04 ships `3.4.35`.

The published `lbug` crate's `build.rs` **prefers a prebuilt binary artifact by
default** (in static link mode) rather than compiling the C++ engine locally. It
selects and caches one via a set of environment knobs, and — critically — honors
a switch that forces a source build instead:

| Env knob | Role |
|----------|------|
| `LBUG_BUILD_FROM_SOURCE` (alias `LBUG_RUST_BUILD_FROM_SOURCE`) | **The fix's lever.** When set, `build.rs` skips the prebuilt ("Skipping prebuilt liblbug because source build was requested") and falls through to the bundled cmake build. |
| `LBUG_PRECOMPILED_RUN_ID` | Which prebuilt CI run to fetch |
| `LBUG_VERSION` | Engine version/tag to resolve |
| `LBUG_GITHUB_REPOSITORY` | Where prebuilts are published (default `LadybugDB/ladybug`) |
| `LBUG_LINUX_VARIANT` | Which Linux/libstdc++ variant of the prebuilt to use |
| `LBUG_LIB_KIND` | Static vs shared artifact |
| `LBUG_SHARED` | Force the shared-library link mode |

with the artifact cached under `PREBUILT_CACHE_DIR=".cache/lbug-prebuilt"`. The
prebuilt download path applies only under static link mode; all of these knob
names and the cache path are the crate's real behavior (verified against
`lbug-0.17.1/build.rs`), not invented for this document.

That prebuilt is compiled on **some** builder image, against **that image's**
libstdc++. When it is linked into the agent binary and that binary runs on a host
with a **newer** libstdc++ (26.04's `3.4.35`), the `std::format` code paths
baked into the prebuilt no longer match the host's runtime `std::format` ABI.
`Database::initBufferManager` uses `std::format` to build its diagnostic/format
strings, reaches `std::vformat`, and dereferences a mismatched formatter layout —
a segfault, not a clean error. On 25.10 the prebuilt's assumed ABI happens to
match the host, so the same code path is fine.

This is a classic **"do not mix C++ standard-library ABIs in one process"**
failure. The trigger is `std::format` because that is the part of libstdc++ whose
ABI moved between the two Ubuntu releases; the underlying rule is general.

### "But newer libstdc++ is backward-compatible" — why that doesn't save you

The obvious objection is that libstdc++ is backward-compatible, so an
older-compiled artifact should run against a newer runtime. That guarantee is
real but **narrow**: symbol versioning (`GLIBCXX_3.4.NN`) only protects
*exported, versioned symbols*. `std::format` arrived in C++20 and much of its
machinery is **header-only, inlined, and templated** — the `std::__format`
formatter types and the argument-store layout are *instantiated into the
prebuilt at its own compile time* against its own libstdc++ headers. Those
instantiations are not exported symbols and are not covered by the backward-compat
guarantee. When the host's libstdc++ changed the internal layout between `3.4.34`
and `3.4.35`, the prebuilt's baked-in instantiations no longer agree with the
host runtime, and the usual "just link against the newer lib" contract simply
does not apply. That is precisely why detection-and-retry at the application
layer cannot help — the mismatch is frozen into the binary at build time.

### Why it presents as a hard SIGSEGV (not an error)

A `dlopen`/link-time symbol-version mismatch on an *exported* symbol would fail
loudly at load. Here the mismatch is in **inlined/templated `std::format`
internals** that were resolved at the prebuilt's compile time against the older
libstdc++ headers. Nothing checks them at runtime; the first call that relies on
the changed layout corrupts or misreads memory and crashes. That is exactly why
the installer cannot "catch and retry" this at the application layer — it has to
prevent the mismatch from existing in the first place.

## The fix: one toolchain, built from source

The durable fix is to **build lbug from source against the host's toolchain**, so
the engine is compiled with the same `g++`/libstdc++ the binary runs against.
Then there is exactly one `std::format` ABI in the process and
`initBufferManager` runs cleanly. Building from source is the **unconditional
choice at deploy time** — not a per-host heuristic — so a fast-path prebuilt can
never silently reintroduce the crash on the next new OS.

Ownership is split cleanly, because of a hard Cargo ordering fact: Cargo runs a
dependency's build script (`lbug`) **before** its dependent's
(`amplihack-memory`), and `lbug` exposes no Cargo *feature* to force a source
build — only the `LBUG_BUILD_FROM_SOURCE` environment variable. So
`amplihack-memory` cannot flip the switch for `lbug` from its own build script.
The fix is therefore realized in two coordinated places:

- **`amplihack-memory-lib` owns the from-source *contract*.** Its
  `rust/amplihack-memory/build.rs` (active only under the `persistent` feature)
  emits a prominent `cargo:warning` at **every consumer build** when a prebuilt
  lbug is being linked without `LBUG_BUILD_FROM_SOURCE` set — naming the SIGSEGV
  and the remedy — so a prebuilt can never slip in silently. A dedicated CI job
  (`lbug-from-source`) compiles lbug from source and **opens a real persistent
  store**, proving `initBufferManager` initializes cleanly from source.
  **Simard consumes this by bumping its `amplihack-memory` pin.** No
  memory-engine logic is forked into Simard; the engine library remains the
  single source of truth, per the de-fork rule recorded in `Cargo.toml`.
- **The platform installer sets the variable where the binary is built.** The
  installer's build phase exports `LBUG_BUILD_FROM_SOURCE=1`
  (`scripts/install.sh`, `installer_build_env`) and its preflight doctor
  provisions the native toolchain the source build needs, so the *produced
  daemon binary* never contains a mismatched prebuilt.

Building from source is why the platform installer's
[preflight doctor](../howto/run-the-installer-preflight-doctor.md) insists on the
native build toolchain (`build-essential`, `cmake`, `clang`, `pkg-config`,
`libssl-dev`) — those are the inputs the from-source lbug build needs.

### Why not "just pick a matching prebuilt"?

Selecting a prebuilt `LBUG_LINUX_VARIANT` that matches the host's libstdc++ is a
*conceivable* remediation, but it is brittle: it requires a published variant for
every host libstdc++ the fleet will ever run on, and it fails closed with no
recourse the day a host ships a libstdc++ newer than any published variant
(exactly the 26.04 situation). Building from source needs only the toolchain the
installer already provisions, and it is correct for **any** host libstdc++,
including future ones. Source-build is therefore the unconditional fix; a
variant-match is at best a non-relied-upon fallback.

## How the installer guarantees a working store on both 25.10 and 26.04

The [platform installer](./platform-installer.md) makes the guarantee concrete:

1. **Detect.** The preflight doctor reads the host's libstdc++ version
   (`GLIBCXX_3.4.NN`) and confirms the source-build toolchain is present. Its
   `lbug-source` check **verifies the from-source prerequisites** and informs the
   operator; it does not choose prebuilt-vs-source (source is unconditional).
2. **Prevent.** The build phase compiles lbug from source against the host
   toolchain — the installer exports `LBUG_BUILD_FROM_SOURCE=1` (and
   `amplihack-memory-lib`'s guard warns if it were ever missing). There is no
   prebuilt in the process to mismatch.
3. **Verify.** After start, the installer confirms cognitive memory **opened**
   (a positive store-opened / first-OODA-cycle log marker) with no
   `initBufferManager`/`std::vformat` SIGSEGV and a stable PID. If the store
   still cannot open cleanly, the install fails closed at the verify phase with
   the ABI diagnosis rather than shipping a crash-loop.

The net result: a **working cognitive-memory store on both Ubuntu 25.10 and
26.04** from the same install command.

## Proven end-to-end on host `dev` (Ubuntu 26.04)

The installer was run against the real target. The prebuilt binary's
crash was reproduced (systemd journal: `status=11/SEGV` during startup), then a
from-source rebuild produced a daemon whose cognitive memory **opens cleanly**
(no `initBufferManager`/`std::vformat` SIGSEGV) and reaches **live OODA cycles**
with a stable PID (`NRestarts=0`), side-by-side with the primary identity.

Two *further* lbug 0.17.1 from-source defects surfaced during that run and are
tracked in [amplihack-memory-lib#130](https://github.com/rysweet/amplihack-memory-lib/issues/130),
each with a downstream workaround the installer now applies:

1. **Duplicate-symbol link failure** — the from-source static build links its
   bundled `utf8proc`/`antlr4` objects twice (inside `liblbug.a` and as separate
   `--whole-archive` libs), so `LBUG_BUILD_FROM_SOURCE=1` alone does not link.
   The installer adds `RUSTFLAGS=-Clink-arg=-Wl,--allow-multiple-definition`.
2. **Post-verdict static-teardown SIGSEGV** — a clean from-source binary still
   SIGSEGVs in C++ global-destructor teardown *after* returning success
   (reproduces on prebuilt and from-source alike, so it is not the open-time ABI
   bug). The read-only guardrail gate therefore keys on the explicit affirmative
   verdict marker (fail-closed preserved) rather than the exit code alone, so a
   post-verdict teardown crash cannot false-fail a proven-safe gate.

## Boundaries and ownership

- **Engine/portability *contract* → `amplihack-memory-lib`.** The persistent
  build guard (`build.rs`), the from-source ABI proof CI job, and the root-cause
  documentation live in the memory library. It cannot flip lbug's build mode
  itself (Cargo build-script ordering + no lbug feature), so it makes the
  requirement loud and proves the from-source path.
- **Simard → pin bump only.** Simard changes its `amplihack-memory` revision to
  pull the guard/proof. Nothing about lbug is duplicated in Simard.
- **Installer → set the variable, provision the toolchain, and verify.** The
  installer exports `LBUG_BUILD_FROM_SOURCE=1` for the deployment build,
  provisions the native toolchain the from-source build needs, and *proves* the
  store opens cleanly on the target host.

## See also

- [The Simard platform installer](./platform-installer.md) — how detect →
  build-from-source → verify fits into the install phase machine.
- [Run the preflight doctor](../howto/run-the-installer-preflight-doctor.md) —
  the libstdc++/toolchain/ABI checks.
- [Cognitive memory](../memory.md) — the embedded lbug store Simard depends on.
