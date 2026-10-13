// A `ref` that focuses an element when it appears. The function is the same on every render, so
// Preact calls it on mount only: a re-render (a poll, a progress tick) never moves the focus.

export function focusOnMount(el) {
  el?.focus({ preventScroll: true });
}
