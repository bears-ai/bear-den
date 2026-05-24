# Bear Den technical rename inventory

This document tracks remaining technical identifiers related to the BEARS → Bear Den rename.

Naming policy:

- **Product name:** Bear Den
- **Flagship service name:** Den
- **Preferred new technical namespace:** `den`
- **Conceptual manifest filename:** `bear.yaml`

This inventory is intentionally conservative. It distinguishes low-risk user-facing display changes from operational identifiers, compatibility-sensitive environment variables, infrastructure names, external published endpoints, and historical artifacts.

## Classification legend

| Class | Meaning | Default action |
|---|---|---|
| rename-now | Safe or desirable to rename soon with low compatibility risk. | Update directly when permissions allow. |
| migrate-with-alias | Should move to `den` or Bear Den naming, but requires temporary compatibility support. | Add new preferred name, keep old name working during transition. |
| defer-coordinated | Operational identifier that affects deployment, networking, images, backups, or service discovery. | Change only in a coordinated infra migration. |
| external-published | External URL, release path, registry path, or other published identifier not controlled only by local repo text changes. | Change only after republishing/repointing. |
| historical-generated | Logs, git metadata, generated artifacts, or machine history. | Do not rename. |

## Inventory

| Occurrence | File(s) | Category | Proposed replacement / policy | Class | Notes |
|---|---|---|---|---|---|
| `APP_DISPLAY_NAME=BEARS` | `.env.example` | User-facing config default | `APP_DISPLAY_NAME=Bear Den` | rename-now | Conceptually low risk, but `.env.example` is sensitive in this environment and may require explicit owner-mediated change. |
| `BEARS_ACP_ADAPTER_MANIFEST_URL` | `.devcontainer/install-workspace-tools.sh` | Env var | Prefer `DEN_ACP_ADAPTER_MANIFEST_URL`, keep old var supported | migrate-with-alias | Read new var first, then fall back to old var during transition. |
| `BEARS_ACP_ADAPTER_VERSION` | `.devcontainer/install-workspace-tools.sh` | Env var | Prefer `DEN_ACP_ADAPTER_VERSION`, keep old var supported | migrate-with-alias | Same compatibility strategy as above. |
| `BEARS_ACP_ADAPTER_CHANNEL` | `.devcontainer/install-workspace-tools.sh` | Env var | Prefer `DEN_ACP_ADAPTER_CHANNEL`, keep old var supported | migrate-with-alias | Same compatibility strategy as above. |
| `BEARS_ACP_ADAPTER_INSTALL_DIR` | `.devcontainer/install-workspace-tools.sh` | Env var | Prefer `DEN_ACP_ADAPTER_INSTALL_DIR`, keep old var supported | migrate-with-alias | Same compatibility strategy as above. |
| `bears-acp-adapter` binary/package/path name | `.devcontainer/install-workspace-tools.sh`, `tools/bears-acp-adapter/*` references | Binary/tool distribution name | Defer rename until artifact publishing and packaging migration | defer-coordinated | Renaming affects local build paths, installed binary names, and release artifacts. |
| `https://bears-ai.github.io/bear-den/...` | `.devcontainer/install-workspace-tools.sh` | Published URL | Migrate to the current published location under the new repo/org when artifacts are confirmed there | external-published | Historical location was under `TheArtificial/BEARS`; user clarified the GitHub location has moved to `bear-ai/bear-den`. Update local references only after the replacement published path is confirmed. |
| `https://github.com/TheArtificial/BEARS/releases/...` | `.devcontainer/install-workspace-tools.sh` | Published release URL | Migrate to the current release location under `bear-ai/bear-den` when release assets are confirmed there | external-published | Historical location was `TheArtificial/BEARS`; user clarified the repo has moved to `bear-ai/bear-den`. Keep references working until asset publishing/cutover is verified. |
| `bears-postgres`, `bears-letta-postgres`, `bears-bifrost`, `bears-redis`, `bears-memfs-manager`, `bears-letta`, `bears-codepool`, `bears-den` | `docker-compose.yaml`, `.env.example`, `.devcontainer/devcontainer.json` | Compose service names / DNS names | Case-by-case migration plan; prefer `den` for new naming, but do not bulk rename yet | defer-coordinated | These affect service discovery, container startup, scripts, docs, and operator expectations. |
| `name: bears-stack` and `bears-stack/*` backup/network naming | `docker-compose.yaml`, `.env.example`, `.devcontainer/logs/*` | Compose project / backup prefix / network name | Defer until coordinated infra migration | defer-coordinated | Renaming changes network names, backup paths, and local/dev assumptions. |
| `bears-dev`, `bears-devcontainer:latest` | `.devcontainer/devcontainer.json` | Devcontainer image/name | Decide in a separate dev tooling pass | defer-coordinated | Safe locally only if synchronized with onboarding/docs/scripts. |
| `bears-bifrost:configured`, `bears-den-dev:latest`, `bears-codepool-dev:latest`, `bears-letta-postgres:pg16-vector` | `.env.example`, `.devcontainer/logs/*`, `docker-compose.yaml` | Image names / tags | Defer until build/publish workflow is updated | defer-coordinated | Image names are operational identifiers, not merely display labels. |
| `APP_SLUG=bears-den` | `.env.example`, `docker-compose.yaml` | App slug / route identity | Review separately; do not assume direct rename to `den` | defer-coordinated | Slugs can affect URLs, cookies, app identity, and integrations. |
| `api.bears.artificial.design` | `.env.example` | Public domain | Change only with DNS/cert rollout plan | external-published | Domain migration requires external coordination. |
| `noreply@bears.artificial.design` | `.env.example` | Public email/domain | Change only with email/domain rollout plan | external-published | Depends on mail domain readiness and deliverability setup. |
| `http://bears-bifrost:8081/bears/models` | `docker-compose.yaml` | Internal metadata path / service route | Review with Bifrost owners; not a blind rename | defer-coordinated | May reflect an actual API path rather than branding only. |
| `SCALEWAY_BACKUP_PREFIX=bears-stack/volumes/bears-letta-data` | `.env.example`, `docker-compose.yaml` | Backup object prefix | Usually defer; migrate only with explicit data/retention plan | defer-coordinated | Storage prefixes often encode retention and restore assumptions. |
| GitHub org/repo references such as `https://github.com/bear-ai/bear-den.git` | `.git/config`, `.git/FETCH_HEAD`, `.git/logs/*` | Git remote / repository identity | Keep unless repo hosting changes again | historical-generated | `.git/*` is not a prose/config rename target. Historical references may still mention older locations such as `TheArtificial/BEARS` or `bears-ai/bear-den`; current canonical repo location is `bear-ai/bear-den`. |
| `.devcontainer/logs/*` `bears-*` entries | `.devcontainer/logs/startup.log` | Historical logs | Do not rename | historical-generated | Generated operational history, not source of truth. |
| `.git/*` `bears-*` entries | `.git/*` | VCS metadata | Do not rename | historical-generated | Machine-managed repository metadata. |

## Suggested migration order

1. **Display/config defaults**
   - Update user-facing defaults like `APP_DISPLAY_NAME` to `Bear Den`.
2. **Compatibility aliases for env vars**
   - Introduce preferred `DEN_ACP_ADAPTER_*` variables while still supporting `BEARS_ACP_ADAPTER_*`.
3. **Operator/dev tooling review**
   - Review devcontainer names, local image tags, and local-only scripts.
4. **Infra/service identifier migration**
   - Rename compose service names, network/project names, internal hostnames, and backup prefixes only in a coordinated cutover.
5. **External endpoint migration**
   - Move public URLs, artifact publishing paths, release endpoints, domains, and email identities only after external readiness.

## Proposed compatibility policy for ACP adapter env vars

When the ACP adapter install script is updated, prefer this resolution order:

- `DEN_ACP_ADAPTER_MANIFEST_URL`, else `BEARS_ACP_ADAPTER_MANIFEST_URL`
- `DEN_ACP_ADAPTER_VERSION`, else `BEARS_ACP_ADAPTER_VERSION`
- `DEN_ACP_ADAPTER_CHANNEL`, else `BEARS_ACP_ADAPTER_CHANNEL`
- `DEN_ACP_ADAPTER_INSTALL_DIR`, else `BEARS_ACP_ADAPTER_INSTALL_DIR`

This keeps the new `den` namespace preferred without breaking existing environments immediately.

## Repository location note

The user clarified that the GitHub repository location has already moved:

- historical location: `TheArtificial/BEARS`
- current location: `bear-ai/bear-den`

This means future migration work should treat `bear-ai/bear-den` as the canonical GitHub location, while older published URLs under `TheArtificial/BEARS` should be considered legacy references to be retired or redirected.

## Open questions

- Should `APP_SLUG` remain `bears-den` for continuity, or move later to a new slug?
- Should compose service names be migrated to `den-*` or to a mixed scheme that preserves component names?
- Does the Bifrost metadata route `/bears/models` represent a stable API contract that should remain unchanged for compatibility?
- When published artifacts move, should the ACP adapter binary itself remain `bears-acp-adapter` for backward compatibility, or be renamed with a shim/symlink strategy?
