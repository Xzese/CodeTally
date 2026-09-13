import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowUpCircle, ExternalLink, LoaderCircle, X } from 'lucide-react'
import { checkForUpdates, openExternalUrl, type AppUpdate } from './api'

const CHECK_INTERVAL = 6 * 60 * 60 * 1000

export default function UpdateStatus() {
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
    void check()
    const timer = window.setInterval(() => void check(), CHECK_INTERVAL)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [check])

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
      <p className="update-note">Checks at startup and every six hours.</p>
    </section>}
  </div>
}
