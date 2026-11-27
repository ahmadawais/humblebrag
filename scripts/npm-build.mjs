#!/usr/bin/env node
// Builds the single `humblebrag` npm package: a Node launcher plus the
// prebuilt binary for every platform in bin/.
//   node scripts/npm-build.mjs --version 0.1.0 --bins <dir> --out <dir>
// <dir> for --bins holds humblebrag-<platform>[.exe], e.g. humblebrag-darwin-arm64.
import fs from 'node:fs';
import path from 'node:path';
import { parseArgs } from 'node:util';

const { values: a } = parseArgs({
	options: { version: { type: 'string' }, bins: { type: 'string' }, out: { type: 'string' } },
});
if (!a.version || !a.bins || !a.out) {
	console.error('usage: npm-build.mjs --version <x.y.z> --bins <dir> --out <dir>');
	process.exit(2);
}
const root = path.resolve(import.meta.dirname, '..');
const platforms = JSON.parse(fs.readFileSync(path.join(root, 'npm/platforms.json'), 'utf8'));
const pkg = path.join(a.out, 'humblebrag');
const bin = path.join(pkg, 'bin');
fs.rmSync(a.out, { recursive: true, force: true });
fs.mkdirSync(bin, { recursive: true });

for (const [key, p] of Object.entries(platforms)) {
	const file = `humblebrag-${key}${p.os === 'win32' ? '.exe' : ''}`;
	const src = path.join(a.bins, file);
	if (!fs.existsSync(src)) {
		console.error(`missing ${src}`);
		process.exit(1);
	}
	fs.copyFileSync(src, path.join(bin, file));
	fs.chmodSync(path.join(bin, file), 0o755);
}

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
	os: [...new Set(Object.values(platforms).map((p) => p.os))],
	cpu: [...new Set(Object.values(platforms).map((p) => p.cpu))],
	engines: { node: '>=16' },
};
fs.writeFileSync(path.join(pkg, 'package.json'), JSON.stringify(manifest, null, '\t') + '\n');
fs.copyFileSync(path.join(root, 'npm/bin/humblebrag.js'), path.join(bin, 'humblebrag.js'));
fs.chmodSync(path.join(bin, 'humblebrag.js'), 0o755);
for (const f of ['README.md', 'LICENSE']) fs.copyFileSync(path.join(root, f), path.join(pkg, f));
console.log(`built humblebrag@${a.version} with ${Object.keys(platforms).length} binaries in ${pkg}`);
