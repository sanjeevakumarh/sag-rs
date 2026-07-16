# SAG-RS — build guide for Claude Code

**Read `ARCHITECTURE.md` first. It is the source of truth. Follow it exactly.**
This file is the short operational contract; `ARCHITECTURE.md` has the full design.

SAG-RS is a Linux-first, vendor-agnostic multi-agent orchestration runtime in
Rust — a clean-slate evolution of the earlier SAGIDE project (which was C#).
**Reuse and port SAGIDE's proven behavior; only invent what SAGIDE genuinely did
not solve.** Its core logic has been tested through real failures — re-deriving it
from scratch just re-introduces bugs someone already fixed. But *port behavior,
not structure*: reproduce what a component does in idiomatic Rust, do not
transliterate C# classes or shared-mutable-object graphs (Rust's ownership model
will reject that shape). This is a rewrite of the chassis around proven logic.
Predecessor repo (read for the domain model and behavior, not the C# structure):
https://github.com/sanjeevakumarh/Structured-Agent-Graph-IDE

## Non-negotiables

- **Rust workspace, one crate per concept** (see the crate table in `ARCHITECTURE.md`).
- **One trait per crate = its swap seam.** State it at the top of `lib.rs`.
- **Every crate ships a `Fake` impl and passes `cargo test -p <crate>` with NO
  cluster, network, or GPU.** This is how both personas (deploy / customize) are served.
- **Wiring is config-driven** (figment: defaults → `sag.toml` → env → flags).
  Never wire components together in code.
- **Wrap, don't rebuild:** Qdrant, SearXNG, ripgrep, tree-sitter, bubblewrap, the
  model servers, and the FastContext model. Only hand-build domain logic: the
  DAG engine, scheduler, context compiler.
- **Bootstrap scripts are THIN wrappers** (~15 lines) over tested Rust
  (`sag-node bootstrap`). No install logic in bash.
- **Artifacts, not mutation:** agents produce immutable artifacts; the real
  workspace changes only after test → review → gates → user approval → apply.

## Build order (one vertical slice first — do NOT build breadth early)

1. **Workspace skeleton.** All 10 crates compile: each with its trait stub, a
   `Fake` impl, and one passing test. Run `cargo test`, show it green. Commit.
2. **`sag-engine`.** DAG of steps, actor-style (`tokio::sync::mpsc` + owned state
   per task, **NOT** `Arc<Mutex<Graph>>` — the borrow checker will fight a naive
   translation of shared mutable graph state).
3. **`sag-inference`.** `InferenceEndpoint` trait + `reqwest` OpenAI-compatible
   impl + `Fake`.
4. **`sag-scheduler`.** Hard eligibility + weighted selection over
   `Vec<NodeSnapshot>`. Test against hand-built fake snapshots.
5. **`sag-retrieval` scout seam.** `CodeExplorer` trait + `NativeExplorer`
   (ripgrep/tree-sitter) + `FakeExplorer`. Wire the engine to call `explore()` at
   the retrieval stage. Green against the fake — no model yet. (FastContext on
   GMKMini comes later; it's a config flip, not a prerequisite. See the
   "Scout rollout" section in `ARCHITECTURE.md`.)
6. **Wire `sag run fix`:** worktree → scout explore → context bundle → plan →
   implement → sandboxed test → review → diff → approve → apply.

## Style

- `tokio` async. `clap` (derive) for CLI, `ratatui` for the `attach` TUI.
- `thiserror` in libraries, `anyhow` in binaries.
- `sqlx` (compile-time-checked SQL, SQLite WAL) for persistence.
- `serde` + `figment` for config. `tracing` (+ OTLP) for observability.
- Prefer message-passing over shared mutable state.
- LAN transport is `axum` + JSON for v1 (curl-debuggable). Do NOT reach for
  gRPC/`tonic` unless profiling proves serialization is a bottleneck.

## Working agreement

- Drive one build-order step at a time. After each, run the tests, show them
  green, and stop for review before the next step.
- Don't add dependencies that pull in a heavy framework where a small native
  impl + a swap seam would do. When in doubt, native-first + trait.
- If a design question isn't answered by `ARCHITECTURE.md`, ask before inventing.
