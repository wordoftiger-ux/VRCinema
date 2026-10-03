import fs from 'node:fs';
import path from 'node:path';

export const VIDEO_EXTENSIONS = new Set([
  '.mp4', '.mkv', '.avi', '.mov', '.webm', '.m4v', '.ts', '.wmv', '.flv', '.mpg', '.mpeg',
]);

/** Błąd biblioteki z komunikatem dla użytkownika (np. próba wyjścia poza folder). */
export class LibraryError extends Error {}

export interface LibraryRoot {
  index: number;
  name: string;
  path: string;
}

export interface LibraryEntry {
  type: 'dir' | 'file';
  name: string;
  /** Ścieżka względem folderu głównego, z "/" jako separatorem. */
  path: string;
  sizeBytes?: number;
}

export interface LibraryListing {
  root: number;
  path: string;
  /** Ścieżka folderu nadrzędnego albo null w folderze głównym. */
  parent: string | null;
  entries: LibraryEntry[];
}

const collator = new Intl.Collator('pl', { numeric: true, sensitivity: 'base' });
const caseInsensitive = process.platform === 'win32' || process.platform === 'darwin';

function isInside(child: string, parent: string): boolean {
  const c = caseInsensitive ? child.toLowerCase() : child;
  const p = caseInsensitive ? parent.toLowerCase() : parent;
  return c === p || c.startsWith(p.endsWith(path.sep) ? p : p + path.sep);
}

function toRelative(rel: string): string {
  return rel.split(path.sep).join('/');
}

export function isVideoFile(name: string): boolean {
  return VIDEO_EXTENSIONS.has(path.extname(name).toLowerCase());
}

/** Foldery z filmami właściciela. Każda ścieżka jest sprawdzana, żeby nie wyjść poza nie. */
export class MediaLibrary {
  private dirs: string[];

  constructor(dirs: string[]) {
    this.dirs = dirs.map((d) => path.resolve(d));
  }

  get enabled(): boolean {
    return this.dirs.length > 0;
  }

  roots(): LibraryRoot[] {
    return this.dirs.map((p, index) => ({ index, name: path.basename(p) || p, path: p }));
  }

  /** Zwraca bezwzględną ścieżkę istniejącego pliku lub folderu wewnątrz folderu głównego. */
  resolve(root: number, rel: string): string {
    const base = this.dirs[root];
    if (base === undefined || !Number.isInteger(root)) throw new LibraryError('Nie ma takiego folderu głównego.');
    if (typeof rel !== 'string' || rel.includes('\0')) throw new LibraryError('Niepoprawna ścieżka.');
    if (path.isAbsolute(rel) || /^[a-zA-Z]:/.test(rel)) throw new LibraryError('Niepoprawna ścieżka.');
    const full = path.resolve(base, rel);
    if (!isInside(full, base)) throw new LibraryError('Ścieżka wychodzi poza folder z filmami.');
    if (!fs.existsSync(full)) throw new LibraryError(`Plik nie istnieje: ${path.basename(full)}`);
    let realBase: string;
    let realFull: string;
    try {
      realBase = fs.realpathSync(base);
      realFull = fs.realpathSync(full);
    } catch {
      throw new LibraryError(`Plik nie istnieje: ${path.basename(full)}`);
    }
    if (!isInside(realFull, realBase)) throw new LibraryError('Ścieżka wychodzi poza folder z filmami.');
    return full;
  }

  list(root: number, rel: string): LibraryListing {
    const dir = this.resolve(root, rel);
    if (!fs.statSync(dir).isDirectory()) throw new LibraryError('To nie jest folder.');
    const relNorm = toRelative(path.relative(this.dirs[root], dir));
    const entries: LibraryEntry[] = [];
    for (const d of fs.readdirSync(dir, { withFileTypes: true })) {
      if (d.name.startsWith('.') || d.name.startsWith('$')) continue;
      const childRel = relNorm ? `${relNorm}/${d.name}` : d.name;
      let stat: fs.Stats;
      try {
        const child = path.join(dir, d.name);
        stat = fs.statSync(child);
        // Dowiązania i skróty prowadzące poza bibliotekę nie są pokazywane.
        if (d.isSymbolicLink() && !isInside(fs.realpathSync(child), fs.realpathSync(this.dirs[root]))) continue;
      } catch {
        continue;
      }
      if (stat.isDirectory()) entries.push({ type: 'dir', name: d.name, path: childRel });
      else if (stat.isFile() && isVideoFile(d.name)) {
        entries.push({ type: 'file', name: d.name, path: childRel, sizeBytes: stat.size });
      }
    }
    entries.sort((a, b) => (a.type === b.type ? collator.compare(a.name, b.name) : a.type === 'dir' ? -1 : 1));
    const parent = relNorm === '' ? null : relNorm.includes('/') ? relNorm.slice(0, relNorm.lastIndexOf('/')) : '';
    return { root, path: relNorm, parent, entries };
  }

  /** Filmy bezpośrednio w folderze (bez podfolderów), w kolejności naturalnej. */
  videosIn(root: number, rel: string): LibraryEntry[] {
    return this.list(root, rel).entries.filter((e) => e.type === 'file');
  }

  toSourceUrl(root: number, rel: string): string {
    return `local:${root}/${rel.split('/').map(encodeURIComponent).join('/')}`;
  }

  static parseSourceUrl(url: string): { root: number; path: string } | null {
    const m = url.match(/^local:(\d+)\/(.*)$/);
    if (!m) return null;
    try {
      return { root: Number(m[1]), path: m[2].split('/').map(decodeURIComponent).join('/') };
    } catch {
      return null;
    }
  }
}
