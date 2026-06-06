import { useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { DICOM_TAG_OPTIONS, DicomTagOption } from '../data/dicomTags'
import type { DicomElement } from '../types/dicom'

type Props = {
  existingTags: string[]
  onAdd: (node: DicomElement) => void
  onClose: () => void
}

const EDITABLE_TEXT_VRS = new Set([
  'AE',
  'AS',
  'CS',
  'DA',
  'DS',
  'DT',
  'IS',
  'LO',
  'LT',
  'PN',
  'SH',
  'ST',
  'TM',
  'UC',
  'UI',
  'UR',
  'UT',
])

function normalizeTag(input: string) {
  const compact = input.trim().replace(/[()]/g, '').replace(/\s/g, '').toUpperCase()
  const match = compact.match(/^([0-9A-F]{4}),?([0-9A-F]{4})$/)
  return match ? `(${match[1]},${match[2]})` : undefined
}

function isPrivateTag(tag: string) {
  const group = Number.parseInt(tag.slice(1, 5), 16)
  return Number.isFinite(group) && group % 2 === 1
}

export default function AddTagDialog({ existingTags, onAdd, onClose }: Props) {
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState<DicomTagOption>(DICOM_TAG_OPTIONS[0])
  const [customTag, setCustomTag] = useState('')
  const [value, setValue] = useState('')
  const [error, setError] = useState<string>()

  const filteredOptions = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return DICOM_TAG_OPTIONS

    return DICOM_TAG_OPTIONS.filter((option) =>
      `${option.tag} ${option.vr} ${option.description}`.toLowerCase().includes(needle),
    )
  }, [query])

  const isDuplicate = existingTags.includes(selected.tag)
  const isSupported = EDITABLE_TEXT_VRS.has(selected.vr)

  async function lookupCustomTag() {
    setError(undefined)
    const tag = normalizeTag(customTag)
    if (!tag) {
      setError('Invalid tag format. Use (0010,0010).')
      return
    }

    try {
      const info = await invoke<DicomTagOption>('lookup_dicom_tag', { tag })
      setSelected({
        tag: info.tag,
        vr: info.vr,
        description: info.description,
      })
      setQuery(info.description)
    } catch (error) {
      setError(String(error))
    }
  }

  function addSelectedTag() {
    setError(undefined)

    if (isDuplicate) {
      setError(`${selected.tag} already exists at the root level.`)
      return
    }
    if (!isSupported) {
      setError(`${selected.vr} is visible in the dictionary, but Add Tag currently supports text VRs only.`)
      return
    }

    onAdd({
      kind: 'Element',
      tag: selected.tag,
      vr: selected.vr,
      description: selected.description || (isPrivateTag(selected.tag) ? '[Private]' : '[Unknown]'),
      value,
      length: value.length,
      path: [selected.tag],
      editable: true,
    })
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/40 p-4">
      <div className="flex max-h-[82vh] w-full max-w-3xl flex-col rounded bg-white shadow-xl">
        <div className="flex items-center border-b border-slate-200 px-4 py-3">
          <h2 className="flex-1 text-base font-semibold">Add Tag</h2>
          <button className="rounded px-2 py-1 text-sm text-slate-600 hover:bg-slate-100" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="grid min-h-0 flex-1 grid-cols-[1fr_260px] gap-4 p-4">
          <section className="min-h-0">
            <input
              className="mb-3 w-full rounded border border-slate-300 px-3 py-2 text-sm outline-none focus:border-blue-400 focus:ring-1 focus:ring-blue-300"
              placeholder="Search tag, VR, or name"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
            <div className="max-h-[48vh] overflow-auto rounded border border-slate-200">
              {filteredOptions.map((option) => (
                <button
                  key={option.tag}
                  className={`grid w-full grid-cols-[120px_54px_1fr] gap-2 border-b border-slate-100 px-3 py-2 text-left text-sm last:border-b-0 hover:bg-blue-50 ${
                    selected.tag === option.tag ? 'bg-blue-100' : ''
                  }`}
                  onClick={() => setSelected(option)}
                >
                  <span className="font-mono text-xs">{option.tag}</span>
                  <span className="font-mono text-xs">{option.vr}</span>
                  <span className="truncate">{option.description}</span>
                </button>
              ))}
            </div>
          </section>

          <aside className="space-y-3">
            <div>
              <div className="text-xs font-medium uppercase text-slate-500">Selected</div>
              <div className="mt-1 rounded border border-slate-200 bg-slate-50 p-3 text-sm">
                <div className="font-mono text-xs">{selected.tag}</div>
                <div className="mt-1">
                  <span className="font-mono">{selected.vr}</span> {selected.description}
                </div>
              </div>
            </div>

            <label className="block text-sm">
              Initial Value
              <input
                className="mt-1 w-full rounded border border-slate-300 px-2 py-1.5 text-sm outline-none focus:border-blue-400 focus:ring-1 focus:ring-blue-300"
                value={value}
                onChange={(event) => setValue(event.target.value)}
              />
            </label>

            <div className="rounded border border-slate-200 p-3">
              <label className="block text-sm">
                Custom Tag
                <input
                  className="mt-1 w-full rounded border border-slate-300 px-2 py-1.5 font-mono text-sm outline-none focus:border-blue-400 focus:ring-1 focus:ring-blue-300"
                  placeholder="00100010"
                  value={customTag}
                  onChange={(event) => setCustomTag(event.target.value)}
                />
              </label>
              <button className="mt-2 rounded bg-slate-700 px-3 py-1.5 text-sm text-white hover:bg-slate-600" onClick={lookupCustomTag}>
                Lookup
              </button>
            </div>

            {error ? <div className="rounded bg-red-50 p-2 text-sm text-red-700">{error}</div> : null}
            {isDuplicate ? <div className="rounded bg-amber-50 p-2 text-sm text-amber-800">Already exists.</div> : null}
            {!isSupported ? (
              <div className="rounded bg-slate-100 p-2 text-sm text-slate-600">This VR is not addable yet.</div>
            ) : null}

            <button
              className="w-full rounded bg-blue-700 px-3 py-2 text-sm font-medium text-white hover:bg-blue-600 disabled:cursor-not-allowed disabled:opacity-45"
              disabled={isDuplicate || !isSupported}
              onClick={addSelectedTag}
            >
              Add
            </button>
          </aside>
        </div>
      </div>
    </div>
  )
}
