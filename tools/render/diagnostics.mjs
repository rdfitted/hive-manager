import { homedir } from 'node:os';

const escape = value => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const bounded = (value, limit) => value.length <= limit ? value : `${value.slice(0, limit - 14)}...[truncated]`;

// Only assertion text enters the CI log. Never read reports or screenshot files.
export function scrubDiagnostic(value, { instanceRoot, env = process.env, home = homedir() } = {}) {
  const paths = [instanceRoot, home, env.HOME, env.USERPROFILE].filter(Boolean);
  const secrets = Object.entries(env).filter(([key, item]) => item && /token|secret|password|api_?key|credential/i.test(key)).map(([, item]) => item);
  const scrub = input => {
    if (input.includes('\u0089PNG\r\n\u001a\n')) return '[png]';
    let text = input;
    for (const item of secrets) text = text.replaceAll(item, '[secret]');
    for (const item of paths.sort((a, b) => b.length - a.length)) {
      for (const alias of new Set([item, item.replaceAll('\\', '/'), item.replaceAll('\\', '\\\\')])) {
        text = text.replace(new RegExp(escape(alias), 'gi'), '[path]');
      }
    }
    text = text
      .replace(/(?:[a-z]:[\\/]+Users[\\/]+|\/(?:home|Users)\/)[^\\/\s"'<>]+/gi, '[user-path]')
      .replace(/\b(?:sk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[A-Z0-9]{16})\b/g, '[token]')
      .replace(/\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b/g, '[token]')
      .replace(/\b(?:Bearer\s+)[A-Za-z0-9._~+\/-]+=*/gi, 'Bearer [token]')
      .replace(/((?:token|secret|password|api[_-]?key|credential)["']?\s*[:=]\s*["']?)[^\s,"';}]+/gi, '$1[token]')
      .replace(/\b[A-Za-z0-9_+/-]{32,}={0,2}\b/g, value => /^(?:[a-z]+-)+[a-z]+$/.test(value) ? value : '[opaque-token]')
      .replace(/data:image\/png[^\s"'<>]*/gi, '[png]')
      .replace(/iVBORw0KGgo[A-Za-z0-9+/=]*/g, '[png]');
    for (const user of [env.USERNAME, env.USER].filter(Boolean)) text = text.replace(new RegExp(`\\b${escape(user)}\\b`, 'gi'), '[user]');
    return text;
  };
  const seen = new WeakSet();
  const json = JSON.stringify(value, (key, item) => {
    if (/token|secret|password|api[_-]?key|credential/i.test(key)) return '[secret]';
    if (/png|screenshot|pixels/i.test(key)) return '[image omitted]';
    if (typeof item === 'string') return scrub(item);
    if (typeof item === 'bigint') return String(item);
    if (item && typeof item === 'object') {
      if (ArrayBuffer.isView(item) || item instanceof ArrayBuffer || item.type === 'Buffer') return '[binary omitted]';
      if (seen.has(item)) return '[circular]';
      seen.add(item);
    }
    return item;
  });
  // Scrub keys too, plus any toJSON string representation.
  return scrub(json ?? 'null');
}

export function failureDiagnostic({ name, status, details, context = {} }, options) {
  const header = bounded(scrubDiagnostic({ status, name, ...context }, options), 300);
  if (name === 'mutation-proof') {
    // Reserve room for every phase even when one phase has very large details.
    const fields = ['name', 'before', 'mutant', 'restored', 'error'].map(key => {
      const value = details?.[key];
      const ordered = value && typeof value === 'object' ? { passed: value.passed, ...value } : value;
      return `${key}=${bounded(scrubDiagnostic(ordered, options), key === 'name' ? 150 : 360)}`;
    });
    return bounded(`render:check failure ${header} ${fields.join(' ')}`, 2000);
  }
  return bounded(`render:check failure ${header} details=${scrubDiagnostic(details, options)}`, 2000);
}
