// "Skip this film?": the viewers' votes for the film on air and one button that toggles mine.
// With a known number needed (percent, count) there is a bar; in signal mode, or when nobody
// is known to be watching, only the count of votes.

import { useState } from '/static/vendor/hooks.mjs';
import { html } from '../html.js';
import { t } from '../i18n.js';
import { api, errorText } from '../api.js';
import { deviceKey } from './device.js';
import { refresh, setSkipVote, showToast } from './store.js';

export function SkipCard({ state }) {
  const [busy, setBusy] = useState(false);
  const vote = state?.skipVote;
  const current = state?.current;
  if (!vote || !current) return null;
  const mine = Boolean(vote.mine);
  const needed = Number.isFinite(vote.needed) && vote.needed > 0 ? vote.needed : null;
  const share = needed ? Math.min(100, (vote.votes / needed) * 100) : 0;
  const send = async () => {
    if (busy) return;
    setBusy(true);
    try {
      // The film the page shows goes with the vote: one that already changed is ignored.
      const r = await api('POST', '/api/channels/main/skip-vote', { deviceKey: deviceKey(), vote: !mine, itemId: current.itemId });
      setSkipVote(r.skipVote);
      refresh();
    } catch (e) {
      showToast(errorText(e));
      refresh();
    } finally {
      setBusy(false);
    }
  };
  const count = needed ? t('viewer.skip_votes', { votes: vote.votes, needed }) : t('viewer.skip_votes_only', { votes: vote.votes });
  return html`
    <section class="v-skip" aria-labelledby="v-skip-h" data-mode=${vote.mode} data-mine=${mine ? 'yes' : 'no'}>
      <div class="v-skip-head">
        <b id="v-skip-h">${t('viewer.skip_title')}</b>
        <small class="v-skip-count" role="status">${count}</small>
      </div>
      ${needed
        ? html`<div class="v-bar" aria-hidden="true"><span style=${{ width: `${share}%` }}></span></div>`
        : null}
      <button type="button" class=${mine ? 'btn v-skip-btn on' : 'btn v-skip-btn'} aria-pressed=${mine ? 'true' : 'false'}
        disabled=${busy} onClick=${send}>${mine ? t('viewer.skip_withdraw') : t('viewer.skip_button')}</button>
    </section>`;
}
