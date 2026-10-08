import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { invoke } from '@tauri-apps/api/core'
import type { DicomElement, ValidationResult } from '../types/dicom'
import { formattedXml, valueMatrix } from '../data/valueFormats'

type Props = {
  element: DicomElement
  onCommit: (value: string) => void
  onClose: () => void
}

export default function ValueDialog({ element, onCommit, onClose }: Props) {
  const titleId = useId()
  const dialogRef = useRef<HTMLDialogElement>(null)
  const textRef = useRef<HTMLTextAreaElement>(null)
  const activeRef = useRef(false)
  const applyingRef = useRef(false)
  const [draft, setDraft] = useState(element.value)
  const [wrap, setWrap] = useState(true)
  const [applying, setApplying] = useState(false)
  const [validation, setValidation] = useState<ValidationResult>({ valid: true })
  const [copyStatus, setCopyStatus] = useState('')
  const [mode, setMode] = useState<'raw' | 'table' | 'xml'>('raw')
  const [columns, setColumns] = useState('2')
  const [transpose, setTranspose] = useState(false)
  const xml = useMemo(() => formattedXml(draft), [draft])
  const matrix = useMemo(() => mode === 'table' ? valueMatrix(draft, Number(columns), transpose) : [], [draft, columns, transpose, mode])

  useEffect(() => {
    if (mode === 'xml' && xml === null) setMode('raw')
  }, [mode, xml])

  useEffect(() => {
    activeRef.current = true
    const dialog = dialogRef.current!
    const previousFocus = document.activeElement as HTMLElement | null
    dialog.showModal()
    textRef.current?.focus()
    return () => {
      activeRef.current = false
      dialog.close()
      if (previousFocus?.isConnected) previousFocus.focus()
    }
  }, [])

  async function apply() {
    if (!element.editable || applyingRef.current) return
    applyingRef.current = true
    setApplying(true)
    try {
      const result = await invoke<ValidationResult>('validate_value', { vr: element.vr, value: draft })
      if (!activeRef.current) return
      setValidation(result)
      if (result.valid) {
        if (draft !== element.value) onCommit(draft)
        onClose()
      }
    } catch (error) {
      if (activeRef.current) setValidation({ valid: false, message: String(error) })
    } finally {
      applyingRef.current = false
      if (activeRef.current) setApplying(false)
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(draft)
      if (activeRef.current) setCopyStatus('Copied')
    } catch {
      // Some desktop WebViews do not expose the Clipboard API. Copy the actual
      // textarea contents without replacing line breaks or trimming whitespace.
      if (!activeRef.current) return
      const text = document.createElement('textarea')
      text.className = 'sr-only'
      text.value = draft
      text.readOnly = true
      dialogRef.current!.appendChild(text)
      const previousFocus = document.activeElement as HTMLElement | null
      const copyExactText = (event: ClipboardEvent) => {
        if (event.clipboardData) {
          event.clipboardData.setData('text/plain', draft)
          event.preventDefault()
        }
      }
      document.addEventListener('copy', copyExactText)
      let copied = false
      try {
        text.focus()
        text.select()
        copied = document.execCommand('copy')
      } catch {
        copied = false
      } finally {
        document.removeEventListener('copy', copyExactText)
        text.remove()
        previousFocus?.focus()
      }
      setCopyStatus(copied ? 'Copied' : 'Copy failed. Select the text and copy it manually.')
    }
  }

  return createPortal(
    <dialog
      ref={dialogRef}
      aria-labelledby={titleId}
      className="m-auto w-[min(56rem,calc(100vw-2rem))] max-w-none rounded border border-slate-200 bg-white p-0 text-slate-900 shadow-xl backdrop:bg-slate-950/40"
      onCancel={(event) => { event.preventDefault(); onClose() }}
      onClick={(event) => event.stopPropagation()}
      onContextMenu={(event) => event.stopPropagation()}
    >
      <div className="flex max-h-[85vh] flex-col">
        <div className="border-b border-slate-200 px-4 py-3">
          <h2 id={titleId} className="font-semibold">Tag value</h2>
          <p className="mt-1 text-sm text-slate-600">
            <span className="font-mono">{element.tag}</span> · {element.description} · VR: {element.vr}
            {element.inferred_vr ? ` (displayed as ${element.inferred_vr})` : ''}
          </p>
        </div>
        <div className="flex min-h-0 flex-1 flex-col gap-3 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
            <label className="flex items-center gap-2">
              Display
              <select aria-label="Display" disabled={applying} className="rounded border border-slate-300 bg-white px-2 py-1" value={mode} onChange={(event) => setMode(event.target.value as typeof mode)}>
                <option value="raw">Original</option>
                <option value="table">Table</option>
                {xml !== null ? <option value="xml">XML format</option> : null}
              </select>
            </label>
            {mode === 'table' ? <>
              <label className="flex items-center gap-2">Columns
                <input aria-label="Columns" type="number" min="1" max="1024" className="w-20 rounded border border-slate-300 px-2 py-1" value={columns} onChange={(event) => setColumns(event.target.value)} onBlur={() => setColumns(String(Math.max(1, Math.min(1024, Math.trunc(Number(columns)) || 1))))} />
              </label>
              <button aria-pressed={transpose} className={`rounded border px-2 py-1 ${transpose ? 'border-blue-400 bg-blue-50 text-blue-800' : 'border-slate-300'}`} onClick={() => setTranspose((current) => !current)}>Transpose</button>
            </> : <label className="flex items-center gap-2">
              <input type="checkbox" checked={wrap} onChange={(event) => setWrap(event.target.checked)} />
              Wrap text
            </label>}
            <span className="text-xs text-slate-500">
              {Array.from(draft).length.toLocaleString()} characters · DICOM Length: {element.length.toLocaleString()} bytes (loaded)
            </span>
          </div>
          <textarea
            hidden={mode !== 'raw'}
            ref={textRef}
            aria-label="Full value"
            readOnly={!element.editable}
            disabled={applying}
            wrap={wrap ? 'soft' : 'off'}
            spellCheck={false}
            className={`dicom-value-font h-[min(50vh,28rem)] min-h-32 w-full resize-none overflow-auto rounded border p-3 text-sm outline-none focus:ring-2 ${validation.valid ? 'border-slate-300 focus:border-blue-400 focus:ring-blue-200' : 'border-red-500 focus:ring-red-200'} ${wrap ? 'whitespace-pre-wrap break-all' : 'whitespace-pre'}`}
            value={draft}
            onChange={(event) => { setDraft(event.target.value); setValidation({ valid: true }); setCopyStatus('') }}
          />
          {mode === 'table' ? <div className="h-[min(50vh,28rem)] min-h-32 overflow-auto rounded border border-slate-300">
            <table aria-label="Value table" className="w-full border-collapse text-sm">
              <thead className="sticky top-0 bg-slate-100 text-slate-600"><tr>
                <th scope="col" className="border-b border-slate-300 px-3 py-2 text-left font-normal">#</th>
                {matrix[0]?.map((_, index) => <th key={index} scope="col" className="border-b border-slate-300 px-3 py-2 text-right font-normal">{index + 1}</th>)}
              </tr></thead>
              <tbody>{matrix.map((row, rowIndex) => <tr key={rowIndex}>
                <th scope="row" className="border-b border-slate-200 bg-slate-50 px-3 py-2 text-left font-normal text-slate-500">{rowIndex + 1}</th>
                {row.map((value, columnIndex) => <td key={columnIndex} className="dicom-value-font whitespace-pre border-b border-slate-200 px-3 py-2 text-right tabular-nums">{value === null ? <span className="text-slate-400">—</span> : value === '' ? <span className="text-slate-400">(empty)</span> : value}</td>)}
              </tr>)}</tbody>
            </table>
          </div> : null}
          {mode === 'xml' ? <pre aria-label="Formatted XML" className={`dicom-value-font m-0 h-[min(50vh,28rem)] min-h-32 overflow-auto rounded border border-slate-300 p-3 text-sm ${wrap ? 'whitespace-pre-wrap break-all' : 'whitespace-pre'}`}>{xml}</pre> : null}
          {!validation.valid ? <p role="alert" className="text-sm text-red-700">{validation.message ?? 'Invalid value'}</p> : null}
          <p className="text-xs text-slate-500">
            {mode !== 'raw' ? 'Display only. Copy uses the original text; select Original to edit.' : element.editable ? 'Apply confirms this value. Save the file to write your changes.' : 'Read-only value. You can view and copy the text.'}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-3 border-t border-slate-200 px-4 py-3">
          <button disabled={applying} className="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50 disabled:opacity-50" onClick={() => void copy()}>Copy full value</button>
          <span role="status" className="flex-1 text-xs text-slate-600">{copyStatus}</span>
          <button className="rounded px-3 py-1.5 text-sm hover:bg-slate-100" onClick={onClose}>{element.editable ? 'Cancel' : 'Close'}</button>
          {element.editable && mode === 'raw' ? <button disabled={applying} className="rounded bg-blue-700 px-4 py-1.5 text-sm text-white hover:bg-blue-600 disabled:opacity-50" onClick={() => void apply()}>{applying ? 'Validating…' : 'Apply'}</button> : null}
        </div>
      </div>
    </dialog>,
    document.body,
  )
}
