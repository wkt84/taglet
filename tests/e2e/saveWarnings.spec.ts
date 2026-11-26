import { expect, test } from '@playwright/test'
import { files } from './fixtures'
import { openMockFiles } from './tauriMock'

// What saving reports although the file was saved (#32): shown as a warning, not as an
// error, and the file counts as saved.

const WARNING = '/fixtures/plan-a.dcm was saved, but the previous version could not be removed and is still in /fixtures/.taglet-1-0.bak.'

test('warnings from saving are shown, the file having been saved', async ({ page }) => {
  await openMockFiles(page, files, { save_dicom_file: [WARNING] })
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  const warning = page.getByRole('heading', { name: 'DICOM Warning' }).locator('xpath=../..')
  await expect(warning).toContainText(WARNING)
  await warning.getByRole('button', { name: 'OK', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'DICOM Warning' })).toHaveCount(0)
})

test('saving without warnings shows none', async ({ page }) => {
  await openMockFiles(page, files, { save_dicom_file: [] })
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as unknown as { __testIpcCalls: { command: string }[] })
    .__testIpcCalls.some((call) => call.command === 'save_dicom_file'))).toBe(true)
  await expect(page.getByRole('heading', { name: 'DICOM Warning' })).toHaveCount(0)
})
