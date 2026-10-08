import assert from 'node:assert/strict'
import test from 'node:test'
import { valueMatrix } from '../src/data/valueFormats.ts'

test('table groups values in order, preserves empty components and pads only missing cells', () => {
  assert.deepEqual(valueMatrix('A\\\\C\\D\\', 3), [['A', '', 'C'], ['D', '', null]])
  assert.deepEqual(valueMatrix('A\\\\C\\D\\', 3, true), [['A', 'D'], ['', ''], ['C', null]])
})

test('MLC banks can be displayed as two rows then transposed into paired leaves', () => {
  assert.deepEqual(valueMatrix('-10\\-20\\-30\\10\\20\\30', 3, true), [['-10', '10'], ['-20', '20'], ['-30', '30']])
  assert.deepEqual(valueMatrix('1\\2', 0), [['1'], ['2']])
  assert.deepEqual(valueMatrix('1\\2\\3', 2.9), [['1', '2'], ['3', null]])
})
