import { useEffect, useSyncExternalStore } from 'react'
import { invoke } from '@tauri-apps/api/core'
import {
  TIMINGS_ENABLED,
  currentTimings,
  formatOpenTiming,
  formatStartupTiming,
  recordStartup,
  subscribeTimings,
} from '../data/timings'

/** The latest timings, in test builds only (see `data/timings`). */
export default function TimingBadge() {
  const timings = useSyncExternalStore(subscribeTimings, currentTimings)

  useEffect(() => {
    recordStartup(() => invoke<number>('startup_elapsed_ms'))
  }, [])

  if (!TIMINGS_ENABLED || (!timings.open && !timings.startup)) return null
  return (
    <div
      className="pointer-events-none fixed bottom-2 right-2 z-40 rounded bg-slate-900/80 px-2 py-1 text-right font-mono text-[11px] leading-4 text-white"
      aria-label="Timings"
    >
      {timings.open ? <div>{formatOpenTiming(timings.open)}</div> : null}
      {timings.startup ? <div>{formatStartupTiming(timings.startup)}</div> : null}
    </div>
  )
}
