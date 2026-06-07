import { useEffect, useMemo, useRef, useState, type MouseEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { DicomFramePixels, DicomImageInfo } from '../types/dicom'

type Props = {
  onClose: () => void
}

function valueText(value: unknown) {
  if (value === undefined || value === null || value === '') return '-'
  if (Array.isArray(value)) return value.length === 0 ? '-' : value.join(', ')
  return String(value)
}

function formatWindowNumber(value: number) {
  if (!Number.isFinite(value)) return '0'
  if (Math.abs(value) >= 1000) return value.toFixed(0)
  if (Math.abs(value) >= 10) return value.toFixed(2)
  return value.toPrecision(4).replace(/\.?0+$/, '')
}

function base64ToBytes(value: string) {
  const binary = window.atob(value)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index)
  }
  return bytes
}

function decodeFrameValues(frame: DicomFramePixels) {
  const pixelBytes = base64ToBytes(frame.pixel_base64)
  const sampleCount = frame.width * frame.height
  const values = new Float64Array(sampleCount)
  const signed = frame.pixel_representation === 1
  const view = new DataView(pixelBytes.buffer, pixelBytes.byteOffset, pixelBytes.byteLength)

  for (let index = 0; index < sampleCount; index += 1) {
    let stored = 0
    if (frame.bits_allocated === 8) {
      stored = signed ? view.getInt8(index) : view.getUint8(index)
    } else {
      const bytesPerSample = frame.bits_allocated / 8
      const offset = index * bytesPerSample
      if (frame.bits_allocated === 16) {
        stored = signed ? view.getInt16(offset, true) : view.getUint16(offset, true)
      } else {
        stored = signed ? view.getInt32(offset, true) : view.getUint32(offset, true)
      }
    }

    values[index] = stored * frame.rescale_slope + frame.rescale_intercept
  }

  return values
}

function buildHistogram(values: Float64Array | undefined, min: number, max: number, binCount = 96) {
  if (!values || values.length === 0 || max <= min) return []
  const bins = Array.from({ length: binCount }, () => 0)
  const scale = (binCount - 1) / (max - min)
  const stride = Math.max(1, Math.floor(values.length / 250_000))

  for (let index = 0; index < values.length; index += stride) {
    const bin = Math.min(binCount - 1, Math.max(0, Math.floor((values[index] - min) * scale)))
    bins[bin] += 1
  }

  const peak = Math.max(...bins, 1)
  return bins.map((count) => count / peak)
}

function renderFrameToRgba(
  frame: DicomFramePixels,
  values: Float64Array,
  rgba: Uint8ClampedArray<ArrayBuffer>,
  windowCenter: number,
  windowWidth: number,
) {
  const sampleCount = frame.width * frame.height
  const invert = frame.photometric_interpretation === 'MONOCHROME1'
  const low = windowCenter - windowWidth / 2

  for (let index = 0; index < sampleCount; index += 1) {
    const value = values[index]
    const normalized = Math.min(1, Math.max(0, (value - low) / windowWidth))
    const gray = Math.round((invert ? 1 - normalized : normalized) * 255)
    const rgbaOffset = index * 4
    rgba[rgbaOffset] = gray
    rgba[rgbaOffset + 1] = gray
    rgba[rgbaOffset + 2] = gray
    rgba[rgbaOffset + 3] = 255
  }

  return rgba
}

export default function ImageViewer({ onClose }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const viewportRef = useRef<HTMLDivElement>(null)
  const histogramTrackRef = useRef<HTMLDivElement>(null)
  const rgbaBufferRef = useRef<Uint8ClampedArray<ArrayBuffer>>()
  const dragRef = useRef<{ x: number; y: number; panX: number; panY: number }>()
  const histogramDragRef = useRef<'low' | 'high' | 'window'>()
  const pendingWindowBoundsRef = useRef<{ low: number; high: number }>()
  const windowUpdateFrameRef = useRef<number>()
  const [imageInfo, setImageInfo] = useState<DicomImageInfo>()
  const [framePixels, setFramePixels] = useState<DicomFramePixels>()
  const [frameIndex, setFrameIndex] = useState(0)
  const [windowCenter, setWindowCenter] = useState<number>()
  const [windowWidth, setWindowWidth] = useState<number>()
  const [useAutoWindow, setUseAutoWindow] = useState(false)
  const [zoom, setZoom] = useState(1)
  const [pan, setPan] = useState({ x: 0, y: 0 })
  const [error, setError] = useState<string>()
  const [loading, setLoading] = useState(true)
  const [frameLoading, setFrameLoading] = useState(false)

  const decodedValues = useMemo(() => {
    if (!framePixels) return undefined
    return decodeFrameValues(framePixels)
  }, [framePixels])

  const histogramMin = framePixels?.min_value ?? 0
  const histogramMax = framePixels?.max_value ?? 1
  const histogramRange = Math.max(0, histogramMax - histogramMin)
  const minWindowWidth = Math.max(histogramRange / 4096, 1e-6)
  const windowStep = Math.max(histogramRange / 2048, 1e-6)
  const effectiveWindowCenter = useAutoWindow && framePixels
    ? (framePixels.min_value + framePixels.max_value) / 2
    : (windowCenter ?? 0)
  const effectiveWindowWidth = useAutoWindow && framePixels
    ? Math.max(minWindowWidth, framePixels.max_value - framePixels.min_value)
    : Math.max(minWindowWidth, windowWidth ?? minWindowWidth)
  const windowLow = effectiveWindowCenter - effectiveWindowWidth / 2
  const windowHigh = effectiveWindowCenter + effectiveWindowWidth / 2
  const histogram = useMemo(
    () => buildHistogram(decodedValues, histogramMin, histogramMax),
    [decodedValues, histogramMin, histogramMax],
  )

  useEffect(() => {
    let canceled = false
    setLoading(true)
    setError(undefined)

    invoke<DicomImageInfo>('get_dicom_image_info')
      .then((info) => {
        if (!canceled) {
          setImageInfo(info)
          if (info.window_center[0] !== undefined && info.window_width[0] !== undefined) {
            setWindowCenter(info.window_center[0])
            setWindowWidth(info.window_width[0])
          } else {
            setUseAutoWindow(true)
          }
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

  useEffect(() => {
    if (!imageInfo?.supported) return

    let canceled = false
    setFrameLoading(true)
    setError(undefined)

    invoke<DicomFramePixels>('get_dicom_frame_pixels', {
      frameIndex,
    })
      .then((frame) => {
        if (!canceled) {
          setFramePixels(frame)
          if (useAutoWindow) {
            const frameRange = Math.max(0, frame.max_value - frame.min_value)
            const frameMinWindowWidth = Math.max(frameRange / 4096, 1e-6)
            setWindowCenter((frame.min_value + frame.max_value) / 2)
            setWindowWidth(Math.max(frameMinWindowWidth, frameRange))
          }
        }
      })
      .catch((error) => {
        if (!canceled) setError(String(error))
      })
      .finally(() => {
        if (!canceled) setFrameLoading(false)
      })

    return () => {
      canceled = true
    }
  }, [frameIndex, imageInfo?.supported])

  useEffect(() => {
    return () => {
      if (windowUpdateFrameRef.current !== undefined) {
        window.cancelAnimationFrame(windowUpdateFrameRef.current)
      }
    }
  }, [])

  useEffect(() => {
    if (!framePixels || !decodedValues || !canvasRef.current) return

    const canvas = canvasRef.current
    canvas.width = framePixels.width
    canvas.height = framePixels.height
    const context = canvas.getContext('2d')
    if (!context) return

    const rgbaLength = framePixels.width * framePixels.height * 4
    if (rgbaBufferRef.current?.length !== rgbaLength) {
      rgbaBufferRef.current = new Uint8ClampedArray(new ArrayBuffer(rgbaLength))
    }

    const imageData = new ImageData(
      renderFrameToRgba(
        framePixels,
        decodedValues,
        rgbaBufferRef.current,
        effectiveWindowCenter,
        effectiveWindowWidth,
      ),
      framePixels.width,
      framePixels.height,
    )
    context.putImageData(imageData, 0, 0)
  }, [decodedValues, effectiveWindowCenter, effectiveWindowWidth, framePixels])

  useEffect(() => {
    if (!framePixels || !viewportRef.current) return

    const viewport = viewportRef.current
    const fitZoom = Math.min(
      viewport.clientWidth / framePixels.width,
      viewport.clientHeight / framePixels.height,
      1,
    )
    setZoom(Number.isFinite(fitZoom) && fitZoom > 0 ? fitZoom : 1)
    setPan({ x: 0, y: 0 })
  }, [framePixels?.width, framePixels?.height])

  function fitImage() {
    if (!framePixels || !viewportRef.current) return
    const viewport = viewportRef.current
    const fitZoom = Math.min(
      viewport.clientWidth / framePixels.width,
      viewport.clientHeight / framePixels.height,
      1,
    )
    setZoom(Number.isFinite(fitZoom) && fitZoom > 0 ? fitZoom : 1)
    setPan({ x: 0, y: 0 })
  }

  function resetWindow() {
    if (imageInfo?.window_center[0] !== undefined && imageInfo.window_width[0] !== undefined) {
      setWindowCenter(imageInfo.window_center[0])
      setWindowWidth(imageInfo.window_width[0])
      setUseAutoWindow(false)
    } else {
      setAutoWindow(true)
    }
  }

  function setAutoWindow(enabled: boolean) {
    setUseAutoWindow(enabled)
    if (enabled && framePixels) {
      const frameRange = Math.max(0, framePixels.max_value - framePixels.min_value)
      const frameMinWindowWidth = Math.max(frameRange / 4096, 1e-6)
      setWindowCenter((framePixels.min_value + framePixels.max_value) / 2)
      setWindowWidth(Math.max(frameMinWindowWidth, frameRange))
    }
  }

  function setWindowBounds(low: number, high: number) {
    const nextLow = Math.min(low, high - minWindowWidth)
    const nextHigh = Math.max(high, nextLow + minWindowWidth)
    setWindowCenter((nextLow + nextHigh) / 2)
    setWindowWidth(nextHigh - nextLow)
    setUseAutoWindow(false)
  }

  function scheduleWindowBounds(low: number, high: number) {
    pendingWindowBoundsRef.current = { low, high }
    if (windowUpdateFrameRef.current !== undefined) return

    windowUpdateFrameRef.current = window.requestAnimationFrame(() => {
      windowUpdateFrameRef.current = undefined
      const bounds = pendingWindowBoundsRef.current
      pendingWindowBoundsRef.current = undefined
      if (bounds) setWindowBounds(bounds.low, bounds.high)
    })
  }

  function valueToHistogramPercent(value: number) {
    if (histogramMax <= histogramMin) return 50
    const normalized = (value - histogramMin) / (histogramMax - histogramMin)
    return Math.min(100, Math.max(0, 100 - normalized * 100))
  }

  function histogramEventValue(event: MouseEvent<HTMLElement>) {
    const rect = histogramTrackRef.current?.getBoundingClientRect()
    if (!rect) return histogramMin
    const normalized = Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height))
    return histogramMax - normalized * (histogramMax - histogramMin)
  }

  function updateHistogramWindow(event: MouseEvent<HTMLElement>) {
    if (!histogramDragRef.current || !framePixels) return
    const value = histogramEventValue(event)
    const clamped = Math.min(histogramMax, Math.max(histogramMin, value))

    if (histogramDragRef.current === 'low') {
      scheduleWindowBounds(clamped, Math.max(clamped + minWindowWidth, windowHigh))
    } else if (histogramDragRef.current === 'high') {
      scheduleWindowBounds(Math.min(windowLow, clamped - minWindowWidth), clamped)
    } else {
      const width = Math.max(minWindowWidth, windowHigh - windowLow)
      const low = Math.min(histogramMax - width, Math.max(histogramMin, clamped - width / 2))
      scheduleWindowBounds(low, low + width)
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4">
      <div className="flex h-[82vh] w-full max-w-5xl flex-col rounded bg-white shadow-xl">
        <div className="flex items-center border-b border-slate-200 px-4 py-3">
          <h2 className="flex-1 text-base font-semibold">Image Viewer</h2>
          <button className="rounded px-2 py-1 text-sm text-slate-600 hover:bg-slate-100" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="grid min-h-0 flex-1 grid-cols-[1fr_340px] gap-4 p-4">
          <section className="flex min-h-0 items-center justify-center rounded border border-slate-300 bg-slate-950 text-slate-300">
            {loading ? (
              <span className="text-sm">Loading image metadata...</span>
            ) : error ? (
              <span className="max-w-md text-sm text-red-300">{error}</span>
            ) : imageInfo?.supported ? (
              <div className="flex h-full w-full flex-col items-center justify-center gap-3 p-3">
                <div className="flex min-h-0 w-full flex-1 gap-3">
                  <div
                    ref={viewportRef}
                    className="relative min-h-0 flex-1 cursor-grab overflow-hidden active:cursor-grabbing"
                    onMouseDown={(event) => {
                      dragRef.current = {
                        x: event.clientX,
                        y: event.clientY,
                        panX: pan.x,
                        panY: pan.y,
                      }
                    }}
                    onMouseMove={(event) => {
                      if (!dragRef.current) return
                      setPan({
                        x: dragRef.current.panX + event.clientX - dragRef.current.x,
                        y: dragRef.current.panY + event.clientY - dragRef.current.y,
                      })
                    }}
                    onMouseUp={() => {
                      dragRef.current = undefined
                    }}
                    onMouseLeave={() => {
                      dragRef.current = undefined
                    }}
                    onWheel={(event) => {
                      event.preventDefault()
                      const factor = event.deltaY < 0 ? 1.1 : 0.9
                      setZoom((current) => Math.min(8, Math.max(0.05, current * factor)))
                    }}
                  >
                    <canvas
                      ref={canvasRef}
                      className="absolute left-1/2 top-1/2 bg-black [image-rendering:auto]"
                      style={{
                        width: framePixels ? `${framePixels.width * zoom}px` : undefined,
                        height: framePixels ? `${framePixels.height * zoom}px` : undefined,
                        transform: `translate(calc(-50% + ${pan.x}px), calc(-50% + ${pan.y}px))`,
                      }}
                    />
                    {frameLoading ? (
                      <div className="absolute left-3 top-3 rounded bg-slate-900/80 px-2 py-1 text-xs text-slate-300">
                        Loading frame...
                      </div>
                    ) : null}
                  </div>

                  <div className="flex w-20 flex-col items-center gap-2 text-[10px] text-slate-400">
                    <div className="font-mono">{formatWindowNumber(histogramMax)}</div>
                    <div
                      ref={histogramTrackRef}
                      className="relative min-h-0 w-14 flex-1 cursor-ns-resize rounded border border-slate-600 bg-slate-900"
                      onMouseDown={(event) => {
                        histogramDragRef.current = 'window'
                        updateHistogramWindow(event)
                      }}
                      onMouseMove={updateHistogramWindow}
                      onMouseUp={() => {
                        histogramDragRef.current = undefined
                      }}
                      onMouseLeave={() => {
                        histogramDragRef.current = undefined
                      }}
                    >
                      {histogram.map((count, index) => (
                        <div
                          key={index}
                          className="absolute right-1 bg-cyan-400/70"
                          style={{
                            bottom: `${(index / Math.max(1, histogram.length)) * 100}%`,
                            height: `${100 / Math.max(1, histogram.length)}%`,
                            width: `${Math.max(1, count * 44)}px`,
                          }}
                        />
                      ))}
                      <div
                        className="absolute left-0 right-0 bg-amber-300/20"
                        style={{
                          top: `${valueToHistogramPercent(windowHigh)}%`,
                          height: `${Math.max(2, valueToHistogramPercent(windowLow) - valueToHistogramPercent(windowHigh))}%`,
                        }}
                      />
                      <button
                        className="absolute left-1/2 h-3 w-16 -translate-x-1/2 -translate-y-1/2 rounded border border-amber-100 bg-amber-300 shadow"
                        style={{ top: `${valueToHistogramPercent(windowHigh)}%` }}
                        title="Window upper bound"
                        onMouseDown={(event) => {
                          event.stopPropagation()
                          histogramDragRef.current = 'high'
                          updateHistogramWindow(event)
                        }}
                      />
                      <button
                        className="absolute left-1/2 h-3 w-16 -translate-x-1/2 -translate-y-1/2 rounded border border-amber-100 bg-amber-300 shadow"
                        style={{ top: `${valueToHistogramPercent(windowLow)}%` }}
                        title="Window lower bound"
                        onMouseDown={(event) => {
                          event.stopPropagation()
                          histogramDragRef.current = 'low'
                          updateHistogramWindow(event)
                        }}
                      />
                    </div>
                    <div className="font-mono">{formatWindowNumber(histogramMin)}</div>
                  </div>
                </div>
                {imageInfo.number_of_frames > 1 ? (
                  <label className="flex w-full max-w-md items-center gap-3 text-xs text-slate-300">
                    Frame
                    <input
                      className="flex-1"
                      type="range"
                      min={0}
                      max={imageInfo.number_of_frames - 1}
                      value={frameIndex}
                      onChange={(event) => setFrameIndex(Number(event.target.value))}
                    />
                    <span className="w-20 text-right font-mono">
                      {frameIndex + 1}/{imageInfo.number_of_frames}
                    </span>
                  </label>
                ) : null}
              </div>
            ) : (
              <div className="max-w-md text-center text-sm">
                <div className="font-medium">Image preview is not available yet.</div>
                <div className="mt-2 text-slate-400">{imageInfo?.unsupported_reason ?? 'Unknown reason'}</div>
              </div>
            )}
          </section>

          <aside className="min-h-0 overflow-auto rounded border border-slate-200">
            <div className="border-b border-slate-200 bg-slate-50 px-3 py-2 text-sm font-semibold">
              Viewer Controls
            </div>
            {imageInfo ? (
              <div className="space-y-4 p-3 text-sm">
                <section className="space-y-2">
                  <div className="font-semibold">Window</div>
                  <label className="flex items-center gap-2 text-xs">
                    <input
                      type="checkbox"
                      checked={useAutoWindow}
                      onChange={(event) => setAutoWindow(event.target.checked)}
                    />
                    Auto min/max
                  </label>
                  <div className="grid grid-cols-2 gap-2">
                    <label className="block text-xs text-slate-600">
                      Low
                      <input
                        className="mt-1 w-full rounded border border-slate-300 px-2 py-1 font-mono text-xs"
                        type="number"
                        step={windowStep}
                        disabled={useAutoWindow}
                        value={Number.isFinite(windowLow) ? formatWindowNumber(windowLow) : '0'}
                        onChange={(event) => setWindowBounds(Number(event.target.value), windowHigh)}
                      />
                    </label>
                    <label className="block text-xs text-slate-600">
                      High
                      <input
                        className="mt-1 w-full rounded border border-slate-300 px-2 py-1 font-mono text-xs"
                        type="number"
                        step={windowStep}
                        disabled={useAutoWindow}
                        value={Number.isFinite(windowHigh) ? formatWindowNumber(windowHigh) : '1'}
                        onChange={(event) => setWindowBounds(windowLow, Number(event.target.value))}
                      />
                    </label>
                    <label className="block text-xs text-slate-600">
                      WL
                      <input
                        className="mt-1 w-full rounded border border-slate-300 px-2 py-1 font-mono text-xs"
                        type="number"
                        step={windowStep}
                        disabled={useAutoWindow}
                        value={windowCenter === undefined ? '0' : formatWindowNumber(windowCenter)}
                        onChange={(event) => setWindowCenter(Number(event.target.value))}
                      />
                    </label>
                    <label className="block text-xs text-slate-600">
                      WW
                      <input
                        className="mt-1 w-full rounded border border-slate-300 px-2 py-1 font-mono text-xs"
                        type="number"
                        min={minWindowWidth}
                        step={windowStep}
                        disabled={useAutoWindow}
                        value={windowWidth === undefined ? formatWindowNumber(minWindowWidth) : formatWindowNumber(windowWidth)}
                        onChange={(event) => setWindowWidth(Math.max(minWindowWidth, Number(event.target.value)))}
                      />
                    </label>
                  </div>
                  <button className="rounded bg-slate-700 px-3 py-1.5 text-xs text-white hover:bg-slate-600" onClick={resetWindow}>
                    Reset to DICOM
                  </button>
                </section>

                <section className="space-y-2">
                  <div className="font-semibold">View</div>
                  <div className="flex flex-wrap gap-2">
                    <button className="rounded bg-slate-700 px-3 py-1.5 text-xs text-white hover:bg-slate-600" onClick={fitImage}>
                      Fit
                    </button>
                    <button className="rounded bg-slate-700 px-3 py-1.5 text-xs text-white hover:bg-slate-600" onClick={() => setZoom(1)}>
                      100%
                    </button>
                    <button className="rounded bg-slate-700 px-3 py-1.5 text-xs text-white hover:bg-slate-600" onClick={() => setZoom((value) => Math.max(0.05, value / 1.25))}>
                      -
                    </button>
                    <button className="rounded bg-slate-700 px-3 py-1.5 text-xs text-white hover:bg-slate-600" onClick={() => setZoom((value) => Math.min(8, value * 1.25))}>
                      +
                    </button>
                  </div>
                  <div className="font-mono text-xs text-slate-500">Zoom {(zoom * 100).toFixed(0)}%</div>
                </section>

                <section>
                  <div className="mb-2 font-semibold">DICOM Image Info</div>
                  <dl className="grid grid-cols-[140px_1fr] gap-x-3 gap-y-2">
                <dt className="text-slate-500">Supported</dt>
                <dd>{imageInfo.supported ? 'Yes' : 'No'}</dd>
                <dt className="text-slate-500">Modality</dt>
                <dd>{valueText(imageInfo.modality)}</dd>
                <dt className="text-slate-500">SOP Class UID</dt>
                <dd className="break-all font-mono text-xs">{valueText(imageInfo.sop_class_uid)}</dd>
                <dt className="text-slate-500">Transfer Syntax</dt>
                <dd className="break-all font-mono text-xs">{valueText(imageInfo.transfer_syntax_uid)}</dd>
                <dt className="text-slate-500">Rows</dt>
                <dd>{valueText(imageInfo.rows)}</dd>
                <dt className="text-slate-500">Columns</dt>
                <dd>{valueText(imageInfo.columns)}</dd>
                <dt className="text-slate-500">Frames</dt>
                <dd>{valueText(imageInfo.number_of_frames)}</dd>
                <dt className="text-slate-500">Samples/Pixel</dt>
                <dd>{valueText(imageInfo.samples_per_pixel)}</dd>
                <dt className="text-slate-500">Photometric</dt>
                <dd>{valueText(imageInfo.photometric_interpretation)}</dd>
                <dt className="text-slate-500">Bits Allocated</dt>
                <dd>{valueText(imageInfo.bits_allocated)}</dd>
                <dt className="text-slate-500">Bits Stored</dt>
                <dd>{valueText(imageInfo.bits_stored)}</dd>
                <dt className="text-slate-500">High Bit</dt>
                <dd>{valueText(imageInfo.high_bit)}</dd>
                <dt className="text-slate-500">Pixel Rep</dt>
                <dd>{valueText(imageInfo.pixel_representation)}</dd>
                <dt className="text-slate-500">Window Center</dt>
                <dd>{valueText(imageInfo.window_center)}</dd>
                <dt className="text-slate-500">Window Width</dt>
                <dd>{valueText(imageInfo.window_width)}</dd>
                <dt className="text-slate-500">Rescale</dt>
                <dd>
                  slope {valueText(imageInfo.rescale_slope)}, intercept {valueText(imageInfo.rescale_intercept)}
                </dd>
                <dt className="text-slate-500">Dose Grid Scaling</dt>
                <dd>{valueText(imageInfo.dose_grid_scaling)}</dd>
              </dl>
                </section>
              </div>
            ) : (
              <div className="p-3 text-sm text-slate-500">No image metadata loaded.</div>
            )}
          </aside>
        </div>
      </div>
    </div>
  )
}
