import { expect, test, type Page } from '@playwright/test'
import { files, FILE_A, FILE_B } from './fixtures'
import { openMockFiles } from './tauriMock'

// The full-value dialog shows one element of one document. When another document
// becomes active while it is open (a file opened or dropped), the same table cell shows
// that document's element at the same path; the dialog must not keep the first
// document's whole value or draft, which applying would write into the other (#40).

test.describe.configure({ mode: 'default' })

const CONTOUR = '(3006,0050)'
const KEY = '(3006,0050)'

function whole(start: number) {
  return Array.from({ length: 400 }, (_, index) => `${start + index}.5`).join('\\')
}

const WHOLE_A = whole(0)
const WHOLE_B = whole(1000)

function row(page: Page, tag: string) {
  return page.locator(`tr[data-row-path="${encodeURIComponent(tag)}"]`)
}

test('the dialog follows the document when another one becomes active', async ({ page }) => {
  const fixtureFiles = structuredClone(files)
  for (const [path, value] of [[FILE_A, WHOLE_A], [FILE_B, WHOLE_B]]) {
    fixtureFiles[path].nodes.push({
      kind: 'Element', tag: CONTOUR, vr: 'DS', description: 'Contour Data', value: value.slice(0, 128),
      length: value.length, path: [CONTOUR], editable: true, full_value_key: KEY,
    })
  }
  await openMockFiles(page, fixtureFiles, {}, { [FILE_A]: { [KEY]: WHOLE_A }, [FILE_B]: { [KEY]: WHOLE_B } })

  await row(page, CONTOUR).getByRole('button', { name: 'View full value', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Tag value', exact: true })
  const text = dialog.getByRole('textbox', { name: 'Full value' })
  await expect(text).toHaveValue(WHOLE_A)
  await text.fill('1.5')

  // As when the file is dropped on the window, or opened again from the system.
  await page.evaluate(async (paths) => {
    const { emit } = await import('/node_modules/@tauri-apps/api/event.js' as string)
    await emit('taglet://open-files', paths)
  }, [FILE_B])
  await expect(page.getByRole('heading', { name: 'Taglet - plan-b.dcm' })).toBeVisible()
  // Closed (the cells are rebuilt for the other document), or showing its value.
  await expect.poll(async () => await dialog.count() === 0 || await text.inputValue() === WHOLE_B).toBe(true)
})
