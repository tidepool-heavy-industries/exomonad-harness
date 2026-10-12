export type RichView =
  | { readonly kind: 'text' | 'markdown'; readonly text: string; readonly truncated?: boolean }
  | { readonly kind: 'row' | 'column'; readonly children: readonly RichView[] }
  | { readonly kind: 'caption'; readonly body: RichView; readonly text: string; readonly truncated?: boolean }
  | { readonly kind: 'svg'; readonly source: string }
  | { readonly kind: 'image'; readonly source: ImageSource; readonly alt: string; readonly truncated?: boolean }
  | { readonly kind: 'inspection'; readonly text: string; readonly has_more: boolean; readonly unavailable: boolean }

export type ImageSource =
  | { readonly kind: 'data'; readonly mime: 'image/png' | 'image/jpeg' | 'image/webp'; readonly base64: string }
  | { readonly kind: 'retained'; readonly hash: string }

type Obj = Record<string, unknown>
const obj = (value: unknown): value is Obj => typeof value === 'object' && value !== null && !Array.isArray(value)
const bounded = (value: unknown, max: number) => typeof value === 'string' && new TextEncoder().encode(value).length <= max

export function isRichView(value: unknown, depth = 0): value is RichView {
  if (!obj(value) || depth > 32 || typeof value.kind !== 'string') return false
  if (Object.hasOwn(value, 'truncated') && typeof value.truncated !== 'boolean') return false
  switch (value.kind) {
    case 'text': case 'markdown': return bounded(value.text, 256 * 1024)
    case 'row': case 'column': return Array.isArray(value.children) && value.children.length <= 1024
      && value.children.every(child => isRichView(child, depth + 1))
    case 'caption': return bounded(value.text, 128 * 1024) && isRichView(value.body, depth + 1)
    case 'svg': return bounded(value.source, 128 * 1024)
    case 'image': return bounded(value.alt, 128 * 1024) && isImageSource(value.source)
    case 'inspection': return bounded(value.text, 256 * 1024) && typeof value.has_more === 'boolean' && typeof value.unavailable === 'boolean'
    default: return false
  }
}

function isImageSource(value: unknown): value is ImageSource {
  if (!obj(value)) return false
  if (value.kind === 'retained') return typeof value.hash === 'string' && /^[0-9a-f]{64}$/.test(value.hash)
  return value.kind === 'data' && (value.mime === 'image/png' || value.mime === 'image/jpeg' || value.mime === 'image/webp')
    && typeof value.base64 === 'string' && value.base64.length <= 4 * 1024 * 1024 && /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value.base64)
}

