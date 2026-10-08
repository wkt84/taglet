export const VALUE_PREVIEW_CHARACTERS = 120

export function valuePreview(value: string) {
  const characters = []
  let truncated = false
  for (const character of value) {
    if (characters.length === VALUE_PREVIEW_CHARACTERS) {
      truncated = true
      break
    }
    characters.push(character)
  }
  return {
    text: characters.join('').replace(/\r\n|\r|\n/g, '↵').replace(/\t/g, '⇥') + (truncated ? '…' : ''),
    needsDialog: truncated || /[\r\n\t]/.test(value),
  }
}
