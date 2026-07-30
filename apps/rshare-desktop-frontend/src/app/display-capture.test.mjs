import assert from 'node:assert/strict'
import fs from 'node:fs'
import test from 'node:test'

import {
  createDisplayCaptureObjectUrl,
  createDisplayCaptureUrlStore,
  decodeDisplayCaptureResponse,
  mapWithConcurrency,
} from './display-capture.mjs'
import { encodeIpcFrame, IPC_ENVELOPE_KIND } from './ipc-frame.mjs'

const CAPTURE_ID = '00112233-4455-6677-8899-aabbccddeeff'

function uuidBytes(value) {
  return Uint8Array.from(
    value.replaceAll('-', '').match(/../g).map((part) => Number.parseInt(part, 16)),
  )
}

function concat(...parts) {
  const result = new Uint8Array(parts.reduce((total, part) => total + part.length, 0))
  let offset = 0
  for (const part of parts) {
    result.set(part, offset)
    offset += part.length
  }
  return result
}

function responseBytes({
  status = 'Success',
  payload = {
    capture_id: CAPTURE_ID,
    display_id: 'display-1',
    mime_type: 'image/png',
    width: 900,
    height: 506,
    byte_length: 4,
  },
  image = Uint8Array.of(137, 80, 78, 71),
} = {}) {
  const metadata = {
    request_id: 'ffeeddcc-bbaa-9988-7766-554433221100',
    status,
    message: status === 'Success' ? null : 'capture failed',
    payload: status === 'Success' ? payload : null,
  }
  const json = encodeIpcFrame(
    IPC_ENVELOPE_KIND.JSON,
    new TextEncoder().encode(JSON.stringify(metadata)),
  )
  if (status !== 'Success') {
    return json
  }
  const binary = encodeIpcFrame(
    IPC_ENVELOPE_KIND.BINARY,
    concat(uuidBytes(payload.capture_id), image),
  )
  return concat(json, binary)
}

test('decodes correlated metadata and compressed binary without base64', () => {
  const decoded = decodeDisplayCaptureResponse(responseBytes().buffer)

  assert.equal(decoded.result.payload.capture_id, CAPTURE_ID)
  assert.equal(decoded.result.payload.mime_type, 'image/png')
  assert.deepEqual(Array.from(decoded.imageBytes), [137, 80, 78, 71])
})

test('accepts an error metadata frame only and rejects malformed success pairs', () => {
  const failure = decodeDisplayCaptureResponse(
    responseBytes({ status: 'PermissionDenied' }).buffer,
  )
  assert.equal(failure.result.status, 'PermissionDenied')
  assert.equal(failure.imageBytes, null)

  const wrongId = responseBytes()
  wrongId[wrongId.length - 20] ^= 0xff
  assert.throws(() => decodeDisplayCaptureResponse(wrongId), /capture id/i)

  const wrongLength = responseBytes({
    payload: {
      capture_id: CAPTURE_ID,
      display_id: 'display-1',
      mime_type: 'image/png',
      width: 900,
      height: 506,
      byte_length: 5,
    },
  })
  assert.throws(() => decodeDisplayCaptureResponse(wrongLength), /byte length/i)

  const extra = concat(
    responseBytes(),
    encodeIpcFrame(IPC_ENVELOPE_KIND.HEARTBEAT, new Uint8Array()),
  )
  assert.throws(() => decodeDisplayCaptureResponse(extra), /exactly two frames/i)
})

test('creates and revokes Blob URLs on replacement and disposal', () => {
  const revoked = []
  let nextId = 0
  const urlApi = {
    createObjectURL(blob) {
      assert.equal(blob.type, 'image/png')
      return `blob:capture-${++nextId}`
    },
    revokeObjectURL(url) {
      revoked.push(url)
    },
  }
  const first = createDisplayCaptureObjectUrl(responseBytes(), { urlApi })
  const second = createDisplayCaptureObjectUrl(responseBytes(), { urlApi })
  const store = createDisplayCaptureUrlStore({ urlApi })

  store.replace('display-1', first.url)
  store.replace('display-1', second.url)
  assert.deepEqual(revoked, ['blob:capture-1'])
  store.dispose()
  assert.deepEqual(revoked, ['blob:capture-1', 'blob:capture-2'])

  const staleGeneration = store.generation()
  store.dispose()
  assert.equal(store.replace('display-1', 'blob:late', staleGeneration), false)
  assert.deepEqual(revoked, ['blob:capture-1', 'blob:capture-2', 'blob:late'])
})

test('multi-display capture never exceeds concurrency two', async () => {
  let active = 0
  let maximum = 0
  const gates = []
  const work = mapWithConcurrency([1, 2, 3, 4, 5], 2, async (value) => {
    active += 1
    maximum = Math.max(maximum, active)
    await new Promise((resolve) => gates.push(resolve))
    active -= 1
    return value * 2
  })

  await Promise.resolve()
  assert.equal(active, 2)
  while (gates.length) {
    gates.shift()()
    await Promise.resolve()
  }
  assert.deepEqual(await work, [2, 4, 6, 8, 10])
  assert.equal(maximum, 2)
})

test('production capture path is binary, user-triggered, and has no base64 timer', () => {
  const app = fs.readFileSync(new URL('./App.tsx', import.meta.url), 'utf8')
  const vite = fs.readFileSync(new URL('../../vite.config.ts', import.meta.url), 'utf8')
  const production = `${app}\n${vite}`

  assert.match(app, /capture_display_binary/)
  assert.match(vite, /application\/octet-stream/)
  assert.match(app, /mapWithConcurrency\(view\.displays,\s*2/)
  assert.doesNotMatch(
    production,
    /displayCaptureDataUrl|result\.bytes|btoa\(|data:image\/bmp/,
  )
  assert.doesNotMatch(
    app,
    /set(?:Interval|Timeout)\([^)]*captureDisplayBackgrounds/,
  )
})
