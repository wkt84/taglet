import { expect, test, type Page } from '@playwright/test'
import { openMockFiles } from './tauriMock'
import type { RtPlanBevInfo, RtPlanControlPoint } from '../../src/types/dicom'

function controlPoint(index: number, halfWidth: number): RtPlanControlPoint {
  return {
    control_point_index: index, gantry_angle_inherited: false,
    collimator_angle_inherited: false, couch_angle_inherited: false,
    devices: [
      { device_type: 'X', positions: [-halfWidth, halfWidth], inherited: false },
      { device_type: 'Y', positions: [-50, 50], inherited: false },
    ],
  }
}

const info: RtPlanBevInfo = {
  supported: true, modality: 'RTPLAN', beams: [
    { beam_index: 0, beam_number: 1, devices: [], control_points: [controlPoint(0, 100), controlPoint(1, 220)] },
    { beam_index: 1, beam_number: 2, devices: [{ device_type: 'MLCX', leaf_position_boundaries: [-300, 300] }],
      control_points: [controlPoint(0, 80), controlPoint(1, 150)] },
  ],
}

async function openBev(page: Page) {
  await openMockFiles(page)
  // Extend the existing official Tauri mock for the BEV command only.
  await page.evaluate((bevInfo) => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> }
    }).__TAURI_INTERNALS__
    const originalInvoke = internals.invoke
    internals.invoke = async (command, args, options) => command === 'get_rt_plan_bev_info'
      ? structuredClone(bevInfo) : originalInvoke(command, args, options)
  }, info)
  await page.getByRole('button', { name: 'Viewers', exact: true }).click()
  await page.getByRole('button', { name: 'BEV Viewer', exact: true }).click()
  await expect(page.getByText('Loading RT Plan...', { exact: true })).toBeHidden()
  await expect(page.getByRole('img')).toBeVisible()
}

test('control point changes preserve physical scale even when the aperture grows', async ({ page }) => {
  await openBev(page)
  const svg = page.getByRole('img')
  await expect(svg).toContainText('+/- 250 mm')
  const jaw = svg.locator('rect[stroke="#38bdf8"]')
  const height = await jaw.getAttribute('height')
  const width = Number(await jaw.getAttribute('width'))
  const grid = await svg.locator('line').evaluateAll((lines) => lines.map((line) => line.outerHTML))
  await page.locator('select').nth(1).selectOption('1')
  await expect(svg).toContainText('+/- 250 mm')
  await expect(svg).toHaveAttribute('viewBox', '0 0 560 560')
  await expect(jaw).toHaveAttribute('height', height!)
  expect(Number(await jaw.getAttribute('width')) / width).toBeCloseTo(2.2)
  expect(await svg.locator('line').evaluateAll((lines) => lines.map((line) => line.outerHTML))).toEqual(grid)
  await page.locator('select').nth(1).selectOption('0')
  await expect(jaw).toHaveAttribute('height', height!)
})

test('manual zoom and pan survive control point changes; Fit uses the whole beam', async ({ page }) => {
  await openBev(page)
  await page.getByRole('button', { name: '+', exact: true }).click()
  const svg = page.getByRole('img')
  const box = (await svg.boundingBox())!
  const x = box.x + box.width / 2, y = box.y + box.height / 2
  await page.mouse.move(x, y)
  await page.mouse.down()
  await page.mouse.move(x + 40, y + 30, { steps: 4 })
  await page.mouse.up()
  const viewBox = await svg.getAttribute('viewBox')
  expect(viewBox).not.toBe('56 56 448 448')
  await page.locator('select').nth(1).selectOption('1')
  await expect(svg).toHaveAttribute('viewBox', viewBox!)
  await expect(page.getByText('125%', { exact: true })).toBeVisible()
  await expect(svg).toContainText('+/- 250 mm')
  await page.getByRole('button', { name: 'Fit', exact: true }).click()
  await expect(svg).toHaveAttribute('viewBox', '0 0 560 560')
  await expect(page.getByText('100%', { exact: true })).toBeVisible()
  await expect(svg).toContainText('+/- 250 mm')
})

test('selecting another beam fits its full extent, including leaf boundaries', async ({ page }) => {
  await openBev(page)
  await page.getByRole('button', { name: '+', exact: true }).click()
  await page.locator('select').nth(1).selectOption('1')
  await page.locator('select').nth(0).selectOption('1')
  await expect(page.locator('select').nth(1)).toHaveValue('0')
  await expect(page.getByRole('img')).toContainText('+/- 300 mm')
  await expect(page.getByRole('img')).toHaveAttribute('viewBox', '0 0 560 560')
  await expect(page.getByText('100%', { exact: true })).toBeVisible()
  await page.locator('select').nth(1).selectOption('1')
  await expect(page.getByRole('img')).toContainText('+/- 300 mm')
})
