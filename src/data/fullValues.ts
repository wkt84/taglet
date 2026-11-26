import { invoke } from '@tauri-apps/api/core'
import type { DicomElement } from '../types/dicom'

// Long values are sent as previews (their start) with a key (#20); the backend keeps the
// whole values of each open file. Anything that needs a whole value (to show, edit or
// copy it) fetches it, and changing a value drops its key, so that saving never writes
// a preview (the backend checks this too).

/** Whether the element holds only the start of its value. */
export function isPreview(element: DicomElement) {
  return element.full_value_key !== undefined
}

/** Whether the element holds exactly `value`; never for a preview, whose whole value is not here. */
export function holdsValue(element: DicomElement, value: string) {
  return !isPreview(element) && element.value === value
}

/** The whole value of an element of the open file at `filePath`. */
export async function fullValue(filePath: string | undefined, element: DicomElement) {
  if (!isPreview(element)) return element.value
  if (!filePath) throw new Error('The whole value is not available.')
  return invoke<string>('get_full_value', { path: filePath, key: element.full_value_key })
}

/** The element holding `value`, as a whole value. */
export function withValue<T extends DicomElement>(element: T, value: string): T {
  const next = { ...element, value }
  delete next.full_value_key
  return next
}
