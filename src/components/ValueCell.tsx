import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { DicomElement, ValidationResult } from '../types/dicom'
import { valuePreview } from '../data/valuePreview'
import ValueDialog from './ValueDialog'
import { isPreview } from '../data/fullValues'

type Props = {
  element: DicomElement
  /** The open file the element is from, to fetch its whole value if it is a preview. */
  filePath?: string
  onCommit: (value: string) => void
}

export default function ValueCell({ element, filePath, onCommit }: Props) {
  const [editing, setEditing] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  const preview = useMemo(() => valuePreview(element.value), [element.value])
  const [draft, setDraft] = useState(element.value)
  const [validation, setValidation] = useState<ValidationResult>({ valid: true })

  useEffect(() => {
    setDraft(element.value)
  }, [element.value])

  useEffect(() => {
    if (!editing || !element.editable) return

    let canceled = false
    invoke<ValidationResult>('validate_value', { vr: element.vr, value: draft })
      .then((result) => {
        if (!canceled) setValidation(result)
      })
      .catch((error) => {
        if (!canceled) setValidation({ valid: false, message: String(error) })
      })

    return () => {
      canceled = true
    }
  }, [draft, editing, element.editable, element.vr])

  const hasDisplayOptions = element.value.includes('\\') || element.value.trimStart().startsWith('<')
  // A preview is only the start of the value: it is shown and edited in the dialog,
  // which fetches the whole value.
  if (preview.needsDialog || dialogOpen || hasDisplayOptions || isPreview(element)) {
    return (
      <div className="flex min-w-0 items-center gap-2">
        <span className={`dicom-value-font min-w-0 flex-1 truncate ${element.editable ? '' : 'text-slate-500'}`}>
          {isPreview(element) && !preview.text.endsWith('…') ? `${preview.text}…` : preview.text || '(empty)'}
        </span>
        <button
          className="shrink-0 rounded border border-slate-300 px-2 py-0.5 text-xs text-blue-700 hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-300"
          onClick={() => setDialogOpen(true)}
        >
          View full value
        </button>
        {dialogOpen ? (
          <ValueDialog
            key={element.full_value_key ?? element.path.join('/')}
            element={element}
            filePath={filePath}
            onCommit={onCommit}
            onClose={() => setDialogOpen(false)}
          />
        ) : null}
      </div>
    )
  }

  if (!element.editable) {
    return (
      <span className="dicom-value-font block truncate text-slate-500" title={element.value}>
        {element.value}
      </span>
    )
  }

  if (!editing) {
    return (
      <button
        className="dicom-value-font block w-full truncate rounded px-1 py-0.5 text-left hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-300"
        onClick={() => setEditing(true)}
        title={element.value}
      >
        {element.value || <span className="text-slate-400">(empty)</span>}
      </button>
    )
  }

  function commit() {
    if (!validation.valid) return
    onCommit(draft)
    setEditing(false)
  }

  return (
    <div className="relative">
      <input
        autoFocus
        className={`dicom-value-font w-full rounded border px-2 py-1 text-sm outline-none ${
          validation.valid ? 'border-blue-300 ring-1 ring-blue-200' : 'border-red-500 ring-1 ring-red-300'
        }`}
        value={draft}
        onBlur={commit}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            setDraft(element.value)
            setValidation({ valid: true })
            setEditing(false)
          }
          if (event.key === 'Enter') {
            event.preventDefault()
            commit()
          }
        }}
      />
      {!validation.valid && validation.message ? (
        <div className="absolute left-0 top-full z-20 mt-1 max-w-md rounded bg-red-700 px-2 py-1 text-xs text-white shadow">
          {validation.message}
        </div>
      ) : null}
    </div>
  )
}
