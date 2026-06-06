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

export type DicomImageInfo = {
  has_pixel_data: boolean
  supported: boolean
  unsupported_reason?: string | null
  modality?: string | null
  sop_class_uid?: string | null
  transfer_syntax_uid?: string | null
  rows?: number | null
  columns?: number | null
  samples_per_pixel?: number | null
  photometric_interpretation?: string | null
  bits_allocated?: number | null
  bits_stored?: number | null
  high_bit?: number | null
  pixel_representation?: number | null
  number_of_frames: number
  window_center: number[]
  window_width: number[]
  rescale_intercept?: number | null
  rescale_slope?: number | null
}

export type DicomFrameImage = {
  width: number
  height: number
  frame_index: number
  rgba_base64: string
}

export type DicomFramePixels = {
  width: number
  height: number
  frame_index: number
  bits_allocated: number
  pixel_representation: number
  photometric_interpretation?: string | null
  rescale_intercept: number
  rescale_slope: number
  pixel_base64: string
  min_value: number
  max_value: number
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
