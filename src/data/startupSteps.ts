// The steps of starting the app (#36), from the moments the backend records
// (src-tauri/src/commands/timing.rs) and the page's own, all in milliseconds since the
// Unix epoch (the same clock).

/** A moment the backend records. */
export type StartupMark = { name: string; at_ms: number }

export type StartupStep = { name: string; ms: number }

/**
 * The steps between consecutive moments, in the order they happened, each named after
 * the moment it ends at:
 * - `process`: until `run` (from the process being created, where the backend knows it:
 *   loading the program and its libraries),
 * - `init`: until `built` (plugins and state),
 * - `window`: until `window` (the window and its webview created),
 * - `load`: until `navigation` (the page starting to load: its time origin),
 * - `page`: until `paint` (the first screen painted).
 * The webview can start loading the page before it is reported created: the steps
 * then come in that order (`load` before `window`), so that none is negative and they
 * add up to the total. Moments the backend did not record, or recorded after the
 * first paint, are left out, their time counting in the next step.
 */
const STEP_ENDING_AT: Record<string, string> = {
  run: 'process',
  built: 'init',
  window: 'window',
  navigation: 'load',
  paint: 'page',
}

export function startupSteps(marks: StartupMark[], navigationAt: number, paintedAt: number) {
  const moments = [
    ...marks.filter((mark) => mark.name === 'process' || mark.name in STEP_ENDING_AT),
    { name: 'navigation', at_ms: navigationAt },
    { name: 'paint', at_ms: paintedAt },
  ]
    .map((mark, index) => ({ ...mark, index }))
    // What the backend records after the first paint (it can report the webview
    // created after the page has painted) is not part of starting.
    .filter((mark) => mark.at_ms <= paintedAt)
    // In time order; at the same time, in the order above.
    .sort((a, b) => a.at_ms - b.at_ms || a.index - b.index)
  const steps: StartupStep[] = []
  for (let index = 1; index < moments.length; index++) {
    steps.push({ name: STEP_ENDING_AT[moments[index].name], ms: moments[index].at_ms - moments[index - 1].at_ms })
  }
  return { totalMs: paintedAt - moments[0].at_ms, steps }
}
