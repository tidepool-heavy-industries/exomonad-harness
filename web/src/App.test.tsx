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
    expect(screen.getByText('spawn_agent')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Inbox' }))
    expect(screen.getByText('Continue?')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Command' }))
    fireEvent.change(screen.getByLabelText('Command'), { target: { value: 'wait_agent' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(onCommand).toHaveBeenCalledWith('wait_agent')
    expect(screen.queryByText('Command sent')).not.toBeInTheDocument()
  })
})
