# BEARS documentation

Human-oriented docs for the BEARS stack.

**Start here:** [README.md](../README.md) at the repository root. **Agent/repo conventions:** [AGENTS.md](../AGENTS.md).

## Layout

| Path | Contents |
|------|----------|
| [architecture/](architecture/) | Conceptual models, stable contracts, architecture overviews, and system schemas |
| [decisions/](decisions/) | Architecture Decision Records (ADRs), named `adr-####-slug.md` |
| [guides/](guides/) | Human guides, deployment docs, contributor notes, troubleshooting, and operational runbooks |
| [roadmap/](roadmap/) | Active planning, implementation sequencing, and archived roadmap materials |
| [../services/codepool/](../services/codepool/) | Codepool runtime service |

## Suggested entry points

- [architecture/README.md](architecture/README.md)
- [decisions/README.md](decisions/README.md)
- [guides/README.md](guides/README.md)
- [roadmap/PLAN.md](roadmap/PLAN.md)

Service-specific runbooks may also stay next to service configs under `services/*` where appropriate.

## Cloning and automation

This repo is a light monorepo: documentation and `services/*` deploy artifacts share one Git history.

- Shallow clone if you only need a subset:

  ```bash
  git clone --depth 1 <repo-url>
  ```

- Sparse checkout is optional when machines should only materialize selected paths:

  ```bash
  git clone --filter=blob:none <repo-url> bears-deploy
  cd bears-deploy
  git sparse-checkout init --cone
  git sparse-checkout set services/bifrost docs README.md AGENTS.md
  ```

## Assistant-oriented material

Tooling notes for coding agents live at the repository root in **[AGENTS.md](../AGENTS.md)**. The [`.kilocode/memory_bank/`](../.kilocode/memory_bank/) directory is aligned with the same BEARS vocabulary but is not end-user documentation.
