import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import type { DicomElement, DicomFileContent, DicomNode } from '../types/dicom'

export type DicomDocument = {
  id: string
  filePath: string
  fileMeta: DicomNode[]
  nodes: DicomNode[]
  dirty: boolean
  selectedPath?: string[]
}

const SOP_CLASS_UID = '(0008,0016)'
const SOP_INSTANCE_UID = '(0008,0018)'
const FILE_META_INFORMATION_GROUP_LENGTH = '(0002,0000)'
const MEDIA_STORAGE_SOP_CLASS_UID = '(0002,0002)'
const MEDIA_STORAGE_SOP_INSTANCE_UID = '(0002,0003)'
const LONG_VR_HEADER_BYTES = new Set(['OB', 'OD', 'OF', 'OL', 'OV', 'OW', 'SQ', 'SV', 'UC', 'UN', 'UR', 'UT', 'UV'])

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

function rootElementValue(nodes: DicomNode[], tag: string) {
  const node = nodes.find((current): current is DicomElement => (
    current.kind === 'Element' && current.tag === tag
  ))
  return node?.value
}

function dicomUiLength(value: string) {
  return value.length + (value.length % 2)
}

function fileMetaGroupLength(fileMeta: DicomNode[]) {
  return fileMeta.reduce((total, node) => {
    if (node.kind !== 'Element' || node.tag === FILE_META_INFORMATION_GROUP_LENGTH) return total

    const headerLength = LONG_VR_HEADER_BYTES.has(node.vr) ? 12 : 8
    return total + headerLength + node.length
  }, 0)
}

function syncFileMetaFromNodes(fileMeta: DicomNode[], nodes: DicomNode[]) {
  const sopClassUid = rootElementValue(nodes, SOP_CLASS_UID)
  const sopInstanceUid = rootElementValue(nodes, SOP_INSTANCE_UID)

  const updated = fileMeta.map((node) => {
    if (node.kind === 'Sequence') return node
    if (node.tag === MEDIA_STORAGE_SOP_CLASS_UID && sopClassUid !== undefined) {
      return { ...node, value: sopClassUid, length: dicomUiLength(sopClassUid) }
    }
    if (node.tag === MEDIA_STORAGE_SOP_INSTANCE_UID && sopInstanceUid !== undefined) {
      return { ...node, value: sopInstanceUid, length: dicomUiLength(sopInstanceUid) }
    }
    return node
  })

  const groupLength = fileMetaGroupLength(updated)
  return updated.map((node) => (
    node.kind === 'Element' && node.tag === FILE_META_INFORMATION_GROUP_LENGTH
      ? { ...node, value: String(groupLength), length: 4 }
      : node
  ))
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
  const [documents, setDocuments] = useState<DicomDocument[]>([])
  const documentsRef = useRef<DicomDocument[]>([])
  const [activeDocumentId, setActiveDocumentId] = useState<string>()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string>()
  const [warnings, setWarnings] = useState<string[]>([])
  const activeDocument = useMemo(
    () => documents.find((document) => document.id === activeDocumentId),
    [activeDocumentId, documents],
  )
  const filePath = activeDocument?.filePath
  const fileMeta = activeDocument?.fileMeta ?? []
  const nodes = activeDocument?.nodes ?? []
  const dirty = activeDocument?.dirty ?? false

  useEffect(() => {
    documentsRef.current = documents
  }, [documents])

  const updateDocument = useCallback((id: string, update: (document: DicomDocument) => DicomDocument) => {
    setDocuments((current) => {
      const next = current.map((document) => (
        document.id === id ? update(document) : document
      ))
      documentsRef.current = next
      return next
    })
  }, [])

  const setBackendCurrentPath = useCallback(async (path?: string) => {
    await invoke('set_current_dicom_file', { path: path ?? null })
  }, [])

  const selectDocument = useCallback(async (id: string) => {
    const document = documentsRef.current.find((current) => current.id === id)
    if (!document) return false

    setError(undefined)
    try {
      await setBackendCurrentPath(document.filePath)
      setActiveDocumentId(id)
      return true
    } catch (error) {
      setError(String(error))
      return false
    }
  }, [setBackendCurrentPath])

  const openPath = useCallback(async (path: string) => {
    setError(undefined)
    setLoading(true)
    try {
      const existing = documentsRef.current.find((document) => document.filePath === path)
      if (existing) {
        await setBackendCurrentPath(existing.filePath)
        setActiveDocumentId(existing.id)
        return true
      }

      const loaded = await invoke<DicomFileContent>('open_dicom_file', { path })
      const id = crypto.randomUUID()
      setDocuments((current) => {
        const next = [
          ...current,
          { id, filePath: path, fileMeta: loaded.file_meta, nodes: loaded.nodes, dirty: false },
        ]
        documentsRef.current = next
        return next
      })
      setActiveDocumentId(id)
      if (loaded.warnings.length > 0) {
        setWarnings((current) => [...current, ...loaded.warnings])
      }
      return true
    } catch (error) {
      setError(String(error))
      return false
    } finally {
      setLoading(false)
    }
  }, [setBackendCurrentPath])

  const openPaths = useCallback(async (paths: string[]) => {
    let opened = false
    for (const path of paths) {
      if (await openPath(path)) opened = true
    }
    return opened
  }, [openPath])

  const openFile = useCallback(async () => {
    setError(undefined)
    setLoading(true)
    try {
      const selected = await open({
        multiple: true,
        title: 'Open DICOM file',
      })

      const paths = Array.isArray(selected)
        ? selected
        : typeof selected === 'string'
          ? [selected]
          : []
      if (paths.length === 0) return false

      return await openPaths(paths)
    } catch (error) {
      setError(String(error))
      return false
    } finally {
      setLoading(false)
    }
  }, [openPaths])

  const saveFile = useCallback(async () => {
    if (!activeDocument) return
    setError(undefined)
    setLoading(true)
    try {
      await setBackendCurrentPath(activeDocument.filePath)
      await invoke('save_dicom_file', {
        path: activeDocument.filePath,
        nodes: activeDocument.nodes,
      })
      updateDocument(activeDocument.id, (document) => ({ ...document, dirty: false }))
    } catch (error) {
      setError(String(error))
    } finally {
      setLoading(false)
    }
  }, [activeDocument, setBackendCurrentPath, updateDocument])

  const saveFileAs = useCallback(async () => {
    if (!activeDocument) return
    setError(undefined)
    setLoading(true)
    try {
      const destination = await save({
        defaultPath: activeDocument.filePath,
        filters: [{ name: 'DICOM', extensions: ['dcm', 'dicom'] }],
      })
      if (!destination) return

      await setBackendCurrentPath(activeDocument.filePath)
      await invoke('save_dicom_file_as', { path: destination, nodes: activeDocument.nodes })
      updateDocument(activeDocument.id, (document) => ({
        ...document,
        filePath: destination,
        dirty: false,
      }))
    } catch (error) {
      setError(String(error))
    } finally {
      setLoading(false)
    }
  }, [activeDocument, setBackendCurrentPath, updateDocument])

  const closeDocument = useCallback((id = activeDocumentId) => {
    if (!id) return false
    const closingIndex = documents.findIndex((document) => document.id === id)
    const closing = documents[closingIndex]
    if (!closing) return false
    if (closing.dirty && !window.confirm('Discard unsaved changes and close this file?')) return false

    const nextDocuments = documents.filter((document) => document.id !== id)
    const nextActive = id === activeDocumentId
      ? nextDocuments[Math.min(closingIndex, nextDocuments.length - 1)]
      : activeDocument

    documentsRef.current = nextDocuments
    setDocuments(nextDocuments)
    setActiveDocumentId(nextActive?.id)
    setError(undefined)
    void setBackendCurrentPath(nextActive?.filePath)
    return true
  }, [activeDocument, activeDocumentId, documents, setBackendCurrentPath])

  const closeFile = useCallback(() => closeDocument(), [closeDocument])

  const updateNodeValue = useCallback((path: string[], value: string) => {
    if (!activeDocumentId) return
    updateDocument(activeDocumentId, (document) => {
      const nodes = updateNode(document.nodes, path, value)
      return {
        ...document,
        nodes,
        fileMeta: syncFileMetaFromNodes(document.fileMeta, nodes),
        dirty: true,
      }
    })
  }, [activeDocumentId, updateDocument])

  const addTag = useCallback((parentPath: string[], node: DicomElement) => {
    if (!activeDocument) return false
    setError(undefined)
    const result = insertNode(activeDocument.nodes, parentPath, node)

    if (result.duplicate) {
      setError(`${node.tag} already exists in the selected target.`)
      return false
    }
    if (!result.added) {
      setError('Could not find the selected Sequence item. Adding tags to this target is not available.')
      return false
    }

    updateDocument(activeDocument.id, (document) => ({
      ...document,
      nodes: result.nodes,
      fileMeta: syncFileMetaFromNodes(document.fileMeta, result.nodes),
      dirty: true,
    }))
    return true
  }, [activeDocument, updateDocument])

  const deleteNodeByPath = useCallback((path: string[]) => {
    if (!activeDocumentId) return
    updateDocument(activeDocumentId, (document) => {
      const nodes = deleteNode(document.nodes, path)
      return {
        ...document,
        nodes,
        fileMeta: syncFileMetaFromNodes(document.fileMeta, nodes),
        selectedPath: undefined,
        dirty: true,
      }
    })
  }, [activeDocumentId, updateDocument])

  const setSelectedPath = useCallback((path?: string[]) => {
    if (!activeDocumentId) return
    updateDocument(activeDocumentId, (document) => ({ ...document, selectedPath: path }))
  }, [activeDocumentId, updateDocument])

  const dismissWarning = useCallback(() => {
    setWarnings((current) => current.slice(1))
  }, [])

  return useMemo(
    () => ({
      documents,
      activeDocument,
      activeDocumentId,
      filePath,
      fileMeta,
      nodes,
      loading,
      dirty,
      error,
      warning: warnings[0],
      dismissWarning,
      openFile,
      openPath,
      openPaths,
      selectDocument,
      closeDocument,
      closeFile,
      saveFile,
      saveFileAs,
      updateNodeValue,
      addTag,
      deleteNodeByPath,
      selectedPath: activeDocument?.selectedPath,
      setSelectedPath,
    }),
    [
      activeDocument,
      activeDocumentId,
      addTag,
      closeDocument,
      closeFile,
      deleteNodeByPath,
      dismissWarning,
      documents,
      dirty,
      error,
      filePath,
      fileMeta,
      loading,
      nodes,
      openFile,
      openPath,
      openPaths,
      saveFile,
      saveFileAs,
      selectDocument,
      setSelectedPath,
      updateNodeValue,
      warnings,
    ],
  )
}
