import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getVersion } from '@tauri-apps/api/app'
import AddTagDialog from './components/AddTagDialog'
import AboutDialog from './components/AboutDialog'
import BevViewer from './components/BevViewer'
import ConfirmDeleteDialog from './components/ConfirmDeleteDialog'
import DocumentTabs from './components/DocumentTabs'
import ImageViewer from './components/ImageViewer'
import RtStructViewer from './components/RtStructViewer'
import TagTable from './components/TagTable'
import Toolbar from './components/Toolbar'
import UpdateChecker from './components/UpdateChecker'
import { useDicomFile } from './hooks/useDicomFile'
import type { DicomNode } from './types/dicom'

function fileName(path?: string) {
  if (!path) return undefined
  return path.split(/[\\/]/).pop()
}

function samePath(left: string[], right: string[]) {
  return left.length === right.length && left.every((part, index) => part === right[index])
}

function addTargetPathFromSelection(path?: string[]) {
  if (!path) return []
  let itemIndex = -1
  for (let index = path.length - 1; index >= 0; index -= 1) {
    if (path[index].startsWith('Item#')) {
      itemIndex = index
      break
    }
  }
  return itemIndex === -1 ? [] : path.slice(0, itemIndex + 1)
}

function targetLabel(path: string[]) {
  if (path.length === 0) return 'Root dataset'

  return path
    .map((part) => {
      const itemMatch = /^Item#(\d+)$/.exec(part)
      return itemMatch ? `Item #${Number(itemMatch[1]) + 1}` : part
    })
    .join(' / ')
}

function tagsAtPath(nodes: DicomNode[], parentPath: string[]): string[] {
  if (parentPath.length === 0) return nodes.map((node) => node.tag)

  for (const node of nodes) {
    if (node.kind === 'Element') continue

    for (const [index, item] of node.items.entries()) {
      const itemPath = [...node.path, `Item#${index}`]
      if (samePath(itemPath, parentPath)) return item.map((child) => child.tag)

      const nested = tagsAtPath(item, parentPath)
      if (nested.length > 0) return nested
    }
  }

  return []
}

function findNodeByPath(nodes: DicomNode[], path?: string[]): DicomNode | { kind: 'Item'; path: string[] } | undefined {
  if (!path) return undefined

  for (const node of nodes) {
    if (samePath(node.path, path)) return node
    if (node.kind === 'Element') continue

    for (const [index, item] of node.items.entries()) {
      const itemPath = [...node.path, `Item#${index}`]
      if (samePath(itemPath, path)) return { kind: 'Item', path: itemPath }
      const nested = findNodeByPath(item, path)
      if (nested) return nested
    }
  }

  return undefined
}

function nodeDeleteLabel(node: DicomNode | { kind: 'Item'; path: string[] } | undefined) {
  if (!node || node.kind === 'Item') return undefined
  return node.kind === 'Sequence' ? `${node.tag} ${node.description}` : `${node.tag} ${node.description}`
}

type PendingDelete = {
  path: string[]
  label: string
}

export default function App() {
  const dicom = useDicomFile()
  const [addingTag, setAddingTag] = useState(false)
  const [showingImageViewer, setShowingImageViewer] = useState(false)
  const [showingBevViewer, setShowingBevViewer] = useState(false)
  const [showingRtStructViewer, setShowingRtStructViewer] = useState(false)
  const [showingAbout, setShowingAbout] = useState(false)
  const [pendingDelete, setPendingDelete] = useState<PendingDelete>()
  const [appVersion, setAppVersion] = useState<string>()
  const openPaths = dicom.openPaths
  const selectedPath = dicom.selectedPath
  const addTargetPath = useMemo(() => addTargetPathFromSelection(selectedPath), [selectedPath])
  const existingTagsForTarget = useMemo(
    () => tagsAtPath(dicom.nodes, addTargetPath),
    [addTargetPath, dicom.nodes],
  )
  const selectedNode = useMemo(
    () => findNodeByPath(dicom.nodes, selectedPath),
    [dicom.nodes, selectedPath],
  )
  const canDeleteSelected = Boolean(
    selectedPath
      && selectedNode
      && selectedNode.kind !== 'Item'
      && selectedNode.tag !== '(7FE0,0010)',
  )
  const title = useMemo(() => {
    const name = fileName(dicom.filePath)
    return name ? `Taglet - ${name}` : 'Taglet'
  }, [dicom.filePath])

  useEffect(() => {
    getVersion()
      .then(setAppVersion)
      .catch(() => {})
  }, [])

  useEffect(() => {
    let canceled = false
    let unlistenDrop: (() => void) | undefined
    let unlistenOpenFiles: (() => void) | undefined

    async function openDroppedPaths(paths: string[]) {
      const opened = await openPaths(paths.filter(Boolean))
      if (opened) {
        setAddingTag(false)
        setShowingImageViewer(false)
        setShowingBevViewer(false)
        setShowingRtStructViewer(false)
      }
    }

    invoke<string[]>('take_launch_file_paths')
      .then((paths) => {
        if (!canceled) void openDroppedPaths(paths)
      })
      .catch(() => {})

    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'drop') {
          void openDroppedPaths(event.payload.paths)
        }
      })
      .then((unlisten) => {
        unlistenDrop = unlisten
        if (canceled) unlisten()
      })
      .catch(() => {})

    listen<string[]>('taglet://open-files', (event) => {
      void openDroppedPaths(event.payload)
    })
      .then((unlisten) => {
        unlistenOpenFiles = unlisten
        if (canceled) unlisten()
      })
      .catch(() => {})

    return () => {
      canceled = true
      unlistenDrop?.()
      unlistenOpenFiles?.()
    }
  }, [openPaths])

  return (
    <main className="flex h-screen flex-col bg-slate-100 text-slate-900">
      <UpdateChecker />
      <Toolbar
        title={title}
        {...dicom}
        closeFile={() => {
          const closed = dicom.closeFile()
          if (closed) {
            setAddingTag(false)
            setShowingImageViewer(false)
            setShowingBevViewer(false)
            setShowingRtStructViewer(false)
          }
          return closed
        }}
        openAddTagDialog={() => setAddingTag(true)}
        deleteSelectedTag={() => {
          if (!selectedPath || !canDeleteSelected) return
          const label = nodeDeleteLabel(selectedNode)
          setPendingDelete({
            path: selectedPath,
            label: label ?? 'selected tag',
          })
        }}
        canDeleteSelectedTag={canDeleteSelected}
        openImageViewer={() => setShowingImageViewer(true)}
        openBevViewer={() => setShowingBevViewer(true)}
        openRtStructViewer={() => setShowingRtStructViewer(true)}
        openAboutDialog={() => setShowingAbout(true)}
      />
      <DocumentTabs
        documents={dicom.documents}
        activeDocumentId={dicom.activeDocumentId}
        loading={dicom.loading}
        onSelect={(id) => {
          void dicom.selectDocument(id).then((selected) => {
            if (selected) {
              setAddingTag(false)
              setShowingImageViewer(false)
              setShowingBevViewer(false)
              setShowingRtStructViewer(false)
            }
          })
        }}
        onClose={(id) => {
          const closed = dicom.closeDocument(id)
          if (closed) {
            setAddingTag(false)
            setShowingImageViewer(false)
            setShowingBevViewer(false)
            setShowingRtStructViewer(false)
          }
        }}
      />
      {dicom.error ? (
        <div className="border-b border-red-200 bg-red-50 px-4 py-2 text-sm text-red-700">
          {dicom.error}
        </div>
      ) : null}
      <section className="min-h-0 flex-1 overflow-hidden p-4">
        <TagTable
          nodes={dicom.nodes}
          filePath={dicom.filePath}
          selectedPath={selectedPath}
          onChange={dicom.updateNodeValue}
          onSelect={dicom.setSelectedPath}
        />
      </section>
      {addingTag ? (
        <AddTagDialog
          existingTags={existingTagsForTarget}
          targetLabel={targetLabel(addTargetPath)}
          onClose={() => setAddingTag(false)}
          onAdd={(node) => {
            if (dicom.addTag(addTargetPath, node)) {
              setAddingTag(false)
            }
          }}
        />
      ) : null}
      {showingImageViewer ? <ImageViewer onClose={() => setShowingImageViewer(false)} /> : null}
      {showingBevViewer ? <BevViewer onClose={() => setShowingBevViewer(false)} /> : null}
      {showingRtStructViewer ? <RtStructViewer onClose={() => setShowingRtStructViewer(false)} /> : null}
      {showingAbout ? <AboutDialog version={appVersion} onClose={() => setShowingAbout(false)} /> : null}
      {pendingDelete ? (
        <ConfirmDeleteDialog
          label={pendingDelete.label}
          onCancel={() => setPendingDelete(undefined)}
          onConfirm={() => {
            dicom.deleteNodeByPath(pendingDelete.path)
            setPendingDelete(undefined)
          }}
        />
      ) : null}
    </main>
  )
}
