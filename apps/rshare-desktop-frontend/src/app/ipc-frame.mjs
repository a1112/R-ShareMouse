export const IPC_FRAME_HEADER_LEN = 5
export const DEFAULT_MAX_JSON_FRAME_BYTES = 4 * 1024 * 1024
export const DEFAULT_MAX_BINARY_FRAME_BYTES = 32 * 1024 * 1024

export const IPC_ENVELOPE_KIND = Object.freeze({
  JSON: 1,
  BINARY: 2,
  UI_STATE: 3,
  HEARTBEAT: 4,
})

const SUPPORTED_KINDS = new Set(Object.values(IPC_ENVELOPE_KIND))

function limitForKind(kind, limits) {
  switch (kind) {
    case IPC_ENVELOPE_KIND.JSON:
      return limits.json
    case IPC_ENVELOPE_KIND.BINARY:
      return limits.binary
    case IPC_ENVELOPE_KIND.UI_STATE:
      return limits.uiState
    case IPC_ENVELOPE_KIND.HEARTBEAT:
      return limits.heartbeat
    default:
      throw new Error(`unsupported IPC envelope kind ${kind}`)
  }
}

function normalizeBytes(payload) {
  if (payload instanceof Uint8Array) {
    return payload
  }
  if (payload instanceof ArrayBuffer) {
    return new Uint8Array(payload)
  }
  throw new TypeError('IPC payload must be a Uint8Array or ArrayBuffer')
}

export function encodeIpcFrame(kind, payload, limits = {}) {
  if (!SUPPORTED_KINDS.has(kind)) {
    throw new Error(`unsupported IPC envelope kind ${kind}`)
  }
  const bytes = normalizeBytes(payload)
  const resolvedLimits = {
    json: limits.json ?? DEFAULT_MAX_JSON_FRAME_BYTES,
    binary: limits.binary ?? DEFAULT_MAX_BINARY_FRAME_BYTES,
    uiState: limits.uiState ?? DEFAULT_MAX_JSON_FRAME_BYTES,
    heartbeat: limits.heartbeat ?? DEFAULT_MAX_JSON_FRAME_BYTES,
  }
  const limit = limitForKind(kind, resolvedLimits)
  if (bytes.byteLength > limit) {
    throw new Error(`IPC payload length ${bytes.byteLength} exceeds ${limit}-byte limit`)
  }

  const frame = new Uint8Array(IPC_FRAME_HEADER_LEN + bytes.byteLength)
  new DataView(frame.buffer).setUint32(0, bytes.byteLength, false)
  frame[4] = kind
  frame.set(bytes, IPC_FRAME_HEADER_LEN)
  return frame
}

export class IpcFrameDecoder {
  constructor(limits = {}) {
    this.limits = {
      json: limits.json ?? DEFAULT_MAX_JSON_FRAME_BYTES,
      binary: limits.binary ?? DEFAULT_MAX_BINARY_FRAME_BYTES,
      uiState: limits.uiState ?? DEFAULT_MAX_JSON_FRAME_BYTES,
      heartbeat: limits.heartbeat ?? DEFAULT_MAX_JSON_FRAME_BYTES,
    }
    this.buffer = new Uint8Array(0)
    this.start = 0
    this.end = 0
    this.pendingFrame = null
    this.copiedByteCount = 0
    this.maxBufferedByteCount = 0
    this.maxChunkCount = 0
    this.maxCapacityByteCount = 0
  }

  push(chunk) {
    const bytes = normalizeBytes(chunk)
    const frames = []
    let inputOffset = 0
    while (
      inputOffset < bytes.byteLength ||
      this.pendingFrame?.payloadLength === 0
    ) {
      if (this.pendingFrame === null) {
        const headerBytesNeeded =
          IPC_FRAME_HEADER_LEN - this.bufferedByteCount
        if (headerBytesNeeded > 0) {
          const count = Math.min(
            headerBytesNeeded,
            bytes.byteLength - inputOffset,
          )
          if (count === 0) {
            break
          }
          this.appendBytes(
            bytes.subarray(inputOffset, inputOffset + count),
            IPC_FRAME_HEADER_LEN,
          )
          inputOffset += count
          if (this.bufferedByteCount < IPC_FRAME_HEADER_LEN) {
            break
          }
        }

        const header = this.takeBytes(IPC_FRAME_HEADER_LEN)
        const payloadLength = new DataView(
          header.buffer,
          header.byteOffset,
          header.byteLength,
        ).getUint32(0, false)
        const kind = header[4]
        if (!SUPPORTED_KINDS.has(kind)) {
          throw new Error(`unsupported IPC envelope kind ${kind}`)
        }
        const limit = limitForKind(kind, this.limits)
        if (payloadLength > limit) {
          throw new Error(`IPC payload length ${payloadLength} exceeds ${limit}-byte limit`)
        }
        this.pendingFrame = { kind, payloadLength }
      }

      const payloadBytesNeeded =
        this.pendingFrame.payloadLength - this.bufferedByteCount
      if (payloadBytesNeeded > 0) {
        const count = Math.min(
          payloadBytesNeeded,
          bytes.byteLength - inputOffset,
        )
        if (count === 0) {
          break
        }
        this.appendBytes(
          bytes.subarray(inputOffset, inputOffset + count),
          this.pendingFrame.payloadLength,
        )
        inputOffset += count
        if (this.bufferedByteCount < this.pendingFrame.payloadLength) {
          break
        }
      }

      frames.push({
        kind: this.pendingFrame.kind,
        payload: this.takeBytes(this.pendingFrame.payloadLength),
      })
      this.pendingFrame = null
    }

    return frames
  }

  get bufferedByteCount() {
    return this.end - this.start
  }

  stats() {
    return {
      bufferedBytes: this.bufferedByteCount,
      copiedBytes: this.copiedByteCount,
      maxBufferedBytes: this.maxBufferedByteCount,
      maxChunkCount: this.maxChunkCount,
      maxCapacityBytes: this.maxCapacityByteCount,
    }
  }

  appendBytes(bytes, capacityLimit) {
    if (bytes.byteLength === 0) {
      return
    }

    this.ensureCapacity(this.bufferedByteCount + bytes.byteLength, capacityLimit)
    this.buffer.set(bytes, this.end)
    this.end += bytes.byteLength
    this.copiedByteCount += bytes.byteLength
    this.maxBufferedByteCount = Math.max(
      this.maxBufferedByteCount,
      this.bufferedByteCount,
    )
  }

  ensureCapacity(requiredCapacity, capacityLimit) {
    if (this.end + (requiredCapacity - this.bufferedByteCount) <= this.buffer.byteLength) {
      return
    }

    const liveBytes = this.bufferedByteCount
    if (requiredCapacity <= this.buffer.byteLength) {
      this.buffer.copyWithin(0, this.start, this.end)
      this.copiedByteCount += liveBytes
      this.start = 0
      this.end = liveBytes
      return
    }

    let nextCapacity = Math.max(1, this.buffer.byteLength)
    while (nextCapacity < requiredCapacity) {
      nextCapacity = Math.min(capacityLimit, nextCapacity * 2)
      if (nextCapacity < requiredCapacity && nextCapacity === capacityLimit) {
        nextCapacity = requiredCapacity
      }
    }

    const nextBuffer = new Uint8Array(nextCapacity)
    nextBuffer.set(this.buffer.subarray(this.start, this.end))
    this.copiedByteCount += liveBytes
    this.buffer = nextBuffer
    this.start = 0
    this.end = liveBytes
    this.maxChunkCount = 1
    this.maxCapacityByteCount = Math.max(
      this.maxCapacityByteCount,
      nextCapacity,
    )
  }

  takeBytes(length) {
    const output = new Uint8Array(length)
    output.set(this.buffer.subarray(this.start, this.start + length))
    this.start += length
    this.copiedByteCount += length
    if (this.start === this.end) {
      this.start = 0
      this.end = 0
    }
    return output
  }
}
