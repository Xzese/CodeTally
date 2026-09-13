import { useState, type MouseEvent } from 'react'
import { AlertCircle, Check, Clipboard, ExternalLink, Github, LoaderCircle, Terminal } from 'lucide-react'
import { openExternalUrl } from './api'
import type { DependencyStatus } from './types'

export const GITHUB_AUTH_COMMAND = 'gh auth login --hostname github.com --web'
export const GITHUB_CLI_INSTALL_URL = 'https://cli.github.com/'

export interface GitHubConnectionProps {
  deps: DependencyStatus
  login?: string | null
  compact?: boolean
  checking?: boolean
  onRetry: () => void | Promise<void>
}

function isAuthenticated(deps: DependencyStatus): boolean {
  return deps.gh_authenticated || deps.authenticated === true
}

function couldNotVerifyAuthentication(deps: DependencyStatus): boolean {
  if (!deps.gh || isAuthenticated(deps) || !deps.error) return false
  return /timeout|timed\s*out|network|offline|unreachable|connection|temporar|unavailable/i.test(deps.error)
}

function authenticationMessage(deps: DependencyStatus): string {
  if (couldNotVerifyAuthentication(deps)) {
    return "CodeTally couldn't verify the GitHub CLI session just now. Your cached dashboard is still available; check your connection and try again."
  }
  return 'Run the sign-in command below in Terminal. If you are already signed in, run gh auth status there, then check the connection again.'
}

function ConnectionStatus({ deps, login }: { deps: DependencyStatus; login?: string | null }) {
  const authenticated = isAuthenticated(deps)
  if (!deps.gh) {
    return <div className="github-connection-status github-connection-status-missing"><span className="check-icon"><Terminal size={15} /></span><span><strong>GitHub CLI is not installed</strong><small>Install it before signing in to GitHub.</small></span></div>
  }
  if (authenticated) {
    return <div className="github-connection-status github-connection-status-ready" role="status"><span className="check-icon"><Check size={15} /></span><span><strong>GitHub connected</strong><small>{login || deps.login ? `Signed in as ${login || deps.login}` : 'The local GitHub CLI session is ready.'}</small></span><span className="check-state">Ready</span></div>
  }
  return <div className={couldNotVerifyAuthentication(deps) ? 'github-connection-status github-connection-status-unavailable' : 'github-connection-status github-connection-status-disconnected'} role="status"><span className="check-icon"><AlertCircle size={15} /></span><span><strong>GitHub session could not be verified</strong><small>{authenticationMessage(deps)}</small></span></div>
}

function GitHubCliLink({ compact = false }: { compact?: boolean }) {
  const [error, setError] = useState(false)
  const open = async (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault()
    setError(false)
    try {
      await openExternalUrl(GITHUB_CLI_INSTALL_URL)
    } catch {
      setError(true)
    }
  }
  return <span className="github-install-link-wrap"><a className={compact ? 'github-install-link' : 'button secondary compact github-install-link'} href={GITHUB_CLI_INSTALL_URL} onClick={(event) => void open(event)}>{compact ? <>GitHub CLI help <ExternalLink size={12} /></> : <><Terminal size={14} /> Install GitHub CLI <ExternalLink size={12} /></>}</a>{error && <span className="github-link-error" role="status">Could not open the GitHub CLI page.</span>}</span>
}

function CopyAuthCommand() {
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>('idle')

  const copy = async () => {
    setCopyState('idle')
    try {
      if (typeof navigator === 'undefined' || !navigator.clipboard?.writeText) throw new Error('Clipboard access is unavailable')
      await navigator.clipboard.writeText(GITHUB_AUTH_COMMAND)
      setCopyState('copied')
    } catch {
      setCopyState('failed')
    }
  }

  return <div className="github-auth-command-wrap"><code className="github-auth-command">{GITHUB_AUTH_COMMAND}</code><button className="button secondary compact github-copy-button" type="button" onClick={() => void copy()} aria-label="Copy GitHub authentication command">{copyState === 'copied' ? <Check size={14} /> : <Clipboard size={14} />}{copyState === 'copied' ? 'Copied' : 'Copy command'}</button>{copyState === 'failed' && <span className="github-copy-error" role="status">Could not copy. Select the command above and copy it manually.</span>}</div>
}

function CheckConnectionButton({ checking, onRetry, secondary = false }: { checking: boolean; onRetry: () => void | Promise<void>; secondary?: boolean }) {
  const [localChecking, setLocalChecking] = useState(false)
  const isChecking = checking || localChecking
  const check = async () => {
    setLocalChecking(true)
    try {
      await onRetry()
    } finally {
      setLocalChecking(false)
    }
  }
  return <button className={secondary ? 'button secondary compact github-check-button' : 'button primary compact github-check-button'} type="button" disabled={isChecking} onClick={() => void check()}>{isChecking ? <><LoaderCircle size={14} className="spin" /> Checking connection…</> : <><Github size={14} /> Check connection</>}</button>
}

function AuthGuide({ checking, onRetry, compact = false, showCheckButton = true }: { checking: boolean; onRetry: () => void | Promise<void>; compact?: boolean; showCheckButton?: boolean }) {
  const [localChecking, setLocalChecking] = useState(false)
  const [expanded, setExpanded] = useState(!compact)
  const isChecking = checking || localChecking
  const check = async () => {
    setLocalChecking(true)
    try {
      await onRetry()
    } finally {
      setLocalChecking(false)
    }
  }

  const steps = <ol className="github-auth-steps"><li className="github-auth-step"><span className="github-step-number">1</span><span><strong>Open Terminal</strong><small>Use the Terminal app on your Mac.</small></span></li><li className="github-auth-step"><span className="github-step-number">2</span><span><strong>Copy and run this command</strong><small>It opens GitHub in your browser. Finish the authorization there, then return to CodeTally.</small><CopyAuthCommand /></span></li><li className="github-auth-step"><span className="github-step-number">3</span><span><strong>Check connection</strong><small>CodeTally will recheck the local GitHub CLI session without starting authentication.</small>{showCheckButton && <button className="button primary compact github-check-button" type="button" disabled={isChecking} onClick={() => void check()}>{isChecking ? <><LoaderCircle size={14} className="spin" /> Checking connection…</> : <><Github size={14} /> Check connection</>}</button>}</span></li></ol>
  return <div className={compact ? 'github-auth-guide github-auth-guide-compact' : 'github-auth-guide'}>{compact && <button className="github-auth-toggle" type="button" aria-expanded={expanded} onClick={() => setExpanded((current) => !current)}>{expanded ? 'Hide sign-in instructions' : 'Sign-in instructions'}</button>}{!compact && <p className="github-auth-intro">Authorize GitHub in Terminal and in your browser, then return here to recheck the local session.</p>}{expanded && steps}</div>
}

export default function GitHubConnection({ deps, login, compact = false, checking = false, onRetry }: GitHubConnectionProps) {
  const authenticated = isAuthenticated(deps)
  const showCheck = !deps.gh || !authenticated || !deps.git || !deps.tokei

  if (compact) {
    return <section className="repos-panel github-connection github-connection-compact" aria-labelledby="github-reconnect-heading"><div className="panel-heading github-reconnect-heading"><div><p className="eyebrow">GitHub connection</p><h2 id="github-reconnect-heading">Reconnect GitHub</h2></div><span className="github-reconnect-icon"><Github size={17} /></span></div><p className="github-reconnect-copy">{couldNotVerifyAuthentication(deps) ? "We couldn't verify the GitHub CLI session. Cached data remains available while you check your connection." : 'GitHub session could not be verified. Authorize it in Terminal, then check the connection to refresh your cached data.'}</p><div className="github-reconnect-actions"><CheckConnectionButton checking={checking} onRetry={onRetry} /><GitHubCliLink compact /></div><AuthGuide compact checking={checking} onRetry={onRetry} showCheckButton={false} /></section>
  }

  return <section className="github-connection" aria-labelledby="github-connection-heading"><p className="eyebrow">GitHub connection</p><h1 id="github-connection-heading">Connect GitHub to CodeTally</h1><p className="setup-copy">CodeTally uses the GitHub CLI already installed on this Mac. Your authentication stays with GitHub CLI and no token is stored by CodeTally.</p><ConnectionStatus deps={deps} login={login} />{!deps.gh && <div className="github-install"><p>Install the official GitHub CLI, then come back and check the connection.</p><GitHubCliLink /></div>}{deps.gh && !authenticated && <AuthGuide checking={checking} onRetry={onRetry} />}{showCheck && !deps.gh && <div className="github-missing-check"><button className="button secondary compact github-check-button" type="button" disabled={checking} onClick={() => void onRetry()}>{checking ? <><LoaderCircle size={14} className="spin" /> Checking connection…</> : 'Check connection'}</button></div>}{showCheck && deps.gh && authenticated && <div className="github-tool-check"><p className="github-tool-check-copy">GitHub is connected. Check again after installing any missing local tools below.</p><button className="button secondary compact github-check-button" type="button" disabled={checking} onClick={() => void onRetry()}>{checking ? <><LoaderCircle size={14} className="spin" /> Checking connection…</> : 'Check connection'}</button></div>}</section>
}
