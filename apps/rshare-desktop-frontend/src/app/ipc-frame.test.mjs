import assert from 'node:assert/strict'
import fs from 'node:fs'
import test from 'node:test'

import {
  DEFAULT_MAX_BINARY_FRAME_BYTES,
  DEFAULT_MAX_JSON_FRAME_BYTES,
  IPC_ENVELOPE_KIND,
  IpcFrameDecoder,
  encodeIpcFrame,
} from './ipc-frame.mjs'

test('encodes the five-byte big-endian header without counting the kind byte', () => {
  const frame = encodeIpcFrame(IPC_ENVELOPE_KIND.JSON, new TextEncoder().encode('hello'))

  assert.deepEqual(Array.from(frame.subarray(0, 4)), [0, 0, 0, 5])
  assert.equal(frame[4], IPC_ENVELOPE_KIND.JSON)
  assert.equal(new TextDecoder().decode(frame.subarray(5)), 'hello')
})

test('decodes fragmented and back-to-back frames', () => {
  const first = encodeIpcFrame(IPC_ENVELOPE_KIND.JSON, new TextEncoder().encode('{"ok":true}'))
  const second = encodeIpcFrame(IPC_ENVELOPE_KIND.HEARTBEAT, Uint8Array.of(1, 2, 3))
  const combined = new Uint8Array(first.length + second.length)
  combined.set(first)
  combined.set(second, first.length)
  const decoder = new IpcFrameDecoder()

  assert.deepEqual(decoder.push(combined.subarray(0, 2)), [])
  assert.deepEqual(decoder.push(combined.subarray(2, 7)), [])
  const frames = decoder.push(combined.subarray(7))

  assert.equal(frames.length, 2)
  assert.equal(frames[0].kind, IPC_ENVELOPE_KIND.JSON)
  assert.equal(new TextDecoder().decode(frames[0].payload), '{"ok":true}')
  assert.equal(frames[1].kind, IPC_ENVELOPE_KIND.HEARTBEAT)
  assert.deepEqual(Array.from(frames[1].payload), [1, 2, 3])
})

test('rejects unknown kinds and oversized declarations before receiving a body', () => {
  const decoder = new IpcFrameDecoder()
  assert.throws(
    () => decoder.push(Uint8Array.of(0, 0, 0, 1, 255)),
    /unsupported IPC envelope kind/,
  )

  for (const [kind, limit] of [
    [IPC_ENVELOPE_KIND.JSON, DEFAULT_MAX_JSON_FRAME_BYTES],
    [IPC_ENVELOPE_KIND.BINARY, DEFAULT_MAX_BINARY_FRAME_BYTES],
  ]) {
    const header = new Uint8Array(5)
    new DataView(header.buffer).setUint32(0, limit + 1, false)
    header[4] = kind
    assert.throws(() => new IpcFrameDecoder().push(header), /exceeds.*limit/)
  }
})

test('decodes a large frame one byte at a time with a linear copy budget', () => {
  const payload = new Uint8Array(512 * 1024)
  payload.fill(7)
  const encoded = encodeIpcFrame(IPC_ENVELOPE_KIND.BINARY, payload)
  const decoder = new IpcFrameDecoder()

  assert.equal(typeof decoder.stats, 'function')
  let frames = []
  for (let index = 0; index < encoded.length; index += 1) {
    const decoded = decoder.push(encoded.subarray(index, index + 1))
    if (decoded.length > 0) {
      frames = decoded
    }
  }

  assert.equal(frames.length, 1)
  assert.deepEqual(frames[0].payload, payload)
  const stats = decoder.stats()
  assert.ok(stats.copiedBytes <= encoded.length * 4)
  assert.equal(stats.maxBufferedBytes, payload.length)
  assert.ok(stats.maxChunkCount <= 1)
  assert.ok(stats.maxCapacityBytes <= payload.length)
  assert.equal(stats.bufferedBytes, 0)
})

test('Vite bridge rejects multiple response frames instead of ignoring extras', () => {
  const viteConfig = fs.readFileSync(new URL('../../vite.config.ts', import.meta.url), 'utf8')

  assert.match(viteConfig, /frames\.length\s*>\s*1/)
})
