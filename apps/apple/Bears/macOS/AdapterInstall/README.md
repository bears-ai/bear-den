# AdapterInstall

macOS-specific adapter installation, replacement, and repair logic.

Phase-0 responsibilities:

- resolve Application Support paths;
- copy bundled adapter into the managed per-user location;
- ensure executable permissions;
- compare bundled and installed versions;
- support repair/reinstall behavior;
- record install-state metadata.
