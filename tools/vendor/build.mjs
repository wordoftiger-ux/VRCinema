// Builds public/vendor from the npm packages of the pinned versions (no bundler):
//
//   mkdir <empty temp dir> && cd <it>
//   npm pack preact@10.29.8 htm@3.1.1 @preact/signals@1.3.4 @preact/signals-core@1.14.4 hls.js@1.7.3
//   for each .tgz: mkdir <name> && tar -xzf <name>.tgz -C <name>
//   node <repo>/tools/vendor/build.mjs <that temp dir>
//
// Each library's ESM build is copied; the bare imports between them ("preact",
// "preact/hooks", "@preact/signals-core") become relative paths, so the browser loads them
// without an import map; the source-map comment is dropped (the maps are not shipped).
// manifest.json records the version, the file taken from the package, the SHA-256 of that
// file and of the shipped one. The engine's test checks the shipped sums.

import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const packs = process.argv[2];
if (!packs) {
  console.error('usage: node tools/vendor/build.mjs <folder with the unpacked npm packages>');
  process.exit(2);
}
const out = join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'public', 'vendor');

const LIBS = [
  { file: 'preact.mjs', pkg: 'preact', version: '10.29.8', dir: 'preact-10.29.8', from: 'dist/preact.module.js' },
  { file: 'hooks.mjs', pkg: 'preact', version: '10.29.8', dir: 'preact-10.29.8', from: 'hooks/dist/hooks.module.js' },
  { file: 'htm.mjs', pkg: 'htm', version: '3.1.1', dir: 'htm-3.1.1', from: 'dist/htm.module.js' },
  { file: 'signals-core.mjs', pkg: '@preact/signals-core', version: '1.14.4', dir: 'preact-signals-core-1.14.4', from: 'dist/signals-core.module.js' },
  { file: 'signals.mjs', pkg: '@preact/signals', version: '1.3.4', dir: 'preact-signals-1.3.4', from: 'dist/signals.module.js' },
  { file: 'hls.mjs', pkg: 'hls.js', version: '1.7.3', dir: 'hls.js-1.7.3', from: 'dist/hls.min.mjs' },
];

// Longest first: "preact/hooks" before "preact".
const IMPORTS = [
  ['"preact/hooks"', '"./hooks.mjs"'],
  ['"@preact/signals-core"', '"./signals-core.mjs"'],
  ['"preact"', '"./preact.mjs"'],
];

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

const manifest = {
  note: 'Libraries of the interface, bundled (never loaded from the internet). Built by tools/vendor/build.mjs from the npm packages below: ESM build copied, bare imports between them made relative, source-map comment removed. sha256 = shipped file (checked by the engine tests); upstreamSha256 = the file in the package.',
  files: {},
};

for (const lib of LIBS) {
  const source = readFileSync(join(packs, lib.dir, 'package', lib.from));
  const pkg = JSON.parse(readFileSync(join(packs, lib.dir, 'package', 'package.json'), 'utf8'));
  if (pkg.name !== lib.pkg || pkg.version !== lib.version) {
    throw new Error(`${lib.dir}: package.json says ${pkg.name}@${pkg.version}`);
  }
  let text = source.toString('utf8');
  for (const [from, to] of IMPORTS) {
    text = text.replaceAll(`from${from}`, `from${to}`).replaceAll(`from ${from}`, `from ${to}`);
  }
  text = text.replace(/\n?\/\/# sourceMappingURL=\S+\s*$/, '\n');
  const bad = text.match(/from\s*"(?!\.\/)[^"]*"/);
  if (bad && lib.file !== 'hls.mjs') throw new Error(`${lib.file}: bare import left: ${bad[0]}`);
  const shipped = Buffer.from(text, 'utf8');
  writeFileSync(join(out, lib.file), shipped);
  manifest.files[lib.file] = {
    package: lib.pkg,
    version: lib.version,
    from: lib.from,
    license: pkg.license,
    upstreamSha256: sha256(source),
    sha256: sha256(shipped),
  };
}

// The licences go along with the code (MIT for Preact, htm and signals; Apache-2.0 for hls.js).
const seen = new Set();
let licences = 'Licences of the libraries in this folder (see manifest.json for versions).\n';
for (const lib of LIBS) {
  if (seen.has(lib.dir)) continue;
  seen.add(lib.dir);
  const text = readFileSync(join(packs, lib.dir, 'package', 'LICENSE'), 'utf8').replace(/\r\n/g, '\n');
  licences += `\n==== ${lib.pkg}@${lib.version} ====\n\n${text.trim()}\n`;
}
const licenceBytes = Buffer.from(licences, 'utf8');
writeFileSync(join(out, 'LICENSES.txt'), licenceBytes);
manifest.files['LICENSES.txt'] = { sha256: sha256(licenceBytes) };

writeFileSync(join(out, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
console.log(Object.entries(manifest.files).map(([f, m]) => `${f}  ${m.package}@${m.version}  ${m.sha256}`).join('\n'));
