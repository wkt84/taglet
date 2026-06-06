import { useEffect, useRef, useState } from 'react'
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

function base64ToBytes(value: string) {
  const binary = window.atob(value)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index)
  }
  return bytes
}

function renderFrameToRgba(frame: DicomFramePixels, windowCenter: number, windowWidth: number) {
  const pixelBytes = base64ToBytes(frame.pixel_base64)
  const sampleCount = frame.width * frame.height
  const rgba = new Uint8ClampedArray(sampleCount * 4)
  const signed = frame.pixel_representation === 1
  const invert = frame.photometric_interpretation === 'MONOCHROME1'
  const low = windowCenter - windowWidth / 2
  const view = new DataView(pixelBytes.buffer, pixelBytes.byteOffset, pixelBytes.byteLength)

  for (let index = 0; index < sampleCount; index += 1) {
    let stored = 0
    if (frame.bits_allocated === 8) {
      stored = signed ? view.getInt8(index) : view.getUint8(index)
    } else {
      const offset = index * 2
      stored = signed ? view.getInt16(offset, true) : view.getUint16(offset, true)
    }

    const value = stored * frame.rescale_slope + frame.rescale_intercept
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
  const dragRef = useRef<{ x: number; y: number; panX: number; panY: number }>()
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
            setWindowCenter((frame.min_value + frame.max_value) / 2)
            setWindowWidth(Math.max(1, frame.max_value - frame.min_value))
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
    if (!framePixels || !canvasRef.current) return

    const canvas = canvasRef.current
    canvas.width = framePixels.width
    canvas.height = framePixels.height
    const context = canvas.getContext('2d')
    if (!context) return

    const center = useAutoWindow
      ? (framePixels.min_value + framePixels.max_value) / 2
      : (windowCenter ?? 0)
    const width = useAutoWindow
      ? Math.max(1, framePixels.max_value - framePixels.min_value)
      : Math.max(1, windowWidth ?? 1)
    const imageData = new ImageData(
      renderFrameToRgba(framePixels, center, width),
      framePixels.width,
      framePixels.height,
    )
    context.putImageData(imageData, 0, 0)
  }, [framePixels, useAutoWindow, windowCenter, windowWidth])

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

  const defaultCenter = imageInfo?.window_center[0] ?? 0
  const defaultWidth = imageInfo?.window_width[0] ?? 400
  const windowCenterMin = defaultCenter - Math.max(defaultWidth * 4, 2048)
  const windowCenterMax = defaultCenter + Math.max(defaultWidth * 4, 2048)
  const windowWidthMax = Math.max(defaultWidth * 4, 4096)

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
      setUseAutoWindow(true)
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
                <div
                  ref={viewportRef}
                  className="relative min-h-0 w-full flex-1 cursor-grab overflow-hidden active:cursor-grabbing"
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
                      onChange={(event) => setUseAutoWindow(event.target.checked)}
                    />
                    Auto min/max
                  </label>
                  <label className="block text-xs text-slate-600">
                    WL
                    <input
                      className="mt-1 w-full"
                      type="range"
                      min={windowCenterMin}
                      max={windowCenterMax}
                      step={1}
                      disabled={useAutoWindow || windowCenter === undefined}
                      value={windowCenter ?? 0}
                      onChange={(event) => setWindowCenter(Number(event.target.value))}
                    />
                    <input
                      className="mt-1 w-full rounded border border-slate-300 px-2 py-1 font-mono text-xs"
                      type="number"
                      disabled={useAutoWindow}
                      value={windowCenter ?? 0}
                      onChange={(event) => setWindowCenter(Number(event.target.value))}
                    />
                  </label>
                  <label className="block text-xs text-slate-600">
                    WW
                    <input
                      className="mt-1 w-full"
                      type="range"
                      min={1}
                      max={windowWidthMax}
                      step={1}
                      disabled={useAutoWindow || windowWidth === undefined}
                      value={windowWidth ?? 1}
                      onChange={(event) => setWindowWidth(Number(event.target.value))}
                    />
                    <input
                      className="mt-1 w-full rounded border border-slate-300 px-2 py-1 font-mono text-xs"
                      type="number"
                      min={1}
                      disabled={useAutoWindow}
                      value={windowWidth ?? 1}
                      onChange={(event) => setWindowWidth(Math.max(1, Number(event.target.value)))}
                    />
                  </label>
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
