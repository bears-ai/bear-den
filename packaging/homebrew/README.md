# Homebrew tap for bears-acp-adapter

## Setting up the tap

Create a public GitHub repo named `homebrew-bears` under the `TheArtificial` org, then copy `Formula/` into it:

```
TheArtificial/homebrew-bears/
  Formula/
    bears-acp-adapter.rb
```

Users can then install with:

```bash
brew tap TheArtificial/bears
brew install bears-acp-adapter
```

## Updating SHA256 hashes after a release

The release workflow (`.github/workflows/acp-adapter-release.yml`) prints SHA256 sums for all artifacts. After pushing a `bears-acp-adapter/v*` tag:

1. Find the "Print SHA256 sums" step in the `release` job output on GitHub Actions.
2. Copy the hashes for `bears-acp-adapter-aarch64-apple-darwin.tar.gz` and `bears-acp-adapter-x86_64-apple-darwin.tar.gz`.
3. Update the `sha256` fields in `Formula/bears-acp-adapter.rb` and bump `version`.
4. Push the updated formula to the `homebrew-bears` tap repo.

## macOS Gatekeeper note

Binaries downloaded via Homebrew are automatically cleared of the quarantine attribute. If installing the binary manually (not via `brew`), run:

```bash
xattr -d com.apple.quarantine /path/to/bears-acp-adapter
```
