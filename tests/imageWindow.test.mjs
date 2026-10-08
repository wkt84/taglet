import assert from 'node:assert/strict'
import test from 'node:test'
import { autoWindow, dragWindowBounds, formatWindowNumber, windowLimits } from '../src/data/imageWindow.ts'

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
