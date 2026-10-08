import type { DicomElement, DicomNode, ValidationResult } from '../types/dicom'
import { canBatchEdit } from './privateTags.ts'

export type BatchValueChange = {
  documentId: string
  path: string[]
  previousValue: string
  value: string
}

export type TagOccurrence = { element: DicomElement; location: string }

// Scope paths contain sequence tags only: Item indexes vary between files and beams.
export function sequencePath(path: string[]) {
  return path.filter((part) => !/^Item#\d+$/.test(part))
}

export function ancestorScopes(nodes: DicomNode[], path: string[]) {
  const scopes: Array<{ tags: string[]; label: string }> = []
  function visit(current: DicomNode[], names: string[]) {
    for (const node of current) {
      if (node.kind !== 'Sequence') continue
      if (!node.path.every((part, index) => part === path[index])) continue
      const nextNames = [...names, `${node.description} ${node.tag}`]
      scopes.push({ tags: sequencePath(node.path), label: `${nextNames.join(' / ')} → all Items` })
      node.items.forEach((item) => visit(item, nextNames))
    }
  }
  visit(nodes, [])
  return scopes
}

export function findTagOccurrences(nodes: DicomNode[], tag: string, scope: string[] = []): TagOccurrence[] {
  const matches: TagOccurrence[] = []
  function visit(current: DicomNode[], location: string[]) {
    for (const node of current) {
      if (node.kind === 'Element') {
        const ancestors = sequencePath(node.path.slice(0, -1))
        if (node.tag === tag && scope.every((part, index) => ancestors[index] === part)) {
          matches.push({ element: node, location: location.join(' / ') || 'Root dataset' })
        }
        continue
      }
      node.items.forEach((item, index) => {
        const text = (itemTag: string) => {
          const value = item.find((child) => child.kind === 'Element' && child.tag === itemTag)
          return value?.kind === 'Element' ? value.value : undefined
        }
        const beam = node.tag === '(300A,00B0)'
          ? [text('(300A,00C0)'), text('(300A,00C2)')].filter(Boolean).join(' · ')
          : ''
        visit(item, [...location, `${node.description} ${node.tag}`, `Item #${index + 1}${beam ? ` (Beam ${beam})` : ''}`])
      })
    }
  }
  visit(nodes, [])
  return matches
}

export function occurrenceKey(documentId: string, path: string[]) {
  return JSON.stringify([documentId, path])
}

export async function validateBatchValueChanges(
  documents: Array<{ id: string; nodes: DicomNode[] }>,
  changes: BatchValueChange[],
  validate: (vr: string, value: string) => Promise<ValidationResult>,
) {
  const targets = new Set(changes.map((change) => change.documentId))
  const elements = new Map<string, DicomElement>()
  function index(documentId: string, nodes: DicomNode[]) {
    for (const node of nodes) {
      if (node.kind === 'Sequence') node.items.forEach((item) => index(documentId, item))
      else elements.set(occurrenceKey(documentId, node.path), node)
    }
  }
  documents.filter((document) => targets.has(document.id)).forEach((document) => index(document.id, document.nodes))
  const valuesByVr = new Map<string, Set<string>>()
  for (const change of changes) {
    const element = elements.get(occurrenceKey(change.documentId, change.path))
    if (element && !canBatchEdit(element) && element.editable) {
      throw new Error('Private tags do not support batch editing.')
    }
    if (!element || !canBatchEdit(element) || element.value !== change.previousValue) {
      throw new Error('A target has changed or is no longer editable. Review the preview and try again.')
    }
    const values = valuesByVr.get(element.vr) ?? new Set<string>()
    values.add(change.value)
    valuesByVr.set(element.vr, values)
  }
  for (const [vr, values] of valuesByVr) {
    for (const value of values) {
      const result = await validate(vr, value)
      if (!result.valid) throw new Error(`${vr}: ${result.message ?? 'Invalid value'}`)
    }
  }
}

export function applyBatchValueChanges(nodes: DicomNode[], changes: BatchValueChange[]): DicomNode[] {
  const byPath = new Map(changes.map((change) => [JSON.stringify(change.path), change]))
  function visit(current: DicomNode[]): DicomNode[] {
    return current.map((node) => {
      if (node.kind === 'Sequence') return { ...node, items: node.items.map(visit) }
      const change = byPath.get(JSON.stringify(node.path))
      return change && canBatchEdit(node) && node.value === change.previousValue
        ? { ...node, value: change.value }
        : node
    })
  }
  return visit(nodes)
}
