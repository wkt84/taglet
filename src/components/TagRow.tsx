import { flexRender, Row } from '@tanstack/react-table'
import type { TableDicomRow } from '../types/dicom'

type Props = {
  row: Row<TableDicomRow>
}

export default function TagRow({ row }: Props) {
  const isSequence = row.original.kind === 'Sequence'
  const itemClass = row.original.itemIndex !== undefined ? 'border-t border-blue-100 bg-blue-50/40' : ''
  const sequenceClass = isSequence ? 'bg-sky-50 font-medium' : ''
  const stripeClass = row.index % 2 === 0 ? 'bg-white' : 'bg-slate-50'

  return (
    <tr
      className={`${stripeClass} ${itemClass} ${sequenceClass} hover:bg-blue-100/60 ${
        isSequence ? 'cursor-pointer' : ''
      }`}
      onClick={isSequence ? row.getToggleExpandedHandler() : undefined}
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
