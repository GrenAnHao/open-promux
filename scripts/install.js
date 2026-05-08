#!/usr/bin/env node

const childProcess = require('node:child_process');
const fs = require('node:fs');
const https = require('node:https');
const os = require('node:os');
const path = require('node:path');

const packageJson = require('../package.json');

const REPOSITORY = 'GrenAnHao/openai-responses-proxy';
const VERSION = packageJson.version;
const TAG = `v${VERSION}`;

function platformAsset() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === 'win32' && arch === 'x64') {
    return {
      archive: 'openproxy-x86_64-pc-windows-msvc.zip',
      binary: 'openproxy.exe',
    };
  }

  if (platform === 'linux' && arch === 'x64') {
    return {
      archive: 'openproxy-x86_64-unknown-linux-gnu.tar.gz',
      binary: 'openproxy',
    };
  }

  if (platform === 'darwin' && arch === 'x64') {
    return {
      archive: 'openproxy-x86_64-apple-darwin.tar.gz',
      binary: 'openproxy',
    };
  }

  if (platform === 'darwin' && arch === 'arm64') {
    return {
      archive: 'openproxy-aarch64-apple-darwin.tar.gz',
      binary: 'openproxy',
    };
  }

  throw new Error(`Unsupported platform: ${platform}-${arch}`);
}

function download(url, destination) {
  return new Promise((resolve, reject) => {
    const request = https.get(
      url,
      {
        headers: {
          'User-Agent': `${packageJson.name}/${VERSION}`,
        },
      },
      (response) => {
        if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
          response.resume();
          download(response.headers.location, destination).then(resolve, reject);
          return;
        }

        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`Failed to download ${url}: HTTP ${response.statusCode}`));
          return;
        }

        const file = fs.createWriteStream(destination);
        response.pipe(file);
        file.on('finish', () => file.close(resolve));
        file.on('error', reject);
      },
    );

    request.on('error', reject);
  });
}

async function downloadWithRetries(url, destination) {
  const maxAttempts = 3;

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      await download(url, destination);
      return;
    } catch (error) {
      fs.rmSync(destination, { force: true });

      if (attempt === maxAttempts) {
        throw error;
      }

      console.warn(`Download failed (${attempt}/${maxAttempts}): ${error.message}`);
      await new Promise((resolve) => setTimeout(resolve, 1000 * attempt));
    }
  }
}

function run(command, args) {
  childProcess.execFileSync(command, args, {
    stdio: 'inherit',
  });
}

function extract(archivePath, destination) {
  fs.mkdirSync(destination, { recursive: true });

  if (archivePath.endsWith('.zip')) {
    run('powershell', [
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-Command',
      `Expand-Archive -LiteralPath ${JSON.stringify(archivePath)} -DestinationPath ${JSON.stringify(destination)} -Force`,
    ]);
    return;
  }

  run('tar', ['-xzf', archivePath, '-C', destination]);
}

async function main() {
  const asset = platformAsset();
  const packageRoot = path.resolve(__dirname, '..');
  const vendorDir = path.join(packageRoot, 'vendor');
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'openproxy-'));
  const archivePath = path.join(tempDir, asset.archive);
  const extractDir = path.join(tempDir, 'extract');
  const downloadUrl = `https://github.com/${REPOSITORY}/releases/download/${TAG}/${asset.archive}`;

  fs.mkdirSync(vendorDir, { recursive: true });

  console.log(`Downloading OpenProxy ${TAG} from ${downloadUrl}`);
  await downloadWithRetries(downloadUrl, archivePath);
  extract(archivePath, extractDir);

  const sourceBinary = path.join(extractDir, asset.binary);
  const targetBinary = path.join(vendorDir, asset.binary);

  if (!fs.existsSync(sourceBinary)) {
    throw new Error(`Release archive did not contain ${asset.binary}`);
  }

  fs.copyFileSync(sourceBinary, targetBinary);

  if (process.platform !== 'win32') {
    fs.chmodSync(targetBinary, 0o755);
  }

  fs.rmSync(tempDir, { recursive: true, force: true });
  console.log(`Installed OpenProxy binary to ${targetBinary}`);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
