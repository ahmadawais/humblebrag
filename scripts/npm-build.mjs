#!/usr/bin/env node
// Builds the npm packages: one per platform with its binary, plus the main
// `humblebrag` package that depends on all of them as optionalDependencies.
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
const common = {
	version: a.version,
	license: 'MIT',
	author: 'Ahmad Awais (https://x.com/MrAhmadAwais)',
	homepage: 'https://github.com/ahmadawais/humblebrag',
	repository: { type: 'git', url: 'git+https://github.com/ahmadawais/humblebrag.git' },
};
const write = (dir, file, data) => {
	fs.mkdirSync(dir, { recursive: true });
	fs.writeFileSync(path.join(dir, file), typeof data === 'string' ? data : JSON.stringify(data, null, '\t') + '\n');
};
fs.rmSync(a.out, { recursive: true, force: true });

const built = [];
for (const [key, p] of Object.entries(platforms)) {
	const exe = p.os === 'win32' ? '.exe' : '';
	const src = path.join(a.bins, `humblebrag-${key}${exe}`);
	if (!fs.existsSync(src)) {
		console.warn(`skip ${key}: ${src} not found`);
		continue;
	}
	const name = `humblebrag-${key}`;
	const dir = path.join(a.out, name);
	write(dir, 'package.json', {
		name,
		...common,
		description: `The ${key} binary for humblebrag.`,
		os: [p.os],
		cpu: [p.cpu],
		files: ['bin'],
		preferUnplugged: true,
	});
	write(dir, 'README.md', `# ${name}\n\nThe ${key} binary for [humblebrag](https://github.com/ahmadawais/humblebrag). Install \`humblebrag\` instead.\n`);
	fs.mkdirSync(path.join(dir, 'bin'), { recursive: true });
	const dest = path.join(dir, 'bin', `humblebrag${exe}`);
	fs.copyFileSync(src, dest);
	fs.chmodSync(dest, 0o755);
	built.push(name);
}

const main = path.join(a.out, 'humblebrag');
write(main, 'package.json', {
	name: 'humblebrag',
	...common,
	description: 'How many times have your npm packages been downloaded? All of it, since 2015.',
	keywords: ['npm', 'downloads', 'stats', 'cli', 'npm-stat'],
	bin: { humblebrag: 'bin/humblebrag.js' },
	files: ['bin'],
	engines: { node: '>=16' },
	optionalDependencies: Object.fromEntries(Object.keys(platforms).map((k) => [`humblebrag-${k}`, a.version])),
});
fs.mkdirSync(path.join(main, 'bin'), { recursive: true });
fs.copyFileSync(path.join(root, 'npm/bin/humblebrag.js'), path.join(main, 'bin/humblebrag.js'));
fs.chmodSync(path.join(main, 'bin/humblebrag.js'), 0o755);
for (const f of ['README.md', 'LICENSE']) fs.copyFileSync(path.join(root, f), path.join(main, f));
console.log(`built ${built.length} platform packages + humblebrag@${a.version} in ${a.out}`);
