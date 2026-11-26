import type { DicomElement, DicomFileContent, DicomNode, DicomSequence } from '../types/dicom'

// The form in which `open_dicom_file` sends a file's nodes (#20), described in
// src-tauri/src/dicom/wire.rs: arrays instead of objects, tags, VRs and descriptions
// sent once, and paths and preview keys left out where they can be rebuilt. The nodes
// rebuilt here must be exactly those of the backend (tests/openedFile.test.mjs checks
// them against tests/fixtures/open-wire.json, which the backend writes).

/** A tag, VR (empty for a sequence) and description. */
export type WireEntry = [tag: string, vr: string, description: string]

type WireExtra = {
  path?: string[]
  inferred_vr?: string
  full_value_key?: string
  item_sources?: (number | null)[]
}

type WireElement = [kind: 0, entry: number, value: string, length: number, flags: number, extra?: WireExtra]
type WireSequence = [kind: 1, entry: number, length: number, items: WireNode[][], extra?: WireExtra]
export type WireNode = WireElement | WireSequence

export type WireFileContent = {
  entries: WireEntry[]
  file_meta: WireNode[]
  nodes: WireNode[]
  warnings: string[]
  open_ms?: number
}

const EDITABLE = 1
const PREVIEW = 2
/** The first part of the paths of the file meta information's elements. */
const FILE_META_PATH = 'FileMetaInformation'

export function decodeOpenedFile(wire: WireFileContent): DicomFileContent {
  const content: DicomFileContent = {
    file_meta: decodeNodes(wire.file_meta, wire.entries, [FILE_META_PATH]),
    nodes: decodeNodes(wire.nodes, wire.entries, []),
    warnings: wire.warnings,
  }
  if (wire.open_ms !== undefined) content.open_ms = wire.open_ms
  return content
}

/** The nodes of a list whose paths are `parent` and their tag, unless sent. */
function decodeNodes(nodes: WireNode[], entries: WireEntry[], parent: string[]): DicomNode[] {
  // Plain loops: files can have hundreds of thousands of nodes.
  const decoded: DicomNode[] = new Array(nodes.length)
  for (let index = 0; index < nodes.length; index++) {
    const node = nodes[index]
    const entry = entries[node[1]]
    const tag = entry[0]
    if (node[0] === 0) {
      const extra = node[5]
      const path = extra?.path ?? withPart(parent, tag)
      const flags = node[4]
      const element: DicomElement = {
        kind: 'Element', tag, vr: entry[1], description: entry[2], value: node[2], length: node[3], path,
        editable: (flags & EDITABLE) !== 0,
      }
      if (extra?.inferred_vr !== undefined) element.inferred_vr = extra.inferred_vr
      if (flags & PREVIEW) element.full_value_key = path.join('/')
      else if (extra?.full_value_key !== undefined) element.full_value_key = extra.full_value_key
      decoded[index] = element
    } else {
      const extra = node[4]
      const path = extra?.path ?? withPart(parent, tag)
      const items = node[3]
      const decodedItems: DicomNode[][] = new Array(items.length)
      for (let item = 0; item < items.length; item++) {
        decodedItems[item] = decodeNodes(items[item], entries, withPart(path, `Item#${item}`))
      }
      const sequence: DicomSequence = { kind: 'Sequence', tag, description: entry[2], length: node[2], path, items: decodedItems }
      if (extra?.item_sources !== undefined) sequence.item_sources = extra.item_sources
      decoded[index] = sequence
    }
  }
  return decoded
}

function withPart(path: string[], part: string) {
  const next = path.slice()
  next.push(part)
  return next
}
