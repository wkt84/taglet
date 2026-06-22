type Props = {
  version?: string
  onClose: () => void
}

export default function AboutDialog({ version, onClose }: Props) {
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-slate-950/50 p-4">
      <div className="w-full max-w-md rounded bg-white shadow-xl">
        <div className="flex items-center border-b border-slate-200 px-4 py-3">
          <h2 className="flex-1 text-base font-semibold">About Taglet</h2>
          <button className="rounded px-2 py-1 text-sm text-slate-600 hover:bg-slate-100" onClick={onClose}>
            Close
          </button>
        </div>
        <div className="space-y-3 px-4 py-4 text-sm text-slate-700">
          <div>
            <div className="text-lg font-semibold text-slate-950">Taglet</div>
            <div className="mt-1 text-slate-500">DICOM tag editor and lightweight viewer</div>
          </div>
          <dl className="grid grid-cols-[100px_1fr] gap-x-3 gap-y-2">
            <dt className="text-slate-500">Version</dt>
            <dd className="font-mono">{version ?? '-'}</dd>
            <dt className="text-slate-500">License</dt>
            <dd>MIT</dd>
            <dt className="text-slate-500">Updates</dt>
            <dd>GitHub Releases から起動時に確認します。</dd>
          </dl>
          <p className="rounded border border-amber-200 bg-amber-50 p-3 text-xs text-amber-900">
            Taglet は開発中のソフトウェアです。臨床判断や診療行為に直接使用しないでください。
          </p>
        </div>
      </div>
    </div>
  )
}
