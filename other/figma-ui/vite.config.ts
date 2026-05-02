import { defineConfig } from 'vite'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'path'
import net from 'node:net'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'

const DAEMON_IPC_HOST = '127.0.0.1'
const DAEMON_IPC_PORT = Number(process.env.RSHARE_DAEMON_IPC_PORT ?? 27435)
const ANSI_ESCAPE_PATTERN = /\x1B\[[0-?]*[ -/]*[@-~]/g

type LogEntry = {
  timestamp: string
  level: string
  target: string
  message: string
}

function sendDaemonIpc(request: unknown): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(DAEMON_IPC_PORT, DAEMON_IPC_HOST)
    let buffer = ''
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
      socket.write(`${JSON.stringify(request)}\n`)
    })

    socket.on('data', (chunk) => {
      buffer += chunk.toString('utf8')
      const lineEnd = buffer.indexOf('\n')
      if (lineEnd < 0) {
        return
      }

      const line = buffer.slice(0, lineEnd).trim()
      settle(() => {
        try {
          resolve(JSON.parse(line))
        } catch (error) {
          reject(error)
        }
      })
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
          const daemonResponse = await sendDaemonIpc(daemonRequest)
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

  // File types to support raw imports. Never add .css, .tsx, or .ts files to this.
  assetsInclude: ['**/*.svg', '**/*.csv'],
})
