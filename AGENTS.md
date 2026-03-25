# Agent notes (BEARS / bears-depoy)

Use this file for **repository conventions** when editing or generating changes.

## What this repo is

- **Light monorepo:** docs under `docs/`, Coolify-oriented assets under `services/*`, and the **Den** Rust service at repo root in **`den/`** (add the `cargo` tree there; it is not under `services/`).
- **Terminology:** **Bear** = one assistant backed by a Letta agent. **Den** = control plane (provisioning, **users↔bears** membership, routing, Cabinet API when deployed). **BEARS** = the deployment stack name.

## Where to read

| Topic | Path |
|-------|------|
| Overview + doc map | [README.md](README.md) |
| Doc index, monorepo clone notes | [docs/README.md](docs/README.md) |
| Roadmap and contracts | [docs/planning/PLAN.md](docs/planning/PLAN.md) |
| Phase 1 Den build (bootstrap → operator console) | [docs/planning/PHASE1_BOOTSTRAP.md](docs/planning/PHASE1_BOOTSTRAP.md) |
| Coolify deployment | [docs/deployment/DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md) |
| Stack one-pager | [docs/architecture/ARCHITECTURE_NOTES.md](docs/architecture/ARCHITECTURE_NOTES.md) |
| Den + self-hosted Letta (multi-user web) | [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md) |
| Assistant memory / project brief | [.kilocode/memory_bank/](.kilocode/memory_bank/) |

Prefer **updating existing docs** under `docs/` rather than adding new top-level `.md` files. Root should stay limited to **README.md** and **AGENTS.md** unless the project explicitly expands that rule.

## Cloning

For CI or hosts that only need configs, **`git clone --depth 1`** is recommended in docs. Prefer **sparse checkout** only when a machine must exclude paths (see [docs/README.md](docs/README.md)).

## Links

When linking from `services/*`, use paths relative to the file (for example `../../docs/planning/PLAN.md` from `services/letta/`).
