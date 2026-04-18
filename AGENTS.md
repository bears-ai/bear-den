# Agent notes (BEARS / bears-depoy)

Use this file for **repository conventions** when editing or generating changes.

**vs [README.md](README.md):** The README is the **human** landing page (short overview, links out). **This file** holds what agents need repeatedly: terminology, GitOps, migration immutability, doc map, cloning, and link rules. Do not duplicate long narrative architecture in README—summarize there and keep detail here and under `docs/`.

## What this repo is

- **Light monorepo:** docs under `docs/`, Coolify-oriented assets under `services/*`, and the **Den** Rust service at repo root in **`den/`** (add the `cargo` tree there; it is not under `services/`).
- **Terminology:** **Bear** = the primary assistant backed by a Letta agent; **subagents** (e.g. reflection) are configured per bear when used ([docs/dynamic-skills-subagents-adr.md](docs/dynamic-skills-subagents-adr.md)). **Den** = control plane (provisioning, **users↔bears** membership, routing, Cabinet API when deployed). **BEARS** = the deployment stack name.

**Den builds:** In environments with Rust installed (dev container, CI), run `cargo build` / `cargo test` from **`den/`** to verify changes. See [`den/AGENTS.md`](den/AGENTS.md) (“Verifying Rust changes”).

## GitOps and reproducibility

**Strict GitOps is the default assumption:** configuration that affects how the stack runs should live in **this repository** (or be generated from files in-repo in CI), go through normal review, and avoid **silent drift** from one-off edits in hosting UIs or production consoles. Prefer declarative assets under `services/*`, env templates, and docs over “remember to click this in Coolify.”

**Den SQL migrations:** Files under `den/migrations/` are **immutable once applied**. SQLx stores a checksum per version in `public._sqlx_migrations`; editing an existing `*_up.sql` (even comments) breaks verification and can stop Den from starting. **Add a new migration** for any schema change. Rules and checksum repair steps: [`den/migrations/README.md`](den/migrations/README.md).

**Production should be reconstructible** from three inputs only:

1. **This repository** (configs, compose/Coolify definitions, migrations or schema notes as applicable).
2. **Database backups** — use **as few distinct database products and backup scopes as practical**; do not treat ad-hoc dumps or undocumented DBs as part of the contract unless they are called out in `docs/`.
3. **External object storage** — assume **S3-compatible** buckets (or equivalent) for blobs and large artifacts; credentials are environment/secret injected, not the source of truth for *what* to deploy.

When proposing gateways, proxies, or operators, **favor file- or repo-driven config** over mutable runtime-only admin UIs unless the project explicitly opts in. If a component requires a DB or UI-managed state, document what must be in backups versus what is disposable.

## Where to read

| Topic | Path |
|-------|------|
| Human-oriented overview (short) | [README.md](README.md) |
| Doc index, monorepo clone notes | [docs/README.md](docs/README.md) |
| Roadmap and contracts | [docs/planning/PLAN.md](docs/planning/PLAN.md) |
| Phase 1 Den build (bootstrap → operator console) | [docs/planning/PHASE1_BOOTSTRAP.md](docs/planning/PHASE1_BOOTSTRAP.md) |
| Coolify deployment | [docs/deployment/DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md) |
| Garage (S3 object storage) | [services/garage/COOLIFY_DEPLOY.md](services/garage/COOLIFY_DEPLOY.md) |
| Stack one-pager | [docs/architecture/ARCHITECTURE_NOTES.md](docs/architecture/ARCHITECTURE_NOTES.md) |
| Den + self-hosted Letta (multi-user web) | [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md) |
| Dynamic skills & reflection subagents | [docs/dynamic-skills-subagents-adr.md](docs/dynamic-skills-subagents-adr.md) |
| Routines & scheduling (Phase 1) | [docs/routines-automation-adr.md](docs/routines-automation-adr.md) |
| Artifacts & Garage (S3) | [docs/artifacts-garage-adr.md](docs/artifacts-garage-adr.md) |
| Den meta tools (Den facade, Letta Code–brokered) | [DEN_ARCHITECTURE.md — Den meta tools](docs/architecture/DEN_ARCHITECTURE.md#den-meta-tools-bears-control-plane-tools) |
| Den web UI (templates, CSS: no page-local `<style>`) | [den/docs/frontend-development.md](den/docs/frontend-development.md), [den/AGENTS.md](den/AGENTS.md) |
| Den SQL migrations (immutability, checksum repair) | [den/migrations/README.md](den/migrations/README.md) |
| Assistant memory / project brief | [.kilocode/memory_bank/](.kilocode/memory_bank/) |

Prefer **updating existing docs** under `docs/` rather than adding new top-level `.md` files. Root should stay limited to **README.md** and **AGENTS.md** unless the project explicitly expands that rule.

## Cloning

For CI or hosts that only need configs, **`git clone --depth 1`** is recommended in docs. Prefer **sparse checkout** only when a machine must exclude paths (see [docs/README.md](docs/README.md)).

## Links

When linking from `services/*`, use paths relative to the file (for example `../../docs/planning/PLAN.md` from `services/letta/`).
