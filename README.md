# SAG-RS

**Turn your home GPU cluster into a local coding & research team.**

SAG-RS lets a cluster of your own computers work together as a team of coding
and research assistants, using models running locally on your own hardware
instead of paying for cloud services. It's a Rust rewrite and evolution of an
earlier project called **SAGIDE** — keeping the good ideas but making it
lighter, faster, and command-line based. Simple enough to set up with two
scripts, but easy to take apart and customize if you want to.

---

## Why it's different

Most local-model orchestration projects quietly assume CUDA. SAG-RS is built to
stay **truly vendor-agnostic**, and it's proven that way on real hardware — a
mixed-vendor home cluster spanning AMD, Intel, and NVIDIA backends
simultaneously:

| Node | GPU / Memory | Backend |
|---|---|---|
| **GMKMini** | Radeon 8060S, 108 GB UMA | ROCm |
| **LenovoTowerP** | Intel Arc Pro B70, 32 GB | oneAPI / Vulkan |
| **HPTowerZ** | RTX A4500, 20 GB | CUDA |

Every node serves an OpenAI-compatible endpoint, so the runtime never needs to
know which vendor is underneath.

---

## The two-layer design

SAG-RS is built for two kinds of people, and the architecture serves both from
the same seams.

**Layer 1 — deploy and run.** Two scripts, smart defaults, don't read the code.

- `setup-node.sh` (or `.ps1` on Windows) — run on each GPU box. Detects the OS
  and GPU, picks a serving engine, pulls a sensible default model, and registers
  the node.
- `setup-client.sh` — run on your Linux control box. Installs the `sag` binary,
  writes a starter config, and confirms it can see your nodes.

**Layer 2 — customize.** Every component is a trait with a boring default impl,
wired by config rather than code. Swap any piece (vector store, scheduler,
serving engine, sandbox) by pointing config at a different implementation — or
write your own behind the same trait, and test it in isolation with no cluster,
network, or GPU required.

See [ARCHITECTURE.md](./ARCHITECTURE.md) for the full design.

---

## Quickstart

**On each GPU machine:**

```bash
./setup-node.sh          # Linux / macOS
# or
./setup-node.ps1         # Windows
```

**On your Linux control box:**

```bash
./setup-client.sh
sag doctor               # confirms nodes are discovered
sag run fix --repo . --request "Fix intermittent cache expiry test"
```

---

## Core ideas borrowed from SAGIDE

- **Durable DAG workflows** — steps with dependencies, retries, dead-letter
  handling, and deterministic replay.
- **Task-level parallelism** — run different agents (plan / code / test / review)
  on different nodes, rather than sharding one model across mismatched GPUs.
- **Resource-aware scheduling** — route by measured capability, free VRAM, queue
  depth, and throughput, not by static model-to-host maps.
- **Three-tier retrieval** — code intelligence, long-term vector memory, and live
  web research, kept as separate systems.
- **Artifacts, not mutation** — agents produce immutable artifacts; the real
  workspace changes only after tests, review, and your approval.

---

## Status

Early development. The first milestone is a single vertical slice:

```bash
sag run fix --repo ~/src/project --request "..."
```

→ isolated git worktree → retrieve context → plan → implement → sandboxed test →
review → show diff → require approval → apply.

Everything through the scheduler is testable against fakes, so the whole runtime
can be built on one Linux box before pointing it at a single GPU.

---

## Development

Enable the shared git hooks once per clone (a pre-commit secret guardrail —
keeps credentials, keys, and node addresses out of the repo):

```bash
git config core.hooksPath .githooks
```

`setup-client.sh` does this automatically. See [.githooks/](./.githooks/) for
what it checks and how to override a false positive. Copy
[`sag.toml.example`](./sag.toml.example) to `sag.toml` (git-ignored) for local
config.

---

## License

MIT — see [LICENSE](./LICENSE).
