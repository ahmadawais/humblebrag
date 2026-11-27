#!/usr/bin/env node
'use strict';
// Runs the humblebrag binary for this OS and CPU. On first run it downloads the
// binary from this version's GitHub release, checks it against the SHA-256 sum
// shipped in this package, and caches it.
const { spawnSync } = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { version } = require('../package.json');
const sums = require('./checksums.json');
const REPO = 'ahmadawais/humblebrag';

const key = `${process.platform}-${process.arch}`;
const asset = `humblebrag-${key}${process.platform === 'win32' ? '.exe' : ''}`;
const cacheRoot =
	process.env.XDG_CACHE_HOME ||
	(process.platform === 'win32' ? process.env.LOCALAPPDATA || os.tmpdir() : path.join(os.homedir(), '.cache'));
const bin = path.join(cacheRoot, 'humblebrag', 'bin', version, asset);

const fail = (msg) => {
	console.error(`humblebrag: ${msg}`);
	process.exit(1);
};

async function download() {
	const want = sums[asset];
	if (!want) {
		fail(
			`no prebuilt binary for ${key}.\n` +
				`Supported: ${Object.keys(sums).join(', ')}.\n` +
				`Build from source: https://github.com/${REPO}`
		);
	}
	const url = `https://github.com/${REPO}/releases/download/v${version}/${asset}`;
	process.stderr.write(`humblebrag: downloading v${version} for ${key}... `);
	let buf;
	try {
		const res = await fetch(url);
		if (!res.ok) throw new Error(`HTTP ${res.status}`);
		buf = Buffer.from(await res.arrayBuffer());
	} catch (e) {
		process.stderr.write('\n');
		fail(`download failed (${e.message}): ${url}`);
	}
	const got = crypto.createHash('sha256').update(buf).digest('hex');
	if (got !== want) {
		process.stderr.write('\n');
		fail(`checksum mismatch for ${asset}: expected ${want}, got ${got}. Not running it.`);
	}
	fs.mkdirSync(path.dirname(bin), { recursive: true });
	const tmp = `${bin}.${process.pid}.tmp`;
	fs.writeFileSync(tmp, buf, { mode: 0o755 });
	fs.renameSync(tmp, bin);
	process.stderr.write('done\n');
}

(async () => {
	if (!fs.existsSync(bin)) await download();
	const r = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });
	if (r.error) fail(r.error.message);
	process.exit(r.status ?? 1);
})();
