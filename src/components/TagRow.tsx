import { flexRender, Row } from '@tanstack/react-table'
import type { TableDicomRow } from '../types/dicom'

type Props = {
  row: Row<TableDicomRow>
  selected: boolean
  onSelect: (path: string[]) => void
}

export default function TagRow({ row, selected, onSelect }: Props) {
  const isSequence = row.original.kind === 'Sequence'
  const isItem = row.original.kind === 'Item'
  const isElementInItem = row.original.kind === 'Element' && row.original.itemIndex !== undefined
  const itemClass = isItem ? 'border-t border-blue-300 bg-blue-100 font-medium text-blue-950' : ''
  const itemChildClass = isElementInItem ? 'bg-blue-50/30' : ''
  const sequenceClass = isSequence ? 'bg-sky-900 font-semibold text-white shadow-inner' : ''
  const stripeClass = isSequence || isItem ? '' : row.index % 2 === 0 ? 'bg-white' : 'bg-slate-50'
  const selectedClass = selected
    ? isSequence || isItem
      ? 'outline outline-2 outline-amber-300 outline-offset-[-2px]'
      : 'outline outline-2 outline-blue-400 outline-offset-[-2px]'
    : ''
  const hoverClass = isSequence ? 'hover:bg-sky-800' : isItem ? 'hover:bg-blue-200' : 'hover:bg-blue-100/60'
  const canToggle = isSequence || isItem

  return (
    <tr
      data-row-path={rowPathKey(row.original.path)}
      className={`${stripeClass} ${itemClass} ${itemChildClass} ${sequenceClass} ${selectedClass} ${hoverClass} ${
        canToggle ? 'cursor-pointer' : ''
      }`}
      onClick={() => {
        onSelect(row.original.path)
        if (canToggle) row.toggleExpanded()
      }}
      title={canToggle ? (row.getIsExpanded() ? 'Collapse' : 'Expand') : undefined}
    >
      {row.getVisibleCells().map((cell) => (
        <td
          key={cell.id}
          className={`border-b border-r px-3 py-1.5 align-top last:border-r-0 ${
            isSequence ? 'border-sky-800' : isItem ? 'border-blue-200' : 'border-slate-200'
          }`}
        >
          {flexRender(cell.column.columnDef.cell, cell.getContext())}
        </td>
      ))}
    </tr>
  )
}

function rowPathKey(path: string[]) {
  return path.map((part) => encodeURIComponent(part)).join('/')
}
