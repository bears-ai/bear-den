# FAQ

Short answers to common architecture questions. See [docs/architecture/DEN_ARCHITECTURE.md](docs/architecture/DEN_ARCHITECTURE.md) and [docs/planning/PLAN.md](docs/planning/PLAN.md) for detail.

## Why is web chat `browser → Den → Codepool → Letta` and not straight to Codepool?

The browser is untrusted and sessionless with respect to Codepool, so **Den is the gate**: it authenticates the user, checks bear membership, loads bear config (`letta_agent_id`, runtime plan), and only then calls Codepool. Codepool is an internal harness; it trusts a service token, not end-user identity.

## Do Slack and other Letta Code channels need to go through Den too?

**No** — that is not the default BEARS shape. **Slack brings its own app identity, signing, and workspace model**; the Letta Code channel layer connects **Slack → Letta Code → Letta** without Den in the message path. Den still owns provisioning, bear registry, membership, and materialized harness config upstream of runtime.

## If channels skip Den, do they still make sense next to web?

Yes. Channels give you the same **Letta Code** harness (skills, tools, Letta persistence) with **channel-native** behavior. Web and channels differ in **where trust lives** (Den session vs Slack’s model), not in whether the harness is valuable.

## When would Den sit in the middle of channel traffic?

Only if you explicitly want a **single audit or policy checkpoint** for every channel payload — optional, not required for correctness. See “Canonical paths vs optional channel proxy” in [docs/planning/PLAN.md](docs/planning/PLAN.md).
