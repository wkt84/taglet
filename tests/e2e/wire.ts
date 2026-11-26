import type { DicomFileContent, DicomNode } from '../../src/types/dicom'
import type { WireEntry, WireFileContent, WireNode } from '../../src/data/openedFile'

// The mock backend sends fixtures as the backend does (src-tauri/src/dicom/wire.rs):
// paths and preview keys left out where the page can rebuild them, sent otherwise.

export function encodeOpenedFile(content: DicomFileContent): WireFileContent {
  const entries: WireEntry[] = []
  const index = new Map<string, number>()
  const entryOf = (tag: string, vr: string, description: string) => {
    const key = JSON.stringify([tag, vr, description])
    if (!index.has(key)) {
      index.set(key, entries.length)
      entries.push([tag, vr, description])
    }
    return index.get(key)!
  }
  const sameParts = (a: string[], b: string[]) => a.length === b.length && a.every((part, i) => part === b[i])
  const encode = (nodes: DicomNode[], parent: string[]): WireNode[] => nodes.map((node) => {
    const derived = sameParts(node.path, [...parent, node.tag])
    if (node.kind === 'Element') {
      const derivedKey = node.full_value_key !== undefined && node.full_value_key === node.path.join('/')
      const extra = {
        ...(derived ? {} : { path: node.path }),
        ...(node.inferred_vr === undefined ? {} : { inferred_vr: node.inferred_vr }),
        ...(node.full_value_key === undefined || derivedKey ? {} : { full_value_key: node.full_value_key }),
      }
      const flags = (node.editable ? 1 : 0) | (derivedKey ? 2 : 0)
      const wire: WireNode = [0, entryOf(node.tag, node.vr, node.description), node.value, node.length, flags]
      if (Object.keys(extra).length > 0) wire.push(extra)
      return wire
    }
    const extra = {
      ...(derived ? {} : { path: node.path }),
      ...(node.item_sources === undefined ? {} : { item_sources: node.item_sources }),
    }
    // Before the items' entries, as the backend numbers them.
    const entry = entryOf(node.tag, '', node.description)
    const items = node.items.map((item, i) => encode(item, [...node.path, `Item#${i}`]))
    const wire: WireNode = [1, entry, node.length, items]
    if (Object.keys(extra).length > 0) wire.push(extra)
    return wire
  })
  const wire: WireFileContent = {
    entries,
    file_meta: encode(content.file_meta, ['FileMetaInformation']),
    nodes: encode(content.nodes, []),
    warnings: content.warnings,
  }
  if (content.open_ms !== undefined) wire.open_ms = content.open_ms
  return wire
}
