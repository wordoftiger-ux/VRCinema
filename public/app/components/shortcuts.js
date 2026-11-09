// The keyboard shortcuts dialog: opened with "?", from the player bar's menu and from
// Settings → About.

import { html } from '../html.js';
import { t } from '../i18n.js';
import { openDialog } from './dialog.js';

const kbd = (k) => html`<kbd>${k}</kbd>`;
const plus = (...keys) => keys.map((k, i) => html`${i ? ' + ' : ''}${kbd(k)}`);
const either = (...keys) => keys.map((k, i) => html`${i ? ' / ' : ''}${kbd(k)}`);

export function openShortcuts() {
  const rows = () => [
    [either(t('shortcuts.key_space'), 'K'), 'shortcuts.play_pause'],
    [either('←', '→'), 'shortcuts.seek_5'],
    [either('J', 'L'), 'shortcuts.seek_10'],
    [[...plus(t('shortcuts.key_shift'), '←'), ' / ', kbd('→')], 'shortcuts.seek_60'],
    [plus(t('shortcuts.key_wheel')), 'shortcuts.wheel'],
    [plus(t('shortcuts.key_home')), 'shortcuts.restart'],
    [plus('N'), 'shortcuts.next'],
    [plus('I'), 'shortcuts.stats'],
    [plus('S'), 'shortcuts.sync'],
    [plus(t('shortcuts.key_ctrl'), 'V'), 'shortcuts.paste'],
    [plus('?'), 'shortcuts.help'],
  ];
  openDialog({
    title: () => t('shortcuts.title'),
    body: () => html`
      <p class="dialog-intro">${t('shortcuts.intro')}</p>
      <dl class="shortcuts">
        ${rows().map(([keys, key]) => html`<dt>${keys}</dt><dd>${t(key)}</dd>`)}
      </dl>`,
    buttons: [{ key: 'ui.close', primary: true }],
  });
}
