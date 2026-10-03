type Headers = Record<string, string | string[] | undefined>;

function first(v: string | string[] | undefined): string | undefined {
  return Array.isArray(v) ? v[0] : v;
}

/** Adres IP widza: z nagłówka Cloudflare/proxy tylko wtedy, gdy włączono zaufanie w konfiguracji. */
export function getClientIp(
  headers: Headers,
  remoteAddress: string,
  trust: { trustCloudflare: boolean; trustProxy: boolean },
): string {
  if (trust.trustCloudflare) {
    const cf = first(headers['cf-connecting-ip'])?.trim();
    if (cf) return cf;
  }
  if (trust.trustProxy) {
    const xff = first(headers['x-forwarded-for'])?.split(',')[0]?.trim();
    if (xff) return xff;
  }
  return remoteAddress.replace(/^::ffff:/, '');
}

/** Czy żądanie przyszło przez HTTPS (bezpośrednio albo przez zaufane proxy/Cloudflare). */
export function isHttps(headers: Headers, encrypted: boolean, trusted: boolean): boolean {
  if (encrypted) return true;
  if (!trusted) return false;
  return first(headers['x-forwarded-proto'])?.split(',')[0]?.trim() === 'https';
}
