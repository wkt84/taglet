import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type {
  RtStructBounds,
  RtStructContour,
  RtStructInfo,
  RtStructSliceContours,
} from '../types/dicom'

type Props = {
  onClose: () => void
}

function valueText(value: unknown) {
  if (value === undefined || value === null || value === '') return '-'
  return String(value)
}

function formatNumber(value: number) {
  if (!Number.isFinite(value)) return '0'
  if (Math.abs(value) >= 100) return value.toFixed(1)
  return value.toFixed(3).replace(/\.?0+$/, '')
}

function colorCss(color: [number, number, number] | null | undefined, fallbackIndex = 0, opacity?: number) {
  if (color) {
    return opacity === undefined
      ? `rgb(${color[0]} ${color[1]} ${color[2]})`
      : `rgb(${color[0]} ${color[1]} ${color[2]} / ${opacity})`
  }
  const fallback = [
    '#38bdf8',
    '#f97316',
    '#a78bfa',
    '#34d399',
    '#f472b6',
    '#facc15',
  ]
  const value = fallback[fallbackIndex % fallback.length]
  if (opacity === undefined) return value
  const normalizedOpacity = Math.round(Math.min(1, Math.max(0, opacity)) * 255)
    .toString(16)
    .padStart(2, '0')
  return `${value}${normalizedOpacity}`
}

function roiLabel(roiNumber: number, name?: string | null) {
  return name ? `${roiNumber}: ${name}` : `ROI ${roiNumber}`
}

function boundsCenter(bounds: RtStructBounds) {
  return {
    x: (bounds.min_x + bounds.max_x) / 2,
    y: (bounds.min_y + bounds.max_y) / 2,
  }
}

function boundsExtent(bounds: RtStructBounds) {
  const width = Math.max(1, bounds.max_x - bounds.min_x)
  const height = Math.max(1, bounds.max_y - bounds.min_y)
  return Math.max(width, height) * 1.12
}

function contourPath(contour: RtStructContour) {
  if (contour.points.length === 0) return ''
  const [first, ...rest] = contour.points
  const parts = [`M ${first.x} ${-first.y}`]
  for (const point of rest) {
    parts.push(`L ${point.x} ${-point.y}`)
  }
  if (contour.geometric_type === 'CLOSED_PLANAR') {
    parts.push('Z')
  }
  return parts.join(' ')
}

function zoomBy(value: number, factor: number) {
  return Math.min(12, Math.max(0.5, value * factor))
}

export default function RtStructViewer({ onClose }: Props) {
  const dragStartRef = useRef<{ x: number; y: number }>()
  const [info, setInfo] = useState<RtStructInfo>()
  const [sliceContours, setSliceContours] = useState<RtStructSliceContours>()
  const [sliceIndex, setSliceIndex] = useState(0)
  const [selectedRois, setSelectedRois] = useState<number[]>([])
  const [zoom, setZoom] = useState(1)
  const [pan, setPan] = useState({ x: 0, y: 0 })
  const [loading, setLoading] = useState(true)
  const [contoursLoading, setContoursLoading] = useState(false)
  const [error, setError] = useState<string>()

  useEffect(() => {
    let canceled = false
    setLoading(true)
    setError(undefined)

    invoke<RtStructInfo>('get_rt_struct_info')
      .then((value) => {
        if (canceled) return
        setInfo(value)
        setSliceIndex(0)
        setSelectedRois(value.rois.map((roi) => roi.roi_number))
        setZoom(1)
        setPan({ x: 0, y: 0 })
      })
      .catch((error) => {
        if (!canceled) setError(String(error))
      })
      .finally(() => {
        if (!canceled) setLoading(false)
      })

    return () => {
      canceled = true
    }
  }, [])

  const selectedSlice = info?.slices[sliceIndex]
  useEffect(() => {
    if (!info?.supported || !selectedSlice) return

    let canceled = false
    setContoursLoading(true)
    setError(undefined)

    invoke<RtStructSliceContours>('get_rt_struct_slice_contours', {
      z: selectedSlice.z,
      roiNumbers: selectedRois,
    })
      .then((value) => {
        if (!canceled) setSliceContours(value)
      })
      .catch((error) => {
        if (!canceled) setError(String(error))
      })
      .finally(() => {
        if (!canceled) setContoursLoading(false)
      })

    return () => {
      canceled = true
    }
  }, [info?.supported, selectedRois, selectedSlice?.z])

  const roiColorByNumber = useMemo(
    () => new Map(info?.rois.map((roi, index) => [roi.roi_number, colorCss(roi.color, index)]) ?? []),
    [info?.rois],
  )
  const bounds = info?.bounds
  const center = bounds ? boundsCenter(bounds) : { x: 0, y: 0 }
  const extent = bounds ? boundsExtent(bounds) : 200
  const viewSize = extent / zoom
  const viewBox = `${center.x - viewSize / 2 - pan.x} ${-center.y - viewSize / 2 - pan.y} ${viewSize} ${viewSize}`
  const strokeWidth = Math.max(viewSize / 360, 0.2)

  function fit() {
    setZoom(1)
    setPan({ x: 0, y: 0 })
  }

  function toggleRoi(roiNumber: number) {
    setSelectedRois((current) => (
      current.includes(roiNumber)
        ? current.filter((value) => value !== roiNumber)
        : [...current, roiNumber].sort((left, right) => left - right)
    ))
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4">
      <div className="flex h-[86vh] w-full max-w-6xl flex-col rounded bg-white shadow-xl">
        <div className="flex items-center border-b border-slate-200 px-4 py-3">
          <h2 className="flex-1 text-base font-semibold">RT Structure Viewer</h2>
          <button className="rounded px-2 py-1 text-sm text-slate-600 hover:bg-slate-100" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="grid min-h-0 flex-1 grid-cols-[1fr_340px] gap-4 p-4">
          <section className="relative flex min-h-0 items-center justify-center overflow-hidden rounded border border-slate-300 bg-slate-950 text-slate-300">
            {loading ? (
              <span className="text-sm">Loading RT Structure Set...</span>
            ) : error ? (
              <span className="max-w-md text-sm text-red-300">{error}</span>
            ) : info?.supported && bounds ? (
              <>
                <div className="absolute right-3 top-3 z-10 flex items-center gap-1 rounded bg-slate-900/85 p-1 text-xs text-slate-100 shadow">
                  <button className="rounded px-2 py-1 hover:bg-slate-700" onClick={() => setZoom((value) => zoomBy(value, 1.25))}>+</button>
                  <button className="rounded px-2 py-1 hover:bg-slate-700" onClick={() => setZoom((value) => zoomBy(value, 0.8))}>-</button>
                  <button className="rounded px-2 py-1 hover:bg-slate-700" onClick={fit}>Fit</button>
                  <span className="px-2 text-slate-300">{Math.round(zoom * 100)}%</span>
                </div>
                {contoursLoading ? (
                  <div className="absolute left-3 top-3 z-10 rounded bg-slate-900/80 px-2 py-1 text-xs text-slate-300">
                    Loading contours...
                  </div>
                ) : null}
                <svg
                  className="h-full w-full touch-none select-none bg-slate-950"
                  viewBox={viewBox}
                  role="img"
                  onWheel={(event) => {
                    event.preventDefault()
                    setZoom((value) => zoomBy(value, event.deltaY < 0 ? 1.12 : 1 / 1.12))
                  }}
                  onPointerDown={(event) => {
                    dragStartRef.current = { x: event.clientX, y: event.clientY }
                    event.currentTarget.setPointerCapture(event.pointerId)
                  }}
                  onPointerMove={(event) => {
                    if (!dragStartRef.current) return
                    const bounds = event.currentTarget.getBoundingClientRect()
                    const unitPerPixel = viewSize / Math.max(1, Math.min(bounds.width, bounds.height))
                    const dx = event.clientX - dragStartRef.current.x
                    const dy = event.clientY - dragStartRef.current.y
                    dragStartRef.current = { x: event.clientX, y: event.clientY }
                    setPan((value) => ({
                      x: value.x - dx * unitPerPixel,
                      y: value.y - dy * unitPerPixel,
                    }))
                  }}
                  onPointerUp={(event) => {
                    dragStartRef.current = undefined
                    event.currentTarget.releasePointerCapture(event.pointerId)
                  }}
                  onPointerCancel={() => {
                    dragStartRef.current = undefined
                  }}
                >
                  <line x1={bounds.min_x} y1={0} x2={bounds.max_x} y2={0} stroke="#334155" strokeWidth={strokeWidth} />
                  <line x1={0} y1={-bounds.min_y} x2={0} y2={-bounds.max_y} stroke="#334155" strokeWidth={strokeWidth} />
                  {sliceContours?.contours.map((contour, index) => (
                    <path
                      key={`${contour.roi_number}-${index}`}
                      d={contourPath(contour)}
                      fill={contour.geometric_type === 'CLOSED_PLANAR' ? colorCss(contour.color, index, 0.12) : 'none'}
                      stroke={roiColorByNumber.get(contour.roi_number) ?? colorCss(contour.color, index)}
                      strokeWidth={strokeWidth * 1.8}
                      vectorEffect="non-scaling-stroke"
                    />
                  ))}
                </svg>
              </>
            ) : (
              <div className="max-w-md text-center text-sm">
                <div className="font-medium">RT Structure preview is not available.</div>
                <div className="mt-2 text-slate-400">{info?.unsupported_reason ?? 'Unknown reason'}</div>
              </div>
            )}
          </section>

          <aside className="min-h-0 overflow-auto rounded border border-slate-200">
            <div className="border-b border-slate-200 bg-slate-50 px-3 py-2 text-sm font-semibold">
              Structure Controls
            </div>
            <div className="space-y-4 p-3 text-sm">
              <section>
                <div className="mb-2 font-semibold">Dataset</div>
                <dl className="grid grid-cols-[110px_1fr] gap-x-3 gap-y-2">
                  <dt className="text-slate-500">Modality</dt>
                  <dd>{valueText(info?.modality)}</dd>
                  <dt className="text-slate-500">Label</dt>
                  <dd>{valueText(info?.structure_set_label)}</dd>
                  <dt className="text-slate-500">ROIs</dt>
                  <dd>{info?.rois.length ?? 0}</dd>
                  <dt className="text-slate-500">Slices</dt>
                  <dd>{info?.slices.length ?? 0}</dd>
                </dl>
              </section>

              <section className="space-y-2">
                <div className="font-semibold">Slice</div>
                <input
                  className="w-full"
                  type="range"
                  min={0}
                  max={Math.max(0, (info?.slices.length ?? 1) - 1)}
                  value={sliceIndex}
                  disabled={!info?.supported || !info.slices.length}
                  onChange={(event) => setSliceIndex(Number(event.target.value))}
                />
                <select
                  className="w-full rounded border border-slate-300 px-2 py-1 text-sm"
                  disabled={!info?.supported || !info.slices.length}
                  value={sliceIndex}
                  onChange={(event) => setSliceIndex(Number(event.target.value))}
                >
                  {info?.slices.map((slice, index) => (
                    <option key={`${slice.z}-${index}`} value={index}>
                      {index + 1}: z={formatNumber(slice.z)} ({slice.contour_count})
                    </option>
                  ))}
                </select>
                <div className="text-xs text-slate-500">
                  Current z: {selectedSlice ? formatNumber(selectedSlice.z) : '-'}
                </div>
              </section>

              <section className="space-y-2">
                <div className="flex items-center gap-2">
                  <div className="flex-1 font-semibold">ROIs</div>
                  <button
                    className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
                    disabled={!info}
                    onClick={() => setSelectedRois(info?.rois.map((roi) => roi.roi_number) ?? [])}
                  >
                    All
                  </button>
                  <button
                    className="rounded border border-slate-300 px-2 py-1 text-xs hover:bg-slate-50"
                    disabled={!info}
                    onClick={() => setSelectedRois([])}
                  >
                    None
                  </button>
                </div>
                <div className="max-h-72 space-y-1 overflow-auto pr-1">
                  {info?.rois.map((roi, index) => (
                    <label key={roi.roi_number} className="flex items-center gap-2 rounded px-1 py-1 text-xs hover:bg-slate-50">
                      <input
                        type="checkbox"
                        checked={selectedRois.includes(roi.roi_number)}
                        onChange={() => toggleRoi(roi.roi_number)}
                      />
                      <span
                        className="h-3 w-3 rounded-sm border border-slate-300"
                        style={{ backgroundColor: colorCss(roi.color, index) }}
                      />
                      <span className="min-w-0 flex-1 truncate" title={roiLabel(roi.roi_number, roi.name)}>
                        {roiLabel(roi.roi_number, roi.name)}
                      </span>
                      <span className="font-mono text-slate-500">{roi.contour_count}</span>
                    </label>
                  ))}
                </div>
              </section>
            </div>
          </aside>
        </div>
      </div>
    </div>
  )
}
