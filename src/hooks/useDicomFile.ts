import { useCallback, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import type { DicomNode } from '../types/dicom'

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

export function useDicomFile() {
  const [filePath, setFilePath] = useState<string>()
  const [nodes, setNodes] = useState<DicomNode[]>([])
  const [loading, setLoading] = useState(false)
  const [dirty, setDirty] = useState(false)
  const [error, setError] = useState<string>()

  const openFile = useCallback(async () => {
    setError(undefined)
    const selected = await open({
      multiple: false,
      filters: [{ name: 'DICOM', extensions: ['dcm', 'dicom', '*'] }],
    })

    if (typeof selected !== 'string') return

    setLoading(true)
    try {
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
    const destination = await save({
      defaultPath: filePath,
      filters: [{ name: 'DICOM', extensions: ['dcm', 'dicom'] }],
    })
    if (!destination) return

    setLoading(true)
    try {
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
    }),
    [dirty, error, filePath, loading, nodes, openFile, saveFile, saveFileAs, updateNodeValue],
  )
}
