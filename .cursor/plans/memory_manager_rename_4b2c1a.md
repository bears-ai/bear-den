---
name: Keep MemFS Manager naming
overview: "Decision: do not rename the git smart-HTTP memfs service to a separate product name or slug. Use the human-readable name MemFS Manager, keep the Docker/service identifier bears-memfs-manager, and preserve upstream Letta terms such as LETTA_MEMFS_SERVICE_URL and on-disk memfs paths."
todos:
  - id: cancel-mem-manager-product-rename
    content: "Do not pursue a separate Memory Manager / mem-manager product rename; use MemFS Manager as the human-readable name."
    status: completed
  - id: keep-current-service-name
    content: "Keep Docker Compose service name `bears-memfs-manager` and default URL `http://bears-memfs-manager:8285`."
    status: completed
  - id: update-related-plans
    content: "Remove the rename prerequisite from related plans and refer to the human-readable service as MemFS Manager."
    status: completed
isProject: false
---

# Keep MemFS Manager naming

Decision: **do not** rename the BEARS git smart-HTTP memfs service to a separate product name/slug such as `mem-manager`.

Use this vocabulary:

- **Human-readable name:** MemFS Manager
- **Compose service:** `bears-memfs-manager`
- **Internal URL:** `http://bears-memfs-manager:8285`
- **Service description:** MemFS Manager, the git smart-HTTP service for Letta memfs repositories
- **Letta env var:** `LETTA_MEMFS_SERVICE_URL` stays unchanged
- **On-disk paths:** `/root/.letta/memfs/repository`, `MEMFS_BASE`, and related upstream `memfs` terms stay unchanged

## Consequences

- The bear private-memory UI plan should not depend on any rename plan.
- New docs and UI text should use **MemFS Manager** as the human-readable service label.
- Keep technical identifiers such as `bears-memfs-manager`, `LETTA_MEMFS_SERVICE_URL`, and `memfs` paths as-is.
- Do not introduce a separate `mem-manager` product slug.
