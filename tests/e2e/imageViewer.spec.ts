import { expect, test, type Page } from '@playwright/test'
import { openMockFiles } from './tauriMock'
import { files } from './fixtures'
import type { DicomFramePixels } from '../../src/types/dicom'

function frame(stored: number[], bits: 16 | 32, signed: boolean, slope: number, intercept = 0, invert = false): DicomFramePixels {
  const buffer = Buffer.alloc(stored.length * bits / 8)
  stored.forEach((value, index) => {
    if (bits === 32) buffer.writeUInt32LE(value, index * 4)
    else if (signed) buffer.writeInt16LE(value, index * 2)
    else buffer.writeUInt16LE(value, index * 2)
  })
  const values = stored.map((value) => value * slope + intercept)
  return {
    width: stored.length, height: 1, frame_index: 0, bits_allocated: bits,
    pixel_representation: signed ? 1 : 0, photometric_interpretation: invert ? 'MONOCHROME1' : 'MONOCHROME2',
    rescale_slope: slope, rescale_intercept: intercept, pixel_base64: buffer.toString('base64'),
    min_value: Math.min(...values), max_value: Math.max(...values),
  }
}

async function openViewer(page: Page, modality: string, frames: DicomFramePixels[], dicomWindow?: number[]) {
  await openMockFiles(page, files, {
    get_dicom_image_info: { supported: true, modality, number_of_frames: frames.length,
      window_center: dicomWindow ? [dicomWindow[0]] : [], window_width: dicomWindow ? [dicomWindow[1]] : [] },
    get_dicom_frame_pixels: frames,
  })
  await page.getByRole('button', { name: 'Viewers', exact: true }).click()
  await page.getByRole('button', { name: 'Image Viewer', exact: true }).click()
  await expect(page.getByText('Loading frame...', { exact: true })).toBeHidden()
  await expect.poll(() => page.locator('canvas').evaluate((canvas: HTMLCanvasElement) => canvas.width)).toBe(frames[0].width)
}

async function grays(page: Page) {
  return page.locator('canvas').evaluate((canvas: HTMLCanvasElement) => {
    const data = canvas.getContext('2d')!.getImageData(0, 0, canvas.width, 1).data
    return Array.from({ length: canvas.width }, (_, index) => data[index * 4])
  })
}

for (const [modality, pixels] of [
  ['CT', frame([-1000, 0, 1000], 16, true, 1)],
  ['RTIMAGE', frame([0, 10000, 20000], 16, false, 0.5, -100, true)],
  ['RTDOSE', frame([0, 100000, 200000], 32, false, 1e-15)],
] as const) {
  test(`${modality}: Auto, numeric inputs, slider and canvas share the same window`, async ({ page }) => {
    await openViewer(page, modality, [pixels])
    const range = pixels.max_value - pixels.min_value
    await expect.poll(() => grays(page)).toEqual(pixels.photometric_interpretation === 'MONOCHROME1' ? [255, 128, 0] : [0, 128, 255])
    expect(Number(await page.getByLabel('WL', { exact: true }).inputValue())).toBeCloseTo((pixels.min_value + pixels.max_value) / 2, 10)
    expect(Number(await page.getByLabel('WW', { exact: true }).inputValue())).toBeCloseTo(range, 10)
    await page.getByLabel('Auto min/max').uncheck()
    await page.getByLabel('WL', { exact: true }).fill(String(pixels.min_value + range / 4))
    await page.getByLabel('WW', { exact: true }).fill(String(range / 2))
    await expect.poll(() => grays(page)).toEqual(pixels.photometric_interpretation === 'MONOCHROME1' ? [255, 0, 0] : [0, 255, 255])
    const handle = page.getByTitle('Window upper bound')
    const box = (await handle.boundingBox())!
    const track = (await handle.locator('..').boundingBox())!
    const initialHigh = await page.getByLabel('High', { exact: true }).inputValue()
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 - 4)
    await page.mouse.down()
    expect(await page.getByLabel('High', { exact: true }).inputValue()).toBe(initialHigh)
    // Leave the track horizontally while changing High by a quarter of the frame range.
    await page.mouse.move(track.x + track.width + 25, box.y + box.height / 2 - 4 - track.height / 4, { steps: 5 })
    await page.mouse.up()
    await expect.poll(async () => Number(await page.getByLabel('WW', { exact: true }).inputValue()) / range).toBeCloseTo(0.75, 5)
    await expect.poll(() => grays(page)).toEqual(pixels.photometric_interpretation === 'MONOCHROME1' ? [255, 85, 0] : [0, 170, 255])
    expect(Number(await page.getByLabel('Low', { exact: true }).inputValue())).toBeCloseTo(pixels.min_value, 10)
  })
}

test('RTDOSE: Auto follows frame changes and disabling it freezes the displayed dose window', async ({ page }) => {
  await openViewer(page, 'RTDOSE', [frame([0, 100, 200], 32, false, 0.001), frame([0, 500, 1000], 32, false, 0.001)])
  await page.locator('input[type=range]').fill('1')
  await expect(page.getByLabel('WW', { exact: true })).toHaveValue('1')
  await expect(page.getByLabel('WL', { exact: true })).toHaveValue('0.5')
  await page.getByLabel('Auto min/max').uncheck()
  await page.locator('input[type=range]').fill('0')
  await expect(page.getByLabel('WW', { exact: true })).toHaveValue('1')
  await expect.poll(() => grays(page)).toEqual([0, 26, 51])
  await page.getByRole('button', { name: 'Reset to DICOM' }).click()
  await expect(page.getByLabel('Auto min/max')).toBeChecked()
  await expect.poll(() => grays(page)).toEqual([0, 128, 255])
})

test('crossing bounds and reversing preserves the opposite bound', async ({ page }) => {
  await openViewer(page, 'RTIMAGE', [frame([0, 128, 256], 16, false, 1)], [128, 128])
  const handle = page.getByTitle('Window upper bound')
  const box = (await handle.boundingBox())!
  const track = (await handle.locator('..').boundingBox())!
  const x = box.x + box.width / 2, y = box.y + box.height / 2
  await page.mouse.move(x, y)
  await page.mouse.down()
  await page.mouse.move(x, track.y + track.height * 0.9, { steps: 5 })
  await expect(page.getByLabel('Low', { exact: true })).toHaveValue('64')
  await page.mouse.move(x, y, { steps: 5 })
  await page.mouse.up()
  await expect(page.getByLabel('Low', { exact: true })).toHaveValue('64')
  await expect(page.getByLabel('High', { exact: true })).toHaveValue('192')
})

test('Reset cancels a pending slider update and an active drag', async ({ page }) => {
  await openViewer(page, 'RTIMAGE', [frame([0, 128, 256], 16, false, 1)], [128, 128])
  const handle = page.getByTitle('Window upper bound')
  const box = (await handle.boundingBox())!
  const x = box.x + box.width / 2, y = box.y + box.height / 2
  await page.mouse.move(x, y)
  await page.mouse.down()
  await handle.locator('..').evaluate((track, coordinates) => {
    // Queue an update and reset within one task, before requestAnimationFrame can apply it.
    track.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, pointerId: 1, clientX: coordinates.x, clientY: coordinates.y + 40 }))
    const reset = Array.from(document.querySelectorAll('button')).find((button) => button.textContent === 'Reset to DICOM')!
    reset.click()
  }, { x, y })
  await page.mouse.move(x, y + 100)
  await page.mouse.up()
  await expect(page.getByLabel('WL', { exact: true })).toHaveValue('128')
  await expect(page.getByLabel('WW', { exact: true })).toHaveValue('128')
})

test('flat fractional RTDOSE frame has a finite window and stable controls', async ({ page }) => {
  await openViewer(page, 'RTDOSE', [frame([2, 2, 2], 32, false, 1e-10)])
  await expect(page.getByLabel('WL', { exact: true })).toHaveValue('2e-10')
  expect(Number(await page.getByLabel('WW', { exact: true }).inputValue())).toBeGreaterThan(0)
  await expect.poll(() => grays(page)).toEqual([128, 128, 128])
  await page.getByLabel('Auto min/max').uncheck()
  await page.getByLabel('WW', { exact: true }).fill('1e-10')
  await expect(page.getByLabel('WW', { exact: true })).toHaveValue('1e-10')
  await expect.poll(() => grays(page)).toEqual([128, 128, 128])
})
