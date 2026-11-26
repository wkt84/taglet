import assert from 'node:assert/strict'
import test from 'node:test'
import { deleteNodeAtPath } from '../src/data/nodeTree.ts'

const SEQ = '(3006,0020)'
const NESTED = '(3006,0040)'
const element = (tag, value, path) => ({ kind: 'Element', tag, vr: 'LO', description: '', value, path, editable: true, length: value.length })
const sequence = (tag, path, items, extra = {}) => ({ kind: 'Sequence', tag, description: '', length: 0, path, items, ...extra })

function roi(index, name) {
  const item = [SEQ, `Item#${index}`]
  return [
    element('(3006,0026)', name, [...item, '(3006,0026)']),
    sequence(NESTED, [...item, NESTED], [[element('(3006,0050)', `${name} contour`, [...item, NESTED, 'Item#0', '(3006,0050)'])]]),
  ]
}

const nodes = () => [
  element('(0010,0010)', 'Yamada^Tarou', ['(0010,0010)']),
  sequence(SEQ, [SEQ], [roi(0, 'PTV'), roi(1, 'CTV'), roi(2, 'GTV')]),
]

test('removing an item moves later items up and records which file items remain', () => {
  const result = deleteNodeAtPath(nodes(), [SEQ, 'Item#1'])
  const rois = result[1]
  assert.equal(rois.items.length, 2)
  assert.deepEqual(rois.item_sources, [0, 2])
  assert.equal(rois.items[1][0].value, 'GTV')
  assert.deepEqual(rois.items[1][0].path, [SEQ, 'Item#1', '(3006,0026)'])
  assert.deepEqual(rois.items[1][1].path, [SEQ, 'Item#1', NESTED])
  assert.deepEqual(rois.items[1][1].items[0][0].path, [SEQ, 'Item#1', NESTED, 'Item#0', '(3006,0050)'])
  // Earlier items are untouched.
  assert.deepEqual(rois.items[0][0].path, [SEQ, 'Item#0', '(3006,0026)'])
})

test('removing items again keeps the sources of the file items', () => {
  const once = deleteNodeAtPath(nodes(), [SEQ, 'Item#0'])
  const twice = deleteNodeAtPath(once, [SEQ, 'Item#1'])
  assert.deepEqual(twice[1].item_sources, [1])
  assert.equal(twice[1].items[0][0].value, 'CTV')
  assert.deepEqual(twice[1].items[0][0].path, [SEQ, 'Item#0', '(3006,0026)'])
})

test('items in nested sequences can be removed', () => {
  const result = deleteNodeAtPath(nodes(), [SEQ, 'Item#2', NESTED, 'Item#0'])
  const nested = result[1].items[2][1]
  assert.equal(nested.items.length, 0)
  assert.deepEqual(nested.item_sources, [])
  assert.equal(result[1].item_sources, undefined)
})

test('removing elements and sequences still works', () => {
  const withoutName = deleteNodeAtPath(nodes(), ['(0010,0010)'])
  assert.equal(withoutName.length, 1)
  const withoutContours = deleteNodeAtPath(nodes(), [SEQ, 'Item#1', NESTED])
  assert.equal(withoutContours[1].items[1].length, 1)
  assert.equal(withoutContours[1].items[0].length, 2)
})
