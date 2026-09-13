import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowUpCircle, Download, LoaderCircle, X } from 'lucide-react'
import { checkForUpdates, installAppUpdate, type AppUpdate } from './api'
import type { UpdateCheckInterval } from './types'

const CHECK_INTERVALS: Record<Exclude<UpdateCheckInterval, 'never'>, number> = {
  daily: 24 * 60 * 60 * 1000,
  weekly: 7 * 24 * 60 * 60 * 1000,
  monthly: 30 * 24 * 60 * 60 * 1000
}

// Browsers clamp longer timers to a signed 32-bit delay (about 24.9 days).
// Split monthly waits so they cannot turn into a rapid polling loop.
const MAX_TIMER_DELAY = 2_147_483_647

type Props = { updateCheckInterval?: UpdateCheckInterval | null }

export default function UpdateStatus({ updateCheckInterval = 'daily' }: Props) {
  const interval = updateCheckInterval
  const [update, setUpdate] = useState<AppUpdate | null>(null)
  const [checking, setChecking] = useState(false)
  const [installing, setInstalling] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [checkFailed, setCheckFailed] = useState(false)
  const [open, setOpen] = useState(false)
  const [pinned, setPinned] = useState(false)
  const running = useRef(false)
  const mounted = useRef(false)
  const dismissed = useRef(false)
  const container = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)

  const check = useCallback(async () => {
    if (running.current) return
    running.current = true
    setChecking(true)
    setError(null)
    setCheckFailed(false)
    try {
      const result = await checkForUpdates()
      if (mounted.current) setUpdate(result)
    } catch {
      if (mounted.current) {
        setCheckFailed(true)
        setError("Couldn't check for updates. Check your connection and GitHub sign-in, then try again.")
      }
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
      if (!container.current?.contains(event.target as Node)) {
        setOpen(false)
        setPinned(false)
      }
    }
    document.addEventListener('pointerdown', dismiss)
    return () => document.removeEventListener('pointerdown', dismiss)
  }, [open])

  async function installUpdate() {
    setInstalling(true)
    setError(null)
    try {
      await installAppUpdate()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
      setInstalling(false)
    }
  }

  if (!update?.update_available) return null

  return <div className={open ? 'update-status active' : 'update-status'} ref={container}
    onMouseEnter={() => { dismissed.current = false; setOpen(true) }}
    onMouseLeave={() => { dismissed.current = false; if (!pinned && !container.current?.contains(document.activeElement)) setOpen(false) }}
    onFocusCapture={() => { if (!dismissed.current) setOpen(true) }}
    onBlurCapture={(event) => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
        dismissed.current = false
        if (!pinned) setOpen(false)
      }
    }}
    onKeyDown={(event) => {
      if (event.key === 'Escape') { dismissed.current = true; setOpen(false); setPinned(false); trigger.current?.focus() }
    }}>
    <button ref={trigger} type="button" className={update?.update_available ? 'button compact update-available' : 'icon-button'}
      aria-label={update?.update_available ? `Update available: ${update.latest_version}` : 'Updates'}
      title={update?.update_available ? `CodeTally ${update.latest_version} is available` : 'Updates'}
      aria-expanded={open} aria-controls="app-update-panel" onClick={() => {
        if (open && pinned) {
          dismissed.current = true
          setOpen(false)
          setPinned(false)
        } else {
          dismissed.current = false
          setOpen(true)
          setPinned(true)
        }
      }}>
      <ArrowUpCircle size={17} />{update?.update_available && <span>Update available</span>}
    </button>
    {open && <section className="update-panel" id="app-update-panel" aria-label="Updates">
      <div className="update-heading"><strong>Updates</strong><button type="button" className="icon-button" aria-label="Close updates" onClick={() => { dismissed.current = true; setOpen(false); setPinned(false); trigger.current?.focus() }}><X size={15} /></button></div>
      {update ? <div className="update-versions" role="status"><p><span>Current version:</span><strong>{update.current_version}</strong></p><p><span>New version:</span><strong className={!checking && !checkFailed && update.update_available ? 'available' : ''}>{checking ? 'Checking…' : checkFailed ? 'Unavailable' : update.update_available ? update.latest_version : 'No update available'}</strong></p></div> : checking ? <p role="status">Checking for updates…</p> : null}
      {error && <p role="alert" className="update-error">{error}</p>}
      <div className="update-actions">{update?.update_available && <button type="button" className="button primary" disabled={installing} onClick={() => void installUpdate()}>{installing ? <LoaderCircle size={14} className="spin" /> : <Download size={14} />}{installing ? 'Installing…' : 'Download update'}</button>}<button type="button" className="button" disabled={checking || installing} onClick={() => void check()}>{checking && <LoaderCircle size={14} className="spin" />}{checking ? 'Checking…' : 'Check for updates'}</button></div>
    </section>}
  </div>
}
