import assert from 'node:assert/strict'
import test from 'node:test'
import { extractStoredValue, pixelLayout } from '../src/data/pixelLayout.ts'

test('bits above Bits Stored are ignored', () => {
  const layout = pixelLayout(16, 12, 11, false)
  assert.equal(extractStoredValue(0xFFFF, layout), 0x0FFF)
  assert.equal(extractStoredValue(0xA234, layout), 0x0234)
})

test('signed values sign-extend from Bits Stored', () => {
  const layout = pixelLayout(16, 12, 11, true)
  assert.equal(extractStoredValue(0xFFFF, layout), -1)
  assert.equal(extractStoredValue(0x5800, layout), -2048)
  assert.equal(extractStoredValue(0x07FF, layout), 2047)
})

test('High Bit offsets the stored bits', () => {
  assert.equal(extractStoredValue(0xABC0, pixelLayout(16, 12, 15, false)), 0xABC)
})

test('full 32-bit samples stay exact', () => {
  assert.equal(extractStoredValue(0xFFFFFFFF, pixelLayout(32, 32, 31, false)), 0xFFFFFFFF)
  assert.equal(extractStoredValue(0xFFFFFFFF, pixelLayout(32, 32, 31, true)), -1)
  assert.equal(extractStoredValue(0x80000000, pixelLayout(32, null, null, true)), -(2 ** 31))
})

test('missing or inconsistent tags fall back to the full sample, matching the Rust side', () => {
  assert.deepEqual(pixelLayout(16, undefined, undefined, false), { bitsAllocated: 16, bitsStored: 16, highBit: 15, signed: false })
  assert.equal(pixelLayout(16, 0, null, false).bitsStored, 16)
  assert.equal(pixelLayout(16, 20, null, false).bitsStored, 16)
  assert.equal(pixelLayout(16, 12, 16, false).highBit, 11)
  assert.equal(pixelLayout(16, 12, 5, false).highBit, 11)
})
