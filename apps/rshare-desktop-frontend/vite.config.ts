import { defineConfig } from 'vite'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'path'
import net from 'node:net'
import { spawn } from 'node:child_process'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import {
  IPC_ENVELOPE_KIND,
  IpcFrameDecoder,
  encodeIpcFrame,
} from './src/app/ipc-frame.mjs'

const DAEMON_IPC_HOST = '127.0.0.1'
const DAEMON_IPC_PORT = Number(process.env.RSHARE_DAEMON_IPC_PORT ?? 27435)
const ANSI_ESCAPE_PATTERN = /\x1B\[[0-?]*[ -/]*[@-~]/g

type LogEntry = {
  timestamp: string
  level: string
  target: string
  message: string
}

type ServiceAction = 'start' | 'stop'

function isDaemonIpcUnavailable(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error)
  return (
    message.includes('ECONNREFUSED') ||
    message.includes('ECONNRESET') ||
    message.includes('daemon IPC closed without a response') ||
    message.includes('daemon IPC timed out')
  )
}

function sendDaemonIpc(request: unknown): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(DAEMON_IPC_PORT, DAEMON_IPC_HOST)
    const decoder = new IpcFrameDecoder()
    let settled = false

    const settle = (callback: () => void) => {
      if (settled) {
        return
      }
      settled = true
      socket.destroy()
      callback()
    }

    socket.setTimeout(5000, () => {
      settle(() => reject(new Error('daemon IPC timed out')))
    })

    socket.on('connect', () => {
      const payload = new TextEncoder().encode(JSON.stringify(request))
      socket.write(encodeIpcFrame(IPC_ENVELOPE_KIND.JSON, payload))
    })

    socket.on('data', (chunk) => {
      try {
        const frames = decoder.push(chunk)
        if (frames.length > 1) {
          throw new Error('daemon IPC returned multiple response frames')
        }
        const frame = frames[0]
        if (!frame) {
          return
        }
        if (frame.kind !== IPC_ENVELOPE_KIND.JSON) {
          throw new Error(`expected daemon JSON response, received frame kind ${frame.kind}`)
        }
        const json = new TextDecoder().decode(frame.payload)
        settle(() => resolve(JSON.parse(json)))
      } catch (error) {
        settle(() => reject(error))
      }
    })

    socket.on('error', (error) => {
      settle(() => reject(error))
    })

    socket.on('close', () => {
      if (!settled) {
        settle(() => reject(new Error('daemon IPC closed without a response')))
      }
    })
  })
}

function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const output = new Uint8Array(parts.reduce((total, part) => total + part.byteLength, 0))
  let offset = 0
  for (const part of parts) {
    output.set(part, offset)
    offset += part.byteLength
  }
  return output
}

function sendDaemonDisplayCapture(request: unknown): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(DAEMON_IPC_PORT, DAEMON_IPC_HOST)
    const decoder = new IpcFrameDecoder()
    let settled = false
    let metadataFrame: Uint8Array | null = null
    let expectsBinary = false

    const settle = (callback: () => void) => {
      if (settled) return
      settled = true
      socket.destroy()
      callback()
    }
    socket.setTimeout(5000, () => {
      settle(() => reject(new Error('daemon display capture timed out')))
    })
    socket.on('connect', () => {
      const payload = new TextEncoder().encode(JSON.stringify(request))
      socket.write(encodeIpcFrame(IPC_ENVELOPE_KIND.JSON, payload))
    })
    socket.on('data', (chunk) => {
      try {
        let completed: Uint8Array | null = null
        for (const frame of decoder.push(chunk)) {
          if (completed) {
            throw new Error('daemon returned extra display capture frames')
          }
          if (metadataFrame === null) {
            if (frame.kind !== IPC_ENVELOPE_KIND.JSON) {
              throw new Error('display capture response did not start with JSON')
            }
            const wrapper = JSON.parse(new TextDecoder().decode(frame.payload))
            const result = wrapper?.DisplayCapture
            if (!result || typeof result !== 'object') {
              throw new Error('daemon returned an unexpected display capture response')
            }
            metadataFrame = encodeIpcFrame(
              IPC_ENVELOPE_KIND.JSON,
              new TextEncoder().encode(JSON.stringify(result)),
            )
            expectsBinary = result.status === 'Success' && result.payload != null
            if (!expectsBinary) {
              settle(() => resolve(metadataFrame!))
            }
            continue
          }
          if (!expectsBinary || frame.kind !== IPC_ENVELOPE_KIND.BINARY) {
            throw new Error('daemon returned an unexpected display capture frame')
          }
          const binaryFrame = encodeIpcFrame(IPC_ENVELOPE_KIND.BINARY, frame.payload)
          completed = concatBytes(metadataFrame!, binaryFrame)
        }
        if (completed) {
          settle(() => resolve(completed!))
        }
      } catch (error) {
        settle(() => reject(error))
      }
    })
    socket.on('error', (error) => settle(() => reject(error)))
    socket.on('close', () => {
      if (!settled) {
        settle(() => reject(new Error('daemon closed before display capture completed')))
      }
    })
  })
}

async function waitForDaemonStatus(timeoutMs = 8000): Promise<unknown> {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown = null

  while (Date.now() < deadline) {
    try {
      return await sendDaemonIpc('Status')
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 200))
    }
  }

  throw lastError instanceof Error ? lastError : new Error(String(lastError))
}

async function findDaemonBinary(): Promise<string | null> {
  if (process.env.RSHARE_DAEMON_BIN) {
    try {
      await fs.access(process.env.RSHARE_DAEMON_BIN)
      return process.env.RSHARE_DAEMON_BIN
    } catch {
      return null
    }
  }

  const repoRoot = path.resolve(__dirname, '../..')
  const executableName = process.platform === 'win32' ? 'rshare-daemon.exe' : 'rshare-daemon'
  const candidates = [
    path.join(repoRoot, 'target', 'debug', executableName),
    path.join(repoRoot, 'target', 'release', executableName),
  ]

  for (const candidate of candidates) {
    try {
      await fs.access(candidate)
      return candidate
    } catch {
      // Keep looking.
    }
  }

  return null
}

async function spawnDaemonProcess() {
  const daemonBinary = await findDaemonBinary()
  const repoRoot = path.resolve(__dirname, '../..')
  const command = daemonBinary ?? 'cargo'
  const args = daemonBinary ? [] : ['run', '-p', 'rshare-daemon']
  const child = spawn(command, args, {
    cwd: repoRoot,
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
  })

  child.unref()
}

async function startDaemonService(): Promise<unknown> {
  try {
    return await sendDaemonIpc('Status')
  } catch {
    await spawnDaemonProcess()
    return await waitForDaemonStatus()
  }
}

async function stopDaemonService(): Promise<unknown> {
  return await sendDaemonIpc('Shutdown')
}

async function handleServiceAction(action: ServiceAction): Promise<unknown> {
  if (action === 'start') {
    return startDaemonService()
  }
  return stopDaemonService()
}

function readRequestBody(request: import('node:http').IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let body = ''
    request.setEncoding('utf8')
    request.on('data', (chunk) => {
      body += chunk
      if (body.length > 1024 * 1024) {
        reject(new Error('request body is too large'))
        request.destroy()
      }
    })
    request.on('end', () => resolve(body))
    request.on('error', reject)
  })
}

function resolveLogFilePath() {
  if (process.env.RSHARE_LOG_FILE) {
    return process.env.RSHARE_LOG_FILE
  }

  if (process.platform === 'win32') {
    const appData =
      process.env.APPDATA ??
      path.join(process.env.USERPROFILE ?? os.homedir(), 'AppData', 'Roaming')
    return path.join(appData, 'rshare', 'rshare-daemon.log')
  }

  if (process.platform === 'darwin') {
    return path.join(os.homedir(), 'Library', 'Application Support', 'rshare', 'rshare-daemon.log')
  }

  return path.join(process.env.XDG_CONFIG_HOME ?? path.join(os.homedir(), '.config'), 'rshare', 'rshare-daemon.log')
}

function parseLogLine(line: string): LogEntry | null {
  const clean = line.replace(ANSI_ESCAPE_PATTERN, '').trim()
  if (!clean) {
    return null
  }

  const parts = clean.split(/\s+/, 4)
  if (parts.length >= 4 && /^[A-Z]+$/.test(parts[1])) {
    return {
      timestamp: parts[0],
      level: parts[1],
      target: parts[2].replace(/:$/, ''),
      message: clean.slice(parts[0].length + parts[1].length + parts[2].length + 3),
    }
  }

  return {
    timestamp: '',
    level: 'INFO',
    target: 'rshare',
    message: clean,
  }
}

async function readDaemonLogs(limit: number): Promise<LogEntry[]> {
  const logPath = resolveLogFilePath()
  let content = ''
  try {
    content = await fs.readFile(logPath, 'utf8')
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code
    if (code === 'ENOENT') {
      return []
    }
    throw error
  }

  const entries = content
    .split(/\r?\n/)
    .reverse()
    .map(parseLogLine)
    .filter((entry): entry is LogEntry => Boolean(entry))
    .slice(0, Math.max(1, Math.min(5000, limit || 1000)))

  return entries.reverse()
}

async function clearDaemonLogs() {
  const logPath = resolveLogFilePath()
  await fs.mkdir(path.dirname(logPath), { recursive: true })
  await fs.writeFile(logPath, '', 'utf8')
}

function rshareDaemonBridge() {
  return {
    name: 'rshare-daemon-bridge',
    configureServer(server: import('vite').ViteDevServer) {
      server.middlewares.use('/__rshare/ipc', async (request, response, next) => {
        if (request.method !== 'POST') {
          next()
          return
        }

        response.setHeader('Content-Type', 'application/json; charset=utf-8')
        try {
          const body = await readRequestBody(request)
          const daemonRequest = body ? JSON.parse(body) : 'Status'
          let daemonResponse: unknown
          try {
            daemonResponse = await sendDaemonIpc(daemonRequest)
          } catch (error) {
            if (!isDaemonIpcUnavailable(error)) {
              throw error
            }
            await handleServiceAction('start')
            daemonResponse = await sendDaemonIpc(daemonRequest)
          }
          response.statusCode = 200
          response.end(JSON.stringify(daemonResponse))
        } catch (error) {
          response.statusCode = 502
          response.end(
            JSON.stringify({
              error: error instanceof Error ? error.message : String(error),
            }),
          )
        }
      })

      server.middlewares.use('/__rshare/display-capture', async (request, response, next) => {
        if (request.method !== 'POST') {
          next()
          return
        }
        response.setHeader('Content-Type', 'application/octet-stream')
        try {
          const body = await readRequestBody(request)
          const payload = body ? JSON.parse(body) : {}
          const daemonRequest = {
            CaptureDisplay: {
              display_id: payload.display_id ?? payload.displayId ?? 'primary',
              max_width: payload.max_width ?? payload.maxWidth ?? 900,
            },
          }
          let bytes: Uint8Array
          try {
            bytes = await sendDaemonDisplayCapture(daemonRequest)
          } catch (error) {
            if (!isDaemonIpcUnavailable(error)) throw error
            await handleServiceAction('start')
            bytes = await sendDaemonDisplayCapture(daemonRequest)
          }
          response.statusCode = 200
          response.end(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength))
        } catch (error) {
          response.statusCode = 502
          response.end(
            JSON.stringify({ error: error instanceof Error ? error.message : String(error) }),
          )
        }
      })

      server.middlewares.use('/__rshare/service', async (request, response, next) => {
        if (request.method !== 'POST') {
          next()
          return
        }

        response.setHeader('Content-Type', 'application/json; charset=utf-8')
        try {
          const body = await readRequestBody(request)
          const payload = body ? JSON.parse(body) : {}
          const action = payload?.action === 'stop' ? 'stop' : 'start'
          const serviceResponse = await handleServiceAction(action)
          response.statusCode = 200
          response.end(JSON.stringify(serviceResponse))
        } catch (error) {
          response.statusCode = 500
          response.end(
            JSON.stringify({
              error: error instanceof Error ? error.message : String(error),
            }),
          )
        }
      })

      server.middlewares.use('/__rshare/logs', async (request, response, next) => {
        if (request.method !== 'GET' && request.method !== 'DELETE') {
          next()
          return
        }

        response.setHeader('Content-Type', 'application/json; charset=utf-8')
        try {
          if (request.method === 'DELETE') {
            await clearDaemonLogs()
            response.statusCode = 200
            response.end(JSON.stringify({ ok: true }))
            return
          }

          const requestUrl = new URL(request.url ?? '', 'http://127.0.0.1')
          const limit = Number(requestUrl.searchParams.get('limit') ?? 1000)
          const logs = await readDaemonLogs(limit)
          response.statusCode = 200
          response.end(JSON.stringify(logs))
        } catch (error) {
          response.statusCode = 500
          response.end(
            JSON.stringify({
              error: error instanceof Error ? error.message : String(error),
            }),
          )
        }
      })
    },
  }
}

export default defineConfig({
  base: './',
  plugins: [
    rshareDaemonBridge(),
    // The React and Tailwind plugins are both required for Make, even if
    // Tailwind is not being actively used – do not remove them
    react(),
    tailwindcss(),
  ],
  resolve: {
    alias: {
      // Alias @ to the src directory
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  server: {
    port: 5176,
    strictPort: true,
    proxy: {
      '/ui-state': {
        target: 'ws://127.0.0.1:27436',
        ws: true,
        changeOrigin: false,
      },
    },
  },

  // File types to support raw imports. Never add .css, .tsx, or .ts files to this.
  assetsInclude: ['**/*.svg', '**/*.csv'],
})
