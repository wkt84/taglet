import { useCallback, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import type { DicomElement, DicomNode } from '../types/dicom'

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

function tagExists(nodes: DicomNode[], tag: string) {
  return nodes.some((node) => node.tag === tag)
}

function sortByTag(nodes: DicomNode[]) {
  return [...nodes].sort((left, right) => left.tag.localeCompare(right.tag))
}

function isItemPath(path: string[], target: string[]) {
  return samePath(path, target)
}

function insertNode(
  nodes: DicomNode[],
  parentPath: string[],
  node: DicomElement,
): { nodes: DicomNode[]; added: boolean; duplicate: boolean } {
  if (parentPath.length === 0) {
    if (tagExists(nodes, node.tag)) {
      return { nodes, added: false, duplicate: true }
    }
    return {
      nodes: sortByTag([...nodes, { ...node, path: [node.tag] }]),
      added: true,
      duplicate: false,
    }
  }

  let added = false
  let duplicate = false
  const nextNodes = nodes.map((current) => {
    if (current.kind === 'Element') return current

    return {
      ...current,
      items: current.items.map((item, index) => {
        if (added || duplicate) return item

        const itemPath = [...current.path, `Item#${index}`]
        if (isItemPath(itemPath, parentPath)) {
          if (tagExists(item, node.tag)) {
            duplicate = true
            return item
          }

          added = true
          return sortByTag([...item, { ...node, path: [...parentPath, node.tag] }])
        }

        const nested = insertNode(item, parentPath, node)
        added = nested.added
        duplicate = nested.duplicate
        return nested.nodes
      }),
    }
  })

  return { nodes: nextNodes, added, duplicate }
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

  const addTag = useCallback((parentPath: string[], node: DicomElement) => {
    setError(undefined)
    const result = insertNode(nodes, parentPath, node)

    if (result.duplicate) {
      setError(`${node.tag} already exists in the selected target.`)
      return false
    }
    if (!result.added) {
      setError('Could not find the selected Sequence item. Adding tags to this target is not available.')
      return false
    }

    setNodes(result.nodes)
    setDirty(true)
    return true
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
      addTag,
      deleteNodeByPath,
    }),
    [
      addTag,
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
