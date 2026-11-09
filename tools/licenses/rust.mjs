// Writes the Rust part of docs/third-party-licenses.md from `cargo metadata` (the Windows
// program: desktop/ with the engine): the direct dependencies of engine/ and desktop/ with their
// licences, and how many of all the compiled-in crates carry each licence.
//
//   node tools/licenses/rust.mjs          (from the repository; cargo must be on PATH)
//
// The part between the two markers in the document is replaced; nothing else changes.

import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const doc = join(root, 'docs', 'third-party-licenses.md');
const BEGIN = '<!-- rust-dependencies:begin (node tools/licenses/rust.mjs) -->';
const END = '<!-- rust-dependencies:end -->';

const meta = JSON.parse(
  execFileSync(
    'cargo',
    ['metadata', '--format-version', '1', '--offline', '--filter-platform', 'x86_64-pc-windows-msvc', '--manifest-path', join(root, 'desktop', 'Cargo.toml')],
    { maxBuffer: 256 * 1024 * 1024, encoding: 'utf8' },
  ),
);
const byId = new Map(meta.packages.map((p) => [p.id, p]));
const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));

// Every crate compiled into the program: normal dependencies from the root, transitively.
const shipped = new Set();
const stack = [meta.resolve.root];
while (stack.length) {
  const id = stack.pop();
  if (shipped.has(id)) continue;
  shipped.add(id);
  for (const d of nodes.get(id).deps) if (d.dep_kinds.some((k) => k.kind === null)) stack.push(d.pkg);
}
const external = [...shipped].map((id) => byId.get(id)).filter((p) => p.source != null);

const row = (cells) => `| ${cells.join(' | ')} |`;
const lines = [];
for (const crate of ['vrcinema', 'vrcinema-desktop']) {
  const own = meta.packages.find((p) => p.name === crate && p.source == null);
  const node = nodes.get(own.id);
  lines.push('', `Direct dependencies of \`${crate === 'vrcinema' ? 'engine/' : 'desktop/'}\`:`, '');
  lines.push(row(['Crate', 'Version', 'Licence', 'Used as']), row(['---', '---', '---', '---']));
  const seen = new Set();
  for (const dep of own.dependencies) {
    if (dep.kind === 'dev' || dep.path || seen.has(dep.name)) continue;
    seen.add(dep.name);
    const resolved = node.deps.map((d) => byId.get(d.pkg)).find((p) => p.name === dep.name);
    if (!resolved) continue;
    const link = resolved.repository ? `[${resolved.name}](${resolved.repository})` : resolved.name;
    const use = [dep.kind === 'build' ? 'build only' : 'compiled in', dep.target ? `\`${dep.target}\`` : ''].filter(Boolean).join(', ');
    lines.push(row([link, resolved.version, resolved.license ?? '(see the crate)', use]));
  }
}

const counts = new Map();
for (const p of external) {
  const l = p.license ?? '(no SPDX expression)';
  counts.set(l, [...(counts.get(l) ?? []), p.name]);
}
lines.push('', `All ${external.length} crates the Windows program is built from (normal dependencies, procedural macros included), by licence (SPDX expression as published):`, '');
lines.push(row(['Licence', 'Crates']), row(['---', '---']));
for (const [l, names] of [...counts].sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]))) {
  lines.push(row([l.replaceAll('|', '\\|'), names.length <= 6 ? `${names.length}: ${[...new Set(names)].sort().join(', ')}` : String(names.length)]));
}

const text = readFileSync(doc, 'utf8');
const from = text.indexOf(BEGIN);
const to = text.indexOf(END);
if (from < 0 || to < from) throw new Error(`${doc}: the markers are missing`);
writeFileSync(doc, `${text.slice(0, from + BEGIN.length)}\n${lines.join('\n')}\n\n${text.slice(to)}`);
console.log(`${doc}: ${external.length} crates`);
