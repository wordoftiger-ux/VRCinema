import { spawn } from 'node:child_process';

export interface ProcessResult {
  code: number | null;
  stdout: string;
  stderr: string;
}

export type RunFn = (cmd: string, args: string[], opts: { timeoutMs: number }) => Promise<ProcessResult>;

const MAX_OUTPUT = 64 * 1024 * 1024;

/** Uruchamia program i zbiera wyjście. Po przekroczeniu limitu czasu zabija proces i odrzuca obietnicę. */
export const runProcess: RunFn = (cmd, args, { timeoutMs }) =>
  new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    let size = 0;
    let finished = false;
    const timer = setTimeout(() => {
      if (finished) return;
      finished = true;
      child.kill('SIGKILL');
      reject(new Error(`Przekroczono limit czasu (${Math.round(timeoutMs / 1000)} s): ${cmd}`));
    }, timeoutMs);
    const collect = (target: Buffer[]) => (chunk: Buffer) => {
      size += chunk.length;
      if (size <= MAX_OUTPUT) target.push(chunk);
    };
    child.stdout.on('data', collect(out));
    child.stderr.on('data', collect(err));
    child.on('error', (e) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      reject(e);
    });
    child.on('close', (code) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      resolve({ code, stdout: Buffer.concat(out).toString('utf8'), stderr: Buffer.concat(err).toString('utf8') });
    });
  });
