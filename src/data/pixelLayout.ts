export type PixelLayout = { bitsAllocated: number; bitsStored: number; highBit: number; signed: boolean }

/** Mirrors the Rust PixelLayout: invalid Bits Stored / High Bit fall back to the full sample. */
export function pixelLayout(bitsAllocated: number, bitsStored: number | null | undefined, highBit: number | null | undefined, signed: boolean): PixelLayout {
  const stored = Number.isInteger(bitsStored) && bitsStored! >= 1 && bitsStored! <= bitsAllocated ? bitsStored! : bitsAllocated
  const high = Number.isInteger(highBit) && highBit! >= stored - 1 && highBit! < bitsAllocated ? highBit! : stored - 1
  return { bitsAllocated, bitsStored: stored, highBit: high, signed }
}

/** Extracts bits (highBit - bitsStored + 1)..highBit from an unsigned raw sample, sign-extending if needed. */
export function extractStoredValue(raw: number, layout: PixelLayout) {
  // Arithmetic instead of bitwise operators keeps 32-bit samples exact (JS bit ops are signed 32-bit).
  const shifted = Math.floor(raw / 2 ** (layout.highBit + 1 - layout.bitsStored))
  const modulus = 2 ** layout.bitsStored
  const value = shifted % modulus
  return layout.signed && value >= modulus / 2 ? value - modulus : value
}
