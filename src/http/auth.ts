import crypto from 'node:crypto';

export const SESSION_COOKIE = 'vrcinema_session';
export const SESSION_TTL_MS = 30 * 24 * 3600 * 1000;

function sign(payload: string, secret: string): string {
  return crypto.createHmac('sha256', secret).update(payload).digest('base64url');
}

function safeEqual(a: string, b: string): boolean {
  const ha = crypto.createHash('sha256').update(a).digest();
  const hb = crypto.createHash('sha256').update(b).digest();
  return crypto.timingSafeEqual(ha, hb);
}

/** Token sesji: "<wygaśnięcie ms>.<HMAC>". Bez stanu po stronie serwera. */
export function createSessionToken(secret: string, ttlMs = SESSION_TTL_MS, now = Date.now()): string {
  const expires = String(now + ttlMs);
  return `${expires}.${sign(expires, secret)}`;
}

export function verifySessionToken(token: string | undefined, secret: string, now = Date.now()): boolean {
  if (!token) return false;
  const [expires, mac] = token.split('.');
  if (!expires || !mac || !/^\d+$/.test(expires)) return false;
  if (!safeEqual(mac, sign(expires, secret))) return false;
  return Number(expires) > now;
}

export function checkPassword(input: unknown, expected: string): boolean {
  return typeof input === 'string' && safeEqual(input, expected);
}

export function parseCookies(header: string | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (!header) return out;
  for (const part of header.split(';')) {
    const i = part.indexOf('=');
    if (i < 0) continue;
    const key = part.slice(0, i).trim();
    const value = part.slice(i + 1).trim();
    try {
      out[key] = decodeURIComponent(value);
    } catch {
      out[key] = value;
    }
  }
  return out;
}

export function sessionCookie(token: string, secure: boolean, maxAgeS: number): string {
  return [
    `${SESSION_COOKIE}=${token}`,
    'Path=/',
    'HttpOnly',
    'SameSite=Lax',
    `Max-Age=${maxAgeS}`,
    ...(secure ? ['Secure'] : []),
  ].join('; ');
}
