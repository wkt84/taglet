import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { DicomFrameImage, DicomImageInfo } from '../types/dicom'

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
  const bytes = new Uint8ClampedArray(binary.length)
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index)
  }
  return bytes
}

export default function ImageViewer({ onClose }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const [imageInfo, setImageInfo] = useState<DicomImageInfo>()
  const [frameImage, setFrameImage] = useState<DicomFrameImage>()
  const [frameIndex, setFrameIndex] = useState(0)
  const [error, setError] = useState<string>()
  const [loading, setLoading] = useState(true)
  const [frameLoading, setFrameLoading] = useState(false)

  useEffect(() => {
    let canceled = false
    setLoading(true)
    setError(undefined)

    invoke<DicomImageInfo>('get_dicom_image_info')
      .then((info) => {
        if (!canceled) setImageInfo(info)
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

    invoke<DicomFrameImage>('get_dicom_frame_image', { frameIndex })
      .then((frame) => {
        if (!canceled) setFrameImage(frame)
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
    if (!frameImage || !canvasRef.current) return

    const canvas = canvasRef.current
    canvas.width = frameImage.width
    canvas.height = frameImage.height
    const context = canvas.getContext('2d')
    if (!context) return

    const imageData = new ImageData(
      base64ToBytes(frameImage.rgba_base64),
      frameImage.width,
      frameImage.height,
    )
    context.putImageData(imageData, 0, 0)
  }, [frameImage])

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/50 p-4">
      <div className="flex h-[82vh] w-full max-w-5xl flex-col rounded bg-white shadow-xl">
        <div className="flex items-center border-b border-slate-200 px-4 py-3">
          <h2 className="flex-1 text-base font-semibold">Image Viewer</h2>
          <button className="rounded px-2 py-1 text-sm text-slate-600 hover:bg-slate-100" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="grid min-h-0 flex-1 grid-cols-[1fr_320px] gap-4 p-4">
          <section className="flex min-h-0 items-center justify-center rounded border border-slate-300 bg-slate-950 text-slate-300">
            {loading ? (
              <span className="text-sm">Loading image metadata...</span>
            ) : error ? (
              <span className="max-w-md text-sm text-red-300">{error}</span>
            ) : imageInfo?.supported ? (
              <div className="flex h-full w-full flex-col items-center justify-center gap-3 p-3">
                <div className="min-h-0 max-h-full max-w-full overflow-auto">
                  <canvas
                    ref={canvasRef}
                    className="max-h-full max-w-full bg-black [image-rendering:auto]"
                    style={{
                      width: frameImage ? `${frameImage.width}px` : undefined,
                      height: frameImage ? `${frameImage.height}px` : undefined,
                    }}
                  />
                </div>
                {frameLoading ? <div className="text-xs text-slate-400">Rendering frame...</div> : null}
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
              DICOM Image Info
            </div>
            {imageInfo ? (
              <dl className="grid grid-cols-[140px_1fr] gap-x-3 gap-y-2 p-3 text-sm">
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
            ) : (
              <div className="p-3 text-sm text-slate-500">No image metadata loaded.</div>
            )}
          </aside>
        </div>
      </div>
    </div>
  )
}
