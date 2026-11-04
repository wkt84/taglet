import assert from 'node:assert/strict'
import test from 'node:test'
import { autoWindow, dragStartBounds, dragWindowBounds, formatWindowNumber, windowLimits } from '../src/data/imageWindow.ts'

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

test('a handle drag starts from the drawn handle and keeps the opposite bound', () => {
  assert.deepEqual(dragStartBounds('high', { low: -50, high: 150 }, -3.5, 21.18, 0.01), { low: -50, high: 21.18 })
  assert.deepEqual(dragStartBounds('low', { low: -50, high: 150 }, -3.5, 21.18, 0.01), { low: -3.5, high: 150 })
  assert.deepEqual(dragStartBounds('low', { low: 192, high: 320 }, 0, 256, 1), { low: 192, high: 320 })
  assert.deepEqual(dragStartBounds('high', { low: 192, high: 320 }, 0, 256, 1), { low: 192, high: 256 })
  assert.deepEqual(dragStartBounds('high', { low: 1, high: 2 }, 0, 10, 0.5), { low: 1, high: 2 })
  // Entirely outside the range: both handles are drawn at one edge, so the other bound moves to keep the minimum width.
  assert.deepEqual(dragStartBounds('high', { low: 30, high: 40 }, 0, 10, 0.5), { low: 9.5, high: 10 })
  assert.deepEqual(dragStartBounds('low', { low: -40, high: -30 }, 0, 10, 0.5), { low: 0, high: 0.5 })
  // Flat frames have no slider range to clamp into.
  assert.deepEqual(dragStartBounds('high', { low: 1, high: 3 }, 2, 2, 0.5), { low: 1, high: 3 })
})

test('moving the whole window keeps its width even beyond the pixel range', () => {
  assert.deepEqual(dragStartBounds('window', { low: 192, high: 320 }, 0, 256, 1), { low: 192, high: 320 })
  assert.deepEqual(dragStartBounds('window', { low: -50, high: 150 }, -3.5, 21.18, 0.01), { low: -50, high: 150 })
})
