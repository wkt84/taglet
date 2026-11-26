import { expect, test, type Page } from '@playwright/test'
import { files, FILE_A } from './fixtures'
import { openMockFiles, savedNodes } from './tauriMock'
import type { DicomElement, DicomNode } from '../../src/types/dicom'

// Long values arrive as previews (their start) with a key; the page fetches the whole
// value to show, edit or copy it, and drops the key when it changes the value (#20).

test.describe.configure({ mode: 'default' })

const CONTOUR = '(3006,0050)'
const KEY = '(3006,0050)'
const WHOLE = Array.from({ length: 400 }, (_, index) => `${index}.5`).join('\\')
const PREVIEW = Array.from(WHOLE).slice(0, 200).join('')

function row(page: Page, tag: string) {
  return page.locator(`tr[data-row-path="${encodeURIComponent(tag)}"]`)
}

function contourOf(nodes: DicomNode[]) {
  return nodes.find((node): node is DicomElement => node.kind === 'Element' && node.tag === CONTOUR)!
}

async function openValue(page: Page) {
  await row(page, CONTOUR).getByRole('button', { name: 'View full value', exact: true }).click()
  return page.getByRole('dialog', { name: 'Tag value', exact: true })
}

test.beforeEach(async ({ page }) => {
  const fixtureFiles = structuredClone(files)
  fixtureFiles[FILE_A].nodes.push({
    kind: 'Element', tag: CONTOUR, vr: 'DS', description: 'Contour Data', value: PREVIEW,
    length: WHOLE.length, path: [CONTOUR], editable: true, full_value_key: KEY,
  })
  await openMockFiles(page, fixtureFiles, {}, { [FILE_A]: { [KEY]: WHOLE } })
})

test('previews show their start and the dialog shows, copies and tabulates the whole value', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await expect(row(page, CONTOUR)).toContainText(PREVIEW.slice(0, 120) + '…')
  const dialog = await openValue(page)
  const text = dialog.getByRole('textbox', { name: 'Full value' })
  await expect(text).toHaveValue(WHOLE)
  await expect(dialog).toContainText(`${Array.from(WHOLE).length.toLocaleString()} characters`)
  await dialog.getByRole('button', { name: 'Copy full value', exact: true }).click()
  await expect(dialog.getByRole('status')).toHaveText('Copied')
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(WHOLE)
  await dialog.getByLabel('Display').selectOption('table')
  await dialog.getByLabel('Columns').fill('100')
  await dialog.getByLabel('Columns').blur()
  // 400 values in rows of 100.
  await expect(dialog.getByRole('table', { name: 'Value table' }).locator('tbody tr')).toHaveCount(4)
})

test('previews are saved as they came, with their keys, when not edited', async ({ page }) => {
  const contour = contourOf(await savedNodes(page, FILE_A) as DicomNode[])
  expect(contour.value).toBe(PREVIEW)
  expect(contour.full_value_key).toBe(KEY)
})

test('editing a preview edits its whole value and drops the key', async ({ page }) => {
  const dialog = await openValue(page)
  const text = dialog.getByRole('textbox', { name: 'Full value' })
  await expect(text).toHaveValue(WHOLE)
  await text.fill(WHOLE + '\\400.5')
  await dialog.getByRole('button', { name: 'Apply', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  const contour = contourOf(await savedNodes(page, FILE_A) as DicomNode[])
  expect(contour.value).toBe(WHOLE + '\\400.5')
  expect(contour.full_value_key).toBeUndefined()
})

test('applying a preview unchanged in the dialog keeps it a preview', async ({ page }) => {
  const dialog = await openValue(page)
  await expect(dialog.getByRole('textbox', { name: 'Full value' })).toHaveValue(WHOLE)
  await dialog.getByRole('button', { name: 'Apply', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  const contour = contourOf(await savedNodes(page, FILE_A) as DicomNode[])
  expect(contour.value).toBe(PREVIEW)
  expect(contour.full_value_key).toBe(KEY)
})

test('batch editing a preview, even to its start, changes it and drops the key', async ({ page }) => {
  await row(page, CONTOUR).getByText('Contour Data', { exact: true }).click()
  await page.getByRole('button', { name: 'Batch Edit', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Batch Edit Tag' })
  // The new value equals the preview, but the whole value differs: not "Unchanged".
  await dialog.getByLabel('New value').fill(PREVIEW)
  await expect(dialog).not.toContainText('Unchanged')
  await dialog.getByRole('button', { name: /Apply 1 changes/ }).click()
  await expect(dialog).toHaveCount(0)
  const contour = contourOf(await savedNodes(page, FILE_A) as DicomNode[])
  expect(contour.value).toBe(PREVIEW)
  expect(contour.full_value_key).toBeUndefined()
})
