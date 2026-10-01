import { act, fireEvent, render, screen, within, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { clearDrafts } from './drafts';
import type { HarnessViewModel } from './view-model';
import type { BrowserCommandRecord } from './pending-commands';
vi.mock('./NodeWindow', () => ({
  default: ({ requestId, conversationId, refreshKey, onClose, onAuthExpired }: {
    requestId: string;
    conversationId?: string;
    refreshKey?: string;
    onClose: () => void;
    onAuthExpired?: () => void;
  }) => <section aria-label="Request history">
      {requestId} · {conversationId} · {refreshKey}
      <button onClick={onClose}>Close history</button>
      <button onClick={onAuthExpired}>Expire auth</button>
    </section>
}));
const target = { run: 'run', actor: '/worker', incarnation: 'old' };
const actor = { id: JSON.stringify(['run', '/worker', 'old']), name: '/worker', run: 'run', incarnation: 'old', kind: 'model' as const, lifecycle: 'running', modelConversation: 'conv', activeRound: '00000000-0000-4000-8000-000000000001' };
const data: HarnessViewModel = { hostRun: 'run', actors: [actor, { ...actor, id: JSON.stringify(['run', '/workflow', 'w']), name: '/workflow', incarnation: 'w', kind: 'workflow', modelConversation: undefined }], nodes: [{ id: 'conv', name: 'Conversation A', state: 'active' }, { id: 'other', name: 'Unattached conversation', state: 'idle' }], timeline: [{ id: 'req', key: 'request:req', nodeId: 'conv', label: 'Selected request', kind: 'request', state: 'completed', startedAtMs: 1000, endedAtMs: 2000, historyRefreshKey: 'stable' }, { id: 'req', key: 'job:req', nodeId: 'other', label: 'Unrelated job', kind: 'job', state: 'completed', delivered: false, output: ' retained output ' }], inbox: [{ id: 'one', sender: '/worker', recipient: '/operator', message: 'selected message', type: 'MESSAGE', state: 'MESSAGE' }, { id: 'two', sender: '/other', recipient: '/operator', message: 'other progress', type: 'PROGRESS', state: 'PROGRESS' }] };
function route(query = '?view=tree') { window.history.replaceState(null, '', '/' + query); }
function tab(name: string) { fireEvent.click(screen.getByRole('link', { name })); }
function choose() { fireEvent.change(screen.getByLabelText('Target actor'), { target: { value: JSON.stringify(['run', '/worker', 'old']) } }); }
beforeEach(() => { route(); sessionStorage.clear(); clearDrafts(); });
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); clearDrafts(); });
describe('linked operator views', () => {
  it('links exact actors to supplied model conversation, keeps selection across screens and uses global toggle', () => {
    render(<App data={data} />);
    fireEvent.click(screen.getByRole('link', { name: 'Open chat with /worker, running' }));
    expect(window.location.search).toContain('incarnation=old');
    tab('Timeline');
    expect(screen.getByText('Selected request', { exact: false })).toBeVisible();
    expect(screen.queryByText('Unrelated job', { exact: false })).toBeNull();
    fireEvent.click(screen.getByLabelText('Global activity'));
    expect(screen.getByText('Unrelated job', { exact: false })).toBeVisible();
    tab('Inbox');
    expect(screen.getByText('selected message')).toBeVisible();
    expect(screen.getByText('other progress')).toBeVisible();
    fireEvent.click(screen.getByLabelText('Global activity'));
    expect(screen.queryByText('other progress')).toBeNull();
  });
  it('conjoins endpoint/type filters and restores route on reload/popstate without losing unknown query', () => {
    route('?view=inbox&sender=%2Fother&type=PROGRESS&plugin=keep');
    const mounted = render(<App data={data} />);
    expect(screen.getByText('other progress')).toBeVisible();
    expect(screen.queryByText('selected message')).toBeNull();
    fireEvent.change(screen.getByLabelText('Message type'), { target: { value: 'MESSAGE' } });
    expect(screen.queryByText('other progress')).toBeNull();
    expect(window.location.search).toContain('plugin=keep');
    act(() => { window.history.replaceState(null, '', '/?view=inbox&sender=%2Fother&type=PROGRESS&plugin=keep'); window.dispatchEvent(new PopStateEvent('popstate')); });
    expect(screen.getByText('other progress')).toBeVisible();
    mounted.unmount();
    render(<App data={data} />);
    expect(screen.getByLabelText('Sender')).toHaveValue('/other');
  });
  it('keeps missing incarnation visible, disables controls and never follows a replacement', () => {
    route('?view=host&run=run&actor=%2Fworker&incarnation=old');
    const submit = vi.fn(() => ({ kind: 'retained' as const, operationId: 'op', send: 'unknown' as const }));
    const mounted = render(<App data={data} onHostCommand={submit} />);
    mounted.rerender(<App data={{ ...data, actors: [{ ...actor, incarnation: 'new' }] }} onHostCommand={submit} />);
    expect(screen.getByText(/exact selection remains preserved/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled();
    expect(window.location.search).toContain('incarnation=old');
    fireEvent.click(screen.getByRole('button', { name: 'Retire' }));
    expect(submit).not.toHaveBeenCalled();
  });
  it('conversation-only and malformed URL contexts cannot grant actor controls', () => {
    route('?view=host&conversation=conv');
    const mounted = render(<App data={data} onHostCommand={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Retire' })).toBeDisabled();
    mounted.unmount();
    route('?view=host&actor=%2Fworker&run=run');
    render(<App data={data} onHostCommand={vi.fn()} />);
    expect(screen.getByText(/invalid or incomplete selection/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled();
  });
  it('links workflow actors to exact host details and preserves standalone conversation links', () => {
    const mounted = render(<App data={data} />);
    fireEvent.click(screen.getByRole('link', { name: 'View host details for workflow actor /workflow, running' }));
    expect(screen.getByText('Workflow actor model history is unavailable.')).toBeVisible();
    expect(screen.getByRole('heading', { name: 'Host', level: 1 })).toBeVisible();
    expect(window.location.search).toContain('incarnation=w');
    mounted.unmount();
    route();
    render(<App data={{ ...data, hostRun: undefined }} />);
    expect(screen.getByRole('link', { name: 'Unattached conversation' })).toBeVisible();
  });
  it('requires fresh transport and callback, retaining text across disconnection', () => {
    route('?view=host');
    const callback = vi.fn();
    const mounted = render(<App data={data} onHostCommand={callback} />);
    choose();
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'draft' } });
    mounted.rerender(<App data={data} onHostCommand={callback} transportPhase="disconnected" />);
    expect(screen.getByRole('button', { name: 'Send input' })).toBeDisabled();
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('draft');
    mounted.rerender(<App data={data} />);
    expect(screen.getByRole('button', { name: 'Retire' })).toBeDisabled();
  });
  it('preserves blocked host text and clears only after exact locally retained handoff', () => {
    route('?view=host');
    const callback = vi.fn().mockReturnValueOnce({ kind: 'blocked', reason: 'storage refused' }).mockReturnValueOnce({ kind: 'retained', operationId: 'op', send: 'unknown' });
    render(<App data={data} onHostCommand={callback} />);
    choose();
    const input = screen.getByLabelText('Message to selected actor');
    fireEvent.change(input, { target: { value: ' exact\n text ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }));
    expect(input).toHaveValue(' exact\n text ');
    expect(screen.getByText('storage refused')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }));
    expect(input).toHaveValue('');
    expect(callback).toHaveBeenLastCalledWith({ action: 'input', target, text: ' exact\n text ' });
    expect(screen.getByText(/socket send unknown/)).toBeVisible();
  });
  it('keeps actor drafts separate across views and clears mounted drafts only on explicit logout', () => {
    route('?view=host');
    render(<App data={data} />);
    choose();
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'actor draft' } });
    tab('Timeline');
    tab('Host');
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('actor draft');
    fireEvent.change(screen.getByLabelText('Target actor'), { target: { value: JSON.stringify(['run', '/workflow', 'w']) } });
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('');
    choose();
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('actor draft');
    act(() => clearDrafts());
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('');
  });
  it('preserves live drafts and reports persistence failures inline', () => {
    vi.useFakeTimers();
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    route('?view=host');
    render(<App data={data} />);
    choose();
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'still here' } });
    act(() => vi.advanceTimersByTime(200));
    expect(screen.getByText(/Draft could not be saved/)).toBeVisible();
    expect(screen.getByLabelText('Message to selected actor')).toHaveValue('still here');
  });
  it('keeps demo blocked text and restores exact last sent text beside server refusal', () => {
    route('?view=command');
    const callback = vi.fn().mockReturnValueOnce({ kind: 'blocked', reason: 'socket lost' }).mockReturnValueOnce({ kind: 'sent' });
    const standalone = { ...data, hostRun: undefined };
    const mounted = render(<App data={standalone} onDemoCommand={callback} />);
    const input = screen.getByLabelText('Command');
    fireEvent.change(input, { target: { value: 'echo  exact  ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(input).toHaveValue('echo  exact  ');
    expect(screen.getByText('socket lost')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(input).toHaveValue('');
    mounted.rerender(<App data={standalone} onDemoCommand={callback} demoFeedback="Server refused demo" />);
    expect(screen.getByText('Server refused demo')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Restore last demo command' }));
    expect(input).toHaveValue('echo  exact  ');
  });
  it('uses typing-safe shortcuts, heading focus and inspector close focus restoration', () => {
    vi.useFakeTimers();
    route('?view=host');
    render(<App data={data} />);
    choose();
    const input = screen.getByLabelText('Message to selected actor');
    fireEvent.keyDown(input, { key: 'g' });
    fireEvent.keyDown(input, { key: 'l' });
    expect(screen.getByRole('heading', { name: 'Host', level: 1 })).toBeVisible();
    fireEvent.keyDown(window, { key: 'g' });
    fireEvent.keyDown(window, { key: 'l' });
    expect(screen.getByRole('heading', { name: 'Timeline', level: 1 })).toHaveFocus();
    const trigger = screen.getByRole('button', { name: 'Inspect history' });
    fireEvent.click(trigger);
    expect(screen.getByRole('region', { name: 'Request history' })).toHaveTextContent('req · conv · stable');
    fireEvent.click(screen.getByRole('button', { name: 'Close history' }));
    act(() => vi.advanceTimersByTime(0));
    expect(trigger).toHaveFocus();
  });
  it('restores inspector trigger focus when native URL history closes the inspector', async () => {
    route('?view=timeline');
    render(<App data={data} />);
    const trigger = screen.getByRole('button', { name: 'Inspect history' });
    fireEvent.click(trigger);
    screen.getByRole('button', { name: 'Close history' }).focus();
    expect(screen.getByRole('button', { name: 'Close history' })).toHaveFocus();
    act(() => {
      const url = new URL(window.location.href);
      url.searchParams.delete('request');
      window.history.replaceState(null, '', url);
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    expect(screen.queryByRole('region', { name: 'Request history' })).toBeNull();
    await waitFor(() => expect(trigger).toHaveFocus());
  });
  it('focuses the heading after URL inspector closure when the original trigger disappeared', async () => {
    route('?view=timeline');
    const mounted = render(<App data={data} />);
    const trigger = screen.getByRole('button', { name: 'Inspect history' });
    fireEvent.click(trigger);
    screen.getByRole('button', { name: 'Close history' }).focus();
    mounted.rerender(<App data={{ ...data, timeline: [] }} />);
    expect(trigger.isConnected).toBe(false);
    act(() => {
      const url = new URL(window.location.href);
      url.searchParams.delete('request');
      window.history.replaceState(null, '', url);
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    expect(screen.queryByRole('region', { name: 'Request history' })).toBeNull();
    await waitFor(() => expect(screen.getByRole('heading', { name: 'Timeline', level: 1 })).toHaveFocus());
  });
  it('renders all receipt outcomes distinctly and keeps legacy/local observations separate', () => {
    route('?view=host');
    const record: BrowserCommandRecord = { hostRun: 'run', authority: 'legacy', state: 'input_admitted', submission: { operation_id: '00000000-0000-4000-8000-000000000001', command: { action: 'input', target, text: ' exact retained ' } }, send: 'unknown', lookup: { kind: 'unavailable', reason: 'offline' } };
    render(<App data={{ ...data, commandReceipts: [{ commandId: '1', outcome: 'admitted', envelopeId: 'e' }, { commandId: '2', outcome: 'control_requested', control: 'retire', target }, { commandId: '3', outcome: 'refused', reason: 'no' }, { commandId: '4', outcome: 'unconfirmed', target, reason: 'unknown' }] }} pendingCommands={[record]} />);
    expect(screen.getByText('Unconfirmed')).toBeVisible();
    expect(screen.getByText('Refused')).toBeVisible();
    expect(screen.getByText('Admitted for processing')).toBeVisible();
    expect(screen.getByText('Retire requested')).toBeVisible();
    expect(screen.getByText(/Previously observed input_admitted; fresh lookup required/)).toBeVisible();
    expect(screen.getByText(/exact retained/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Retry same operation' })).toBeDisabled();
  });
  it('bounds large conversation/activity lists and exposes paging/search', () => {
    const large = { ...data, hostRun: undefined, nodes: Array.from({ length: 205 }, (_, i) => ({ id: `c${i}`, name: `Conversation ${i}`, state: 'active' })), timeline: Array.from({ length: 105 }, (_, i) => ({ id: `r${i}`, nodeId: 'c0', label: `Request ${i}`, kind: 'request' as const, state: 'completed' })) };
    render(<App data={large} />);
    expect(within(screen.getByRole('table', { name: 'Conversation tree' })).getAllByRole('row')).toHaveLength(100);
    fireEvent.change(screen.getByLabelText('Search'), { target: { value: 'Conversation 204' } });
    expect(screen.getByRole('link', { name: 'Conversation 204' })).toBeVisible();
    tab('Timeline');
    expect(screen.getAllByRole('button', { name: 'Inspect history' })).toHaveLength(50);
    fireEvent.click(screen.getByRole('button', { name: 'Next page' }));
    expect(screen.getAllByRole('button', { name: 'Inspect history' })).toHaveLength(50);
  });
  it('addresses interrupt and retire to the exact selected identity and round', () => {
    route('?view=host');
    const callback = vi.fn(() => ({ kind: 'retained' as const, operationId: 'op', send: 'sent' as const }));
    render(<App data={data} onHostCommand={callback} />); choose();
    fireEvent.click(screen.getByRole('button', { name: 'Interrupt' }));
    expect(callback).toHaveBeenLastCalledWith({ action: 'interrupt', target, expected_round: actor.activeRound });
    fireEvent.click(screen.getByRole('button', { name: 'Retire' }));
    expect(callback).toHaveBeenLastCalledWith({ action: 'retire', target });
  });
  it('forwards protected history authentication expiry with exact request context', () => {
    route('?view=timeline&request=req');
    const expired = vi.fn(); render(<App data={data} onAuthExpired={expired} />);
    expect(screen.getByRole('region', { name: 'Request history' })).toHaveTextContent('req · conv · stable');
    fireEvent.click(screen.getByRole('button', { name: 'Expire auth' })); expect(expired).toHaveBeenCalledOnce();
  });
  it('uses one clock for visible active rows and stops it outside activity views', () => {
    vi.useFakeTimers(); vi.setSystemTime(5000); route('?view=timeline');
    const interval = vi.spyOn(globalThis, 'setInterval');
    const cancelInterval = vi.spyOn(globalThis, 'clearInterval');
    const activeData = { ...data, timeline: [{ ...data.timeline[0]!, state: 'running', endedAtMs: undefined }, { ...data.timeline[1]!, state: 'running', startedAtMs: 1000, endedAtMs: undefined }] };
    render(<App data={activeData} />); expect(interval).toHaveBeenCalledTimes(1);
    act(() => vi.advanceTimersByTime(1000)); expect(screen.getAllByText(/5s/).length).toBeGreaterThan(0);
    tab('Tree'); expect(cancelInterval).toHaveBeenCalledWith(interval.mock.results[0]!.value);
  });
  it('immediately persists successful draft clearing without keeping host text as a demo restoration', () => {
    route('?view=host'); const callback = vi.fn(() => ({ kind: 'retained' as const, operationId: 'op', send: 'unknown' as const }));
    render(<App data={data} onHostCommand={callback} />); choose();
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: 'sent host text' } });
    fireEvent.click(screen.getByRole('button', { name: 'Send input' }));
    expect(JSON.parse(sessionStorage.getItem('harness.draft.v1:' + JSON.stringify(['run', '/worker', 'old']))!)).toEqual({ key: JSON.stringify(['run', '/worker', 'old']), text: '', lastSubmitted: '' });
  });

  it('flushes pending drafts on pagehide before a fast reload', () => {
    route('?view=host'); render(<App data={data} />); choose();
    fireEvent.change(screen.getByLabelText('Message to selected actor'), { target: { value: ' last typed characters ' } });
    act(() => window.dispatchEvent(new Event('pagehide')));
    const stored = JSON.parse(sessionStorage.getItem('harness.draft.v1:' + JSON.stringify(['run', '/worker', 'old']))!);
    expect(stored.text).toBe(' last typed characters ');
  });

});
