import { flexRender, Row } from '@tanstack/react-table'
import type { TableDicomRow } from '../types/dicom'

type Props = {
  row: Row<TableDicomRow>
  selected: boolean
  onSelect: (path: string[]) => void
}

export default function TagRow({ row, selected, onSelect }: Props) {
  const isSequence = row.original.kind === 'Sequence'
  const itemClass = row.original.itemIndex !== undefined ? 'border-t border-blue-100 bg-blue-50/40' : ''
  const sequenceClass = isSequence ? 'bg-sky-50 font-medium' : ''
  const stripeClass = row.index % 2 === 0 ? 'bg-white' : 'bg-slate-50'
  const selectedClass = selected ? 'outline outline-2 outline-blue-400 outline-offset-[-2px]' : ''

  return (
    <tr
      className={`${stripeClass} ${itemClass} ${sequenceClass} ${selectedClass} hover:bg-blue-100/60 ${
        isSequence ? 'cursor-pointer' : ''
      }`}
      onClick={() => {
        onSelect(row.original.path)
        if (isSequence) row.toggleExpanded()
      }}
      title={isSequence ? (row.getIsExpanded() ? 'Collapse sequence' : 'Expand sequence') : undefined}
    >
      {row.getVisibleCells().map((cell) => (
        <td key={cell.id} className="border-b border-r border-slate-200 px-3 py-1.5 align-top last:border-r-0">
          {flexRender(cell.column.columnDef.cell, cell.getContext())}
        </td>
      ))}
    </tr>
  )
}
