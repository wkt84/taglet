import { useCallback, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import type { DicomElement, DicomNode } from '../types/dicom'

const EDITABLE_TEXT_VRS = new Set([
  'AE',
  'AS',
  'CS',
  'DA',
  'DS',
  'DT',
  'IS',
  'LO',
  'LT',
  'PN',
  'SH',
  'ST',
  'TM',
  'UC',
  'UI',
  'UR',
  'UT',
])

function updateNode(nodes: DicomNode[], path: string[], nextValue: string): DicomNode[] {
  return nodes.map((node) => {
    if (node.kind === 'Element') {
      return samePath(node.path, path) ? { ...node, value: nextValue } : node
    }

    return {
      ...node,
      items: node.items.map((item) => updateNode(item, path, nextValue)),
    }
  })
}

function samePath(left: string[], right: string[]) {
  return left.length === right.length && left.every((part, index) => part === right[index])
}

function deleteNode(nodes: DicomNode[], path: string[]): DicomNode[] {
  return nodes
    .filter((node) => !samePath(node.path, path))
    .map((node) => {
      if (node.kind === 'Element') return node

      return {
        ...node,
        items: node.items.map((item) => deleteNode(item, path)),
      }
    })
}

function normalizeTag(input: string) {
  const compact = input.trim().replace(/[()]/g, '').replace(/\s/g, '').toUpperCase()
  const match = compact.match(/^([0-9A-F]{4}),?([0-9A-F]{4})$/)
  return match ? `(${match[1]},${match[2]})` : undefined
}

function tagExists(nodes: DicomNode[], tag: string) {
  return nodes.some((node) => node.tag === tag)
}

function sortByTag(nodes: DicomNode[]) {
  return [...nodes].sort((left, right) => left.tag.localeCompare(right.tag))
}

function isPrivateTag(tag: string) {
  const group = Number.parseInt(tag.slice(1, 5), 16)
  return Number.isFinite(group) && group % 2 === 1
}

export function useDicomFile() {
  const [filePath, setFilePath] = useState<string>()
  const [nodes, setNodes] = useState<DicomNode[]>([])
  const [loading, setLoading] = useState(false)
  const [dirty, setDirty] = useState(false)
  const [error, setError] = useState<string>()

  const openFile = useCallback(async () => {
    setError(undefined)
    setLoading(true)
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: 'DICOM', extensions: ['dcm', 'dicom'] }],
      })

      if (typeof selected !== 'string') return

      const loaded = await invoke<DicomNode[]>('open_dicom_file', { path: selected })
      setFilePath(selected)
      setNodes(loaded)
      setDirty(false)
    } catch (error) {
      setError(String(error))
    } finally {
      setLoading(false)
    }
  }, [])

  const saveFile = useCallback(async () => {
    if (!filePath) return
    setError(undefined)
    setLoading(true)
    try {
      await invoke('save_dicom_file', { path: filePath, nodes })
      setDirty(false)
    } catch (error) {
      setError(String(error))
    } finally {
      setLoading(false)
    }
  }, [filePath, nodes])

  const saveFileAs = useCallback(async () => {
    if (!filePath) return
    setError(undefined)
    setLoading(true)
    try {
      const destination = await save({
        defaultPath: filePath,
        filters: [{ name: 'DICOM', extensions: ['dcm', 'dicom'] }],
      })
      if (!destination) return

      await invoke('save_dicom_file_as', { path: destination, nodes })
      setFilePath(destination)
      setDirty(false)
    } catch (error) {
      setError(String(error))
    } finally {
      setLoading(false)
    }
  }, [filePath, nodes])

  const updateNodeValue = useCallback((path: string[], value: string) => {
    setNodes((current) => updateNode(current, path, value))
    setDirty(true)
  }, [])

  const addRootTag = useCallback(() => {
    setError(undefined)
    const tag = normalizeTag(window.prompt('Tag, e.g. (0010,0010)', '') ?? '')
    if (!tag) {
      setError('Invalid tag format. Use (0010,0010).')
      return
    }
    if (tagExists(nodes, tag)) {
      setError(`${tag} already exists at the root level.`)
      return
    }

    const vr = (window.prompt('VR, e.g. PN, LO, DS', 'LO') ?? '').trim().toUpperCase()
    if (!EDITABLE_TEXT_VRS.has(vr)) {
      setError(`${vr || '(empty)'} is not supported for Add Tag yet.`)
      return
    }

    const value = window.prompt('Initial value', '') ?? ''
    const node: DicomElement = {
      kind: 'Element',
      tag,
      vr,
      description: isPrivateTag(tag) ? '[Private]' : '[New]',
      value,
      length: value.length,
      path: [tag],
      editable: true,
    }

    setNodes((current) => sortByTag([...current, node]))
    setDirty(true)
  }, [nodes])

  const deleteNodeByPath = useCallback((path: string[]) => {
    setNodes((current) => deleteNode(current, path))
    setDirty(true)
  }, [])

  return useMemo(
    () => ({
      filePath,
      nodes,
      loading,
      dirty,
      error,
      openFile,
      saveFile,
      saveFileAs,
      updateNodeValue,
      addRootTag,
      deleteNodeByPath,
    }),
    [
      addRootTag,
      deleteNodeByPath,
      dirty,
      error,
      filePath,
      loading,
      nodes,
      openFile,
      saveFile,
      saveFileAs,
      updateNodeValue,
    ],
  )
}
