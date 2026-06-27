type Props = {
  label: string
  onCancel: () => void
  onConfirm: () => void
}

export default function ConfirmDeleteDialog({ label, onCancel, onConfirm }: Props) {
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-slate-950/50 p-4">
      <div className="w-full max-w-md rounded bg-white shadow-xl">
        <div className="border-b border-slate-200 px-4 py-3">
          <h2 className="text-base font-semibold">タグを削除しますか？</h2>
        </div>
        <div className="space-y-3 px-4 py-4 text-sm text-slate-700">
          <p>次のタグを削除します。</p>
          <div className="rounded border border-red-100 bg-red-50 px-3 py-2 font-mono text-xs text-red-900">
            {label}
          </div>
          <p className="text-xs text-slate-500">
            削除後は `Save` するまで元のファイルには書き込まれません。
          </p>
        </div>
        <div className="flex justify-end gap-2 border-t border-slate-200 px-4 py-3">
          <button className="rounded px-3 py-1.5 text-sm text-slate-600 hover:bg-slate-100" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="rounded bg-red-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-red-500"
            onClick={onConfirm}
          >
            Delete
          </button>
        </div>
      </div>
    </div>
  )
}
