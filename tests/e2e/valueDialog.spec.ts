import { expect, test, type Page } from '@playwright/test'
import { files, FILE_A } from './fixtures'
import { openMockFiles, savedNodes } from './tauriMock'
import type { DicomElement, DicomNode } from '../../src/types/dicom'

// Clipboard contents are shared by the browser, so copy tests run sequentially.
test.describe.configure({ mode: 'default' })

const LONG_TEXT = '(0008,4000)'
const PRIVATE_TEXT = '(0019,1002)'
const LONG_SH = '(0008,0050)'
const text = 'Long report 😀 '.repeat(20) + '\r\nSecond line\twith trailing spaces  '
const privateText = 'ASCII_PRIVATE_'.repeat(25) + '  '
const longSh = 'VALUE\\'.repeat(25) + 'LAST'

function element(tag: string, value: string, vr: string, editable = true): DicomElement {
  return { kind: 'Element', tag, value, vr, description: editable ? 'Report text' : '[Private]', editable, length: new TextEncoder().encode(value).length, path: [tag] }
}

function row(page: Page, tag: string) {
  return page.locator(`tr[data-row-path="${encodeURIComponent(tag)}"]`)
}

async function openValue(page: Page, tag: string) {
  await row(page, tag).getByRole('button', { name: 'View full value', exact: true }).click()
  return page.getByRole('dialog', { name: 'Tag value', exact: true })
}

function valueAt(nodes: DicomNode[], tag: string) {
  return nodes.find((node): node is DicomElement => node.kind === 'Element' && node.tag === tag)?.value
}

test.beforeEach(async ({ page }) => {
  const fixtureFiles = structuredClone(files)
  fixtureFiles[FILE_A].nodes.push(
    element(LONG_TEXT, text, 'LT'),
    { ...element(PRIVATE_TEXT, privateText, 'UN', false), inferred_vr: 'LT?' },
    element(LONG_SH, longSh, 'SH'),
  )
  await openMockFiles(page, fixtureFiles)
})

test('long values use bounded previews and read-only dialog preserves full text', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  const privateRow = row(page, PRIVATE_TEXT)
  await expect(privateRow).toContainText(privateText.slice(0, 120) + '…')
  await expect(privateRow.locator('td').nth(3).locator('[title]')).toHaveCount(0)
  const dialog = await openValue(page, PRIVATE_TEXT)
  const fullValue = dialog.getByRole('textbox', { name: 'Full value' })
  await expect(fullValue).toHaveValue(privateText)
  await expect(fullValue).toHaveAttribute('readonly', '')
  await expect(fullValue).toBeFocused()
  await expect(dialog).toContainText('VR: UN (displayed as LT?)')
  await expect(dialog.getByRole('button', { name: 'Apply', exact: true })).toHaveCount(0)
  await dialog.getByLabel('Wrap text').uncheck()
  await expect(fullValue).toHaveAttribute('wrap', 'off')
  await dialog.getByRole('button', { name: 'Copy full value' }).click()
  await expect(dialog.getByRole('status')).toHaveText('Copied')
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(privateText)
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  expect(valueAt(await savedNodes(page, FILE_A) as DicomNode[], PRIVATE_TEXT)).toBe(privateText)
})

test('copy retains line breaks and whitespace while blur and cancel leave stored text intact', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  const dialog = await openValue(page, LONG_TEXT)
  await dialog.getByRole('button', { name: 'Copy full value' }).click()
  await expect(dialog.getByRole('status')).toHaveText('Copied')
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(text)
  await dialog.getByRole('textbox', { name: 'Full value' }).fill('Unconfirmed change')
  await dialog.getByLabel('Wrap text').uncheck()
  await expect(page.getByRole('button', { name: 'plan-a.dcm', exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(valueAt(await savedNodes(page, FILE_A) as DicomNode[], LONG_TEXT)).toBe(text)
})

test('Apply validates the latest draft, rejects invalid values and accepts corrected text', async ({ page }) => {
  const dialog = await openValue(page, LONG_SH)
  await dialog.getByRole('textbox', { name: 'Full value' }).fill('THIS_VALUE_IS_TOO_LONG')
  await dialog.getByRole('button', { name: 'Apply', exact: true }).click()
  await expect(dialog.getByRole('alert')).toContainText('SH must be 16 characters or fewer')
  await expect(page.getByRole('button', { name: 'plan-a.dcm', exact: true })).toBeVisible()
  await dialog.getByRole('textbox', { name: 'Full value' }).fill('CORRECTED')
  await dialog.getByRole('button', { name: 'Apply', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  expect(valueAt(await savedNodes(page, FILE_A) as DicomNode[], LONG_SH)).toBe('CORRECTED')
})

test('clipboard fallback copies exact text when the Clipboard API is unavailable', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await page.evaluate(() => {
    Object.defineProperty(navigator.clipboard, 'writeText', {
      value: async () => { throw new Error('Clipboard API unavailable') },
    })
  })
  const dialog = await openValue(page, LONG_TEXT)
  await dialog.getByRole('button', { name: 'Copy full value' }).click()
  await expect(dialog.getByRole('status')).toHaveText('Copied')
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(text)
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
})

test('multiline drafts apply only explicitly and Escape discards changes', async ({ page }) => {
  let dialog = await openValue(page, LONG_TEXT)
  const newText = 'First line\nSecond line  '
  await dialog.getByRole('textbox', { name: 'Full value' }).fill(newText)
  await dialog.getByRole('button', { name: 'Apply', exact: true }).click()
  await expect(dialog).not.toBeVisible()
  await expect(row(page, LONG_TEXT)).toContainText('First line↵Second line  ')
  expect(valueAt(await savedNodes(page, FILE_A) as DicomNode[], LONG_TEXT)).toBe(newText)
  dialog = await openValue(page, LONG_TEXT)
  await dialog.getByRole('textbox', { name: 'Full value' }).fill('Discard this')
  await dialog.getByRole('textbox', { name: 'Full value' }).press('Escape')
  await expect(dialog).not.toBeVisible()
  await expect(row(page, LONG_TEXT).getByRole('button', { name: 'View full value' })).toBeFocused()
  expect(valueAt(await savedNodes(page, FILE_A) as DicomNode[], LONG_TEXT)).toBe(newText)
})
