export function valueMatrix(value: string, columns: number, transpose = false) {
  const width = Math.max(1, Math.min(1024, Math.trunc(columns) || 1))
  const values = value.split('\\')
  const rows = Array.from({ length: Math.ceil(values.length / width) }, (_, row) =>
    Array.from({ length: width }, (_, column) => values[row * width + column] ?? null))
  return transpose
    ? Array.from({ length: width }, (_, column) => rows.map((row) => row[column]))
    : rows
}

let parserErrorNamespace: string | null | undefined

export function formattedXml(value: string): string | null {
  if (!value.trimStart().startsWith('<')) return null
  const parser = new DOMParser()
  // Chromium and WebKit/Firefox use different namespaces for parse errors.
  // Probe this engine once rather than rejecting a valid <parsererror> element.
  if (parserErrorNamespace === undefined) {
    parserErrorNamespace = parser.parseFromString('<', 'application/xml').getElementsByTagName('parsererror')[0]?.namespaceURI ?? null
  }
  const document = parser.parseFromString(value, 'application/xml')
  if (!document.documentElement || document.getElementsByTagNameNS(parserErrorNamespace, 'parsererror').length) return null
  const serializer = new XMLSerializer()

  function format(node: Node, depth: number, preserveSpace = false): string {
    const indent = '  '.repeat(depth)
    if (node.nodeType !== 1) return indent + serializer.serializeToString(node)
    const element = node as Element
    const space = element.getAttributeNS('http://www.w3.org/XML/1998/namespace', 'space')
    const preserve = space === 'preserve' || (space !== 'default' && preserveSpace)
    const children = Array.from(element.childNodes)
    // Adding indentation to mixed content or xml:space="preserve" changes its
    // meaning. Keep those subtrees together exactly as parsed.
    if (preserve || !children.some((child) => child.nodeType === 1)
      || children.some((child) => child.nodeType === 4 || child.nodeType === 3 && child.textContent?.trim())) {
      return indent + serializer.serializeToString(element)
    }
    const opening = serializer.serializeToString(element.cloneNode(false)).replace(/\s*\/>$/, '>')
    const content = children.filter((child) => child.nodeType !== 3 || child.textContent?.trim())
      .map((child) => format(child, depth + 1, preserve)).join('\n')
    return `${indent}${opening}\n${content}\n${indent}</${element.tagName}>`
  }

  const declaration = value.trimStart().match(/^<\?xml\s[\s\S]*?\?>/)?.[0]
  const output = Array.from(document.childNodes).map((node) => format(node, 0)).join('\n')
  return declaration ? `${declaration}\n${output}` : output
}
