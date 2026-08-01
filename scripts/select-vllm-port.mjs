import net from 'node:net'
import { execFile } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'

export async function chooseVllmPort(
  preferredPort,
  {
    isVllmReady = probeModelsEndpoint,
    isPortAvailable = canListen,
    getLoadingPort = async () => null,
  } = {},
) {
  if (await isVllmReady(preferredPort)) {
    return { action: 'ready', port: preferredPort, preferredPortBusy: false }
  }

  const loadingPort = await getLoadingPort()
  if (Number.isInteger(loadingPort) && loadingPort >= 1 && loadingPort <= 65_535) {
    return {
      action: (await isVllmReady(loadingPort)) ? 'ready' : 'loading',
      port: loadingPort,
      preferredPortBusy: loadingPort !== preferredPort,
    }
  }

  for (let port = preferredPort; port < preferredPort + 100; port += 1) {
    if (await isPortAvailable(port)) {
      return {
        action: 'start',
        port,
        preferredPortBusy: port !== preferredPort,
      }
    }
  }

  throw new Error(`No free vLLM port found in ${preferredPort}..${preferredPort + 99}`)
}

async function probeModelsEndpoint(port) {
  try {
    const response = await fetch(`http://127.0.0.1:${port}/v1/models`, {
      signal: AbortSignal.timeout(1_500),
    })
    if (!response.ok) return false
    const payload = await response.json()
    return Array.isArray(payload?.data)
  } catch {
    return false
  }
}

function canListen(port) {
  return new Promise((resolve) => {
    const server = net.createServer()
    server.unref()
    server.once('error', () => resolve(false))
    server.listen({ host: '127.0.0.1', port, exclusive: true }, () => {
      server.close(() => resolve(true))
    })
  })
}

async function readTrackedLoadingPort(statePath) {
  if (!statePath) return null
  try {
    const state = JSON.parse(await readFile(statePath, 'utf8'))
    if (
      state?.status !== 'loading' ||
      !Number.isInteger(state.port) ||
      !Number.isInteger(state.pid) ||
      state.port < 1 ||
      state.port > 65_535 ||
      state.pid < 1
    ) {
      return null
    }
    return (await isProcessRunning(state.pid)) ? state.port : null
  } catch {
    return null
  }
}

function isProcessRunning(pid) {
  if (process.platform === 'win32') {
    return new Promise((resolve) => {
      execFile(
        'wsl.exe',
        ['-u', 'root', 'kill', '-0', String(pid)],
        { windowsHide: true },
        (error) => resolve(error === null),
      )
    })
  }
  try {
    process.kill(pid, 0)
    return Promise.resolve(true)
  } catch {
    return Promise.resolve(false)
  }
}

async function main() {
  const preferredPort = Number.parseInt(process.argv[2] ?? '8000', 10)
  if (!Number.isInteger(preferredPort) || preferredPort < 1 || preferredPort > 65_535) {
    throw new Error('VLLM_PORT must be an integer in 1..65535')
  }
  const statePath = process.argv[3]
  const selection = await chooseVllmPort(preferredPort, {
    getLoadingPort: () => readTrackedLoadingPort(statePath),
  })
  process.stdout.write(
    `${selection.action}:${selection.port}:${selection.preferredPortBusy ? 'conflict' : 'preferred'}\n`,
  )
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`[ERROR] ${error instanceof Error ? error.message : String(error)}\n`)
    process.exitCode = 1
  })
}
