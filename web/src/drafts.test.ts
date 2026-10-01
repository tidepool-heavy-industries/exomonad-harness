import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
beforeEach(() => { sessionStorage.clear(); vi.resetModules(); vi.useFakeTimers(); });
afterEach(async () => { (await import('./drafts')).clearDrafts(); vi.restoreAllMocks(); vi.useRealTimers(); });
describe('tab-local exact-context drafts', () => {
  it('preserves exact text and separates run, incarnation and demo after reload', async () => {
    const drafts = await import('./drafts');
    const first = drafts.actorDraftKey({ run: 'r', actor: 'a', incarnation: 'i' }), second = drafts.actorDraftKey({ run: 'r', actor: 'a', incarnation: 'I' });
    drafts.writeDraft(first, '  hello\n日本  ');
    drafts.writeDraft(second, 'other');
    drafts.writeDraft('demo', 'echo demo');
    vi.advanceTimersByTime(200);
    vi.resetModules();
    const reloaded = await import('./drafts');
    expect(reloaded.readDraft(first).text).toBe('  hello\n日本  ');
    expect(reloaded.readDraft(second).text).toBe('other');
    expect(reloaded.readDraft('demo').text).toBe('echo demo');
    expect(reloaded.readDraft(reloaded.actorDraftKey({ run: 'other-run', actor: 'a', incarnation: 'i' })).text).toBe('');
  });
  it('keeps live text and exposes storage refusal', async () => {
    const drafts = await import('./drafts');
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    drafts.writeDraft('demo', ' exact text ');
    vi.advanceTimersByTime(200);
    expect(drafts.readDraft('demo').text).toBe(' exact text ');
    expect(drafts.readDraft('demo').error).toMatch(/could not be saved/);
  });
  it('validates persisted key/payload and rejects malformed context', async () => {
    sessionStorage.setItem('harness.draft.v1:demo', JSON.stringify({ key: 'another', text: 'secret', lastSubmitted: '' }));
    const drafts = await import('./drafts');
    expect(drafts.readDraft('demo').text).toBe('');
    expect(drafts.readDraft('demo').error).toBeTruthy();
    expect(drafts.readDraft('bad-key').error).toBeTruthy();
  });
  it('clears only owned keys on explicit logout and cancels delayed writes', async () => {
    const drafts = await import('./drafts');
    sessionStorage.setItem('other-app', 'keep');
    drafts.writeDraft('demo', 'pending');
    drafts.clearDrafts();
    vi.advanceTimersByTime(1000);
    expect(drafts.readDraft('demo').text).toBe('');
    expect(sessionStorage.getItem('harness.draft.v1:demo')).toBeNull();
    expect(sessionStorage.getItem('other-app')).toBe('keep');
  });
});
