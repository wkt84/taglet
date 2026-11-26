// The steps of starting the app (#36), from the moments the backend records
// (src-tauri/src/commands/timing.rs) and the page's own, all in milliseconds since the
// Unix epoch (the same clock).

/** A moment the backend records. */
export type StartupMark = { name: string; at_ms: number }

export type StartupStep = { name: string; ms: number }

/**
 * The steps, each named after what it takes, ending at the moment of the next:
 * - `process`: loading the program and its libraries (from the process being created,
 *   where the backend knows it, until `run` starts),
 * - `init`: plugins and state (until `built`),
 * - `window`: creating the window and its webview (until `window`),
 * - `load`: until the page starts loading (`navigation`, the page's time origin),
 * - `page`: loading the page and painting it (until `paint`).
 * Moments the backend did not record are left out, with their steps.
 */
const STEPS: [step: string, endsAt: string][] = [
  ['process', 'run'],
  ['init', 'built'],
  ['window', 'window'],
  ['load', 'navigation'],
  ['page', 'paint'],
]

export function startupSteps(marks: StartupMark[], navigationAt: number, paintedAt: number) {
  const at = new Map(marks.map((mark) => [mark.name, mark.at_ms]))
  at.set('navigation', navigationAt)
  at.set('paint', paintedAt)
  const moments: [step: string | undefined, at: number][] = []
  if (at.has('process')) moments.push([undefined, at.get('process')!])
  for (const [step, endsAt] of STEPS) {
    const time = at.get(endsAt)
    if (time !== undefined) moments.push([moments.length === 0 ? undefined : step, time])
  }
  const steps: StartupStep[] = []
  for (let index = 1; index < moments.length; index++) {
    steps.push({ name: moments[index][0]!, ms: moments[index][1] - moments[index - 1][1] })
  }
  return { totalMs: paintedAt - moments[0][1], steps }
}
