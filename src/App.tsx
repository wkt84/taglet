import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import AddTagDialog from './components/AddTagDialog'
import ImageViewer from './components/ImageViewer'
import TagTable from './components/TagTable'
import Toolbar from './components/Toolbar'
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
  return path.length === 0 ? 'Root dataset' : path.join(' / ')
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

export default function App() {
  const dicom = useDicomFile()
  const [addingTag, setAddingTag] = useState(false)
  const [showingImageViewer, setShowingImageViewer] = useState(false)
  const [selectedPath, setSelectedPath] = useState<string[]>()
  const openPath = dicom.openPath
  const addTargetPath = useMemo(() => addTargetPathFromSelection(selectedPath), [selectedPath])
  const existingTagsForTarget = useMemo(
    () => tagsAtPath(dicom.nodes, addTargetPath),
    [addTargetPath, dicom.nodes],
  )
  const title = useMemo(() => {
    const name = fileName(dicom.filePath)
    return name ? `Taglet - ${name}` : 'Taglet'
  }, [dicom.filePath])

  useEffect(() => {
    let canceled = false
    let unlistenDrop: (() => void) | undefined
    let unlistenOpenFiles: (() => void) | undefined

    async function openFirstPath(paths: string[]) {
      const path = paths.find(Boolean)
      if (!path) return
      const opened = await openPath(path)
      if (opened) {
        setSelectedPath(undefined)
        setAddingTag(false)
        setShowingImageViewer(false)
      }
    }

    invoke<string[]>('take_launch_file_paths')
      .then((paths) => {
        if (!canceled) void openFirstPath(paths)
      })
      .catch(() => {})

    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'drop') {
          void openFirstPath(event.payload.paths)
        }
      })
      .then((unlisten) => {
        unlistenDrop = unlisten
        if (canceled) unlisten()
      })
      .catch(() => {})

    listen<string[]>('taglet://open-files', (event) => {
      void openFirstPath(event.payload)
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
  }, [openPath])

  return (
    <main className="flex h-screen flex-col bg-slate-100 text-slate-900">
      <Toolbar
        title={title}
        {...dicom}
        closeFile={() => {
          const closed = dicom.closeFile()
          if (closed) {
            setSelectedPath(undefined)
            setAddingTag(false)
            setShowingImageViewer(false)
          }
          return closed
        }}
        openAddTagDialog={() => setAddingTag(true)}
        openImageViewer={() => setShowingImageViewer(true)}
      />
      {dicom.error ? (
        <div className="border-b border-red-200 bg-red-50 px-4 py-2 text-sm text-red-700">
          {dicom.error}
        </div>
      ) : null}
      <section className="min-h-0 flex-1 overflow-hidden p-4">
        <TagTable
          nodes={dicom.nodes}
          selectedPath={selectedPath}
          onChange={dicom.updateNodeValue}
          onDelete={dicom.deleteNodeByPath}
          onSelect={setSelectedPath}
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
    </main>
  )
}
