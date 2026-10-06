import type { DicomElement, DicomFileContent, DicomNode } from '../../src/types/dicom'

export const BEAMS = '(300A,00B0)'
export const MACHINE = '(300A,00B2)'
export const OTHER = '(300A,0070)'
export const FILE_A = '/fixtures/plan-a.dcm'
export const FILE_B = '/fixtures/plan-b.dcm'
export const FILE_C = '/fixtures/no-machine.dcm'

function element(tag: string, value: string, path: string[], description: string, vr = 'SH'): DicomElement {
  return { kind: 'Element', tag, vr, description, value, path, editable: true, length: value.length }
}

function plan(beamCount: number): DicomFileContent {
  const nodes: DicomNode[] = [
    element(MACHINE, 'ROOT_MACHINE', [MACHINE], 'Treatment Machine Name'),
    {
      kind: 'Sequence', tag: BEAMS, description: 'Beam Sequence', length: 0, path: [BEAMS],
      items: Array.from({ length: beamCount }, (_, index) => {
        const prefix = [BEAMS, `Item#${index}`]
        return [
          element('(300A,00C0)', String(index + 1), [...prefix, '(300A,00C0)'], 'Beam Number', 'IS'),
          element('(300A,00C2)', `BEAM_${index + 1}`, [...prefix, '(300A,00C2)'], 'Beam Name', 'LO'),
          element(MACHINE, `MACHINE_${index + 1}`, [...prefix, MACHINE], 'Treatment Machine Name'),
        ]
      }),
    },
    {
      kind: 'Sequence', tag: OTHER, description: 'Fraction Group Sequence', length: 0, path: [OTHER],
      items: [[element(MACHINE, 'OTHER_MACHINE', [OTHER, 'Item#0', MACHINE], 'Treatment Machine Name')]],
    },
  ]
  return { nodes, file_meta: [], warnings: [] }
}

export const files: Record<string, DicomFileContent> = {
  [FILE_A]: plan(2),
  [FILE_B]: plan(3),
  [FILE_C]: { nodes: [element('(0010,0020)', 'TEST_PATIENT', ['(0010,0020)'], 'Patient ID', 'LO')], file_meta: [], warnings: [] },
}
