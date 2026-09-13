import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowUpCircle, ExternalLink, LoaderCircle, X } from 'lucide-react'
import { checkForUpdates, openExternalUrl, type AppUpdate } from './api'
import type { UpdateCheckInterval } from './types'

const CHECK_INTERVALS: Record<Exclude<UpdateCheckInterval, 'never'>, number> = {
  daily: 24 * 60 * 60 * 1000,
  weekly: 7 * 24 * 60 * 60 * 1000,
  monthly: 30 * 24 * 60 * 60 * 1000
}

// Browsers clamp longer timers to a signed 32-bit delay (about 24.9 days).
// Split monthly waits so they cannot turn into a rapid polling loop.
const MAX_TIMER_DELAY = 2_147_483_647

const CHECK_INTERVAL_LABELS: Record<UpdateCheckInterval, string> = {
  daily: 'every day',
  weekly: 'every week',
  monthly: 'every month',
  never: 'disabled'
}

type Props = { updateCheckInterval?: UpdateCheckInterval | null }

export default function UpdateStatus({ updateCheckInterval = 'daily' }: Props) {
  const interval = updateCheckInterval
  const [update, setUpdate] = useState<AppUpdate | null>(null)
  const [checking, setChecking] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [open, setOpen] = useState(false)
  const running = useRef(false)
  const mounted = useRef(false)
  const container = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)

  const check = useCallback(async () => {
    if (running.current) return
    running.current = true
    setChecking(true)
    setError(null)
    try {
      const result = await checkForUpdates()
      if (mounted.current) setUpdate(result)
    } catch {
      if (mounted.current) setError('Could not check for updates. Check your connection and GitHub CLI sign-in, then try again.')
    } finally {
      running.current = false
      if (mounted.current) setChecking(false)
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])

  useEffect(() => {
    if (interval === null || interval === 'never') return
    let timer: number | undefined
    let cancelled = false
    const schedule = (remaining: number) => {
      const delay = Math.min(remaining, MAX_TIMER_DELAY)
      timer = window.setTimeout(() => {
        if (cancelled) return
        if (remaining > MAX_TIMER_DELAY) {
          schedule(remaining - MAX_TIMER_DELAY)
          return
        }
        void check()
        schedule(CHECK_INTERVALS[interval])
      }, delay)
    }
    void check()
    schedule(CHECK_INTERVALS[interval])
    return () => {
      cancelled = true
      if (timer !== undefined) window.clearTimeout(timer)
    }
  }, [check, interval])

  useEffect(() => {
    if (!open) return
    const dismiss = (event: PointerEvent) => {
      if (!container.current?.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('pointerdown', dismiss)
    return () => document.removeEventListener('pointerdown', dismiss)
  }, [open])

  async function viewRelease() {
    if (!update) return
    try {
      await openExternalUrl(update.release_url)
    } catch {
      setError('Could not open the release page. Please try again.')
    }
  }

  return <div className="update-status" ref={container} onKeyDown={(event) => {
    if (event.key === 'Escape') { setOpen(false); trigger.current?.focus() }
  }}>
    <button ref={trigger} type="button" className={update?.update_available ? 'button compact update-available' : 'icon-button'}
      aria-label={update?.update_available ? `Update available: ${update.latest_version}` : 'App updates'}
      title={update?.update_available ? `CodeTally ${update.latest_version} is available` : 'App updates'}
      aria-expanded={open} aria-controls="app-update-panel" onClick={() => setOpen(!open)}>
      <ArrowUpCircle size={17} />{update?.update_available && <span>Update available</span>}
    </button>
    {open && <section className="update-panel" id="app-update-panel" aria-label="App updates">
      <div className="update-heading"><strong>App updates</strong><button type="button" className="icon-button" aria-label="Close app updates" onClick={() => { setOpen(false); trigger.current?.focus() }}><X size={15} /></button></div>
      {update && <p>Installed version: {update.current_version}</p>}
      <div role="status">
        {checking ? <p>Checking GitHub releases…</p> : update && (!error || update.update_available) && <p>{update.update_available ? `CodeTally ${update.latest_version} is available.` : 'You’re up to date.'}</p>}
      </div>
      {error && <p role="alert" className="update-error">{error}</p>}
      {update?.update_available && <><p>View release notes and download the latest version from GitHub.</p><button type="button" className="button primary" onClick={() => void viewRelease()}><ExternalLink size={14} />View release</button></>}
      <button type="button" className="button" disabled={checking} onClick={() => void check()}>{checking && <LoaderCircle size={14} className="spin" />}{checking ? 'Checking…' : 'Check for updates'}</button>
      <p className="update-note">{interval === null || interval === 'never' ? 'Automatic checks are disabled. Check manually whenever you like.' : `Checks at startup and ${CHECK_INTERVAL_LABELS[interval]}.`}</p>
    </section>}
  </div>
}
