export type DicomElement = {
  kind: 'Element'
  tag: string
  vr: string
  description: string
  value: string
  length: number
  path: string[]
  editable: boolean
}

export type DicomSequence = {
  kind: 'Sequence'
  tag: string
  description: string
  length: number
  path: string[]
  items: DicomNode[][]
}

export type DicomNode = DicomElement | DicomSequence

export type ValidationResult = {
  valid: boolean
  message?: string
}

export type TableDicomRow =
  | (DicomElement & {
      rowId: string
      depth: number
      itemIndex?: number
      subRows?: TableDicomRow[]
    })
  | (Omit<DicomSequence, 'items'> & {
      rowId: string
      depth: number
      itemIndex?: number
      subRows: TableDicomRow[]
      items: DicomNode[][]
    })

