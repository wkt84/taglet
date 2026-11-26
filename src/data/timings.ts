// Timings that test builds show (and log to the console), to measure how long opening
// files and starting the app take (#20). Built with VITE_TAGLET_TIMINGS=1, as the
// Windows test build is; release builds do not measure.

export const TIMINGS_ENABLED = import.meta.env.VITE_TAGLET_TIMINGS === '1'

export type OpenTiming = {
  file: string
  /** From asking the backend to open the file until the table is painted. */
  totalMs: number
  /** Reading the file in the backend. */
  readMs?: number
  /** The rest of the call: sending the result and parsing it. */
  transferMs: number
  /** From receiving the result until the table is painted. */
  renderMs: number
}

export type StartupTiming = {
  /** From the backend starting until the first screen is painted. */
  totalMs: number
  /** From the page starting to load until then. */
  pageMs: number
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

/**
 * Record opening `file`: asked for at `started`, received at `received` (both
 * `performance.now()`), `readMs` the backend's own time. Call after the result has been
 * put in state; the table is painted in the next frame.
 */
export function recordOpen(file: string, started: number, received: number, readMs?: number) {
  if (!TIMINGS_ENABLED) return
  afterNextPaint(() => {
    const painted = performance.now()
    const callMs = received - started
    const open: OpenTiming = {
      file,
      totalMs: painted - started,
      readMs,
      transferMs: readMs === undefined ? callMs : Math.max(0, callMs - readMs),
      renderMs: painted - received,
    }
    console.info('[taglet] open', open)
    update({ open })
  })
}

/**
 * Record startup, once the first screen is painted. `backendStartedAt` gives when the
 * backend started, in milliseconds since the Unix epoch: the time until the first paint
 * is taken by the same clock, so the call itself does not count.
 */
export function recordStartup(backendStartedAt: () => Promise<number>) {
  if (!TIMINGS_ENABLED) return
  afterNextPaint(() => {
    const pageMs = performance.now()
    const paintedAt = performance.timeOrigin + pageMs
    void backendStartedAt().then((startedAt) => {
      const startup: StartupTiming = { totalMs: paintedAt - startedAt, pageMs }
      console.info('[taglet] startup', startup)
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
  return `Started in ${Math.round(startup.totalMs)} ms (page ${Math.round(startup.pageMs)})`
}
