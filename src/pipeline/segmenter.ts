import { spawn, type ChildProcess } from 'node:child_process';
import { EventEmitter } from 'node:events';
import fs from 'node:fs';
import path from 'node:path';

export interface SegmentInfo {
  path: string;
  duration: number;
  index: number;
}

export type SegmenterState = 'idle' | 'running' | 'done' | 'failed' | 'killed';

/** Czyta listę CSV muxera segment: "nazwa,start,koniec" w każdej pełnej linii. */
export function parseSegmentCsv(text: string): { name: string; duration: number }[] {
  const out: { name: string; duration: number }[] = [];
  const lines = text.split('\n');
  // Ostatni element po split to niepełna linia (albo pusty string), więc ją pomijamy.
  for (const line of lines.slice(0, -1)) {
    const parts = line.trim().split(',');
    if (parts.length < 3) continue;
    const start = Number(parts[parts.length - 2]);
    const end = Number(parts[parts.length - 1]);
    const name = parts.slice(0, -2).join(',');
    if (!name || !Number.isFinite(start) || !Number.isFinite(end)) continue;
    out.push({ name, duration: Math.round((end - start) * 1e6) / 1e6 });
  }
  return out;
}

export interface ClipSegmenterOptions {
  ffmpegPath: string;
  args: string[];
  outDir: string;
  /** Katalog roboczy procesu (np. dla względnych ścieżek w filtrach). */
  cwd?: string;
}

/**
 * Proces ffmpeg tnący jeden film na fragmenty. Zdarzenie "exit" z końcowym stanem
 * (done | failed | killed). poll() zwraca fragmenty, które pojawiły się od ostatniego wywołania.
 */
export class ClipSegmenter extends EventEmitter {
  state: SegmenterState = 'idle';
  error: string | null = null;
  private child: ChildProcess | null = null;
  private consumed = 0;
  private stderrTail: string[] = [];
  private opts: ClipSegmenterOptions;

  constructor(opts: ClipSegmenterOptions) {
    super();
    this.opts = opts;
  }

  start(): void {
    fs.mkdirSync(this.opts.outDir, { recursive: true });
    this.state = 'running';
    const child = spawn(this.opts.ffmpegPath, this.opts.args, {
      windowsHide: true,
      stdio: ['ignore', 'ignore', 'pipe'],
      cwd: this.opts.cwd,
    });
    this.child = child;
    child.stderr!.setEncoding('utf8');
    child.stderr!.on('data', (chunk: string) => {
      for (const line of chunk.split(/\r?\n/)) {
        if (line.trim()) this.stderrTail.push(line.trim());
      }
      if (this.stderrTail.length > 20) this.stderrTail.splice(0, this.stderrTail.length - 20);
    });
    child.on('error', (e) => this.finish('failed', e.message));
    child.on('close', (code) => {
      if (this.state === 'killed') return this.finish('killed', null);
      if (code === 0) this.finish('done', null);
      else this.finish('failed', this.stderrTail.at(-1) ?? `ffmpeg zakończył się kodem ${code}`);
    });
  }

  private finish(state: SegmenterState, error: string | null): void {
    if (this.state !== 'running' && this.state !== 'killed') return;
    if (this.state === 'killed' && state !== 'killed') return;
    this.state = state;
    this.error = error;
    this.child = null;
    this.emit('exit', state);
  }

  poll(): SegmentInfo[] {
    let text: string;
    try {
      text = fs.readFileSync(path.join(this.opts.outDir, 'list.csv'), 'utf8');
    } catch {
      return [];
    }
    const all = parseSegmentCsv(text);
    const fresh = all.slice(this.consumed).map((s, i) => ({
      path: path.join(this.opts.outDir, s.name),
      duration: s.duration,
      index: this.consumed + i,
    }));
    this.consumed = all.length;
    return fresh;
  }

  kill(): void {
    if (this.state !== 'running') return;
    this.state = 'killed';
    if (this.child) this.child.kill('SIGKILL');
    else this.finish('killed', null);
  }
}
