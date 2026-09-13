import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import UpdateStatus from '../UpdateStatus'
import type { AppUpdate } from '../api'
import type { UpdateCheckInterval } from '../types'

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

const CHECK_INTERVALS: Record<UpdateCheckInterval, number> = {
  daily: 24 * 60 * 60 * 1000,
  weekly: 7 * 24 * 60 * 60 * 1000,
  monthly: 30 * 24 * 60 * 60 * 1000,
  never: 0
}

async function flushPromises() {
  await act(async () => {
    for (let index = 0; index < 6; index += 1) await Promise.resolve()
  })
}

function updatesPanel() {
  return screen.getByRole('region', { name: 'Updates' })
}

function renderUpdateStatus(interval: UpdateCheckInterval = 'daily') {
  return render(<UpdateStatus updateCheckInterval={interval} />)
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
  it('checks for updates on startup and stays out of the top bar when up to date', async () => {
    invokeMock.mockResolvedValue(upToDate)

    renderUpdateStatus()

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('check_for_updates', undefined))
    expect(screen.queryByRole('button', { name: 'Updates' })).not.toBeInTheDocument()
  })

  it('announces an available update and installs it through the backend', async () => {
    invokeMock.mockResolvedValue(updateAvailable)
    const user = userEvent.setup()

    renderUpdateStatus()

    const trigger = await screen.findByRole('button', { name: 'Update available: v0.2.0' })
    expect(trigger).toHaveAttribute('title', 'CodeTally v0.2.0 is available')
    await user.click(trigger)
    const panel = updatesPanel()
    expect(within(panel).getByText('Current version:').nextSibling).toHaveTextContent('v0.1.2')
    expect(within(panel).getByText('New version:').nextSibling).toHaveTextContent('v0.2.0')

    await user.click(within(panel).getByRole('button', { name: 'Download update' }))
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('install_update', undefined))
  })

  it('keeps the available version visible when installation fails', async () => {
    invokeMock.mockImplementation((command: string) => command === 'install_update' ? Promise.reject(new Error('Installation failed')) : Promise.resolve(updateAvailable))
    const user = userEvent.setup()

    renderUpdateStatus()
    const trigger = await screen.findByRole('button', { name: 'Update available: v0.2.0' })
    await user.click(trigger)
    const panel = updatesPanel()
    await user.click(within(panel).getByRole('button', { name: 'Download update' }))

    expect(await within(panel).findByRole('alert')).toHaveTextContent('Installation failed')
    expect(within(panel).getByText('New version:').nextSibling).toHaveTextContent('v0.2.0')
    expect(within(panel).getByRole('button', { name: 'Download update' })).toBeEnabled()
  })

  it('shows a retry error without stale status and recovers after a rejected check', async () => {
    invokeMock
      .mockResolvedValueOnce(updateAvailable)
      .mockRejectedValueOnce(new Error('network unavailable'))
      .mockResolvedValueOnce(updateAvailable)
    const user = userEvent.setup()

    renderUpdateStatus()
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(1))

    await user.click(await screen.findByRole('button', { name: 'Update available: v0.2.0' }))
    const panel = updatesPanel()
    await waitFor(() => expect(within(panel).getByText('New version:').nextSibling).toHaveTextContent('v0.2.0'))

    await user.click(within(panel).getByRole('button', { name: 'Check for updates' }))
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(2))
    expect(await within(panel).findByRole('alert')).toHaveTextContent("Couldn't check for updates.")
    expect(within(panel).getByText('New version:').nextSibling).toHaveTextContent('Unavailable')

    await user.click(within(panel).getByRole('button', { name: 'Check for updates' }))
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(3))
    await waitFor(() => expect(within(panel).getByText('New version:').nextSibling).toHaveTextContent('v0.2.0'))
    expect(within(panel).queryByRole('alert')).not.toBeInTheDocument()
  })

  it.each([
    ['daily', CHECK_INTERVALS.daily],
    ['weekly', CHECK_INTERVALS.weekly],
    ['monthly', CHECK_INTERVALS.monthly]
  ] as const)('polls on the %s cadence and stops polling after unmount', async (interval, checkInterval) => {
    vi.useFakeTimers()
    invokeMock.mockResolvedValue(upToDate)
    const { unmount } = renderUpdateStatus(interval)

    await flushPromises()
    const checks = () => invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates').length
    expect(checks()).toBe(1)

    await act(async () => {
      vi.advanceTimersByTime(checkInterval - 1)
      await Promise.resolve()
    })
    expect(checks()).toBe(1)

    await act(async () => {
      vi.advanceTimersByTime(1)
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(checks()).toBe(2)

    unmount()
    await act(async () => {
      vi.advanceTimersByTime(checkInterval * 2)
      await Promise.resolve()
    })
    expect(checks()).toBe(2)
  })

  it('does not overlap a scheduled check while the startup check is still pending', async () => {
    vi.useFakeTimers()
    let resolve!: (value: AppUpdate) => void
    const pending = new Promise<AppUpdate>((promiseResolve) => { resolve = promiseResolve })
    invokeMock.mockReturnValue(pending)
    const { unmount } = renderUpdateStatus()

    await flushPromises()
    await act(async () => {
      vi.advanceTimersByTime(CHECK_INTERVALS.daily)
      await Promise.resolve()
    })
    expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(1)

    resolve(upToDate)
    await flushPromises()
    unmount()
  })

  it('does not check automatically or show a top-bar control when the update cadence is never', async () => {
    invokeMock.mockResolvedValue(upToDate)

    renderUpdateStatus('never')
    await flushPromises()
    expect(invokeMock.mock.calls.filter(([command]) => command === 'check_for_updates')).toHaveLength(0)
    expect(screen.queryByRole('button', { name: 'Updates' })).not.toBeInTheDocument()
  })

  it('opens on hover or click and closes with pointer exit or Escape', async () => {
    invokeMock.mockResolvedValue(updateAvailable)
    const user = userEvent.setup()

    renderUpdateStatus()
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('check_for_updates', undefined))
    const trigger = await screen.findByRole('button', { name: 'Update available: v0.2.0' })

    await user.hover(trigger)
    const hoveredPanel = updatesPanel()
    const focusedAction = within(hoveredPanel).getByRole('button', { name: 'Check for updates' })
    act(() => focusedAction.focus())
    await user.unhover(trigger)
    expect(hoveredPanel).toBeInTheDocument()
    expect(focusedAction).toHaveFocus()
    await user.tab()
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Updates' })).not.toBeInTheDocument())

    await user.click(trigger)
    expect(updatesPanel()).toBeInTheDocument()

    await user.keyboard('{Escape}')
    expect(screen.queryByRole('region', { name: 'Updates' })).not.toBeInTheDocument()
    expect(trigger).toHaveFocus()

    await user.tab()
    await user.tab({ shift: true })
    expect(updatesPanel()).toBeInTheDocument()
  })
})
