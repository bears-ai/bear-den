# Den testing regime

Den uses layered checks. A passing build, lint, or readiness probe is not a substitute for a test that exercises the behavior changed.

## Local development

Run the smallest relevant checks first. Host-side compile-only checks normally use the checked-in SQLx metadata:

```bash
cd services/den
cargo fmt --check
SQLX_OFFLINE=true cargo check -p <touched-crate>
SQLX_OFFLINE=true cargo test -p <touched-crate> <focused-test-name>
```

A command that reports `0 tests` compiled a target but **did not validate the intended behavior**. Correct the test target, filter, features, or environment before claiming coverage.

For release or deployment-impacting changes, also build the production image:

```bash
cd services/den
docker build .
```

## Test layers

### Unit tests

Unit tests run in-process and should cover deterministic behavior, including ownership cancellation and transcript-repair logic. They should not require a live PostgreSQL instance.

Run the full unit layer with:

```bash
cd services/den
SQLX_OFFLINE=true cargo test --workspace --lib
```

### Domain integration regimes

Docket is the first domain-owned integration regime. Run it against a disposable Postgres database:

```bash
cd services/den
DATABASE_URL=postgres://postgres:postgres@localhost:5432/den_test \
  ./scripts/test-docket.sh all
```

Its explicit `policy`, `postgres`, `pair-loop`, and `recovery` lanes separate pure control rules, durable state, client-visible loop lifecycle, and stale-owner recovery. The runner rejects selectors that discover zero tests. The existing focus/settlement test is a Postgres control-plane test with immediate start and pre-settlement nonterminal regression assertions: it requires the exact focused host run to become visible, stay `running`/`continuing`, retain one matching running Docket attempt, and emit no terminal event before the test decides the outcome. It is not evidence that a Pair loop survives multiple bounded slices. See [Docket test regime](testing/docket.md) for the canonical-attempt contract, selectors, and fixture rules. Future domains add sibling runners (`test-cabinet.sh`, `test-armature.sh`, `test-runtime.sh`) rather than silently expanding a generic integration command.

### SQLx metadata and linting

The `Den – clippy gate` workflow runs on Den pull requests and relevant pushes. It verifies migration/query metadata freshness and runs strict workspace Clippy. It is a compile-and-lint gate, not behavior coverage.

### Container build

The image workflow builds and publishes deployable images on the deployment branches. It verifies the production Docker build and target platform, but does not replace unit or integration testing.

### Deployment checks

Deployment readiness and Git-SHA checks verify that the intended image started. Deterministic Pair/Docket focus-to-terminal coverage belongs in the Postgres integration lane and must not call an LLM. On a disposable development or UAT stack, `./scripts/smoke.sh` additionally runs one live Armature ACP focus cycle when a real provider key is configured; set `BEARS_LIVE_MODEL_SMOKE=off` to skip that cost-bearing check. Never run mutating smoke tests against production.

## CI policy

For changes under runtime, BearWire, Docket, or migrations, CI should require:

1. SQLx metadata freshness and strict Clippy;
2. workspace unit tests;
3. a Postgres-backed BearWire/Docket integration lane;
4. a production image build before a deployment-impacting release is considered complete.

The Postgres integration lane uses a service-container database. It must run the actual `#[sqlx::test]` target; verify that the selected target and filter discover tests before labeling it a pass.

## Definition of done for Pair/Docket lifecycle changes

- Focused unit regressions pass.
- The live-Postgres lifecycle tests execute and pass for both completed and blocked termination.
- No test command intended as evidence reports zero discovered tests.
- SQLx metadata and Clippy pass.
- Docker build passes for release/deployment-impacting changes, or its omission is explicitly recorded.
- A test-deployment smoke test confirms focus control is released after a terminal run.
