export type ImageWindow = { center: number; width: number; auto: boolean }
export type WindowBounds = { low: number; high: number }
export type WindowDrag = WindowBounds & {
  kind: 'low' | 'high' | 'window'
  pointerId: number
  startY: number
  unitsPerPixel: number
  minWidth: number
  moved?: boolean
}

export function windowLimits(min: number, max: number) {
  const range = Math.max(0, max - min)
  // Keep fractional dose values usable, while allowing a nonzero window on flat images.
  const precision = Math.max(Math.abs(min), Math.abs(max), 1) * Number.EPSILON * 16
  return {
    minWidth: Math.max(range / 4096, precision),
    step: Math.max(range / 2048, precision),
  }
}

export function autoWindow(min: number, max: number): ImageWindow {
  return { center: min + (max - min) / 2, width: Math.max(max - min, windowLimits(min, max).minWidth), auto: true }
}

export function formatWindowNumber(value: number) {
  return Number.isFinite(value) ? Number(value.toPrecision(8)).toString() : '0'
}

export function dragWindowBounds(drag: WindowDrag, clientY: number): WindowBounds {
  const delta = (drag.startY - clientY) * drag.unitsPerPixel
  if (drag.kind === 'low') {
    return { low: Math.min(drag.low + delta, drag.high - drag.minWidth), high: drag.high }
  }
  if (drag.kind === 'high') {
    return { low: drag.low, high: Math.max(drag.high + delta, drag.low + drag.minWidth) }
  }
  return { low: drag.low + delta, high: drag.high + delta }
}

// A handle drag starts from where that handle is drawn: its bound is clamped into the slider range,
// so a DICOM window beyond the pixel range (common for RTIMAGE) has no dead zone. The opposite bound
// is kept unless the minimum width forces it to move. Moving the whole window keeps its width.
export function dragStartBounds(kind: WindowDrag['kind'], bounds: WindowBounds, min: number, max: number, minWidth: number): WindowBounds {
  if (kind === 'window' || !(max - min >= minWidth)) return bounds
  if (kind === 'low') {
    const low = Math.min(Math.max(bounds.low, min), max - minWidth)
    return { low, high: Math.max(bounds.high, low + minWidth) }
  }
  const high = Math.max(Math.min(bounds.high, max), min + minWidth)
  return { low: Math.min(bounds.low, high - minWidth), high }
}
