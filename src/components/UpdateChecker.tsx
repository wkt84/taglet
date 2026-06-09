import { useEffect, useRef } from 'react'
import { relaunch } from '@tauri-apps/plugin-process'
import { check } from '@tauri-apps/plugin-updater'

export default function UpdateChecker() {
  const checkedRef = useRef(false)

  useEffect(() => {
    if (!import.meta.env.PROD || checkedRef.current) return
    checkedRef.current = true

    async function checkForUpdates() {
      try {
        const update = await check()
        if (!update) return

        const notes = update.body ? `\n\n${update.body}` : ''
        const shouldInstall = window.confirm(
          `Taglet ${update.version} is available.\n\nInstall the update now?${notes}`,
        )
        if (!shouldInstall) return

        await update.downloadAndInstall()

        if (window.confirm('The update has been installed. Restart Taglet now?')) {
          await relaunch()
        }
      } catch (error) {
        console.warn('Update check failed', error)
      }
    }

    void checkForUpdates()
  }, [])

  return null
}
