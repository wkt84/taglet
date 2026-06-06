import type { DicomNode } from '../types/dicom'

type Props = {
  title: string
  filePath?: string
  nodes: DicomNode[]
  loading: boolean
  dirty: boolean
  openFile: () => Promise<void>
  openAddTagDialog: () => void
  saveFile: () => Promise<void>
  saveFileAs: () => Promise<void>
}

export default function Toolbar({
  title,
  filePath,
  loading,
  dirty,
  openFile,
  openAddTagDialog,
  saveFile,
  saveFileAs,
}: Props) {
  return (
    <header className="flex min-h-14 items-center gap-3 border-b border-slate-300 bg-slate-800 px-4 text-white">
      <h1 className="min-w-0 flex-1 truncate text-base font-semibold">
        {title}
        {dirty ? <span className="ml-2 text-amber-300">*</span> : null}
      </h1>
      <button className="toolbar-button" disabled={loading} onClick={openFile}>
        Open
      </button>
      <button className="toolbar-button" disabled={!filePath || loading} onClick={openAddTagDialog}>
        Add Tag
      </button>
      <button className="toolbar-button" disabled={!filePath || loading} onClick={saveFile}>
        Save
      </button>
      <button className="toolbar-button" disabled={!filePath || loading} onClick={saveFileAs}>
        Save As
      </button>
    </header>
  )
}
