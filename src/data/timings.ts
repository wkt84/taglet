// Timings that test builds show (and log to the console), to measure how long opening
// files and starting the app take (#20). Built with VITE_TAGLET_TIMINGS=1, as the
// Windows test build is; release builds do not measure.

import { startupSteps, type StartupMark, type StartupStep } from './startupSteps'

export const TIMINGS_ENABLED = import.meta.env.VITE_TAGLET_TIMINGS === '1'

export type OpenTiming = {
  file: string
  /** From asking the backend to open the file until the table is painted. */
  totalMs: number
  /** Reading the file in the backend. */
  readMs?: number
  /** The rest of the call: sending the result, parsing it and rebuilding the nodes (see `openedFile`). */
  transferMs: number
  /** From receiving the result until the table is painted. */
  renderMs: number
}

export type StartupTiming = {
  /** From the process being created (or the backend starting) until the first screen is painted. */
  totalMs: number
  /** What that took, step by step (see `startupSteps`). */
  steps: StartupStep[]
}

export type Timings = { open?: OpenTiming; startup?: StartupTiming }

let latest: Timings = {}
const listeners = new Set<() => void>()

export function subscribeTimings(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function currentTimings() {
  return latest
}

function update(next: Timings) {
  latest = { ...latest, ...next }
  for (const listener of listeners) listener()
}

/** Run `callback` once the next frame has been painted. */
function afterNextPaint(callback: () => void) {
  requestAnimationFrame(() => setTimeout(callback, 0))
}

type PendingOpen = { path: string; started: number; received: number; readMs?: number }
let pendingOpen: PendingOpen | undefined

/**
 * Record that the file at `path` was asked for at `started` and received at
 * `received` (both `performance.now()`), `readMs` being the backend's own time. The
 * timing ends once its nodes are shown (see `openShown`).
 */
export function recordOpen(path: string, started: number, received: number, readMs?: number) {
  if (!TIMINGS_ENABLED) return
  pendingOpen = { path, started, received, readMs }
}

/**
 * Call after React has committed the nodes of the file at `path` (from an effect): the
 * timing of opening it ends when the next frame has been painted. Files whose nodes
 * were never shown (others opened at the same time) are not timed.
 */
export function openShown(path: string | undefined) {
  const pending = pendingOpen
  if (!pending || pending.path !== path) return
  pendingOpen = undefined
  afterNextPaint(() => {
    const painted = performance.now()
    const callMs = pending.received - pending.started
    const open: OpenTiming = {
      file: pending.path.split(/[\\/]/).pop() ?? pending.path,
      totalMs: painted - pending.started,
      readMs: pending.readMs,
      transferMs: pending.readMs === undefined ? callMs : Math.max(0, callMs - pending.readMs),
      renderMs: painted - pending.received,
    }
    console.info('[taglet] open', open)
    update({ open })
  })
}

/**
 * Record startup, once the first screen is painted. `backendMarks` gives the moments the
 * backend recorded, in milliseconds since the Unix epoch: the page's moments are taken
 * by the same clock, so the call itself does not count.
 */
export function recordStartup(backendMarks: () => Promise<StartupMark[]>) {
  if (!TIMINGS_ENABLED) return
  afterNextPaint(() => {
    // When the first content was painted, as the browser records it; otherwise now
    // (just after the first paint).
    const firstPaint = performance
      .getEntriesByType('paint')
      .find((entry) => entry.name === 'first-contentful-paint')
    const navigationAt = performance.timeOrigin
    const paintedAt = navigationAt + (firstPaint?.startTime ?? performance.now())
    void backendMarks().then((marks) => {
      const startup: StartupTiming = startupSteps(marks, navigationAt, paintedAt)
      const first = Math.min(...marks.map((mark) => mark.at_ms), navigationAt)
      const moments = [...marks, { name: 'navigation', at_ms: navigationAt }, { name: 'paint', at_ms: paintedAt }]
        .map((mark) => `${mark.name} +${Math.round(mark.at_ms - first)}`)
      console.info('[taglet] startup', startup, moments.join(', '))
      update({ startup })
    }).catch(() => {})
  })
}

export function formatOpenTiming(open: OpenTiming) {
  const parts = [
    open.readMs === undefined ? undefined : `read ${Math.round(open.readMs)}`,
    `transfer ${Math.round(open.transferMs)}`,
    `render ${Math.round(open.renderMs)}`,
  ].filter(Boolean)
  return `Opened ${open.file} in ${Math.round(open.totalMs)} ms (${parts.join(' · ')})`
}

export function formatStartupTiming(startup: StartupTiming) {
  const steps = startup.steps.map((step) => `${step.name} ${Math.round(step.ms)}`)
  return `Started in ${Math.round(startup.totalMs)} ms (${steps.join(' · ')})`
}
