// The Settings view: the list of sections on the left, the chosen section on the right
// (`#/settings/<section>`). The sections are in views/settings/.

import { html } from '../html.js';
import { t } from '../i18n.js';
import { route } from '../components/rail.js';
import { General } from './settings/general.js';
import { Internet } from './settings/internet.js';
import { ChannelSettings } from './settings/channel.js';
import { Tools } from './settings/tools.js';
import { Remote } from './settings/remote.js';
import { Advanced } from './settings/advanced.js';
import { Log } from './settings/log.js';
import { About } from './settings/about.js';

const SECTIONS = {
  general: { key: 'settings.nav.general', Section: General },
  internet: { key: 'settings.nav.internet', Section: Internet },
  channel: { key: 'settings.nav.channel', Section: ChannelSettings },
  tools: { key: 'settings.nav.tools', Section: Tools },
  remote: { key: 'settings.nav.remote', Section: Remote },
  advanced: { key: 'settings.nav.advanced', Section: Advanced },
  log: { key: 'settings.nav.log', Section: Log },
  about: { key: 'settings.nav.about', Section: About },
};

export function SettingsView() {
  const current = route.value.section ?? 'general';
  const { key, Section } = SECTIONS[current] ?? SECTIONS.general;
  return html`
    <main class="view settings" id="view" data-view="settings" data-section=${current}>
      <nav class="snav" aria-label=${t('settings.nav.label')}>
        ${Object.entries(SECTIONS).map(
          ([name, s]) => html`<a href=${`#/settings/${name}`} data-section=${name}
            aria-current=${current === name ? 'page' : undefined}>${t(s.key)}</a>`,
        )}
      </nav>
      <div class="sform">
        <h1>${t(key)}</h1>
        <${Section} key=${current} />
      </div>
    </main>`;
}
