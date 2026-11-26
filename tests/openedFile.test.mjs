import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import { decodeOpenedFile } from '../src/data/openedFile.ts'

// The backend writes this fixture (src-tauri/src/dicom/wire.rs) and checks that it is
// up to date: each case has the nodes as the backend sends them, and as JSON objects.
const fixture = JSON.parse(readFileSync(new URL('./fixtures/open-wire.json', import.meta.url), 'utf8'))

test('the fixture has cases', () => {
  assert.ok(fixture.cases.length >= 2)
})

for (const { name, wire, nodes } of fixture.cases) {
  test(`opened files are rebuilt exactly as the backend's nodes: ${name}`, () => {
    // Through JSON, as the nodes are saved: no key holds `undefined`.
    const decoded = decodeOpenedFile(wire)
    assert.deepStrictEqual(JSON.parse(JSON.stringify(decoded)), nodes)
    assert.deepStrictEqual(decoded, nodes)
  })
}

for (const { name, wire, nodes } of fixture.cases) {
  test(`the mock backend of the page tests sends files as the backend does: ${name}`, async () => {
    const { encodeOpenedFile } = await import('./e2e/wire.ts')
    assert.deepStrictEqual(encodeOpenedFile(nodes), wire)
  })
}
