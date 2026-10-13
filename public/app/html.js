// `html` tagged templates (htm) rendering Preact elements — the interface has no build step.

import { h } from '/static/vendor/preact.mjs';
import htm from '/static/vendor/htm.mjs';

export const html = htm.bind(h);
