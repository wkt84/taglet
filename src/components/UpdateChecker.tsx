import { useEffect, useRef, useState } from 'react'
import { relaunch } from '@tauri-apps/plugin-process'
import { check, type Update } from '@tauri-apps/plugin-updater'

export default function UpdateChecker() {
  const checkedRef = useRef(false)
  const [update, setUpdate] = useState<Update>()
  const [installing, setInstalling] = useState(false)
  const [error, setError] = useState<string>()

  useEffect(() => {
    if (!import.meta.env.PROD || checkedRef.current) return
    checkedRef.current = true

    async function checkForUpdates() {
      try {
        const nextUpdate = await check()
        if (nextUpdate) setUpdate(nextUpdate)
      } catch (error) {
        console.warn('Update check failed', error)
      }
    }

    void checkForUpdates()
  }, [])

  if (!update) return null

  async function installUpdate() {
    if (!update) return
    setInstalling(true)
    setError(undefined)
    try {
      await update.downloadAndInstall()
      await relaunch()
    } catch (error) {
      setError(String(error))
      setInstalling(false)
    }
  }

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-slate-950/50 p-4">
      <div className="w-full max-w-md rounded bg-white shadow-xl">
        <div className="border-b border-slate-200 px-4 py-3">
          <h2 className="text-base font-semibold">アップデートがあります</h2>
        </div>
        <div className="space-y-3 px-4 py-4 text-sm text-slate-700">
          <p>
            Taglet {update.version} が利用できます。今すぐ更新しますか？
          </p>
          {update.body ? (
            <div className="max-h-48 overflow-auto rounded border border-slate-200 bg-slate-50 p-3 text-xs whitespace-pre-wrap">
              {update.body}
            </div>
          ) : null}
          <p className="text-xs text-slate-500">
            更新を開始するとインストーラーが起動する場合があります。作業中のファイルがある場合は、先に保存してください。
          </p>
          {error ? <div className="rounded bg-red-50 p-2 text-xs text-red-700">{error}</div> : null}
        </div>
        <div className="flex justify-end gap-2 border-t border-slate-200 px-4 py-3">
          <button
            className="rounded px-3 py-1.5 text-sm text-slate-600 hover:bg-slate-100 disabled:opacity-50"
            disabled={installing}
            onClick={() => setUpdate(undefined)}
          >
            後で
          </button>
          <button
            className="rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-blue-500 disabled:cursor-not-allowed disabled:opacity-60"
            disabled={installing}
            onClick={() => void installUpdate()}
          >
            {installing ? '更新中...' : '更新して再起動'}
          </button>
        </div>
      </div>
    </div>
  )
}
