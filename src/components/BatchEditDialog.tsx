import { useEffect, useMemo, useState } from 'react'
import type { DicomDocument } from '../hooks/useDicomFile'
import type { DicomElement } from '../types/dicom'
import { ancestorScopes, findTagOccurrences, occurrenceKey, type BatchValueChange } from '../data/batchEdit'
import { canBatchEdit } from '../data/privateTags'
import ValueCell from './ValueCell'

type Props = {
  element: DicomElement
  documents: DicomDocument[]
  activeDocumentId: string
  error?: string
  onApply: (changes: BatchValueChange[]) => Promise<boolean>
  onClose: () => void
}

const inputClass = 'w-full rounded border border-slate-300 px-3 py-2 text-sm outline-none focus:border-blue-500 focus:ring-2 focus:ring-blue-100'

export default function BatchEditDialog({ element, documents, activeDocumentId, error, onApply, onClose }: Props) {
  const active = documents.find((document) => document.id === activeDocumentId)
  const scopes = useMemo(() => ancestorScopes(active?.nodes ?? [], element.path), [active?.nodes, element.path])
  const [scopeKey, setScopeKey] = useState(() => JSON.stringify(scopes[scopes.length - 1]?.tags ?? []))
  const [multipleFiles, setMultipleFiles] = useState(false)
  const [selectedFiles, setSelectedFiles] = useState(() => new Set([activeDocumentId]))
  const [excluded, setExcluded] = useState<Set<string>>(() => new Set())
  const [value, setValue] = useState(element.value)
  const [applying, setApplying] = useState(false)
  const [failed, setFailed] = useState(false)
  const targetDocuments = documents.filter((document) => multipleFiles ? selectedFiles.has(document.id) : document.id === activeDocumentId)
  const preview = targetDocuments.map((document) => ({
    document,
    matches: findTagOccurrences(document.nodes, element.tag, JSON.parse(scopeKey) as string[]),
  }))
  const changes = preview.flatMap(({ document, matches }) => matches
    .filter(({ element: match }) => canBatchEdit(match) && match.value !== value && !excluded.has(occurrenceKey(document.id, match.path)))
    .map(({ element: match }): BatchValueChange => ({ documentId: document.id, path: match.path, previousValue: match.value, value })))
  const fileCount = new Set(changes.map((change) => change.documentId)).size

  useEffect(() => {
    function escape(event: KeyboardEvent) {
      if (event.key === 'Escape' && !applying) onClose()
    }
    window.addEventListener('keydown', escape)
    return () => window.removeEventListener('keydown', escape)
  }, [applying, onClose])

  async function apply() {
    if (applying || changes.length === 0) return
    setApplying(true)
    setFailed(false)
    try {
      if (await onApply(changes)) onClose()
      else setFailed(true)
    } catch {
      setFailed(true)
    } finally {
      setApplying(false)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/40 p-4">
      <div role="dialog" aria-modal="true" aria-labelledby="batch-edit-title" className="flex max-h-[90vh] w-full max-w-6xl flex-col overflow-hidden rounded bg-white shadow-xl">
        <div className="flex items-center justify-between border-b border-slate-200 px-4 py-3">
          <div>
            <h2 id="batch-edit-title" className="font-semibold">Batch Edit Tag</h2>
            <p className="mt-1 text-sm text-slate-600">{element.tag} · {element.description}</p>
          </div>
          <button disabled={applying} className="rounded px-3 py-1 text-sm hover:bg-slate-100 disabled:opacity-50" onClick={onClose}>Close</button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-4">
          <fieldset disabled={applying} className="space-y-4">
            <div className="grid gap-4 md:grid-cols-2">
              <label className="block space-y-1 text-sm">
                <span className="font-medium">Files</span>
                <select className={inputClass} value={multipleFiles ? 'multiple' : 'current'} onChange={(event) => setMultipleFiles(event.target.value === 'multiple')}>
                  <option value="current">Current file</option>
                  <option value="multiple">Select open files</option>
                </select>
              </label>
              <label className="block space-y-1 text-sm">
                <span className="font-medium">Scope</span>
                <select className={inputClass} value={scopeKey} onChange={(event) => setScopeKey(event.target.value)}>
                  <option value="[]">Entire dataset</option>
                  {scopes.map((scope) => <option key={JSON.stringify(scope.tags)} value={JSON.stringify(scope.tags)}>{scope.label}</option>)}
                </select>
                <span className="block text-xs text-slate-500">Includes all Items and nested Sequences in this scope.</span>
              </label>
            </div>
            {multipleFiles ? (
              <div className="rounded border border-slate-200 p-3">
                <div className="mb-2 flex gap-3 text-xs">
                  <button className="text-blue-700 hover:underline" onClick={() => setSelectedFiles(new Set(documents.map((document) => document.id)))}>Select all files</button>
                  <button className="text-blue-700 hover:underline" onClick={() => setSelectedFiles(new Set())}>Clear selection</button>
                </div>
                <div className="max-h-32 space-y-2 overflow-auto">
                  {documents.map((document) => (
                    <label key={document.id} className="flex items-center gap-2 text-sm">
                      <input type="checkbox" checked={selectedFiles.has(document.id)} onChange={(event) => setSelectedFiles((current) => {
                        const next = new Set(current)
                        if (event.target.checked) next.add(document.id)
                        else next.delete(document.id)
                        return next
                      })} />
                      <span className="break-all">{document.filePath}{document.dirty ? ' *' : ''}</span>
                    </label>
                  ))}
                </div>
              </div>
            ) : null}
            <label className="block space-y-1 text-sm">
              <span className="font-medium">New value</span>
              <input autoFocus className={`${inputClass} dicom-value-font`} value={value} onChange={(event) => { setValue(event.target.value); setFailed(false) }} />
              <span className="block text-xs text-slate-500">An empty value clears the selected tags. Values are validated before any changes are applied.</span>
            </label>
            <div>
              <div className="mb-2 flex items-center justify-between text-sm">
                <span className="font-medium">Preview · {changes.length} changes in {fileCount} files</span>
                <div className="flex gap-3 text-xs">
                  <button className="text-blue-700 hover:underline" onClick={() => setExcluded(new Set())}>Select all matches</button>
                  <button className="text-blue-700 hover:underline" onClick={() => setExcluded(new Set(preview.flatMap(({ document, matches }) => matches.map((match) => occurrenceKey(document.id, match.element.path)))))}>Exclude all matches</button>
                </div>
              </div>
              <div className="max-h-[40vh] overflow-auto rounded border border-slate-200">
                <table className="w-full text-left text-sm">
                  <thead className="sticky top-0 bg-slate-100 text-xs text-slate-600">
                    <tr><th className="p-2">Apply</th><th className="p-2">File / location</th><th className="p-2">VR</th><th className="p-2">Current value</th><th className="p-2">New value</th></tr>
                  </thead>
                  <tbody>
                    {preview.map(({ document, matches }) => matches.length === 0 ? (
                      <tr key={document.id}><td colSpan={5} className="border-t p-3 text-slate-500">{document.filePath} — No matching tags in this scope.</td></tr>
                    ) : matches.map(({ element: match, location }) => {
                      const key = occurrenceKey(document.id, match.path)
                      const unchanged = match.value === value
                      return (
                        <tr key={key} className={`border-t border-slate-100 ${!match.editable || unchanged || excluded.has(key) ? 'bg-slate-50 text-slate-500' : ''}`}>
                          <td className="p-2 align-top">
                            <input type="checkbox" aria-label={`Apply to ${document.filePath} / ${location}`} disabled={!match.editable || unchanged} checked={match.editable && !unchanged && !excluded.has(key)} onChange={(event) => setExcluded((current) => {
                              const next = new Set(current)
                              if (event.target.checked) next.delete(key)
                              else next.add(key)
                              return next
                            })} />
                          </td>
                          <td className="max-w-sm break-words p-2"><div title={document.filePath} className="font-medium">{document.filePath.split(/[\\/]/).pop()}</div><div className="mt-1 text-xs">{location}</div></td>
                          <td className="p-2 align-top font-mono text-xs">{match.vr}</td>
                          <td className="max-w-xs p-2 align-top"><ValueCell element={{ ...match, editable: false }} onCommit={() => {}} /></td>
                          <td className="max-w-xs p-2 align-top">{!match.editable ? 'Read-only · skipped' : unchanged ? 'Unchanged' : <ValueCell element={{ ...match, value, editable: false }} onCommit={() => {}} />}</td>
                        </tr>
                      )
                    }))}
                    {targetDocuments.length === 0 ? <tr><td colSpan={5} className="p-3 text-slate-500">Select at least one file.</td></tr> : null}
                  </tbody>
                </table>
              </div>
            </div>
          </fieldset>
          {failed ? <p role="alert" className="mt-3 rounded bg-red-50 p-3 text-sm text-red-700">{error ?? 'Could not apply changes. Review the targets and try again.'}</p> : null}
        </div>
        <div className="flex items-center justify-between gap-4 border-t border-slate-200 px-4 py-3">
          <p className="text-xs text-slate-500">Changes remain unsaved. Use Save or Save As for each changed file.</p>
          <button className="shrink-0 rounded bg-blue-700 px-4 py-2 text-sm font-medium text-white hover:bg-blue-600 disabled:opacity-45" disabled={applying || changes.length === 0} onClick={() => void apply()}>{applying ? 'Validating…' : `Apply ${changes.length} changes`}</button>
        </div>
      </div>
    </div>
  )
}
