import { expect, test, type Page } from '@playwright/test'
import { BEAMS, MACHINE } from './fixtures'
import { openMockFiles } from './tauriMock'

function row(page: Page, path: string[]) {
  return page.locator(`tr[data-row-path="${path.map(encodeURIComponent).join('/')}"]`)
}

test.beforeEach(async ({ page }) => {
  await openMockFiles(page)
})

// Rows inside collapsed sequences and items are only built once expanded (#20).
test('collapsed sequences show their rows once expanded', async ({ page }) => {
  await expect(row(page, [BEAMS, 'Item#1'])).toHaveCount(0)
  await row(page, [BEAMS]).click()
  await expect(row(page, [BEAMS, 'Item#1'])).toBeVisible()
  await expect(row(page, [BEAMS, 'Item#1', MACHINE])).toHaveCount(0)
  await row(page, [BEAMS, 'Item#1']).click()
  await expect(row(page, [BEAMS, 'Item#1', MACHINE])).toContainText('MACHINE_2')
})

test('search finds tags inside collapsed items and opens the way to them', async ({ page }) => {
  await page.getByPlaceholder(/Search tag key or tag name/).fill('Treatment Machine Name')
  const result = page.getByRole('button', { name: /Item #2 \/ \(300A,00B2\) · MACHINE_2/ })
  await expect(result).toBeVisible()
  await result.click()
  await expect(row(page, [BEAMS, 'Item#1', MACHINE])).toBeVisible()
  await expect(row(page, [BEAMS, 'Item#1', MACHINE])).toContainText('MACHINE_2')
})
