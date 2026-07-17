# SAG-RS Architecture

This document is the source of truth for the design. Build to it exactly.

SAG-RS is a lightweight, Linux-first multi-agent orchestration runtime that
distributes coding and research work across a LAN of mixed-VRAM GPU machines
running local, OpenAI-compatible model servers. It is part port, part evolution
of the **Structured-Agent-Graph-IDE (SAGIDE)** project
(https://github.com/sanjeevakumarh/Structured-Agent-Graph-IDE) — it borrows the proven
*ideas* but is a clean-slate Rust implementation, not a line-by-line port.

---

## Guiding principles

1. **Wrap, don't rebuild.** Anything that is a solved infrastructure problem
   (Qdrant, SearXNG, ripgrep, tree-sitter, bubblewrap, the model servers) is
   wrapped, never reimplemented. Only hand-build the actual domain logic: the
   DAG engine, the scheduler, the context compiler.

2. **One trait per component = one swap seam.** Every crate exposes a single
   trait at the top of its `lib.rs`. That trait is the seam a customizer swaps.

3. **Config wires components, not code.** Layered config (defaults → `sag.toml`
   → env → CLI flags) selects which implementation is used. Persona 1 never
   edits wiring; Persona 2 flips a config line or writes a new impl behind the
   same trait.

4. **Everything testable without a cluster.** Every crate ships a `Fake` impl
   and passes `cargo test -p <crate>` with no network, no GPU, no live nodes.

5. **Task-level parallelism, not model sharding.** Use the cluster to run
   different agents on different nodes. Do not split one model across
   heterogeneous machines.

6. **Artifacts, not mutation.** Agents produce immutable artifacts. The real
   workspace changes only after test → review → constraint gates → user
   approval → apply.

---

## The predecessor: SAGIDE (what to borrow, what to leave)

Source of truth for the original ideas:
**https://github.com/sanjeevakumarh/Structured-Agent-Graph-IDE**

SAGIDE is C#/.NET and VS Code–centric. Its core logic is battle-tested; the
default posture is **reuse and port it, and only invent what SAGIDE genuinely did
not solve.** Re-deriving proven logic from scratch just re-introduces bugs someone
already fixed.

**Port behavior, not structure.** This is the one distinction that decides whether
the rewrite succeeds:
- *Porting behavior* (do this): read what a SAGIDE component does — the state
  transitions, conditions, ordering — and reproduce that behavior in idiomatic
  Rust. The tests you write are effectively the tests SAGIDE already passed.
- *Porting structure* (never do this): transliterating C# classes, inheritance,
  or shared-mutable-object graphs line-by-line. C# lets a workflow engine mutate a
  passed-around graph object from anywhere; Rust's ownership model rejects that.
  Transliterating the shape means fighting the borrow checker over a problem you
  imported. Re-derive the structure in the message-passing, trait-per-crate shape
  defined below.

**Port (proven behavior — reproduce faithfully in Rust):**
- The durable DAG workflow engine: dependencies, retries + backoff, DLQ,
  approvals, convergence policies, feedback loops, shadow workspaces, and the
  "which stages re-run on failure" logic.
- Model routing by capability + the SearXNG three-layer approach (memory cache,
  persistent cache, live search) with quality scoring and stale fallback.
- SQLite-backed workflow state, run tracing, prompt/skill registries.
- Workspace setup/teardown and the artifacts-not-mutation discipline.

**Genuinely unsolved (invent here — SAGIDE has no answer):**
- The resource-aware scheduler routing on *live* VRAM / KV-cache / throughput
  across *heterogeneous vendor backends* (ROCm + oneAPI + CUDA). SAGIDE routed on
  "is the model loaded"; scheduling on measured capacity across mismatched GPUs is
  the real new problem.
- The FastContext scout as a token-saving retrieval front-end.
- The vendor-agnostic node bootstrap (detect-and-serve across three GPU stacks).

**Leave behind (deliberately dropped in SAG-RS):**
- The VS Code extension and web dashboard as core (they become optional clients).
- C#/.NET and any provider-named coupling (`OllamaProvider`, `CodexProvider`).
- Static model-to-host config maps — replaced by dynamic discovery + the scheduler.
- Generic fixed-size character chunking for code — replaced by symbol-aware
  indexing and the FastContext scout.

When a design question traces back to "how did SAGIDE do X", go read that repo,
port the behavior, and re-derive the structure in the shape defined below.

---

## The two personas (the design driver)

- **Persona 1 — deploy and run.** Wants opacity: two scripts, sensible
  defaults, never open a source file.
- **Persona 2 — customize.** Wants transparency: small pieces, clear seams,
  test one thing in isolation.

These pull in opposite directions. The trait-per-component + config-wiring
commitment resolves the tension: the same seams serve both. Smart defaults are
just the impls selected by default config; customizing is pointing config at a
different impl.

---

## The two bootstrap scripts (Layer 1)

The two personas map onto two roles, and the scripts mirror that.

**`setup-node.sh` / `setup-node.ps1`** — runs on every GPU box (Linux / macOS /
Windows). Detects rather than asks:

```
detect OS + GPU vendor  →  pick serving engine
  Linux + NVIDIA  → vLLM (fallback: llama.cpp)
  Linux + AMD/CPU → llama.cpp / ROCm
  Linux + Intel   → llama.cpp (Vulkan/SYCL) / oneAPI
  macOS           → Ollama now, MLX opt-in later
  Windows         → Ollama
→ pull the smart-default model for this box's VRAM tier
→ launch server + tiny sag-node agent (reports VRAM / queue / tok-s)
→ print: "node 'hptowerz' registered, serving qwen3-coder:30b"
```

**`setup-client.sh`** — runs on the Linux control box. Installs the `sag`
binary + controller, writes a starter `sag.toml`, runs `sag doctor`.

**Critical:** scripts are THIN wrappers (~15 lines) over tested Rust
(`sag-node bootstrap`). No install logic lives in bash — otherwise it's
untestable and Persona 2 can't debug it the way they debug everything else.
The script is the doorknob; the binary is the door.

---

## Crate layout (Layer 2)

Rule: **directory name = concept name = the thing you'd want to swap.** A person
thinking "I want to change how research search works" finds it by reading the
folder list, never by grep.

```
crates/
  sag-cli/         "how I talk to it"        → clap + ratatui
  sag-controller/  "the brain"               → schedules, persists, decides
  sag-node/        "what runs on a GPU box"  → inventory + inference proxy
  sag-engine/      "run a DAG of steps"      ← the crown jewel, worth owning
  sag-scheduler/   "pick which node"         → eligibility + weighted score
  sag-inference/   "talk to a model"         → trait + reqwest impls
  sag-retrieval/   "find relevant context"   → scout / code / vector / web
  sag-tools/       "let agents do things"    → native / sandboxed / mcp
  sag-store/       "remember + artifacts"    → sqlx + content-addressed dir
  sag-proto/       "shared vocabulary"       → serde types
```

Workspace with a virtual manifest;
`default-members = ["sag-cli", "sag-controller", "sag-node"]`.

Two things make this fast to customize, not just tidy:

1. **One trait per crate, stated at the top of `lib.rs`.** Opening any crate,
   the first thing you see is the seam.
2. **Every crate tests standalone with a `Fake`.** `cargo test -p sag-scheduler`
   runs weighted selection against hand-built fake `NodeSnapshot`s — no GPUs, no
   network, milliseconds.

---

## Recommended stack

| Layer | Pick | Native-first fallback |
|---|---|---|
| Async runtime | `tokio` | — |
| CLI | `clap` (derive) + `ratatui` for `attach` | — |
| Local IPC | Unix domain socket, length-prefixed | raw socket |
| LAN protocol | `axum` + JSON for v1 | curl-debuggable; add `tonic`/gRPC only if profiling demands |
| Durable state | `sqlx` (compile-checked SQL, SQLite WAL) | — |
| Serialization | `serde` + `serde_json` | — |
| Artifact store | content-addressed dir, `blake3` | hand-rolled (~50 lines) |
| Inference client | `reqwest` vs OpenAI-compatible `/v1` | the thin client *is* the native impl |
| Vector retrieval | Qdrant (Rust client) | `sqlite-vec` local impl |
| Lexical / code search | `ripgrep` + `tree-sitter` crate | tree-sitter has Rust bindings |
| Sandbox | `bubblewrap` / rootless podman via subprocess | — (never reimplement a sandbox) |
| Config | `serde` + `figment` (layered) | this *is* the smart-defaults mechanism |
| Observability | `tracing` + OTLP via `opentelemetry` | — |
| Errors | `thiserror` in libs, `anyhow` in binaries | — |

**Two deliberate calls:**
- **Drop gRPC for v1.** JSON-over-HTTP is inspectable with curl and needs no
  `protoc` build step. Add `tonic` later only if serialization is measured as a
  bottleneck (it won't be at homelab scale).
- **Traits map the "native impl + swap a library" instinct perfectly.** Ship a
  naive native impl always compiled; gate the fast one behind a feature flag.

```rust
trait VectorStore {
    async fn upsert(&self, points: Vec<Point>) -> Result<()>;
    async fn search(&self, q: Query) -> Result<Vec<Hit>>;
}
struct SqliteVecStore { /* first-principles, always compiled */ }
struct QdrantStore    { /* behind #[cfg(feature = "qdrant")] */ }
```

---

## Config as the shared seam

```toml
# sag.toml — smart defaults; every line optional
[inference]        engine = "auto"        # auto-detect; or "vllm" | "ollama" | "llamacpp"
[retrieval]        vector = "sqlite-vec"  # zero-setup default; or "qdrant"
[retrieval.scout]  enabled = true         # FastContext 4B repo scout (token filter)
                   node    = "gmkmini"     # move to "mac-mini" as load dictates
                   gate    = "repo-tasks" # only runs where it can save tokens
[scheduler]        strategy = "weighted"  # or "round-robin" for debugging
[tools]            sandbox = "bubblewrap" # or "none" for trusted local dev
```

`figment` layers: built-in defaults → `sag.toml` → env → CLI flags. Persona 1
can delete the file and it still runs. Persona 2 flips `sqlite-vec` → `qdrant`
and nothing else changes, because both sit behind `VectorStore`.

---

## The scheduler

Two stages.

**Hard eligibility** — reject nodes that can't satisfy: model availability,
minimum context, estimated weights + KV-cache memory, required tool-calling /
structured-output support, required GPU/backend, maximum queue delay.

**Weighted selection** among eligible nodes:

```
30%  capability + benchmark quality
20%  warm-model preference
15%  queue / active-request load
15%  estimated tokens/sec
10%  KV-cache / VRAM headroom
 5%  data / repo locality
 5%  recent reliability
```

VRAM alone is not enough — context length consumes KV cache, and real
concurrency depends on available KV-cache tokens. Roles below are *preferences*,
not fixed assignments; benchmark results eventually determine routing.

| VRAM tier | Preferred role |
|---:|---|
| ~16 GB | query rewriting, extraction, test diagnosis, light critic, embeddings |
| ~20 GB (HPTowerZ) | interactive coding, patch generation, debugging |
| ~32 GB (LenovoTowerP) | strong coding, architecture review, security review |
| ~108 GB (GMKMini) | planning, deep research, synthesis, large-context review |

---

## Inference seam

Normalize every server behind one trait:

```rust
#[async_trait]
pub trait InferenceEndpoint: Send + Sync {
    async fn discover(&self) -> Result<ModelCatalog>;
    async fn generate(&self, req: Request) -> Result<Stream>;
    async fn embed(&self, req: EmbedRequest) -> Result<Vec<Vec<f32>>>;
    async fn health(&self) -> Result<Health>;
}
```

Use an OpenAI-compatible request model internally. The engine asks for
*capabilities* (`code.edit`, `code.explore`, `reason.deep`, `embed`, `rerank`) —
never for provider names like `OllamaProvider`. The FastContext scout is the
`code.explore` capability; routing it to GMKMini (or elsewhere) is pure config.

---

## The FastContext scout (token-saving retrieval front-end)

**Requirement:** SAG-RS integrates Microsoft Research's **FastContext** — a
lightweight (4B) repository-explorer model — as a read-only "scout" that sits in
front of repository retrieval. Instead of the main agent reading whole files into
its context to find what's relevant, it delegates a natural-language query to the
scout, which explores the repo in its *own* scratch context (Read / Glob / Grep)
and returns only compact file-path + line-range citations. The main agent then
reasons over ~hundreds of tokens of citations rather than tens of thousands of
tokens of raw source. On repo-touching tasks this cuts exploration token cost
substantially (the paper reports up to ~60% end-to-end per task).

**Why it's a "filter" and where it applies.** The scout is gated to tasks *that
can save tokens* — i.e. any task with a repo to explore (coding, fixing,
reviewing, refactoring, repo-grounded research). It is **not** a blanket proxy;
tasks with no repo (pure planning, live-web research synthesis) bypass it. The
scheduler/engine consults the scout at the **retrieval stage** before assembling
the context bundle for a coding agent.

**Placement — configurable, default GMKMini.** FastContext is only a 4B model, so
it runs comfortably in spare capacity. Default node is **GMKMini** (co-located
with the existing ROCm/Ollama serving). Because placement is config-driven, the
scout can be moved to any node (e.g. the Mac Mini) with a one-line change the day
GMKMini is over-utilized — no code change:

```toml
[retrieval.scout]
enabled = true
model   = "fastcontext-1.0-4b"   # SFT or RL variant
node    = "gmkmini"               # move to "mac-mini" / "hptowerz" as load dictates
gate    = "repo-tasks"           # "repo-tasks" (default) | "always" | "off"
max_citations = 40
```

The scout lives behind the same swap seam as everything else: it implements the
`CodeExplorer` trait in `sag-retrieval`, ships with a `Fake` explorer for
offline testing, and can be disabled entirely (`gate = "off"`) to fall back to
the native ripgrep/tree-sitter path below. FastContext is the smart default; the
native explorer is the first-principles fallback for dev/test/debug.

```rust
// sag-retrieval/src/lib.rs — the scout seam
#[async_trait]
pub trait CodeExplorer: Send + Sync {
    /// Explore a repo for a task and return compact citations,
    /// NOT file contents. The explorer holds its own scratch context.
    async fn explore(&self, repo: &RepoRef, query: &str) -> Result<Vec<Citation>>;
}

pub struct FastContextExplorer { /* calls the 4B scout via InferenceEndpoint */ }
pub struct NativeExplorer      { /* ripgrep + tree-sitter + LSP, always compiled */ }
pub struct FakeExplorer        { /* deterministic citations for tests */ }

pub struct Citation { pub path: String, pub lines: (u32, u32), pub reason: String }
```

Note the scout reaches the model through the existing `InferenceEndpoint` seam —
it is just another capability (`code.explore`) the engine can request, so moving
it between nodes is a routing/config concern, never a code concern.

---

## Three-tier retrieval

1. **Repository intelligence** — the FastContext scout (above) is the token-saving
   front-end; behind it, ripgrep exact match + tree-sitter symbols + LSP
   defs/refs + semantic vectors + git-history relevance. Index code as symbols
   (files, classes, functions, signatures, imports, call graphs, linked tests),
   not generic character chunks. The scout decides *what* to pull; this tier is
   *how* the underlying evidence is indexed and resolved.
2. **Long-term memory / documents** — Qdrant (dense + sparse, RRF fusion) for
   retrieval; SQLite stays for workflow state and artifact metadata only.
3. **Live internet research** — SearXNG for *discovery only*. Fetch → extract
   main content → classify date/source → extract evidence → check
   contradiction/freshness → rerank → synthesize with citations. Search snippets
   never become the final answer directly; every claim carries source, publisher,
   dates, and a support strength.

---

## Coding-agent runtime

Each coding agent gets a dedicated git worktree, a restricted tool environment,
a context bundle, and a typed task contract with iteration/token budgets.
Outputs are immutable artifacts:

```
plan.json  context-manifest.json  retrieval-results.json  tool-transcript.jsonl
changes.patch  test-results.xml  review-findings.json  final-summary.md
route-decision.json
```

Workspace apply order:

```
generate patch → build/test → review → constraint gates → user approval → apply
```

**Tools** come in three classes: native trusted (git, fs reads, ripgrep,
tree-sitter, LSP, build/test runners), sandboxed shell (bubblewrap / rootless
podman with CPU/mem/net/timeout limits), and MCP (external integrations — a
plugin boundary, never a replacement for the workflow engine).

---

## One Rust-specific caution

The hardest part of translating SAGIDE's ideas isn't syntax — it's state. A
workflow engine that passes mutable graph state around will fight the borrow
checker. Design the engine **actor-style / message-passing** from the start
(`tokio::sync::mpsc`, owned state per task) rather than `Arc<Mutex<Graph>>`. Get
this right up front and the engine stays clean.

---

## Deliberately avoided

- No Kubernetes for a personal cluster.
- No LangGraph / AutoGen rewrite — borrow patterns, own the engine.
- No Ray as the agent scheduler.
- No multi-node model sharding over mismatched consumer hardware.
- No direct multi-agent edits to one working tree.
- No unbounded raw-shell access.
- No giant prompt containing the whole repository.
- No permanent "planner model" / "coder model" assignments — route by measured
  capability.
- No NATS initially — one controller + SQLite leases is enough for several nodes.

---

## First milestone

The smallest slice that proves the architecture:

```bash
sag run fix --repo ~/src/project --request "Fix intermittent cache expiry test"
```

1. Create an isolated git worktree.
2. **Send the task to the FastContext scout on GMKMini**, which explores the repo
   and returns compact file/line citations instead of whole files.
3. Assemble the context bundle from those citations (+ tests, history, docs).
4. Ask a large-memory node to plan.
5. Ask a coding node to implement.
6. Run tests locally in a sandbox.
7. Ask another node to review the patch.
8. Loop only the failing stages.
9. Show the final diff in the terminal.
10. Require approval.
11. Apply the patch and persist every artifact needed for replay.

Everything through the scheduler tests against fakes, so the whole runtime can
be built on one Linux box before touching a GPU.

### Scout rollout (build order)

Build the scout seam in two passes so nothing blocks on the model being live:

1. **Seam first (offline).** Implement `CodeExplorer` + `NativeExplorer` +
   `FakeExplorer` in `sag-retrieval`. Wire the engine to call `explore()` at the
   retrieval stage. `cargo test -p sag-retrieval` passes against `FakeExplorer`
   with no model, no GPU. At this point `gate` can default to the native path.
2. **FastContext live (on GMKMini).** Stand up the 4B model on GMKMini's serving
   stack (it already speaks OpenAI-compatible), register it as the `code.explore`
   capability, implement `FastContextExplorer` against `InferenceEndpoint`, and
   flip `[retrieval.scout] enabled = true`. Because it's config, moving the scout
   to the Mac Mini later is a one-line change, never a rebuild.

Serving note: FastContext ships as GGUF and can run under llama.cpp — which is
already a supported backend — so on GMKMini it can sit alongside the ROCm/Ollama
models without a new serving stack.
