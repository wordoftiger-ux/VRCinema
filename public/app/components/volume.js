// The two sounds the owner hears, each with its own volume and mute switch, remembered in this
// browser: the Preview (the film itself, ahead) and On air (the channel as viewers get it).
// They only change what the host hears, never the stream.

import { signal, effect } from '/static/vendor/signals.mjs';

function stored(key, fallback) {
  try {
    const value = localStorage.getItem(key);
    return value === null ? fallback : JSON.parse(value);
  } catch {
    return fallback;
  }
}

function remember(key, sig) {
  effect(() => {
    try {
      localStorage.setItem(key, JSON.stringify(sig.value));
    } catch {
      // No storage (private mode): not remembered.
    }
  });
}

/** On air's volume (the names of the first player bar, kept for the stored values). */
export const previewVolume = signal(stored('vrcinema.previewVolume', 0.6));
export const previewMuted = signal(stored('vrcinema.previewMuted', false));
/** The Preview's volume; it starts muted (two sounds at once would clash). */
export const monitorVolume = signal(stored('vrcinema.monitorVolume', 0.6));
export const monitorMuted = signal(stored('vrcinema.monitorMuted', true));

remember('vrcinema.previewVolume', previewVolume);
remember('vrcinema.previewMuted', previewMuted);
remember('vrcinema.monitorVolume', monitorVolume);
remember('vrcinema.monitorMuted', monitorMuted);

/** Changes the Preview's volume by `delta` (the mouse wheel over it); unmutes it. */
export function nudgeMonitorVolume(delta) {
  const next = Math.min(1, Math.max(0, Math.round((monitorVolume.peek() + delta) * 100) / 100));
  monitorVolume.value = next;
  monitorMuted.value = next === 0;
}
