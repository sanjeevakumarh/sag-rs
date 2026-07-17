# Git hooks

Version-controlled hooks so the whole team shares the same guardrails. Git does
**not** use this directory automatically — point git at it once per clone:

```bash
git config core.hooksPath .githooks
```

`setup-client.sh` runs that for you; do it manually in any other clone.

## `pre-commit` — secret guardrail

Blocks a commit that adds credentials, private keys, or local machine config,
so passwords / API keys / node addresses never land in the repo by accident. It
scans only the **staged additions**, using nothing but git + grep (no external
tool to install — consistent with the project's native-first rule).

It flags:

- local/secret files staged by mistake — `sag.toml`, `.env`, `*.pem`, `*.key`, … ;
- private-key blocks and known token formats (AWS, GitHub, Slack, OpenAI/Anthropic, Google);
- `password = "…"` / `api_key: …` style hardcoded assignments;
- private-range IPv4 addresses (node addresses belong in your local `sag.toml`).

**Escape hatches** (for genuine false positives):

- append `# pragma: allowlist secret` to the offending line, or
- override the whole hook with `git commit --no-verify`.

This is a *safety net, not a vault* — it catches common mistakes, not a
determined leak. `.gitignore` is the first line of defense; this is the second.
For a stronger, entropy-based scan you can later add `gitleaks` behind the same
`core.hooksPath` without changing anything here.
