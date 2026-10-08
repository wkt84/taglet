import type { DicomElement } from '../types/dicom'

export function isPrivateTag(tag: string) {
  const match = /^\(([0-9A-Fa-f]{4}),[0-9A-Fa-f]{4}\)$/.exec(tag)
  return Boolean(match && Number.parseInt(match[1], 16) % 2 === 1)
}

export function canBatchEdit(element: DicomElement) {
  return element.editable && !isPrivateTag(element.tag)
}
