import { spawn, spawnSync } from 'node:child_process'
import { once } from 'node:events'
import { access, mkdir, mkdtemp, rm, rmdir, writeFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const projectDir = path.resolve(frontendDir, '..')
const backendManifest = path.join(projectDir, 'backend', 'Cargo.toml')
const staticDir = path.join(frontendDir, 'dist')
const temporaryBase = path.join(frontendDir, '.tmp')
const playwrightCli = path.join(frontendDir, 'node_modules', '@playwright', 'test', 'cli.js')
const png = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAF/gL+XyZT8QAAAABJRU5ErkJggg==',
  'base64',
)

let backendProcess
let runRoot
let backendOutput = ''
let cleaning = false

function findCargo() {
  const candidates = process.env.CARGO
    ? [process.env.CARGO]
    : process.platform === 'win32'
      ? ['cargo.exe', 'cargo']
      : ['cargo', 'cargo.exe']

  for (const cargo of candidates) {
    const result = spawnSync(cargo, ['--version'], { encoding: 'utf8', windowsHide: true })
    if (!result.error && result.status === 0) return cargo
    if (result.error?.code !== 'ENOENT') {
      throw result.error ?? new Error(`无法运行 ${cargo}`)
    }
  }
  throw new Error('找不到 Cargo；请安装 Rust，或通过 CARGO 环境变量指定可执行文件。')
}

function buildBackend(cargo) {
  const result = spawnSync(
    cargo,
    ['build', '--manifest-path', backendManifest, '--locked', '--message-format=json-render-diagnostics'],
    { cwd: projectDir, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, windowsHide: true },
  )
  if (result.stderr) process.stderr.write(result.stderr)
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`Rust 后端构建失败，退出码 ${result.status ?? 'unknown'}`)

  const executable = result.stdout
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => {
      try {
        return JSON.parse(line)
      } catch {
        return null
      }
    })
    .findLast((message) =>
      message?.reason === 'compiler-artifact'
      && message?.target?.name === 'danbooru-download-tool-pro'
      && message?.target?.kind?.includes('bin')
      && typeof message?.executable === 'string',
    )?.executable

  if (!executable) throw new Error('Cargo 未返回后端二进制路径。')
  return executable
}

async function reserveLoopbackPort() {
  const server = createServer()
  server.unref()
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('无法分配本机测试端口。')
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()))
  return address.port
}

function captureBackendOutput(stream) {
  stream.on('data', (chunk) => {
    backendOutput = `${backendOutput}${chunk.toString()}`.slice(-64 * 1024)
    if (process.env.REAL_E2E_VERBOSE === '1') process.stderr.write(chunk)
  })
}

async function waitForHealth(baseUrl) {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (backendProcess.exitCode !== null) {
      throw new Error(`Rust 后端提前退出（${backendProcess.exitCode}）。\n${backendOutput}`)
    }
    try {
      const response = await fetch(`${baseUrl}/api/health`, { signal: AbortSignal.timeout(1_000) })
      if (response.ok) return
    } catch {
      // The listener may not be ready yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`等待 Rust 后端启动超时。\n${backendOutput}`)
}

async function stopBackend() {
  if (!backendProcess || backendProcess.exitCode !== null) return
  backendProcess.kill()
  await Promise.race([
    once(backendProcess, 'exit'),
    new Promise((resolve) => setTimeout(resolve, 3_000)),
  ])
  if (backendProcess.exitCode === null) backendProcess.kill('SIGKILL')
}

async function cleanup() {
  if (cleaning) return
  cleaning = true
  await stopBackend()
  if (runRoot) await rm(runRoot, { recursive: true, force: true })
  try {
    await rmdir(temporaryBase)
  } catch (error) {
    if (!['ENOENT', 'ENOTEMPTY'].includes(error?.code)) throw error
  }
}

async function main() {
  await access(path.join(staticDir, 'index.html'))
  await access(playwrightCli)
  await mkdir(temporaryBase, { recursive: true })
  runRoot = await mkdtemp(path.join(temporaryBase, 'real-e2e-'))
  const dataDir = path.join(runRoot, 'data')
  const mediaDir = path.join(runRoot, 'media')
  await Promise.all([mkdir(dataDir), mkdir(mediaDir)])
  await Promise.all([
    writeFile(path.join(mediaDir, '123_score_5.png'), png),
    writeFile(path.join(mediaDir, '123_score_5.txt'), 'cat test_fixture\n', 'utf8'),
  ])

  const cargo = findCargo()
  const executable = buildBackend(cargo)
  const port = await reserveLoopbackPort()
  const baseUrl = `http://127.0.0.1:${port}`
  backendProcess = spawn(executable, [], {
    cwd: projectDir,
    env: {
      ...process.env,
      APP_ISOLATED_MODE: '1',
      HOST: '127.0.0.1',
      PORT: String(port),
      DATA_DIR: dataDir,
      STATIC_DIR: staticDir,
      DEV_CORS: '0',
      RUST_LOG: process.env.RUST_LOG ?? 'danbooru_download_tool_pro=warn',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: true,
  })
  captureBackendOutput(backendProcess.stdout)
  captureBackendOutput(backendProcess.stderr)
  await waitForHealth(baseUrl)

  const playwright = spawn(
    process.execPath,
    [playwrightCli, 'test', '--config', path.join(frontendDir, 'playwright.real.config.ts'), ...process.argv.slice(2)],
    {
      cwd: frontendDir,
      env: {
        ...process.env,
        REAL_E2E_BASE_URL: baseUrl,
        REAL_E2E_DATA_DIR: dataDir,
        REAL_E2E_MEDIA_DIR: mediaDir,
      },
      stdio: 'inherit',
      windowsHide: true,
    },
  )
  const [code, signal] = await once(playwright, 'exit')
  if (signal) throw new Error(`Playwright 被信号 ${signal} 终止。`)
  return code ?? 1
}

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => {
    void cleanup().finally(() => process.exit(signal === 'SIGINT' ? 130 : 143))
  })
}

let exitCode = 1
try {
  exitCode = await main()
} catch (error) {
  console.error(error instanceof Error ? error.message : error)
  if (backendOutput) console.error(backendOutput)
} finally {
  await cleanup()
}
process.exitCode = exitCode
