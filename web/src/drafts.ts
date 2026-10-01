import { useEffect, useState } from 'react';
import type { HostActorIdentity } from './protocol';
const prefix = 'harness.draft.v1:';
export const demoDraftKey = 'demo';
export function actorDraftKey(identity: HostActorIdentity): string { return JSON.stringify([identity.run, identity.actor, identity.incarnation]); }
type Draft = {
  text: string;
  lastSubmitted: string;
  error?: string;
};
const memory = new Map<string, Draft>();
let cleared = false;
const listeners = new Set<() => void>();
const timers = new Map<string, ReturnType<typeof setTimeout>>();
const notify = () => listeners.forEach(listener => listener());
function validKey(key: string): boolean {
  if (key === demoDraftKey)
    return true;
  try {
    const parts: unknown = JSON.parse(key);
    return Array.isArray(parts) && parts.length === 3 && parts.every(part => typeof part === 'string' && part.length > 0);
  }
  catch {
    return false;
  }
}
export function readDraft(key: string): Draft {
  if (!validKey(key))
    return { text: '', lastSubmitted: '', error: 'Invalid draft context.' };
  const existing = memory.get(key);
  if (existing)
    return existing;
  let draft: Draft = { text: '', lastSubmitted: '' };
  try {
    const raw = cleared ? null : sessionStorage.getItem(prefix + key);
    if (raw) {
      const value: unknown = JSON.parse(raw);
      if (typeof value !== 'object' || value === null || !('key' in value) || value.key !== key || !('text' in value) || typeof value.text !== 'string' || !('lastSubmitted' in value) || typeof value.lastSubmitted !== 'string')
        throw new Error('Invalid stored draft');
      draft = { text: value.text, lastSubmitted: value.lastSubmitted };
    }
  }
  catch {
    draft.error = 'Draft storage is unavailable or invalid. Text remains in this tab while it is open.';
  }
  memory.set(key, draft);
  return draft;
}
function persistDraft(key: string): void {
  const draft = memory.get(key);
  if (!draft) return;
  try {
    sessionStorage.setItem(prefix + key, JSON.stringify({ key, text: draft.text, lastSubmitted: draft.lastSubmitted }));
    memory.set(key, { ...draft, error: undefined });
  } catch {
    memory.set(key, { ...draft, error: 'Draft could not be saved. Text remains in this tab while it is open.' });
  }
  notify();
}
export function writeDraft(key: string, text: string, lastSubmitted = readDraft(key).lastSubmitted): void {
  if (!validKey(key))
    return;
  memory.set(key, { text, lastSubmitted, error: readDraft(key).error });
  const timer = timers.get(key);
  if (timer)
    clearTimeout(timer);
  timers.set(key, setTimeout(() => {
    timers.delete(key);
    persistDraft(key);
  }, 200));
  notify();
}
/** Called only after confirmed deliberate signout, never on transport loss. */
export function clearDrafts(): void {
  timers.forEach(clearTimeout);
  timers.clear();
  memory.clear();
  cleared = true;
  try {
    for (let i = sessionStorage.length - 1; i >= 0; i--) {
      const key = sessionStorage.key(i);
      if (key?.startsWith(prefix))
        sessionStorage.removeItem(key);
    }
  }
  catch { /* Memory still clears on deliberate signout. */ }
  notify();
}
function flushPendingDrafts(): void {
  const pending = [...timers.keys()];
  timers.forEach(clearTimeout);
  timers.clear();
  pending.forEach(persistDraft);
}
export function useDraft(key?: string) {
  const [, update] = useState(0);
  useEffect(() => {
    const listener = () => update(value => value + 1);
    if (listeners.size === 0) window.addEventListener('pagehide', flushPendingDrafts);
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0) {
        flushPendingDrafts();
        window.removeEventListener('pagehide', flushPendingDrafts);
      }
    };
  }, []);
  const draft = key ? readDraft(key) : { text: '', lastSubmitted: '' };
  return {
    ...draft, setText: (text: string) => {
      if (key)
        writeDraft(key, text);
    }, submitted: (lastSubmitted = '') => {
      if (!key) return;
      writeDraft(key, '', lastSubmitted);
      const timer = timers.get(key);
      if (timer) clearTimeout(timer);
      timers.delete(key);
      persistDraft(key);
    }, restore: () => {
      if (key)
        writeDraft(key, draft.lastSubmitted);
    }
  };
}
