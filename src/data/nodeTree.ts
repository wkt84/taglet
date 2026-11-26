import type { DicomNode } from '../types/dicom'

const ITEM_SEGMENT = /^Item#(\d+)$/

function samePath(left: string[], right: string[]) {
  return left.length === right.length && left.every((part, index) => part === right[index])
}

/** Set the item segment at `depth` of every path below these nodes. */
function renumberItem(nodes: DicomNode[], depth: number, segment: string): DicomNode[] {
  return nodes.map((node) => {
    const path = [...node.path]
    path[depth] = segment
    if (node.kind === 'Element') return { ...node, path }
    return {
      ...node,
      path,
      items: node.items.map((item) => renumberItem(item, depth, segment)),
    }
  })
}

/**
 * Remove the node (element or sequence) or the sequence item at `path`.
 *
 * Removing an item moves later items up: their paths follow their new positions, as
 * everywhere in the editor, and `item_sources` keeps which of the file's items each
 * one shows, so that saving removes the right item from the file.
 */
export function deleteNodeAtPath(nodes: DicomNode[], path: string[]): DicomNode[] {
  const last = path[path.length - 1]
  const itemMatch = last ? ITEM_SEGMENT.exec(last) : null
  if (!itemMatch) {
    return nodes
      .filter((node) => !samePath(node.path, path))
      .map((node) => (
        node.kind === 'Element'
          ? node
          : { ...node, items: node.items.map((item) => deleteNodeAtPath(item, path)) }
      ))
  }

  const sequencePath = path.slice(0, -1)
  const removed = Number(itemMatch[1])
  return nodes.map((node) => {
    if (node.kind === 'Element') return node
    if (!samePath(node.path, sequencePath)) {
      return { ...node, items: node.items.map((item) => deleteNodeAtPath(item, path)) }
    }
    if (removed >= node.items.length) return node

    const sources = node.item_sources ?? node.items.map((_, index) => index)
    const depth = node.path.length
    return {
      ...node,
      items: node.items
        .filter((_, index) => index !== removed)
        .map((item, index) => (index < removed ? item : renumberItem(item, depth, `Item#${index}`))),
      item_sources: sources.filter((_, index) => index !== removed),
    }
  })
}
