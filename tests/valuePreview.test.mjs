import assert from 'node:assert/strict'
import test from 'node:test'
import { valuePreview } from '../src/data/valuePreview.ts'

test('preview counts Unicode characters without splitting surrogate pairs', () => {
  const boundary = '😀'.repeat(120)
  assert.deepEqual(valuePreview(boundary), { text: boundary, needsDialog: false })
  assert.deepEqual(valuePreview(boundary + 'X'), { text: boundary + '…', needsDialog: true })
})

test('preview represents line breaks and tabs without changing the source text', () => {
  const text = 'A\r\nB\rC\nD\tE  '
  assert.deepEqual(valuePreview(text), { text: 'A↵B↵C↵D⇥E  ', needsDialog: true })
  assert.equal(text, 'A\r\nB\rC\nD\tE  ')
  assert.deepEqual(valuePreview(''), { text: '', needsDialog: false })
})
