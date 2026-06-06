import { useMemo } from 'react'
import TagTable from './components/TagTable'
import Toolbar from './components/Toolbar'
import { useDicomFile } from './hooks/useDicomFile'

function fileName(path?: string) {
  if (!path) return undefined
  return path.split(/[\\/]/).pop()
}

export default function App() {
  const dicom = useDicomFile()
  const title = useMemo(() => {
    const name = fileName(dicom.filePath)
    return name ? `Taglet - ${name}` : 'Taglet'
  }, [dicom.filePath])

  return (
    <main className="flex h-screen flex-col bg-slate-100 text-slate-900">
      <Toolbar title={title} {...dicom} />
      {dicom.error ? (
        <div className="border-b border-red-200 bg-red-50 px-4 py-2 text-sm text-red-700">
          {dicom.error}
        </div>
      ) : null}
      <section className="min-h-0 flex-1 overflow-hidden p-4">
        <TagTable
          nodes={dicom.nodes}
          onChange={dicom.updateNodeValue}
          onDelete={dicom.deleteNodeByPath}
        />
      </section>
    </main>
  )
}
