# 🐻 BEARS — Basic Environment for Agents Runtime Server

**BEARS** is the stack name. Each product assistant is a **bear** (one [Letta](https://github.com/letta-ai/letta) agent). **Den** (Rust, in `den/`) is the control plane: provisioning, **users↔bears** membership, first-party web chat (Deep Chat), and Cabinet when Outline is deployed.

This repo is a **light monorepo**: `docs/`, Coolify-oriented **`services/`**, and **`den/`** for the Den application (not under `services/`).

## Start here

| If you want to… | Open |
|-----------------|------|
| **Deploy** (Coolify order, services) | [docs/deployment/DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md) |
| **Roadmap & architecture** | [docs/planning/PLAN.md](docs/planning/PLAN.md), [docs/architecture/ARCHITECTURE_NOTES.md](docs/architecture/ARCHITECTURE_NOTES.md) |
| **Den + Letta + web chat** | [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md) |
| **Every doc in one place** | [docs/README.md](docs/README.md) |

**Stack (high level):** Letta → Bifrost for models; **Letta Code** harness for channels/web; **Den** for operators and browser chat; **Garage** for S3 artifacts; **Outline** + Den for Cabinet when you add shared knowledge. Self-hosted Letta only.

**Quick deploy order:** Bifrost → Garage → Letta → Den → Outline/Cabinet when ready — details in [DEPLOYMENT.md](docs/deployment/DEPLOYMENT.md).

---

**Coding agents & repo conventions** (GitOps, migrations, terminology, link rules): **[AGENTS.md](AGENTS.md)** — kept separate so this README stays short for humans.

*Assistant-oriented notes also live under [.kilocode/memory_bank/](.kilocode/memory_bank/).*

## License

Add a `LICENSE` at the repo root when you publish or distribute this configuration.
