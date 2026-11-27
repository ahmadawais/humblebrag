#!/usr/bin/env node
'use strict';
// Runs the prebuilt binary from the optional package npm installed for this OS and CPU.
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');

const key = `${process.platform}-${process.arch}`;
const exe = process.platform === 'win32' ? 'humblebrag.exe' : 'humblebrag';
let bin;
try {
	bin = require.resolve(`humblebrag-${key}/bin/${exe}`);
} catch {
	console.error(
		`humblebrag: no prebuilt binary for ${key}.\n` +
			'Supported: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64.\n' +
			'If you installed with --no-optional or --omit=optional, reinstall without it.\n' +
			'Build from source: https://github.com/ahmadawais/humblebrag'
	);
	process.exit(1);
}
if (process.platform !== 'win32') {
	try {
		fs.accessSync(bin, fs.constants.X_OK);
	} catch {
		try {
			fs.chmodSync(bin, 0o755);
		} catch {}
	}
}
const r = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });
if (r.error) {
	console.error(`humblebrag: ${r.error.message}`);
	process.exit(1);
}
process.exit(r.status ?? 1);
