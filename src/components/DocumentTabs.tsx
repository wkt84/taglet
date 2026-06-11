import type { DicomDocument } from '../hooks/useDicomFile'

type Props = {
  documents: DicomDocument[]
  activeDocumentId?: string
  loading: boolean
  onSelect: (id: string) => void
  onClose: (id: string) => void
}

function fileName(path: string) {
  return path.split(/[\\/]/).pop() || path
}

export default function DocumentTabs({
  documents,
  activeDocumentId,
  loading,
  onSelect,
  onClose,
}: Props) {
  if (documents.length === 0) return null

  return (
    <div className="flex min-h-10 items-end gap-1 overflow-x-auto border-b border-slate-300 bg-slate-200 px-3 pt-2">
      {documents.map((document) => {
        const active = document.id === activeDocumentId
        return (
          <div
            key={document.id}
            className={`group flex max-w-64 items-center rounded-t border text-sm ${
              active
                ? 'border-slate-300 border-b-white bg-white text-slate-950'
                : 'border-slate-300 bg-slate-100 text-slate-600 hover:bg-white'
            }`}
            title={document.filePath}
          >
            <button
              className="min-w-0 flex-1 px-3 py-1.5 text-left disabled:cursor-not-allowed"
              disabled={loading}
              onClick={() => onSelect(document.id)}
            >
              <span className="block truncate">
                {fileName(document.filePath)}
                {document.dirty ? <span className="ml-1 text-amber-600">*</span> : null}
              </span>
            </button>
            <button
              className={`mr-1 rounded px-1 text-xs ${
                active ? 'text-slate-500 hover:bg-slate-200' : 'text-slate-400 hover:bg-slate-200'
              }`}
              disabled={loading}
              title="Close tab"
              onClick={() => onClose(document.id)}
            >
              x
            </button>
          </div>
        )
      })}
    </div>
  )
}
