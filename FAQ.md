# FAQ

Short answers to common architecture questions. See [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md) and [docs/planning/PLAN.md](docs/planning/PLAN.md) for detail.

## Why is web chat `browser → Den → Codepool → Letta` and not straight to Codepool?

The browser is untrusted and sessionless with respect to Codepool, so **Den is the gate**: it authenticates the user, checks bear membership, resolves the bear's `talk` role agent (`role_agent_id`) and runtime plan, and only then calls Codepool. Codepool is an internal harness; it trusts a service token, not end-user identity. **Channels bring their own app identity, signing, and workspace model**.
