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

/**
 * `method` `path` with an optional JSON `body`; resolves to the JSON answer (null when there
 * is none). Errors: `ApiError` with the engine's `code` (`code: "network"` when nothing
 * answered).
 */
export async function api(method, path, body) {
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
