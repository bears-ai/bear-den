# Adapter bundle resources

The app expects a bundled `bears-acp-adapter` executable at:

- `Resources/Adapter/bears-acp-adapter`

For local development, populate that path with:

```bash
cd apps/apple/Bears
bash Scripts/prepare_adapter.sh
```

The script prefers an already-built adapter under either the workspace-level `target/` directory or the crate-local `tools/bears-acp-adapter/target/` directory, and only falls back to `cargo build` when needed and available.

For release-style local testing:

```bash
cd apps/apple/Bears
PROFILE=release bash Scripts/prepare_adapter.sh
```

To use an explicit prebuilt adapter binary:

```bash
cd apps/apple/Bears
ADAPTER_BINARY=/path/to/bears-acp-adapter bash Scripts/prepare_adapter.sh
```

This setup is intentionally designed to map cleanly onto future GitHub Actions builds, where CI can run the same preparation step before compiling and packaging the app.
