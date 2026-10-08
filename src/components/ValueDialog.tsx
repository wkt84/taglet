import { useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { invoke } from '@tauri-apps/api/core'
import type { DicomElement, ValidationResult } from '../types/dicom'

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
      const text = textRef.current!
      const previousFocus = document.activeElement as HTMLElement | null
      const { selectionStart, selectionEnd, selectionDirection } = text
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
        text.setSelectionRange(selectionStart, selectionEnd, selectionDirection)
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
              <input type="checkbox" checked={wrap} onChange={(event) => setWrap(event.target.checked)} />
              Wrap text
            </label>
            <span className="text-xs text-slate-500">
              {Array.from(draft).length.toLocaleString()} characters · DICOM Length: {element.length.toLocaleString()} bytes (loaded)
            </span>
          </div>
          <textarea
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
          {!validation.valid ? <p role="alert" className="text-sm text-red-700">{validation.message ?? 'Invalid value'}</p> : null}
          <p className="text-xs text-slate-500">
            {element.editable ? 'Apply confirms this value. Save the file to write your changes.' : 'Read-only value. You can view and copy the text.'}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-3 border-t border-slate-200 px-4 py-3">
          <button disabled={applying} className="rounded border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50 disabled:opacity-50" onClick={() => void copy()}>Copy full value</button>
          <span role="status" className="flex-1 text-xs text-slate-600">{copyStatus}</span>
          <button className="rounded px-3 py-1.5 text-sm hover:bg-slate-100" onClick={onClose}>{element.editable ? 'Cancel' : 'Close'}</button>
          {element.editable ? <button disabled={applying} className="rounded bg-blue-700 px-4 py-1.5 text-sm text-white hover:bg-blue-600 disabled:opacity-50" onClick={() => void apply()}>{applying ? 'Validating…' : 'Apply'}</button> : null}
        </div>
      </div>
    </dialog>,
    document.body,
  )
}
