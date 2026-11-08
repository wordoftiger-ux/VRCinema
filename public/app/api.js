// Requests to the engine. Every request carries the interface's language, so error messages
// come back in it.

import { lang, t } from './i18n.js';

/** A failed request: `message` is for people, `code` (from the engine) for the code. */
export class ApiError extends Error {
  constructor(message, { status = 0, code = null, data = null } = {}) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.data = data;
  }
}

/** Longest wait for an answer; a request that takes longer fails like an unreachable engine. */
const DEFAULT_TIMEOUT_MS = 10000;

/**
 * `method` `path` with an optional JSON `body`; resolves to the JSON answer (null when there
 * is none). Errors: `ApiError` with the engine's `code` (`code: "network"` when nothing
 * answered within `timeoutMs`; reading the details of a link may take much longer than the
 * default).
 */
export async function api(method, path, body, { timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
  const headers = { 'X-VRCinema-Lang': lang.value };
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  let res;
  try {
    res = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: 'same-origin',
      cache: 'no-store',
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch {
    throw new ApiError(t('ui.no_answer'), { code: 'network' });
  }
  let data = null;
  try {
    data = await res.json();
  } catch {
    data = null;
  }
  if (!res.ok) {
    const message = data?.error || t('ui.error_status', { status: res.status });
    throw new ApiError(message, { status: res.status, code: data?.code ?? null, data });
  }
  return data;
}

/**
 * Sends the file `blob` as the raw body of `method` `path`; resolves to the JSON answer. Errors
 * as for `api` (a large file may take long: `timeoutMs`).
 */
export async function apiUpload(method, path, blob, { timeoutMs = 300000 } = {}) {
  let res;
  try {
    res = await fetch(path, {
      method,
      headers: { 'X-VRCinema-Lang': lang.value, 'Content-Type': blob.type || 'application/octet-stream' },
      body: blob,
      credentials: 'same-origin',
      cache: 'no-store',
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch {
    throw new ApiError(t('ui.no_answer'), { code: 'network' });
  }
  let data = null;
  try {
    data = await res.json();
  } catch {
    data = null;
  }
  if (!res.ok) {
    const message = data?.error || t('ui.error_status', { status: res.status });
    throw new ApiError(message, { status: res.status, code: data?.code ?? null, data });
  }
  return data;
}

/**
 * `POST` `path` with the JSON `body`; resolves to the answer as a `Blob` (a picture). Errors as
 * for `api`.
 */
export async function apiBlob(path, body, { timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
  let res;
  try {
    res = await fetch(path, {
      method: 'POST',
      headers: { 'X-VRCinema-Lang': lang.value, 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      credentials: 'same-origin',
      cache: 'no-store',
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch {
    throw new ApiError(t('ui.no_answer'), { code: 'network' });
  }
  if (!res.ok) {
    let data = null;
    try {
      data = await res.json();
    } catch {
      data = null;
    }
    const message = data?.error || t('ui.error_status', { status: res.status });
    throw new ApiError(message, { status: res.status, code: data?.code ?? null, data });
  }
  return res.blob();
}

/**
 * The text of an error for people, in the interface's language: the dictionary's
 * `errors.<code>` when it has the code and needs no field the answer did not bring, else the
 * engine's own message (it was asked in the same language).
 */
export function errorText(e) {
  if (e?.code) {
    const key = `errors.${e.code}`;
    const text = t(key, e.data);
    if (text !== key && !/\{\w+\}/.test(text)) return text;
  }
  return e?.message ?? t('ui.no_answer');
}
