import assert from 'node:assert/strict'
import test from 'node:test'
import { ancestorScopes, findTagOccurrences, applyBatchValueChanges, validateBatchValueChanges, occurrenceKey } from '../src/data/batchEdit.ts'

const BEAMS = '(300A,00B0)'
const NESTED = '(300A,0111)'
const OTHER = '(300A,0070)'
const MACHINE = '(300A,00B2)'
const element = (tag, value, path, editable = true) => ({ kind: 'Element', tag, vr: 'SH', description: 'Test tag', value, path, editable, length: value.length })
const sequence = (tag, path, items) => ({ kind: 'Sequence', tag, description: tag === BEAMS ? 'Beam Sequence' : 'Other Sequence', length: 0, path, items })

function plan(beamCount = 2) {
  const beams = sequence(BEAMS, [BEAMS], Array.from({ length: beamCount }, (_, index) => {
    const prefix = [BEAMS, `Item#${index}`]
    return [
      element('(300A,00C0)', String(index + 1), [...prefix, '(300A,00C0)']),
      element('(300A,00C2)', `Beam ${index + 1}`, [...prefix, '(300A,00C2)']),
      element(MACHINE, 'OLD', [...prefix, MACHINE]),
      sequence(NESTED, [...prefix, NESTED], [[element(MACHINE, 'NESTED', [...prefix, NESTED, 'Item#0', MACHINE])]]),
    ]
  }))
  return [element(MACHINE, 'ROOT', [MACHINE]), beams, sequence(OTHER, [OTHER], [[element(MACHINE, 'OTHER', [OTHER, 'Item#0', MACHINE])]])]
}

test('dataset scope finds the same tag in every sequence and root', () => {
  assert.equal(findTagOccurrences(plan(), MACHINE).length, 6)
})

test('Beam scope includes every Beam and nested sequences, excluding siblings and root', () => {
  const matches = findTagOccurrences(plan(), MACHINE, [BEAMS])
  assert.equal(matches.length, 4)
  assert.ok(matches.every(({ element }) => element.path[0] === BEAMS))
  assert.match(matches[2].location, /Item #2 \(Beam 2 · Beam 2\)/)
})

test('nested scope matches across all parent Items, including files with different Beam counts', () => {
  assert.equal(findTagOccurrences(plan(1), MACHINE, [BEAMS, NESTED]).length, 1)
  assert.equal(findTagOccurrences(plan(3), MACHINE, [BEAMS, NESTED]).length, 3)
  assert.equal(findTagOccurrences(plan(), MACHINE, [OTHER, NESTED]).length, 0)
})

test('ancestor choices remove Item indexes and retain ordered sequence hierarchy', () => {
  const scopes = ancestorScopes(plan(), [BEAMS, 'Item#1', NESTED, 'Item#0', MACHINE])
  assert.deepEqual(scopes.map((scope) => scope.tags), [[BEAMS], [BEAMS, NESTED]])
  assert.ok(scopes.every((scope) => scope.label.endsWith('→ all Items')))
})

test('only selected paths change, clearing values works and original data stays intact', () => {
  const nodes = plan()
  const target = findTagOccurrences(nodes, MACHINE, [BEAMS])[2].element
  const next = applyBatchValueChanges(nodes, [{ documentId: 'a', path: target.path, previousValue: 'OLD', value: '' }])
  assert.deepEqual(findTagOccurrences(next, MACHINE).map(({ element }) => element.value), ['ROOT', 'OLD', 'NESTED', '', 'NESTED', 'OTHER'])
  assert.equal(target.value, 'OLD')
  assert.equal(next[1].items[1][1].value, 'Beam 2')
})

test('read-only and stale targets remain unchanged and missing tags are never added', () => {
  const nodes = [element(MACHINE, 'OLD', [MACHINE], false), element('(0010,0020)', 'CURRENT', ['(0010,0020)'])]
  const next = applyBatchValueChanges(nodes, [
    { documentId: 'a', path: [MACHINE], previousValue: 'OLD', value: 'NEW' },
    { documentId: 'a', path: ['(0010,0020)'], previousValue: 'STALE', value: 'NEW' },
    { documentId: 'a', path: ['(0010,0010)'], previousValue: '', value: 'NEW' },
  ])
  assert.deepEqual(next, nodes)
})

test('individual exclusions are independent across files with identical paths', () => {
  assert.notEqual(occurrenceKey('file-a', [BEAMS, 'Item#0', MACHINE]), occurrenceKey('file-b', [BEAMS, 'Item#0', MACHINE]))
})

test('validation checks each distinct VR and value across every file before applying', async () => {
  const documents = [{ id: 'a', nodes: plan(1) }, { id: 'b', nodes: plan(3) }]
  documents[1].nodes[1].items[2][2].vr = 'LO'
  const changes = documents.flatMap((document) => findTagOccurrences(document.nodes, MACHINE, [BEAMS])
    .map(({ element }) => ({ documentId: document.id, path: element.path, previousValue: element.value, value: 'NEW' })))
  const calls = []
  await validateBatchValueChanges(documents, changes, async (vr, value) => { calls.push([vr, value]); return { valid: true } })
  assert.deepEqual(calls, [['SH', 'NEW'], ['LO', 'NEW']])
  assert.equal(findTagOccurrences(documents[0].nodes, MACHINE, [BEAMS])[0].element.value, 'OLD')
})

test('failure for a VR in a later file rejects the batch and leaves all inputs unchanged', async () => {
  const documents = [{ id: 'a', nodes: plan(1) }, { id: 'b', nodes: plan(1) }]
  documents[1].nodes[1].items[0][2].vr = 'IS'
  const before = structuredClone(documents)
  const changes = documents.map((document) => ({ documentId: document.id, path: [BEAMS, 'Item#0', MACHINE], previousValue: 'OLD', value: 'NEW' }))
  await assert.rejects(validateBatchValueChanges(documents, changes, async (vr) => ({ valid: vr !== 'IS', message: 'Invalid integer' })), /IS: Invalid integer/)
  assert.deepEqual(documents, before)
})

test('stale, closed, missing and read-only targets fail preflight before value validation', async () => {
  const documents = [{ id: 'a', nodes: plan(1) }]
  documents[0].nodes[0].editable = false
  const cases = [
    { documentId: 'a', path: [BEAMS, 'Item#0', MACHINE], previousValue: 'STALE', value: 'NEW' },
    { documentId: 'closed', path: [MACHINE], previousValue: 'OLD', value: 'NEW' },
    { documentId: 'a', path: ['(0010,0010)'], previousValue: '', value: 'NEW' },
    { documentId: 'a', path: [MACHINE], previousValue: 'ROOT', value: 'NEW' },
  ]
  for (const change of cases) {
    let called = false
    await assert.rejects(validateBatchValueChanges(documents, [change], async () => { called = true; return { valid: true } }), /no longer editable/)
    assert.equal(called, false)
  }
})
