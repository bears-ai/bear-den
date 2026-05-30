# Bundled adapter resource

The SwiftPM executable target expects the adapter resource here:

- `BearsApp/Resources/Adapter/bears-acp-adapter`

Populate it with:

```bash
cd apps/apple/Bears
bash Scripts/prepare_adapter.sh
```

This keeps the resource path inside the SwiftPM target tree so it can be copied reliably into build products.
