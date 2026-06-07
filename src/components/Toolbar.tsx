import type { DicomNode } from '../types/dicom'

type Props = {
  title: string
  filePath?: string
  nodes: DicomNode[]
  loading: boolean
  dirty: boolean
  openFile: () => Promise<void>
  closeFile: () => boolean
  openAddTagDialog: () => void
  openImageViewer: () => void
  saveFile: () => Promise<void>
  saveFileAs: () => Promise<void>
}

export default function Toolbar({
  title,
  filePath,
  loading,
  dirty,
  openFile,
  closeFile,
  openAddTagDialog,
  openImageViewer,
  saveFile,
  saveFileAs,
}: Props) {
  return (
    <header className="flex min-h-16 items-center gap-4 border-b border-slate-950 bg-gradient-to-r from-slate-950 via-slate-900 to-slate-800 px-4 text-white shadow-sm">
      <div className="min-w-0 flex-1">
        <h1 className="truncate text-base font-semibold tracking-wide">
          {title}
          {dirty ? <span className="ml-2 text-amber-300">*</span> : null}
        </h1>
        <div className="mt-0.5 truncate text-xs text-slate-400">
          {filePath ?? 'No file open'}
        </div>
      </div>

      <nav className="flex items-center gap-2">
        <button className="toolbar-button toolbar-button-primary" disabled={loading} onClick={openFile}>
          Open
        </button>
        <button className="toolbar-button" disabled={!filePath || loading} onClick={openAddTagDialog}>
          Add Tag
        </button>
        <button className="toolbar-button" disabled={!filePath || loading} onClick={openImageViewer}>
          Image Viewer
        </button>
      </nav>

      <div className="h-8 w-px bg-white/10" />

      <nav className="flex items-center gap-2">
        <button className="toolbar-button" disabled={!filePath || loading} onClick={saveFile}>
          Save
        </button>
        <button className="toolbar-button" disabled={!filePath || loading} onClick={saveFileAs}>
          Save As
        </button>
        <button className="toolbar-button toolbar-button-danger" disabled={!filePath || loading} onClick={closeFile}>
          Close
        </button>
      </nav>
    </header>
  )
}
