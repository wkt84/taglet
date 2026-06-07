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
  const sequenceClass = isSequence ? 'bg-slate-700 font-semibold text-white shadow-inner' : ''
  const stripeClass = isSequence ? '' : row.index % 2 === 0 ? 'bg-white' : 'bg-slate-50'
  const selectedClass = selected
    ? isSequence
      ? 'outline outline-2 outline-amber-300 outline-offset-[-2px]'
      : 'outline outline-2 outline-blue-400 outline-offset-[-2px]'
    : ''
  const hoverClass = isSequence ? 'hover:bg-slate-600' : 'hover:bg-blue-100/60'

  return (
    <tr
      className={`${stripeClass} ${itemClass} ${sequenceClass} ${selectedClass} ${hoverClass} ${
        isSequence ? 'cursor-pointer' : ''
      }`}
      onClick={() => {
        onSelect(row.original.path)
        if (isSequence) row.toggleExpanded()
      }}
      title={isSequence ? (row.getIsExpanded() ? 'Collapse sequence' : 'Expand sequence') : undefined}
    >
      {row.getVisibleCells().map((cell) => (
        <td
          key={cell.id}
          className={`border-b border-r px-3 py-1.5 align-top last:border-r-0 ${
            isSequence ? 'border-slate-600' : 'border-slate-200'
          }`}
        >
          {flexRender(cell.column.columnDef.cell, cell.getContext())}
        </td>
      ))}
    </tr>
  )
}
