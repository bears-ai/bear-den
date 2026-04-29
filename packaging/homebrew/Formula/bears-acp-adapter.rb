class BearsAcpAdapter < Formula
  desc "BEARS ACP stdio adapter for Zed and other ACP clients"
  homepage "https://github.com/TheArtificial/BEARS"
  version "0.1.0"
  # License: see LICENSE in the upstream repo once added.

  on_macos do
    on_arm do
      url "https://github.com/TheArtificial/BEARS/releases/download/bears-acp-adapter%2Fv#{version}/bears-acp-adapter-aarch64-apple-darwin.tar.gz"
      sha256 "" # fill in from `sha256sum` output printed by the release workflow
    end

    on_intel do
      url "https://github.com/TheArtificial/BEARS/releases/download/bears-acp-adapter%2Fv#{version}/bears-acp-adapter-x86_64-apple-darwin.tar.gz"
      sha256 "" # fill in from `sha256sum` output printed by the release workflow
    end
  end

  def install
    bin.install "bears-acp-adapter"
  end

  test do
    # --help exits 0 and prints usage to stderr
    system "#{bin}/bears-acp-adapter", "--help"
  end
end
