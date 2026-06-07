import {
  ColumnDef,
  ExpandedState,
  flexRender,
  getCoreRowModel,
  getExpandedRowModel,
  useReactTable,
} from '@tanstack/react-table'
import { useMemo, useState } from 'react'
import TagRow from './TagRow'
import ValueCell from './ValueCell'
import type { DicomElement, DicomNode, TableDicomRow } from '../types/dicom'

type Props = {
  nodes: DicomNode[]
  selectedPath?: string[]
  onChange: (path: string[], value: string) => void
  onDelete: (path: string[]) => void
  onSelect: (path: string[]) => void
}

export default function TagTable({ nodes, selectedPath, onChange, onDelete, onSelect }: Props) {
  const [expanded, setExpanded] = useState<ExpandedState>({})
  const rows = useMemo(() => toTableRows(nodes), [nodes])

  const columns = useMemo<ColumnDef<TableDicomRow>[]>(
    () => [
      {
        accessorKey: 'tag',
        header: 'Tag',
        cell: ({ row, getValue }) => (
          <div className="flex items-center" style={{ paddingLeft: `${row.original.depth * 18}px` }}>
            {row.getCanExpand() ? (
              <button
                className={`mr-1 w-5 rounded ${
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
              <span className="mr-1 w-5" />
            )}
            <span className="dicom-value-font text-xs">{String(getValue())}</span>
          </div>
        ),
      },
      {
        accessorKey: 'description',
        header: 'Description',
        cell: ({ getValue }) => <span className="block truncate">{String(getValue())}</span>,
      },
      {
        accessorKey: 'vr',
        header: 'VR',
        cell: ({ row }) => (
          <span className="font-mono text-xs">
            {row.original.kind === 'Element' ? row.original.vr : row.original.kind === 'Sequence' ? 'SQ' : ''}
          </span>
        ),
      },
      {
        accessorKey: 'value',
        header: 'Value',
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
        cell: ({ getValue }) => {
          const value = Number(getValue())
          return <span className="font-mono text-xs">{value === 4294967295 ? 'Undefined' : value}</span>
        },
      },
      {
        id: 'actions',
        header: '',
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
    state: { expanded },
    onExpandedChange: setExpanded,
    getRowId: (row) => row.rowId,
    getSubRows: (row) => row.subRows,
    getCoreRowModel: getCoreRowModel(),
    getExpandedRowModel: getExpandedRowModel(),
  })

  if (nodes.length === 0) {
    return (
      <div className="flex h-full items-center justify-center rounded border border-dashed border-slate-300 bg-white text-sm text-slate-500">
        Open a DICOM file to inspect and edit tags.
      </div>
    )
  }

  return (
    <div className="h-full overflow-auto rounded border border-slate-300 bg-white shadow-sm">
      <table className="w-full table-fixed border-collapse text-sm">
        <colgroup>
          <col className="w-48" />
          <col className="w-72" />
          <col className="w-20" />
          <col />
          <col className="w-28" />
          <col className="w-24" />
        </colgroup>
        <thead className="sticky top-0 z-10 bg-slate-700 text-left text-xs uppercase tracking-wide text-white">
          {table.getHeaderGroups().map((headerGroup) => (
            <tr key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <th key={header.id} className="border-r border-slate-600 px-3 py-2 last:border-r-0">
                  {flexRender(header.column.columnDef.header, header.getContext())}
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
  )
}

function samePath(left: string[], right?: string[]) {
  if (!right) return false
  return left.length === right.length && left.every((part, index) => part === right[index])
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
