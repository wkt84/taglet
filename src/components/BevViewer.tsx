import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type {
  RtPlanBeam,
  RtPlanBeamLimitingDeviceDefinition,
  RtPlanBevInfo,
  RtPlanControlPoint,
} from '../types/dicom'

type Props = {
  onClose: () => void
}

function valueText(value: unknown) {
  if (value === undefined || value === null || value === '') return '-'
  return String(value)
}

function inheritedText(value: unknown, inherited?: boolean) {
  const text = valueText(value)
  return inherited && text !== '-' ? `${text} (inherited)` : text
}

function beamLabel(beam: RtPlanBeam) {
  const number = beam.beam_number ?? beam.beam_index + 1
  return `Beam ${number}${beam.beam_name ? ` - ${beam.beam_name}` : ''}`
}

function controlPointLabel(controlPoint: RtPlanControlPoint) {
  const index = controlPoint.nominal_index ?? controlPoint.control_point_index
  return `Control Point ${index}`
}

function findDevice(controlPoint: RtPlanControlPoint, names: string[]) {
  return controlPoint.devices.find((device) => names.includes(device.device_type.toUpperCase()))
}

function isMlcType(deviceType: string) {
  return /^MLC[XY]/.test(deviceType.toUpperCase())
}

function mlcAxis(deviceType: string) {
  const normalized = deviceType.toUpperCase()
  if (normalized.startsWith('MLCY')) return 'Y'
  if (normalized.startsWith('MLCX')) return 'X'
  return undefined
}

function findDeviceDefinition(beam: RtPlanBeam, deviceType: string) {
  const normalized = deviceType.toUpperCase()
  return beam.devices.find((device) => device.device_type.toUpperCase() === normalized)
    ?? beam.devices.find((device) => normalized.startsWith(device.device_type.toUpperCase()))
}

function apertureExtent(beam: RtPlanBeam, controlPoint: RtPlanControlPoint) {
  const values = [
    ...controlPoint.devices.flatMap((device) => device.positions),
    ...beam.devices.flatMap((device) => device.leaf_position_boundaries),
  ]
  const maxAbs = Math.max(100, ...values.map((value) => Math.abs(value)))
  return Math.ceil(maxAbs / 50) * 50
}

function pointsToSvg(value: number, extent: number, size: number) {
  return size / 2 + (value / extent) * (size / 2)
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value))
}

function zoomBy(value: number, factor: number) {
  return clamp(value * factor, 0.5, 8)
}

function boundaryPairs(
  definition: RtPlanBeamLimitingDeviceDefinition | undefined,
  pairCount: number,
  fallbackStart: number,
  fallbackEnd: number,
) {
  const boundaries = definition?.leaf_position_boundaries
  if (boundaries && boundaries.length >= pairCount + 1) {
    return Array.from({ length: pairCount }, (_, index) => [
      boundaries[index],
      boundaries[index + 1],
    ])
  }

  const step = (fallbackEnd - fallbackStart) / pairCount
  return Array.from({ length: pairCount }, (_, index) => [
    fallbackStart + index * step,
    fallbackStart + (index + 1) * step,
  ])
}

function BevCanvas({ beam, controlPoint }: { beam: RtPlanBeam; controlPoint: RtPlanControlPoint }) {
  const size = 560
  const dragStartRef = useRef<{ x: number; y: number }>()
  const [zoom, setZoom] = useState(1)
  const [pan, setPan] = useState({ x: 0, y: 0 })
  const extent = apertureExtent(beam, controlPoint)
  const xDevice = findDevice(controlPoint, ['ASYMX', 'X'])
  const yDevice = findDevice(controlPoint, ['ASYMY', 'Y'])
  const mlcDevices = controlPoint.devices.filter((device) => isMlcType(device.device_type))
  const x = xDevice?.positions.length === 2 ? xDevice.positions : [-extent / 2, extent / 2]
  const y = yDevice?.positions.length === 2 ? yDevice.positions : [-extent / 2, extent / 2]
  const left = Math.min(pointsToSvg(x[0], extent, size), pointsToSvg(x[1], extent, size))
  const right = Math.max(pointsToSvg(x[0], extent, size), pointsToSvg(x[1], extent, size))
  const top = Math.min(pointsToSvg(-y[0], extent, size), pointsToSvg(-y[1], extent, size))
  const bottom = Math.max(pointsToSvg(-y[0], extent, size), pointsToSvg(-y[1], extent, size))
  const collimatorAngle = controlPoint.collimator_angle ?? 0
  const svgRotationAngle = -collimatorAngle
  const viewBoxSize = size / zoom
  const viewBoxX = (size - viewBoxSize) / 2 - pan.x
  const viewBoxY = (size - viewBoxSize) / 2 - pan.y

  return (
    <div className="relative h-full w-full bg-slate-950">
      <div className="absolute right-3 top-3 z-10 flex items-center gap-1 rounded bg-slate-900/85 p-1 text-xs text-slate-100 shadow">
        <button className="rounded px-2 py-1 hover:bg-slate-700" onClick={() => setZoom((value) => zoomBy(value, 1.25))}>+</button>
        <button className="rounded px-2 py-1 hover:bg-slate-700" onClick={() => setZoom((value) => zoomBy(value, 0.8))}>-</button>
        <button
          className="rounded px-2 py-1 hover:bg-slate-700"
          onClick={() => {
            setZoom(1)
            setPan({ x: 0, y: 0 })
          }}
        >
          Fit
        </button>
        <span className="px-2 text-slate-300">{Math.round(zoom * 100)}%</span>
      </div>
      <svg
        className="h-full w-full touch-none select-none bg-slate-950"
        viewBox={`${viewBoxX} ${viewBoxY} ${viewBoxSize} ${viewBoxSize}`}
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
          const unitPerPixel = viewBoxSize / Math.max(1, Math.min(bounds.width, bounds.height))
          const dx = event.clientX - dragStartRef.current.x
          const dy = event.clientY - dragStartRef.current.y
          dragStartRef.current = { x: event.clientX, y: event.clientY }
          setPan((value) => ({
            x: value.x + dx * unitPerPixel,
            y: value.y + dy * unitPerPixel,
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
        <defs>
          <pattern id="bev-grid" width="28" height="28" patternUnits="userSpaceOnUse">
            <path d="M 28 0 L 0 0 0 28" fill="none" stroke="#334155" strokeWidth="1" />
          </pattern>
        </defs>
        <rect width={size} height={size} fill="url(#bev-grid)" />
        <line x1={size / 2} y1="0" x2={size / 2} y2={size} stroke="#64748b" strokeWidth="1" />
        <line x1="0" y1={size / 2} x2={size} y2={size / 2} stroke="#64748b" strokeWidth="1" />
        <g transform={`rotate(${svgRotationAngle} ${size / 2} ${size / 2})`}>
          {mlcDevices.map((mlcDevice, deviceIndex) => {
            const normalizedType = mlcDevice.device_type.toUpperCase()
            const axis = mlcAxis(normalizedType)
            const mlcPairs = Math.floor(mlcDevice.positions.length / 2)
            const mlcDefinition = findDeviceDefinition(beam, normalizedType)
            const leafBoundaries = mlcPairs > 0
              ? boundaryPairs(
                  mlcDefinition,
                  mlcPairs,
                  axis === 'Y' ? x[0] : y[0],
                  axis === 'Y' ? x[1] : y[1],
                )
              : []
            const fill = deviceIndex % 2 === 0 ? '#f59e0b66' : '#a855f766'
            if (!axis || mlcPairs === 0) return null

            return Array.from({ length: mlcPairs }, (_, index) => {
              const bank1 = mlcDevice.positions[index]
              const bank2 = mlcDevice.positions[index + mlcPairs]
              const [boundary1, boundary2] = leafBoundaries[index]
              if (axis === 'Y') {
                const leafLeft = clamp(
                  Math.min(pointsToSvg(boundary1, extent, size), pointsToSvg(boundary2, extent, size)),
                  0,
                  size,
                )
                const leafRight = clamp(
                  Math.max(pointsToSvg(boundary1, extent, size), pointsToSvg(boundary2, extent, size)),
                  0,
                  size,
                )
                const apertureTop = clamp(
                  Math.min(pointsToSvg(-bank1, extent, size), pointsToSvg(-bank2, extent, size)),
                  0,
                  size,
                )
                const apertureBottom = clamp(
                  Math.max(pointsToSvg(-bank1, extent, size), pointsToSvg(-bank2, extent, size)),
                  0,
                  size,
                )
                return (
                  <g key={`${normalizedType}-${index}`}>
                    <rect x={leafLeft} y={0} width={Math.max(0, leafRight - leafLeft)} height={Math.max(0, apertureTop)} fill={fill} />
                    <rect x={leafLeft} y={apertureBottom} width={Math.max(0, leafRight - leafLeft)} height={Math.max(0, size - apertureBottom)} fill={fill} />
                  </g>
                )
              }

              const leafTop = clamp(
                Math.min(pointsToSvg(-boundary1, extent, size), pointsToSvg(-boundary2, extent, size)),
                0,
                size,
              )
              const leafBottom = clamp(
                Math.max(pointsToSvg(-boundary1, extent, size), pointsToSvg(-boundary2, extent, size)),
                0,
                size,
              )
              const apertureLeft = clamp(
                Math.min(pointsToSvg(bank1, extent, size), pointsToSvg(bank2, extent, size)),
                0,
                size,
              )
              const apertureRight = clamp(
                Math.max(pointsToSvg(bank1, extent, size), pointsToSvg(bank2, extent, size)),
                0,
                size,
              )
              return (
                <g key={`${normalizedType}-${index}`}>
                  <rect x={0} y={leafTop} width={Math.max(0, apertureLeft)} height={Math.max(0, leafBottom - leafTop)} fill={fill} />
                  <rect x={apertureRight} y={leafTop} width={Math.max(0, size - apertureRight)} height={Math.max(0, leafBottom - leafTop)} fill={fill} />
                </g>
              )
            })
          })}
          <rect
            x={left}
            y={top}
            width={Math.max(1, right - left)}
            height={Math.max(1, bottom - top)}
            fill="#38bdf82a"
            stroke="#38bdf8"
            strokeWidth="2"
          />
        </g>
        {collimatorAngle ? (
          <text x="12" y="44" fill="#cbd5e1" fontSize="13">
            Collimator {collimatorAngle} deg
          </text>
        ) : null}
        <text x="12" y="24" fill="#cbd5e1" fontSize="13">
          +/- {extent} mm
        </text>
      </svg>
    </div>
  )
}

export default function BevViewer({ onClose }: Props) {
  const [info, setInfo] = useState<RtPlanBevInfo>()
  const [beamIndex, setBeamIndex] = useState(0)
  const [controlPointIndex, setControlPointIndex] = useState(0)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string>()

  useEffect(() => {
    let canceled = false
    setLoading(true)
    setError(undefined)
    invoke<RtPlanBevInfo>('get_rt_plan_bev_info')
      .then((value) => {
        if (!canceled) {
          setInfo(value)
          setBeamIndex(0)
          setControlPointIndex(0)
        }
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

  const selectedBeam = info?.beams[beamIndex]
  const selectedControlPoint = selectedBeam?.control_points[controlPointIndex]
  const deviceSummary = useMemo(
    () => selectedControlPoint?.devices.map((device) => (
      `${device.device_type}: ${device.positions.length}${device.inherited ? ' inherited' : ''}`
    )).join(', '),
    [selectedControlPoint],
  )

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4">
      <div className="flex h-[82vh] w-full max-w-6xl flex-col rounded bg-white shadow-xl">
        <div className="flex items-center border-b border-slate-200 px-4 py-3">
          <h2 className="flex-1 text-base font-semibold">BEV Viewer</h2>
          <button className="rounded px-2 py-1 text-sm text-slate-600 hover:bg-slate-100" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="grid min-h-0 flex-1 grid-cols-[1fr_340px] gap-4 p-4">
          <section className="flex min-h-0 items-center justify-center overflow-hidden rounded border border-slate-300 bg-slate-950 text-slate-300">
            {loading ? (
              <span className="text-sm">Loading RT Plan...</span>
            ) : error ? (
              <span className="max-w-md text-sm text-red-300">{error}</span>
            ) : info?.supported && selectedControlPoint ? (
              <BevCanvas beam={selectedBeam} controlPoint={selectedControlPoint} />
            ) : (
              <div className="max-w-md text-center text-sm">
                <div className="font-medium">BEV is not available.</div>
                <div className="mt-2 text-slate-400">{info?.unsupported_reason ?? 'Unknown reason'}</div>
              </div>
            )}
          </section>

          <aside className="min-h-0 overflow-auto rounded border border-slate-200">
            <div className="border-b border-slate-200 bg-slate-50 px-3 py-2 text-sm font-semibold">
              Plan Controls
            </div>
            <div className="space-y-4 p-3 text-sm">
              <label className="block text-xs text-slate-600">
                Beam
                <select
                  className="mt-1 w-full rounded border border-slate-300 px-2 py-1 text-sm"
                  disabled={!info?.supported}
                  value={beamIndex}
                  onChange={(event) => {
                    setBeamIndex(Number(event.target.value))
                    setControlPointIndex(0)
                  }}
                >
                  {info?.beams.map((beam, index) => (
                    <option key={beam.beam_index} value={index}>
                      {beamLabel(beam)}
                    </option>
                  ))}
                </select>
              </label>

              <label className="block text-xs text-slate-600">
                Control Point
                <select
                  className="mt-1 w-full rounded border border-slate-300 px-2 py-1 text-sm"
                  disabled={!selectedBeam}
                  value={controlPointIndex}
                  onChange={(event) => setControlPointIndex(Number(event.target.value))}
                >
                  {selectedBeam?.control_points.map((controlPoint, index) => (
                    <option key={controlPoint.control_point_index} value={index}>
                      {controlPointLabel(controlPoint)}
                    </option>
                  ))}
                </select>
              </label>

              <section>
                <div className="mb-2 font-semibold">Geometry</div>
                <dl className="grid grid-cols-[130px_1fr] gap-x-3 gap-y-2">
                  <dt className="text-slate-500">Modality</dt>
                  <dd>{valueText(info?.modality)}</dd>
                  <dt className="text-slate-500">Beam Number</dt>
                  <dd>{valueText(selectedBeam?.beam_number)}</dd>
                  <dt className="text-slate-500">Beam Name</dt>
                  <dd>{valueText(selectedBeam?.beam_name)}</dd>
                  <dt className="text-slate-500">Gantry</dt>
                  <dd>
                    {inheritedText(
                      selectedControlPoint?.gantry_angle,
                      selectedControlPoint?.gantry_angle_inherited,
                    )}
                  </dd>
                  <dt className="text-slate-500">Collimator</dt>
                  <dd>
                    {inheritedText(
                      selectedControlPoint?.collimator_angle,
                      selectedControlPoint?.collimator_angle_inherited,
                    )}
                  </dd>
                  <dt className="text-slate-500">Couch</dt>
                  <dd>
                    {inheritedText(
                      selectedControlPoint?.couch_angle,
                      selectedControlPoint?.couch_angle_inherited,
                    )}
                  </dd>
                  <dt className="text-slate-500">Devices</dt>
                  <dd>{valueText(deviceSummary)}</dd>
                </dl>
              </section>
            </div>
          </aside>
        </div>
      </div>
    </div>
  )
}
