import { expect, test, type Page } from '@playwright/test'
import { BEAMS, MACHINE, FILE_A, FILE_B } from './fixtures'
import { openMockFiles, savedNodes } from './tauriMock'
import { findTagOccurrences } from '../../src/data/batchEdit'
import type { DicomNode } from '../../src/types/dicom'

function row(page: Page, path: string[]) {
  return page.locator(`tr[data-row-path="${path.map(encodeURIComponent).join('/')}"]`)
}

async function openBatch(page: Page, contextMenu = false) {
  await row(page, [BEAMS]).click()
  await row(page, [BEAMS, 'Item#0']).click()
  const machineRow = row(page, [BEAMS, 'Item#0', MACHINE])
  if (contextMenu) {
    await machineRow.click({ button: 'right' })
    await page.getByRole('menuitem', { name: 'Batch edit this tag…' }).click()
  } else {
    await machineRow.getByText('Treatment Machine Name', { exact: true }).click()
    await page.getByRole('button', { name: 'Batch Edit', exact: true }).click()
  }
  return page.getByRole('dialog', { name: 'Batch Edit Tag' })
}

test.beforeEach(async ({ page }) => {
  await openMockFiles(page)
})

test('parent Sequence scope changes all Beams while individual exclusions preserve other values', async ({ page }) => {
  const dialog = await openBatch(page)
  await expect(dialog.getByLabel('Scope')).toHaveValue(JSON.stringify([BEAMS]))
  await expect(dialog.getByText('Item #1 (Beam 1 · BEAM_1)', { exact: false })).toBeVisible()
  await dialog.getByLabel('New value').fill('NEW_MACHINE')
  await expect(dialog.getByRole('button', { name: 'Apply 2 changes' })).toBeEnabled()
  await dialog.getByRole('row').filter({ hasText: 'BEAM_2' }).getByRole('checkbox').uncheck()
  await dialog.getByRole('button', { name: 'Apply 1 changes' }).click()
  await expect(dialog).not.toBeVisible()
  await expect(row(page, [BEAMS, 'Item#0', MACHINE]).getByRole('button', { name: 'NEW_MACHINE' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'plan-a.dcm*', exact: true })).toBeVisible()
  const nodes = await savedNodes(page, FILE_A) as DicomNode[]
  expect(findTagOccurrences(nodes, MACHINE).map(({ element }) => element.value)).toEqual(['ROOT_MACHINE', 'NEW_MACHINE', 'MACHINE_2', 'OTHER_MACHINE'])
  await expect(page.getByRole('button', { name: 'plan-b.dcm', exact: true })).toBeVisible()
})

test('context menu and entire dataset scope include matching tags in separate Sequences', async ({ page }) => {
  const dialog = await openBatch(page, true)
  await dialog.getByLabel('Scope').selectOption('[]')
  await dialog.getByLabel('New value').fill('ALL_MACHINES')
  await dialog.getByRole('button', { name: 'Apply 4 changes' }).click()
  await expect(dialog).not.toBeVisible()
  const nodes = await savedNodes(page, FILE_A) as DicomNode[]
  expect(findTagOccurrences(nodes, MACHINE).map(({ element }) => element.value)).toEqual(Array(4).fill('ALL_MACHINES'))
})

test('multiple files match differing Beam counts, skip missing tags and keep changes unsaved', async ({ page }) => {
  const dialog = await openBatch(page)
  await dialog.getByLabel('Files').selectOption('multiple')
  await dialog.getByRole('button', { name: 'Select all files' }).click()
  await dialog.getByLabel('New value').fill('NEW_MACHINE')
  await expect(dialog.getByText('Preview · 5 changes in 2 files')).toBeVisible()
  await expect(dialog.getByText('/fixtures/no-machine.dcm — No matching tags in this scope.')).toBeVisible()
  await dialog.getByRole('button', { name: 'Apply 5 changes' }).click()
  await expect(dialog).not.toBeVisible()
  await expect(page.getByRole('button', { name: 'plan-a.dcm*', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'plan-b.dcm*', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'no-machine.dcm', exact: true })).toBeVisible()
  const a = await savedNodes(page, FILE_A) as DicomNode[]
  expect(findTagOccurrences(a, MACHINE, [BEAMS]).map(({ element }) => element.value)).toEqual(Array(2).fill('NEW_MACHINE'))
  await page.getByRole('button', { name: 'plan-b.dcm*', exact: true }).click()
  const b = await savedNodes(page, FILE_B) as DicomNode[]
  expect(findTagOccurrences(b, MACHINE, [BEAMS]).map(({ element }) => element.value)).toEqual(Array(3).fill('NEW_MACHINE'))
})

test('validation errors apply nothing and a corrected empty value can clear tags', async ({ page }) => {
  const dialog = await openBatch(page)
  await dialog.getByLabel('New value').fill('THIS_VALUE_IS_TOO_LONG')
  await dialog.getByRole('button', { name: 'Apply 2 changes' }).click()
  await expect(dialog.getByRole('alert')).toContainText('SH must be 16 characters or fewer')
  await expect(page.getByRole('button', { name: 'plan-a.dcm', exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  let nodes = await savedNodes(page, FILE_A) as DicomNode[]
  expect(findTagOccurrences(nodes, MACHINE, [BEAMS]).map(({ element }) => element.value)).toEqual(['MACHINE_1', 'MACHINE_2'])
  // The original selected row remains expanded, so use the toolbar to reopen.
  await page.getByRole('button', { name: 'Batch Edit', exact: true }).click()
  await dialog.getByLabel('New value').fill('')
  await dialog.getByRole('button', { name: 'Apply 2 changes' }).click()
  await expect(dialog).not.toBeVisible()
  nodes = await savedNodes(page, FILE_A) as DicomNode[]
  expect(findTagOccurrences(nodes, MACHINE, [BEAMS]).map(({ element }) => element.value)).toEqual(['', ''])
})
