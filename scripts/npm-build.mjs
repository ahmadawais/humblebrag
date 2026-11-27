#!/usr/bin/env node
// Builds the `humblebrag` npm package: a small Node launcher plus the SHA-256
// sum of every platform binary. The binaries stay on the GitHub release; the
// launcher downloads the right one on first run and checks it against the sum.
//   node scripts/npm-build.mjs --version 0.1.0 --sums <SHA256SUMS> --out <dir>
import fs from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';

const { values: a } = parseArgs({
	options: { version: { type: 'string' }, sums: { type: 'string' }, out: { type: 'string' } },
});
if (!a.version || !a.sums || !a.out) {
	console.error('usage: npm-build.mjs --version <x.y.z> --sums <SHA256SUMS> --out <dir>');
	process.exit(2);
}
const root = path.resolve(import.meta.dirname, '..');
const platforms = JSON.parse(fs.readFileSync(path.join(root, 'npm/platforms.json'), 'utf8'));

// SHA256SUMS lines look like "<hex>  humblebrag-darwin-arm64".
const all = Object.fromEntries(
	fs
		.readFileSync(a.sums, 'utf8')
		.trim()
		.split('\n')
		.map((l) => l.trim().split(/\s+\*?/).reverse())
);
const sums = {};
for (const [key, p] of Object.entries(platforms)) {
	const file = `humblebrag-${key}${p.os === 'win32' ? '.exe' : ''}`;
	if (!/^[0-9a-f]{64}$/.test(all[file] || '')) {
		console.error(`no checksum for ${file} in ${a.sums}`);
		process.exit(1);
	}
	sums[file] = all[file];
}

const pkg = path.join(a.out, 'humblebrag');
fs.rmSync(a.out, { recursive: true, force: true });
fs.mkdirSync(path.join(pkg, 'bin'), { recursive: true });
const manifest = {
	name: 'humblebrag',
	version: a.version,
	description: 'How many times have your npm packages been downloaded? All of it, since 2015.',
	keywords: ['npm', 'downloads', 'stats', 'cli', 'npm-stat'],
	license: 'MIT',
	author: 'Ahmad Awais (https://x.com/MrAhmadAwais)',
	homepage: 'https://github.com/ahmadawais/humblebrag',
	repository: { type: 'git', url: 'git+https://github.com/ahmadawais/humblebrag.git' },
	bin: { humblebrag: 'bin/humblebrag.js' },
	files: ['bin'],
	engines: { node: '>=18' },
};
fs.writeFileSync(path.join(pkg, 'package.json'), JSON.stringify(manifest, null, '\t') + '\n');
fs.writeFileSync(path.join(pkg, 'bin/checksums.json'), JSON.stringify(sums, null, '\t') + '\n');
fs.copyFileSync(path.join(root, 'npm/bin/humblebrag.js'), path.join(pkg, 'bin/humblebrag.js'));
fs.chmodSync(path.join(pkg, 'bin/humblebrag.js'), 0o755);
for (const f of ['README.md', 'LICENSE']) fs.copyFileSync(path.join(root, f), path.join(pkg, f));
console.log(`built humblebrag@${a.version} (launcher + ${Object.keys(sums).length} checksums) in ${pkg}`);
