import {
  DEFAULT_MAX_BINARY_FRAME_BYTES,
  IPC_ENVELOPE_KIND,
  IpcFrameDecoder,
} from './ipc-frame.mjs'

const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const SUPPORTED_IMAGE_TYPES = new Set(['image/png', 'image/jpeg'])
const MAX_DISPLAY_DIMENSION = 16_384

function normalizeBytes(value) {
  if (value instanceof Uint8Array) return value
  if (value instanceof ArrayBuffer) return new Uint8Array(value)
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
  }
  throw new TypeError('display capture response must be binary')
}

function uuidToBytes(value) {
  if (!UUID_PATTERN.test(value)) {
    throw new Error('invalid display capture id')
  }
  const compact = value.replaceAll('-', '')
  const bytes = new Uint8Array(16)
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(compact.slice(index * 2, index * 2 + 2), 16)
  }
  return bytes
}

function equalBytes(left, right) {
  if (left.byteLength !== right.byteLength) return false
  for (let index = 0; index < left.byteLength; index += 1) {
    if (left[index] !== right[index]) return false
  }
  return true
}

function validateMetadata(result) {
  if (!result || typeof result !== 'object' || typeof result.status !== 'string') {
    throw new Error('invalid display capture metadata')
  }
  if (!UUID_PATTERN.test(result.request_id ?? '')) {
    throw new Error('invalid display capture request id')
  }
  if (result.status !== 'Success') {
    if (result.payload != null) {
      throw new Error('failed display capture must not include a payload')
    }
    return
  }

  const descriptor = result.payload
  if (!descriptor || typeof descriptor !== 'object') {
    throw new Error('successful display capture requires a payload')
  }
  if (!UUID_PATTERN.test(descriptor.capture_id ?? '')) {
    throw new Error('invalid display capture id')
  }
  if (
    typeof descriptor.display_id !== 'string' ||
    descriptor.display_id.length === 0 ||
    descriptor.display_id.length > 1024
  ) {
    throw new Error('invalid display id')
  }
  if (!SUPPORTED_IMAGE_TYPES.has(descriptor.mime_type)) {
    throw new Error('unsupported display capture MIME type')
  }
  for (const dimension of [descriptor.width, descriptor.height]) {
    if (
      !Number.isSafeInteger(dimension) ||
      dimension <= 0 ||
      dimension > MAX_DISPLAY_DIMENSION
    ) {
      throw new Error('invalid display capture dimensions')
    }
  }
  if (
    !Number.isSafeInteger(descriptor.byte_length) ||
    descriptor.byte_length <= 0 ||
    descriptor.byte_length > DEFAULT_MAX_BINARY_FRAME_BYTES - 16
  ) {
    throw new Error('invalid display capture byte length')
  }
}

export function decodeDisplayCaptureResponse(value) {
  const decoder = new IpcFrameDecoder()
  const frames = decoder.push(normalizeBytes(value))
  if (decoder.pendingFrame !== null || decoder.stats().bufferedBytes !== 0) {
    throw new Error('truncated display capture response')
  }
  if (frames.length === 0 || frames[0].kind !== IPC_ENVELOPE_KIND.JSON) {
    throw new Error('display capture response must start with JSON metadata')
  }

  let result
  try {
    result = JSON.parse(
      new TextDecoder('utf-8', { fatal: true }).decode(frames[0].payload),
    )
  } catch {
    throw new Error('invalid display capture metadata JSON')
  }
  validateMetadata(result)

  if (result.status !== 'Success') {
    if (frames.length !== 1) {
      throw new Error('failed display capture must contain exactly one frame')
    }
    return { result, imageBytes: null }
  }
  if (
    frames.length !== 2 ||
    frames[1].kind !== IPC_ENVELOPE_KIND.BINARY
  ) {
    throw new Error('successful display capture must contain exactly two frames')
  }

  const binary = frames[1].payload
  const descriptor = result.payload
  if (binary.byteLength !== descriptor.byte_length + 16) {
    throw new Error('display capture byte length mismatch')
  }
  if (
    !equalBytes(
      binary.subarray(0, 16),
      uuidToBytes(descriptor.capture_id),
    )
  ) {
    throw new Error('display capture id mismatch')
  }
  return {
    result,
    imageBytes: binary.slice(16),
  }
}

export function createDisplayCaptureObjectUrl(
  value,
  {
    urlApi = globalThis.URL,
    BlobCtor = globalThis.Blob,
  } = {},
) {
  const decoded = decodeDisplayCaptureResponse(value)
  if (decoded.imageBytes === null) {
    return { ...decoded, url: null }
  }
  const blob = new BlobCtor([decoded.imageBytes], {
    type: decoded.result.payload.mime_type,
  })
  return {
    ...decoded,
    url: urlApi.createObjectURL(blob),
  }
}

export function createDisplayCaptureUrlStore({
  urlApi = globalThis.URL,
} = {}) {
  const urls = new Map()
  let generation = 0
  return {
    generation() {
      return generation
    },
    replace(displayId, url, expectedGeneration = generation) {
      if (expectedGeneration !== generation) {
        if (url) urlApi.revokeObjectURL(url)
        return false
      }
      const previous = urls.get(displayId)
      if (previous && previous !== url) {
        urlApi.revokeObjectURL(previous)
      }
      if (url) {
        urls.set(displayId, url)
      } else {
        urls.delete(displayId)
      }
      return true
    },
    get(displayId) {
      return urls.get(displayId) ?? null
    },
    snapshot() {
      return Object.fromEntries(urls)
    },
    dispose() {
      generation += 1
      for (const url of urls.values()) {
        urlApi.revokeObjectURL(url)
      }
      urls.clear()
    },
  }
}

export async function mapWithConcurrency(values, limit, worker) {
  if (!Number.isInteger(limit) || limit < 1) {
    throw new RangeError('concurrency limit must be a positive integer')
  }
  const items = Array.from(values)
  const results = new Array(items.length)
  let cursor = 0
  async function run() {
    while (true) {
      const index = cursor
      cursor += 1
      if (index >= items.length) return
      results[index] = await worker(items[index], index)
    }
  }
  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, () => run()),
  )
  return results
}
