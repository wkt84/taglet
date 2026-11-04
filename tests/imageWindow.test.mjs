import assert from 'node:assert/strict'
import test from 'node:test'
import { autoWindow, clampWindowToRange, dragWindowBounds, formatWindowNumber, windowLimits } from '../src/data/imageWindow.ts'

test('fractional and scientific dose values keep their magnitude when displayed', () => {
  for (const value of [1e-10, 2e-20, 1.2345678e-10, 0.00000025, -1e-10, 1000, 0]) {
    assert.equal(Number(formatWindowNumber(value)), value)
  }
  assert.ok(windowLimits(0, 1e-8).minWidth < 1e-8)
  const flat = autoWindow(0.002, 0.002)
  assert.ok(flat.width > 0)
  assert.equal(flat.center, 0.002)
})

test('drag uses the original window, preserves the opposite bound, and reverses at crossing', () => {
  const drag = { kind: 'high', low: 64, high: 192, startY: 100, unitsPerPixel: 1, minWidth: 1, pointerId: 1 }
  assert.deepEqual(dragWindowBounds(drag, 100), { low: 64, high: 192 })
  assert.deepEqual(dragWindowBounds(drag, 300), { low: 64, high: 65 })
  assert.deepEqual(dragWindowBounds(drag, 100), { low: 64, high: 192 })
  assert.deepEqual(dragWindowBounds({ ...drag, kind: 'low' }, -100), { low: 191, high: 192 })
  assert.deepEqual(dragWindowBounds({ ...drag, kind: 'window' }, 110), { low: 54, high: 182 })
})

test('windows beyond the pixel range are clamped to the drawn slider range', () => {
  assert.deepEqual(clampWindowToRange({ low: -50, high: 150 }, -3.5, 21.18, 0.01), { low: -3.5, high: 21.18 })
  assert.deepEqual(clampWindowToRange({ low: 2, high: 150 }, -3.5, 21.18, 0.01), { low: 2, high: 21.18 })
  assert.deepEqual(clampWindowToRange({ low: 1, high: 2 }, 0, 10, 0.5), { low: 1, high: 2 })
  // Entirely above or below the range: keep a minimal window at the edge where both handles are drawn.
  assert.deepEqual(clampWindowToRange({ low: 30, high: 40 }, 0, 10, 0.5), { low: 9.5, high: 10 })
  assert.deepEqual(clampWindowToRange({ low: -40, high: -30 }, 0, 10, 0.5), { low: 0, high: 0.5 })
  // Flat frames have no slider range to clamp into.
  assert.deepEqual(clampWindowToRange({ low: 1, high: 3 }, 2, 2, 0.5), { low: 1, high: 3 })
})
