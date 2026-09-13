import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import UpdateStatus from '../UpdateStatus'
import type { AppUpdate } from '../api'

const invokeMock = vi.hoisted(() => vi.fn())

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

const upToDate: AppUpdate = {
  current_version: 'v0.1.2',
  latest_version: 'v0.1.2',
  release_url: 'https://github.com/samfaid/codetally/releases/tag/v0.1.2',
  update_available: false
}

const updateAvailable: AppUpdate = {
  current_version: 'v0.1.2',
  latest_version: 'v0.2.0',
  release_url: 'https://github.com/samfaid/codetally/releases/tag/v0.2.0',
  update_available: true
}

const CHECK_INTERVAL = 6 * 60 * 60 * 1000

async function flushPromises() {
  await act(async () => {
    for (let index = 0; index < 6; index += 1) await Promise.resolve()
  })
}

function updatesPanel() {
  return screen.getByRole('region', { name: 'App updates' })
}

beforeEach(() => {
  invokeMock.mockReset()
})

afterEach(() => {
  cleanup()
  vi.clearAllTimers()
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('UpdateStatus', () => {
  it('checks for updates on startup and shows the current status when opened', async () => {
    invokeMock.mockResolvedValue(upToDate)
    const user = userEvent.setup()

    render(<UpdateStatus />)

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('check_for_updates', undefined))
    const trigger = screen.getByRole('button', { name: 'App updates' })
    expect(trigger).toHaveAttribute('aria-expanded', 'false')

    await user.click(trigger)
    const panel = updatesPanel()
    expect(within(panel).getByText('Installed version: v0.1.2')).toBeInTheDocument()
    await waitFor(() => expect(within(panel).getByText('You’re up to date.')).toBeInTheDocument())
  })

  it('announces an available update and opens its release page through the backend', async () => {
    invokeMock.mockResolvedValue(updateAvailable)
    const user = userEvent.setup()

    render(<UpdateStatus />)

    const trigger = await screen.findByRole('button', { name: 'Update available: v0.2.0' })
    expect(trigger).toHaveAttribute('title', 'CodeTally v0.2.0 is available')
    await user.click(trigger)
    const panel = updatesPanel()
    expect(within(panel).getByText('Installed version: v0.1.2')).toBeInTheDocument()
    expect(within(panel).getByText('CodeTally v0.2.0 is available.')).toBeInTheDocument()

    await user.click(within(panel).getByRole('button', { name: 'View release' }))
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('open_external_url', { url: updateAvailable.release_url }))
  })

  it('shows a retry error without stale status and recovers after a rejected check', async () => {
    invokeMock
      .mockResolvedValueOnce(upToDate)
      .mockRejectedValueOnce(new Error('network unavailable'))
      .mockResolvedValueOnce(upToDate)
    const user = userEvent.setup()

    render(<UpdateStatus />)
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(1))

    await user.click(screen.getByRole('button', { name: 'App updates' }))
    const panel = updatesPanel()
    await waitFor(() => expect(within(panel).getByText('You’re up to date.')).toBeInTheDocument())

    await user.click(within(panel).getByRole('button', { name: 'Check for updates' }))
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(2))
    expect(await within(panel).findByRole('alert')).toHaveTextContent('Could not check for updates.')
    expect(within(panel).queryByText('You’re up to date.')).not.toBeInTheDocument()

    await user.click(within(panel).getByRole('button', { name: 'Check for updates' }))
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(3))
    await waitFor(() => expect(within(panel).getByText('You’re up to date.')).toBeInTheDocument())
    expect(within(panel).queryByRole('alert')).not.toBeInTheDocument()
  })

  it('polls every six hours and stops polling after unmount', async () => {
    vi.useFakeTimers()
    invokeMock.mockResolvedValue(upToDate)
    const { unmount } = render(<UpdateStatus />)

    await flushPromises()
    const checks = () => invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates').length
    expect(checks()).toBe(1)

    await act(async () => {
      vi.advanceTimersByTime(CHECK_INTERVAL)
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(checks()).toBe(2)

    unmount()
    await act(async () => {
      vi.advanceTimersByTime(CHECK_INTERVAL * 2)
      await Promise.resolve()
    })
    expect(checks()).toBe(2)
  })

  it('does not overlap a scheduled check while the startup check is still pending', async () => {
    vi.useFakeTimers()
    let resolve!: (value: AppUpdate) => void
    const pending = new Promise<AppUpdate>((promiseResolve) => { resolve = promiseResolve })
    invokeMock.mockReturnValue(pending)
    const { unmount } = render(<UpdateStatus />)

    await flushPromises()
    await act(async () => {
      vi.advanceTimersByTime(CHECK_INTERVAL)
      await Promise.resolve()
    })
    expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(1)

    resolve(upToDate)
    await flushPromises()
    unmount()
  })

  it('closes the panel with Escape and returns focus to the trigger', async () => {
    invokeMock.mockResolvedValue(upToDate)
    const user = userEvent.setup()

    render(<UpdateStatus />)
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('check_for_updates', undefined))
    const trigger = screen.getByRole('button', { name: 'App updates' })
    await user.click(trigger)
    expect(updatesPanel()).toBeInTheDocument()

    await user.keyboard('{Escape}')
    expect(screen.queryByRole('region', { name: 'App updates' })).not.toBeInTheDocument()
    expect(trigger).toHaveFocus()
  })
})
