import { expect, type Page } from '@playwright/test'
import { files, FILE_A, FILE_B, FILE_C } from './fixtures'
import type { DicomFileContent } from '../../src/types/dicom'

export async function openMockFiles(page: Page, fixtureFiles: Record<string, DicomFileContent> = files) {
  // Inject the official Tauri mocks before React mounts; production code is untouched.
  await page.route('**/src/main.tsx', async (route) => {
    const response = await route.fetch()
    const setup = `
      import { mockIPC, mockWindows } from '/node_modules/@tauri-apps/api/mocks.js';
      mockWindows('main');
      const fixtureFiles = ${JSON.stringify(fixtureFiles)};
      window.__testIpcCalls = [];
      mockIPC((command, args) => {
        window.__testIpcCalls.push({ command, args });
        switch (command) {
          case 'plugin:app|version': return '0.1.13';
          case 'take_launch_file_paths': return [];
          case 'plugin:dialog|open': return ${JSON.stringify([FILE_A, FILE_B, FILE_C])};
          case 'open_dicom_file': {
            const file = fixtureFiles[args.path];
            if (!file) throw new Error('Unknown fixture: ' + args.path);
            return structuredClone(file);
          }
          case 'set_current_dicom_file':
          case 'save_dicom_file': return null;
          case 'validate_value': {
            // SH success/failure only; actual VR rules are tested in Rust.
            const valid = args.vr !== 'SH' || args.value.length <= 16;
            return { valid, message: valid ? undefined : 'SH must be 16 characters or fewer' };
          }
          default: throw new Error('Unmocked Tauri command: ' + command);
        }
      }, { shouldMockEvents: true });
    `
    await route.fulfill({ response, body: setup + await response.text(), contentType: 'application/javascript' })
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('button', { name: 'no-machine.dcm', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'plan-a.dcm', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Taglet - plan-a.dcm' })).toBeVisible()
}

export async function savedNodes(page: Page, filePath: string) {
  // Verify the real frontend's save payload, without writing to disk.
  const previousCount = await page.evaluate((path) => {
    const calls = (window as unknown as { __testIpcCalls: Array<{ command: string; args: { path?: string } }> }).__testIpcCalls
    return calls.filter((call) => call.command === 'save_dicom_file' && call.args.path === path).length
  }, filePath)
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect.poll(() => page.evaluate((path) => {
    const calls = (window as unknown as { __testIpcCalls: Array<{ command: string; args: { path?: string; nodes?: unknown } }> }).__testIpcCalls
    return calls.filter((call) => call.command === 'save_dicom_file' && call.args.path === path).length
  }, filePath)).toBeGreaterThan(previousCount)
  return page.evaluate((path) => {
    const calls = (window as unknown as { __testIpcCalls: Array<{ command: string; args: { path?: string; nodes?: unknown } }> }).__testIpcCalls
    return calls.filter((call) => call.command === 'save_dicom_file' && call.args.path === path).at(-1)!.args.nodes
  }, filePath)
}
