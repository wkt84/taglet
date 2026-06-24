import {
  ColumnDef,
  ColumnSizingState,
  ExpandedState,
  flexRender,
  getCoreRowModel,
  getExpandedRowModel,
  useReactTable,
} from '@tanstack/react-table'
import { useEffect, useMemo, useRef, useState } from 'react'
import TagRow from './TagRow'
import ValueCell from './ValueCell'
import type { DicomElement, DicomNode, TableDicomRow } from '../types/dicom'

const SEARCH_DEBOUNCE_MS = 150
const MAX_SEARCH_RESULTS = 200
const INDENT_WIDTH = 18
const MAX_TAG_INDENT_DEPTH = 8

type Props = {
  nodes: DicomNode[]
  filePath?: string
  selectedPath?: string[]
  onChange: (path: string[], value: string) => void
  onDelete: (path: string[]) => void
  onSelect: (path: string[]) => void
}

export default function TagTable({ nodes, filePath, selectedPath, onChange, onDelete, onSelect }: Props) {
  const tableScrollRef = useRef<HTMLDivElement>(null)
  const [expanded, setExpanded] = useState<ExpandedState>({})
  const [query, setQuery] = useState('')
  const [debouncedQuery, setDebouncedQuery] = useState('')
  const [pendingScrollPath, setPendingScrollPath] = useState<string[]>()
  const [columnSizing, setColumnSizing] = useState<ColumnSizingState>({})
  const rows = useMemo(() => toTableRows(nodes), [nodes])
  const searchResults = useMemo(() => searchRows(rows, debouncedQuery), [debouncedQuery, rows])
  const searching = query.trim() !== debouncedQuery.trim()

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setDebouncedQuery(query)
    }, SEARCH_DEBOUNCE_MS)

    return () => window.clearTimeout(timer)
  }, [query])

  useEffect(() => {
    setQuery('')
    setDebouncedQuery('')
    setPendingScrollPath(undefined)
  }, [filePath])

  const columns = useMemo<ColumnDef<TableDicomRow>[]>(
    () => [
      {
        accessorKey: 'tag',
        header: 'Tag',
        size: 220,
        minSize: 150,
        maxSize: 520,
        cell: ({ row, getValue }) => (
          <div
            className="flex min-w-0 items-center"
            style={{ paddingLeft: `${tagIndent(row.original.depth)}px` }}
          >
            {row.getCanExpand() ? (
              <button
                className={`mr-1 h-5 w-5 shrink-0 rounded ${
                  row.original.kind === 'Sequence'
                    ? 'text-slate-100 hover:bg-white/10'
                    : row.original.kind === 'Item'
                      ? 'text-blue-800 hover:bg-blue-100'
                      : 'text-slate-700 hover:bg-slate-200'
                }`}
                onClick={(event) => {
                  event.stopPropagation()
                  onSelect(row.original.path)
                  row.toggleExpanded()
                }}
                title={row.getIsExpanded() ? 'Collapse' : 'Expand'}
              >
                {row.getIsExpanded() ? '▼' : '▶'}
              </button>
            ) : (
              <span className="mr-1 w-5 shrink-0" />
            )}
            <span className="dicom-value-font min-w-0 truncate text-xs" title={String(getValue())}>
              {String(getValue())}
            </span>
          </div>
        ),
      },
      {
        accessorKey: 'description',
        header: 'Description',
        size: 280,
        minSize: 160,
        maxSize: 640,
        cell: ({ getValue }) => <span className="block truncate">{String(getValue())}</span>,
      },
      {
        accessorKey: 'vr',
        header: 'VR',
        size: 80,
        minSize: 60,
        maxSize: 120,
        cell: ({ row }) => (
          <span className="font-mono text-xs">
            {row.original.kind === 'Element' ? row.original.vr : row.original.kind === 'Sequence' ? 'SQ' : ''}
          </span>
        ),
      },
      {
        accessorKey: 'value',
        header: 'Value',
        size: 520,
        minSize: 240,
        maxSize: 1200,
        cell: ({ row }) =>
          row.original.kind === 'Element' ? (
            <ValueCell
              element={row.original as DicomElement}
              onCommit={(value) => onChange(row.original.path, value)}
            />
          ) : row.original.kind === 'Item' ? (
            <span className="inline-flex items-center rounded bg-blue-100 px-2 py-0.5 text-xs font-medium text-blue-800">
              {row.original.childCount} {row.original.childCount === 1 ? 'tag' : 'tags'}
            </span>
          ) : (
            <span className="inline-flex items-center rounded bg-white/15 px-2 py-0.5 text-xs font-medium text-white ring-1 ring-white/20">
              {row.original.items.length} {row.original.items.length === 1 ? 'item' : 'items'}
            </span>
          ),
      },
      {
        accessorKey: 'length',
        header: 'Length',
        size: 120,
        minSize: 90,
        maxSize: 180,
        cell: ({ getValue }) => {
          const value = Number(getValue())
          return <span className="font-mono text-xs">{value === 4294967295 ? 'Undefined' : value}</span>
        },
      },
      {
        id: 'actions',
        header: '',
        size: 96,
        minSize: 82,
        maxSize: 140,
        cell: ({ row }) => {
          const isPixelData = row.original.kind === 'Element' && row.original.tag === '(7FE0,0010)'
          const isItem = row.original.kind === 'Item'
          return (
            <button
              className="rounded px-2 py-0.5 text-xs text-red-700 hover:bg-red-50 disabled:cursor-not-allowed disabled:text-slate-400 disabled:hover:bg-transparent"
              disabled={isPixelData || isItem}
              onClick={(event) => {
                event.stopPropagation()
                if (window.confirm(`Delete ${row.original.tag}?`)) {
                  onDelete(row.original.path)
                }
              }}
              title={
                isItem
                  ? 'Sequence items cannot be deleted here yet'
                  : isPixelData
                    ? 'Pixel Data cannot be deleted here'
                    : `Delete ${row.original.tag}`
              }
            >
              Delete
            </button>
          )
        },
      },
    ],
    [onChange, onDelete],
  )

  const table = useReactTable({
    data: rows,
    columns,
    state: { expanded, columnSizing },
    onExpandedChange: setExpanded,
    onColumnSizingChange: setColumnSizing,
    columnResizeMode: 'onChange',
    getRowId: (row) => row.rowId,
    getSubRows: (row) => row.subRows,
    getCoreRowModel: getCoreRowModel(),
    getExpandedRowModel: getExpandedRowModel(),
  })

  useEffect(() => {
    if (!pendingScrollPath) return
    const row = tableScrollRef.current?.querySelector<HTMLElement>(
      `[data-row-path="${rowPathKey(pendingScrollPath)}"]`,
    )
    if (!row) return

    const scrollContainer = tableScrollRef.current
    const header = scrollContainer?.querySelector<HTMLElement>('thead')
    if (!scrollContainer) return

    scrollContainer.scrollTo({
      top: row.offsetTop - (header?.offsetHeight ?? 0),
      behavior: 'smooth',
    })
    setPendingScrollPath(undefined)
  }, [pendingScrollPath, table.getRowModel().rows])

  if (nodes.length === 0) {
    return (
      <div className="flex h-full items-center justify-center rounded border border-dashed border-slate-300 bg-white text-sm text-slate-500">
        Open a DICOM file to inspect and edit tags.
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col overflow-hidden rounded border border-slate-300 bg-white shadow-sm">
      <div className="border-b border-slate-200 bg-slate-50 p-3">
        <div className="flex items-center gap-2">
          <input
            className="w-full rounded border border-slate-300 px-3 py-1.5 text-sm outline-none focus:border-blue-500 focus:ring-2 focus:ring-blue-100"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search tag key or tag name, e.g. (0010,0010), 300A00B0, PatientName"
          />
          {query ? (
            <button
              className="rounded px-2 py-1 text-xs text-slate-600 hover:bg-slate-200"
              onClick={() => setQuery('')}
            >
              Clear
            </button>
          ) : null}
        </div>
        {query.trim() ? (
          <div className="mt-2 rounded border border-slate-200 bg-white">
            <div className="flex items-center justify-between border-b border-slate-200 px-3 py-1.5 text-xs text-slate-600">
              <span>
                {searching
                  ? 'Searching...'
                  : `${searchResults.length} result${searchResults.length === 1 ? '' : 's'}${searchResults.length === MAX_SEARCH_RESULTS ? '+' : ''}`}
              </span>
              <span>
                {searchResults.length === MAX_SEARCH_RESULTS
                  ? `Showing first ${MAX_SEARCH_RESULTS}. Refine the query to narrow results.`
                  : 'Click a result to reveal it in the table.'}
              </span>
            </div>
            {searching ? (
              <div className="px-3 py-3 text-sm text-slate-500">Searching...</div>
            ) : searchResults.length > 0 ? (
              <div className="max-h-44 overflow-auto">
                {searchResults.map((result) => (
                  <button
                    key={result.path.join('/')}
                    className={`grid w-full grid-cols-[150px_220px_1fr] gap-3 border-b border-slate-100 px-3 py-2 text-left text-xs last:border-b-0 hover:bg-blue-50 ${
                      samePath(result.path, selectedPath) ? 'bg-blue-50' : ''
                    }`}
                    onClick={() => {
                      setExpanded((current) => ({
                        ...(current === true ? {} : current),
                        ...expandedStateForPath(rows, result.path),
                      }))
                      setPendingScrollPath(result.path)
                      onSelect(result.path)
                    }}
                  >
                    <span className="dicom-value-font font-semibold text-slate-900">{result.tag}</span>
                    <span className="truncate text-slate-700">{result.description}</span>
                    <span className="min-w-0 truncate text-slate-500">
                      {result.pathLabel} · {result.value}
                    </span>
                  </button>
                ))}
              </div>
            ) : (
              <div className="px-3 py-3 text-sm text-slate-500">No matching tags.</div>
            )}
          </div>
        ) : null}
      </div>
      <div ref={tableScrollRef} className="min-h-0 flex-1 overflow-auto">
        <table className="table-fixed border-collapse text-sm" style={{ width: `${table.getTotalSize()}px` }}>
          <colgroup>
            {table.getVisibleLeafColumns().map((column) => (
              <col key={column.id} style={{ width: `${column.getSize()}px` }} />
            ))}
          </colgroup>
          <thead className="sticky top-0 z-10 bg-slate-900 text-left text-xs uppercase tracking-wide text-white">
            {table.getHeaderGroups().map((headerGroup) => (
              <tr key={headerGroup.id}>
                {headerGroup.headers.map((header) => (
                  <th
                    key={header.id}
                    className="relative select-none border-r border-slate-700 px-3 py-2 last:border-r-0"
                    style={{ width: `${header.getSize()}px` }}
                  >
                    <div className="truncate pr-2">
                      {flexRender(header.column.columnDef.header, header.getContext())}
                    </div>
                    {header.column.getCanResize() ? (
                      <div
                        className={`absolute right-0 top-0 h-full w-1 cursor-col-resize touch-none bg-transparent hover:bg-blue-400 ${
                          header.column.getIsResizing() ? 'bg-blue-400' : ''
                        }`}
                        onDoubleClick={() => header.column.resetSize()}
                        onMouseDown={header.getResizeHandler()}
                        onTouchStart={header.getResizeHandler()}
                        title="Drag to resize. Double click to reset."
                      />
                    ) : null}
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody>
            {table.getRowModel().rows.map((row) => (
              <TagRow
                key={row.id}
                row={row}
                selected={samePath(row.original.path, selectedPath)}
                onSelect={onSelect}
              />
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

function samePath(left: string[], right?: string[]) {
  if (!right) return false
  return left.length === right.length && left.every((part, index) => part === right[index])
}

function pathStartsWith(path: string[], prefix: string[]) {
  return prefix.length <= path.length && prefix.every((part, index) => part === path[index])
}

function normalizedTagKey(value: string) {
  return value.replace(/[^0-9A-Fa-f]/g, '').toUpperCase()
}

function compactSearchText(value: string) {
  return value.toLowerCase().replace(/[^a-z0-9]/g, '')
}

function displayPath(path: string[]) {
  return path
    .map((part) => {
      const itemMatch = /^Item#(\d+)$/.exec(part)
      return itemMatch ? `Item #${Number(itemMatch[1]) + 1}` : part
    })
    .join(' / ')
}

function rowPathKey(path: string[]) {
  return path.map((part) => encodeURIComponent(part)).join('/')
}

function tagIndent(depth: number) {
  return Math.min(depth, MAX_TAG_INDENT_DEPTH) * INDENT_WIDTH
}

function rowValue(row: TableDicomRow) {
  if (row.kind === 'Element') return row.value || '-'
  if (row.kind === 'Sequence') return `${row.items.length} item${row.items.length === 1 ? '' : 's'}`
  return `${row.childCount} tag${row.childCount === 1 ? '' : 's'}`
}

function searchRows(rows: TableDicomRow[], query: string) {
  const textNeedle = query.trim().toLowerCase()
  const compactNeedle = compactSearchText(query)
  const tagNeedle = normalizedTagKey(query)
  if (!textNeedle && !tagNeedle) return []

  const results: Array<{
    tag: string
    description: string
    value: string
    path: string[]
    pathLabel: string
  }> = []

  function visit(nextRows: TableDicomRow[]) {
    for (const row of nextRows) {
      if (results.length >= MAX_SEARCH_RESULTS) return
      const tagMatch = tagNeedle.length > 0 && normalizedTagKey(row.tag).includes(tagNeedle)
      const nameMatch = textNeedle.length > 0
        && (row.description.toLowerCase().includes(textNeedle)
          || compactSearchText(row.description).includes(compactNeedle))
      if (tagMatch || nameMatch) {
        results.push({
          tag: row.tag,
          description: row.description,
          value: rowValue(row),
          path: row.path,
          pathLabel: displayPath(row.path),
        })
      }
      if (row.subRows) visit(row.subRows)
    }
  }

  visit(rows)
  return results
}

function expandedStateForPath(rows: TableDicomRow[], path: string[]) {
  const next: Record<string, boolean> = {}

  function visit(nextRows: TableDicomRow[], ancestors: string[]): boolean {
    for (const row of nextRows) {
      if (samePath(row.path, path)) {
        for (const rowId of ancestors) next[rowId] = true
        return true
      }

      if (row.subRows && pathStartsWith(path, row.path)) {
        if (visit(row.subRows, [...ancestors, row.rowId])) return true
      }
    }
    return false
  }

  visit(rows, [])
  return next
}

function toTableRows(nodes: DicomNode[], depth = 0, itemIndex?: number): TableDicomRow[] {
  return nodes.map((node, index) => {
    const rowId = `${node.path.join('/')}:${index}`
    if (node.kind === 'Element') {
      return { ...node, rowId, depth, itemIndex }
    }

    return {
      ...node,
      rowId,
      depth,
      itemIndex,
      subRows: node.items.map((item, sequenceItemIndex) => {
        const itemPath = [...node.path, `Item#${sequenceItemIndex}`]
        return {
          kind: 'Item',
          tag: `Item #${sequenceItemIndex + 1}`,
          description: `${node.description} item ${sequenceItemIndex + 1}`,
          length: item.length,
          path: itemPath,
          rowId: `${itemPath.join('/')}`,
          depth: depth + 1,
          itemIndex: sequenceItemIndex,
          childCount: item.length,
          subRows: toTableRows(item, depth + 2, sequenceItemIndex),
        }
      }),
    }
  })
}
