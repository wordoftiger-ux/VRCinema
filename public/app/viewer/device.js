// What the request page remembers about this viewer: the nick, the device key (the proof that
// a nick is theirs) and the icon the channel gave them. localStorage may be missing or throw
// (private mode, blocked site data): then the values live until the page is closed.

const KEYS = {
  nick: 'vrcinema.viewer.nick',
  deviceKey: 'vrcinema.viewer.deviceKey',
  badge: 'vrcinema.viewer.badge',
};

const memory = new Map();

function read(name) {
  try {
    const value = localStorage.getItem(KEYS[name]);
    if (value !== null) return value;
  } catch {
    // fall through to the memory copy
  }
  return memory.get(name) ?? null;
}

function write(name, value) {
  memory.set(name, value);
  try {
    localStorage.setItem(KEYS[name], value);
  } catch {
    // kept in memory only
  }
}

/** 128 random bits as 32 lowercase hexadecimal characters. */
export function newDeviceKey() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** The key already made on this device, or null. */
export function existingDeviceKey() {
  const key = read('deviceKey');
  return key && /^[0-9a-f]{32}$/.test(key) ? key : null;
}

/** The key of this device; made (once) and stored on first use. */
export function deviceKey() {
  const existing = existingDeviceKey();
  if (existing) return existing;
  const key = newDeviceKey();
  write('deviceKey', key);
  return key;
}

export const savedNick = () => read('nick') ?? '';
export const saveNick = (nick) => write('nick', nick);

/** The icon the channel gave this device (`{hue, shape}`), or null while unknown. */
export function savedBadge() {
  try {
    const badge = JSON.parse(read('badge'));
    return badge && Number.isInteger(badge.shape) && Number.isFinite(badge.hue) ? badge : null;
  } catch {
    return null;
  }
}

export function saveBadge(badge) {
  write('badge', JSON.stringify(badge));
}
