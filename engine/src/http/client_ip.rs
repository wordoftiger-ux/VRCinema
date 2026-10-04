//! Client address and scheme behind optional proxies (port of `src/http/clientIp.ts`).

use axum::http::HeaderMap;

/// Which forwarding headers the configuration trusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trust {
    pub cloudflare: bool,
    pub proxy: bool,
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Adres IP widza: z nagłówka Cloudflare/proxy tylko wtedy, gdy włączono zaufanie w konfiguracji.
pub fn get_client_ip(headers: &HeaderMap, remote_address: &str, trust: Trust) -> String {
    if trust.cloudflare
        && let Some(cf) = header(headers, "cf-connecting-ip")
            .map(str::trim)
            .filter(|v| !v.is_empty())
    {
        return cf.to_string();
    }
    if trust.proxy
        && let Some(xff) = header(headers, "x-forwarded-for")
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    {
        return xff.to_string();
    }
    remote_address
        .strip_prefix("::ffff:")
        .unwrap_or(remote_address)
        .to_string()
}

/// Czy żądanie przyszło przez HTTPS (bezpośrednio albo przez zaufane proxy/Cloudflare).
pub fn is_https(headers: &HeaderMap, encrypted: bool, trusted: bool) -> bool {
    if encrypted {
        return true;
    }
    trusted
        && header(headers, "x-forwarded-proto")
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            == Some("https")
}
