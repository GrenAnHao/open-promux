#!/usr/bin/env node

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const executable = process.platform === 'win32' ? 'openproxy.exe' : 'openproxy';
const binaryPath = path.join(__dirname, '..', 'vendor', executable);

if (!fs.existsSync(binaryPath)) {
  console.error(`OpenProxy binary not found at ${binaryPath}. Try reinstalling the package.`);
  process.exit(1);
}

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: false,
});

child.on('error', (error) => {
  console.error(error.message);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }

  process.exit(code ?? 0);
});
