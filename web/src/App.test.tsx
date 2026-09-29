import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import App, { type HarnessViewModel } from './App'

describe('operator views', () => {
  it('teaches the empty inbox and navigates between views by keyboard', () => {
    render(<App />)
    fireEvent.click(screen.getByRole('button', { name: 'Inbox' }))
    expect(screen.getByText('Inbox is clear')).toBeInTheDocument()
    fireEvent.keyDown(window, { key: 'g' })
    fireEvent.keyDown(window, { key: 't' })
    expect(screen.getByRole('heading', { name: 'Tree' })).toBeInTheDocument()
  })

  it('renders protocol-adapted records and passes commands without inventing a result', () => {
    const data: HarnessViewModel = {
      nodes: [{ id: 'n-1', name: '/root', state: 'waiting on operator', model: 'gpt-6-luna' }],
      timeline: [{ id: 'e-1', nodeId: 'n-1', label: 'spawn_agent', kind: 'job', state: 'running' }],
      inbox: [{ id: 'm-1', sender: '/root', message: 'Continue?', state: 'unread' }],
    }
    const onCommand = vi.fn()
    render(<App data={data} onCommand={onCommand} />)
    expect(screen.getByText('/root')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }))
    expect(screen.getByText(/spawn_agent/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Inbox' }))
    expect(screen.getByText('Continue?')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Command' }))
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'wait_agent' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(onCommand).toHaveBeenCalledWith('wait_agent')
    expect(screen.queryByText('Command sent')).not.toBeInTheDocument()
  })

  it('offers deterministic actions and preserves payload whitespace after the verb', () => {
    const onCommand = vi.fn()
    render(<App onCommand={onCommand} />)
    fireEvent.click(screen.getByRole('button', { name: 'Command' }))
    expect(screen.getByText(/Deterministic mode/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel wait' }))
    expect(onCommand).toHaveBeenCalledWith('cancel')
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'echo  keep spaces' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(onCommand).toHaveBeenLastCalledWith('echo  keep spaces')
    expect(screen.getByText(/Sending is not acceptance or completion/)).toBeInTheDocument()
    expect(screen.getByText(/Outcomes, progress, and messages appear only to the extent represented/)).toBeInTheDocument()
  })

  it('presents server command outcomes and real ordered message endpoints', () => {
    const data: HarnessViewModel = {
      nodes: [{ id: 'root', name: '/root', state: 'idle' }],
      timeline: [{
        id: 'req', nodeId: 'root', label: 'echo hello', kind: 'request', state: 'completed',
        commandId: 'cmd-7', command: 'echo hello', outcome: 'completed', detail: 'echoed hello',
      }],
      inbox: [{
        id: 'progress', sender: '/harness', recipient: '/root', message: 'working',
        state: 'PROGRESS', ordinal: 1,
      }],
    }
    render(<App data={data} />)
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }))
    expect(screen.getByText(/command cmd-7/)).toBeInTheDocument()
    expect(screen.getByText(/outcome completed/)).toBeInTheDocument()
    expect(screen.getByText(/echoed hello/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Inbox' }))
    expect(screen.getByText('/harness → /root')).toBeInTheDocument()
    expect(screen.getByText('PROGRESS · #1')).toBeInTheDocument()
    expect(screen.getByText('working')).toBeInTheDocument()
  })

  it('shows authoritative async job identity, lifecycle, delivery, and output', () => {
    const data: HarnessViewModel = {
      nodes: [{ id: 'root', name: '/root', state: 'paused' }],
      timeline: [{
        id: 'job-9', nodeId: 'root', label: 'Async job', kind: 'job', state: 'settled',
        requestId: 'request-1', callId: 'call-3', toolName: 'slow_tool',
        delivered: false, output: 'result pending delivery',
      }],
      inbox: [],
    }
    render(<App data={data} />)
    fireEvent.click(screen.getByRole('button', { name: 'Timeline' }))
    expect(screen.getByText(/job-9/)).toBeInTheDocument()
    expect(screen.getByText(/settled/)).toBeInTheDocument()
    expect(screen.getByText(/request request-1/)).toBeInTheDocument()
    expect(screen.getByText(/call call-3/)).toBeInTheDocument()
    expect(screen.getByText(/slow_tool/)).toBeInTheDocument()
    expect(screen.getByText(/delivered false/)).toBeInTheDocument()
    expect(screen.getByText(/result pending delivery/)).toBeInTheDocument()
  })
})
