# Bundled adapter resource

The SwiftPM executable target can optionally bundle an adapter resource here:

- `BearsApp/Resources/Adapter/bears-acp-adapter`

Populate it with:

```bash
cd apps/apple/Bears
bash Scripts/prepare_adapter.sh
```

If this file is absent, the app now falls back to downloading a macOS adapter artifact from GitHub using its configured download URL.
