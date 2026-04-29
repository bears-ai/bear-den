#!/usr/bin/env node
// Downloads the platform-appropriate bears-acp-adapter binary from GitHub Releases.
// Runs automatically as a postinstall script.
'use strict';

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const os = require('os');

const VERSION = require('./package.json').version;
const BIN_DIR = path.join(__dirname, 'bin');
const EXE = process.platform === 'win32' ? 'bears-acp-adapter.exe' : 'bears-acp-adapter';
const BIN_PATH = path.join(BIN_DIR, EXE);

const PLATFORM_MAP = {
  'darwin:arm64': 'aarch64-apple-darwin',
  'darwin:x64': 'x86_64-apple-darwin',
  'linux:x64': 'x86_64-unknown-linux-musl',
  'win32:x64': 'x86_64-pc-windows-msvc',
};

const key = `${process.platform}:${process.arch}`;
const triple = PLATFORM_MAP[key];

if (!triple) {
  console.warn(`bears-acp-adapter: unsupported platform ${key} — skipping binary download.`);
  console.warn('Build from source: cargo build --release --manifest-path tools/bears-acp-adapter/Cargo.toml');
  process.exit(0);
}

const ext = process.platform === 'win32' ? '.zip' : '.tar.gz';
const filename = `bears-acp-adapter-${triple}${ext}`;
const tag = `bears-acp-adapter%2Fv${VERSION}`;
const url = `https://github.com/TheArtificial/BEARS/releases/download/${tag}/${filename}`;
const tmp = path.join(os.tmpdir(), filename);

function download(url, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 5) return reject(new Error('Too many redirects'));
    https.get(url, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        res.resume();
        return download(res.headers.location, dest, redirects + 1).then(resolve, reject);
      }
      if (res.statusCode !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${res.statusCode} fetching ${url}`));
      }
      const file = fs.createWriteStream(dest);
      res.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', reject);
    }).on('error', reject);
  });
}

async function main() {
  console.log(`bears-acp-adapter: downloading ${filename}…`);

  try {
    await download(url, tmp);
  } catch (err) {
    console.warn(`bears-acp-adapter: download failed: ${err.message}`);
    console.warn('Build from source: cargo build --release --manifest-path tools/bears-acp-adapter/Cargo.toml');
    process.exit(0);
  }

  fs.mkdirSync(BIN_DIR, { recursive: true });

  if (process.platform === 'win32') {
    execSync(`powershell.exe -NoProfile -Command "Expand-Archive -Force '${tmp}' '${BIN_DIR}'"`, { stdio: 'inherit' });
  } else {
    execSync(`tar -xzf '${tmp}' -C '${BIN_DIR}'`, { stdio: 'inherit' });
  }

  try { fs.unlinkSync(tmp); } catch (_) {}

  if (process.platform !== 'win32') {
    fs.chmodSync(BIN_PATH, 0o755);
  }

  console.log(`bears-acp-adapter: installed to ${BIN_PATH}`);
}

main().catch((err) => {
  console.error(`bears-acp-adapter: install error: ${err.message}`);
  process.exit(0); // non-fatal so npm install does not fail
});
