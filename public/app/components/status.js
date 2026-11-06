// The status strip at the top of the interface: slim notices of what needs attention — tools
// missing, the channel not running, the tunnel failing, a slow computer for the info ticker,
// an update — each with a link to the part of the settings that deals with it. A notice can be
// dismissed; it comes back when its content changes (another error, another version). The
// notice of the section on show is left out (that section says it already).

import { signal } from '/static/vendor/signals.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { Icon } from '../icons.js';
import { access, channel, environment, info, tunnel } from '../store.js';
import { route } from './rail.js';

/** `12.5` → `12.5`, in the interface's number format; `—` when not known. */
function formatMbps(v) {
  return Number.isFinite(v) ? new Intl.NumberFormat(document.documentElement.lang || undefined, { maximumFractionDigits: 1 }).format(v) : '—';
}

/** Dismissed notices by `kind:content` (for this page only). */
const dismissed = signal(new Set());

/**
 * The notices for the state of `i` (the window's info) or `env` (the environment elsewhere):
 * `{kind, id, text, href, linkKey, level}`. Settings are linked where the program can be managed
 * (the window or a remote panel; a server without a window has no tools to fix).
 */
export function notices({ i, env, tun, managed, link = null }) {
  const out = [];
  const add = (kind, content, text, href, linkKey, level = 'warn') =>
    out.push({ kind, id: `${kind}:${content}`, text, href, linkKey, level });
  // Viewers × the stream's bitrate close to (or above) the upload: a warning, never a block.
  if (link && (link.level === 'warn' || link.level === 'over')) {
    const fields = {
      viewers: link.viewers,
      stream: formatMbps(link.streamMbps),
      needed: formatMbps(link.neededMbps),
      upload: formatMbps(link.uploadMbps),
    };
    add(
      'link',
      link.level,
      t(link.level === 'over' ? 'banner.link_over' : 'banner.link_warn', fields),
      managed ? '#/settings/channel' : null,
      'banner.go_upload',
      link.level === 'over' ? 'err' : 'warn',
    );
  }
  const src = i ?? env;
  if (!src) return out;
  const toolsMissing = Boolean(i && !i.toolsOk && !i.toolsRunning);
  if (toolsMissing) {
    const tools = (i.missingTools ?? []).join(', ');
    add('tools_missing', tools, t('banner.tools_missing', { tools }), '#/settings/tools', 'banner.go_tools');
  } else if (src.channelRunning === false && !src.restarting && !(i?.toolsRunning)) {
    const reason = src.channelError ?? '';
    add(
      'channel_down',
      reason,
      reason ? t('banner.channel_down_reason', { reason }) : t('banner.channel_down'),
      managed ? '#/settings/tools' : null,
      'banner.go_tools',
      'err',
    );
  }
  if (src.tunnelEnabled && tun?.state === 'error') {
    const reason = tun.message ?? '';
    add(
      'tunnel_error',
      reason,
      reason ? t('banner.tunnel_error_reason', { reason }) : t('banner.tunnel_error'),
      '#/settings/internet',
      'banner.go_internet',
      'err',
    );
  }
  if (src.tickerSlow) add('ticker_slow', '', t('banner.ticker_slow'), '#/settings/channel', 'banner.go_ticker');
  if (i?.update?.version) {
    add('update', i.update.version, t('banner.update', { version: i.update.version }), '#/settings/about', 'banner.go_update', 'info');
  }
  return out;
}

export function StatusStrip() {
  const { view, section } = route.value;
  const here = view === 'settings' ? `#/settings/${section}` : null;
  const all = notices({
    i: info.value,
    env: environment.value,
    tun: tunnel.value,
    managed: access.value === 'window' || access.value === 'remote',
    link: channel.value?.link ?? null,
  }).filter((n) => !dismissed.value.has(n.id) && n.href !== here);
  if (!all.length) return null;
  const dismiss = (id) => (dismissed.value = new Set([...dismissed.value, id]));
  return html`
    <div class="status-strip" role="region" aria-label=${t('banner.label')}>
      ${all.map(
        (n) => html`
          <p key=${n.id} class=${`status-note status-${n.level}`} data-status=${n.kind} role=${n.level === 'err' ? 'alert' : 'status'}>
            <${Icon} name="warning" size=${14} />
            <span class="status-text">${n.text}</span>
            ${n.href ? html`<a class="status-link" href=${n.href}>${t(n.linkKey)}</a>` : null}
            <button type="button" class="icon-button status-close" aria-label=${t('banner.dismiss')} title=${t('banner.dismiss')}
              onClick=${() => dismiss(n.id)}><${Icon} name="close" size=${14} /></button>
          </p>`,
      )}
    </div>`;
}
