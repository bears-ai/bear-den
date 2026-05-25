# 🐻 Bear Den — Basic Environment for Agent Runtimes Stack

**Bear Den** is the product name. Each product assistant is a **bear**: one logical assistant whose roles currently still use Letta-backed runtime pieces in some paths during migration, but whose target architecture is Den-owned role execution and control-plane state. **Den** (Rust, in `services/den/`) is the control plane: provisioning, **users↔bears** membership, first-party web chat (Deep Chat), and Cabinet when Outline is deployed.

This repo is a **light monorepo**: `docs/`, `services/den/` for Den, `services/codepool/` for Codepool, `services/memfs-manager/` for MemFS Manager, and supporting service assets under `services/`.

## Start here

| If you want to… | Open |
|-----------------|------|
| **Deploy** (Coolify; **recommended:** root Docker Compose `bears-*` app stack) | [docs/guides/deployment/deployment.md](docs/guides/deployment/deployment.md), [docker-compose.yaml](docker-compose.yaml) |
| **Roadmap & architecture** | [docs/roadmap/PLAN.md](docs/roadmap/PLAN.md), [docs/architecture/overview.md](docs/architecture/overview.md) |
| **Den + Letta + web chat** | [docs/architecture/den-architecture.md](docs/architecture/den-architecture.md) |
| **Every doc in one place** | [docs/README.md](docs/README.md) |
| **Troubleshoot ACP/Zed/Code token issues** | [docs/guides/acp-troubleshooting.md](docs/guides/acp-troubleshooting.md) |

**Stack (high level, current transitional state):** Letta → Bifrost for models; **Letta Code** harness for channels/web; **Den** for operators and browser chat; **Garage** for S3 artifacts; **Outline** + Den for Cabinet when you add shared knowledge. The migration direction is toward a **Den-native runtime** with Letta retained only as temporary compatibility scaffolding until removed.

**Quick deploy order:** use the root [`docker-compose.yaml`](docker-compose.yaml) for Bifrost + Letta + Codepool + Den on one network; add **`COMPOSE_PROFILES=bundled`** if you want the compose-bundled Postgres instead of a managed database — details in [DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md) (Garage for S3, Outline/Cabinet when ready).

---

**Coding agents & repo conventions** (GitOps, migrations, terminology, link rules): **[AGENTS.md](AGENTS.md)** — kept separate so this README stays short for humans.

*Assistant-oriented notes also live under [.kilocode/memory_bank/](.kilocode/memory_bank/).*

## License

Add a `LICENSE` at the repo root when you publish or distribute this configuration.
