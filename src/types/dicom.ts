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
  item_count: number
  items_truncated: boolean
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
  dose_grid_scaling?: number | null
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
  dose_grid_scaling?: number | null
  pixel_base64: string
  min_value: number
  max_value: number
}

export type RtPlanBevInfo = {
  supported: boolean
  unsupported_reason?: string | null
  modality?: string | null
  beams: RtPlanBeam[]
}

export type RtPlanBeam = {
  beam_index: number
  beam_number?: number | null
  beam_name?: string | null
  devices: RtPlanBeamLimitingDeviceDefinition[]
  control_points: RtPlanControlPoint[]
}

export type RtPlanBeamLimitingDeviceDefinition = {
  device_type: string
  number_of_pairs?: number | null
  leaf_position_boundaries: number[]
}

export type RtPlanControlPoint = {
  control_point_index: number
  nominal_index?: number | null
  gantry_angle?: number | null
  gantry_angle_inherited: boolean
  collimator_angle?: number | null
  collimator_angle_inherited: boolean
  couch_angle?: number | null
  couch_angle_inherited: boolean
  devices: RtPlanBeamLimitingDevicePosition[]
}

export type RtPlanBeamLimitingDevicePosition = {
  device_type: string
  positions: number[]
  inherited: boolean
}

export type RtStructInfo = {
  supported: boolean
  unsupported_reason?: string | null
  modality?: string | null
  structure_set_label?: string | null
  rois: RtStructRoi[]
  slices: RtStructSlice[]
  bounds?: RtStructBounds | null
}

export type RtStructRoi = {
  roi_number: number
  name?: string | null
  color?: [number, number, number] | null
  contour_count: number
}

export type RtStructSlice = {
  z: number
  contour_count: number
}

export type RtStructBounds = {
  min_x: number
  max_x: number
  min_y: number
  max_y: number
  min_z: number
  max_z: number
}

export type RtStructSliceContours = {
  z: number
  contours: RtStructContour[]
}

export type RtStructContour = {
  roi_number: number
  roi_name?: string | null
  color?: [number, number, number] | null
  geometric_type?: string | null
  points: RtStructPoint[]
}

export type RtStructPoint = {
  x: number
  y: number
  z: number
}

export type TableDicomRow =
  | (DicomElement & {
      rowId: string
      depth: number
      itemIndex?: number
      subRows?: TableDicomRow[]
    })
  | {
      kind: 'Item'
      tag: string
      description: string
      length: number
      path: string[]
      rowId: string
      depth: number
      itemIndex: number
      childCount: number
      subRows: TableDicomRow[]
    }
  | (Omit<DicomSequence, 'items'> & {
      rowId: string
      depth: number
      itemIndex?: number
      subRows: TableDicomRow[]
      items: DicomNode[][]
    })
