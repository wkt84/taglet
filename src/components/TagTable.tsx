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
  onChange: (path: string[], value: string) => void
}

export default function TagTable({ nodes, onChange }: Props) {
  const [expanded, setExpanded] = useState<ExpandedState>(true)
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
                className="mr-1 w-5 rounded text-slate-700 hover:bg-slate-200"
                onClick={row.getToggleExpandedHandler()}
                title={row.getIsExpanded() ? 'Collapse sequence' : 'Expand sequence'}
              >
                {row.getIsExpanded() ? '▼' : '▶'}
              </button>
            ) : (
              <span className="mr-1 w-5" />
            )}
            <span className="font-mono text-xs">{String(getValue())}</span>
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
          <span className="font-mono text-xs">{row.original.kind === 'Element' ? row.original.vr : 'SQ'}</span>
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
          ) : (
            <span className="text-slate-500">[Sequence: {row.original.items.length} item(s)]</span>
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
    ],
    [onChange],
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
            <TagRow key={row.id} row={row} />
          ))}
        </tbody>
      </table>
    </div>
  )
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
      subRows: node.items.flatMap((item, sequenceItemIndex) =>
        toTableRows(item, depth + 1, sequenceItemIndex),
      ),
    }
  })
}

