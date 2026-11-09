// In the program window the web view's own right-click menu (Back, Reload, Save as, Print,
// More tools) means nothing to the program, so it is not shown. Text fields keep theirs (copy,
// paste); the program's own menus (the queue's) are drawn by the interface itself.

export function blockNativeMenu() {
  document.addEventListener('contextmenu', (e) => {
    if (e.target instanceof Element && e.target.closest('input, textarea, [contenteditable="true"]')) return;
    e.preventDefault();
  });
}
