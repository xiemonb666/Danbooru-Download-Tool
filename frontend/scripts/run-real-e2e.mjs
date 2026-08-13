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

function isWindowsExecutable(command) {
  return process.platform !== 'win32' && /\.exe$/i.test(command)
}

function convertWslPath(pathname, flag) {
  const converted = spawnSync('wslpath', [flag, pathname], { encoding: 'utf8' })
  if (converted.status !== 0 || !converted.stdout.trim()) {
    throw new Error(`无法转换 WSL/Windows 路径: ${pathname}`)
  }
  return converted.stdout.trim()
}

function pathForWindowsProcess(pathname, command) {
  return isWindowsExecutable(command) ? convertWslPath(pathname, '-w') : pathname
}

function executableForNode(pathname, command) {
  return isWindowsExecutable(command) ? convertWslPath(pathname, '-u') : pathname
}

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
  const manifestPath = pathForWindowsProcess(backendManifest, cargo)
  const result = spawnSync(
    cargo,
    ['build', '--manifest-path', manifestPath, '--locked', '--message-format=json-render-diagnostics'],
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
  return executableForNode(executable, cargo)
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

async function waitForHealth(baseUrl, windowsBackend = false) {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (backendProcess.exitCode !== null) {
      throw new Error(`Rust 后端提前退出（${backendProcess.exitCode}）。\n${backendOutput}`)
    }
    try {
      if (windowsBackend) {
        const result = spawnSync('node.exe', ['-e', `fetch(${JSON.stringify(`${baseUrl}/api/health`)}).then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))`], {
          timeout: 1_500,
          windowsHide: true,
        })
        if (result.status === 0) return
      } else {
        const response = await fetch(`${baseUrl}/api/health`, { signal: AbortSignal.timeout(1_000) })
        if (response.ok) return
      }
    } catch {
      // The listener may not be ready yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`等待 Rust 后端启动超时。\n${backendOutput}`)
}

async function startBackend(executable, cargo, dataDir) {
  const windowsBackend = isWindowsExecutable(cargo)
  const windowsPortStart = 20_000 + Math.floor(Math.random() * 30_000)
  const crossProcessVariables = ['APP_ISOLATED_MODE', 'HOST', 'PORT', 'DATA_DIR', 'STATIC_DIR', 'DEV_CORS', 'RUST_LOG']
  const inheritedWslEnv = (process.env.WSLENV ?? '').split(':').filter(Boolean)
  const wslEnv = [...new Set([...inheritedWslEnv, ...crossProcessVariables])].join(':')
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const port = windowsBackend ? windowsPortStart + attempt : await reserveLoopbackPort()
    const baseUrl = `http://127.0.0.1:${port}`
    backendOutput = ''
    backendProcess = spawn(executable, [], {
      cwd: projectDir,
      env: {
        ...process.env,
        APP_ISOLATED_MODE: '1',
        HOST: '127.0.0.1',
        PORT: String(port),
        DATA_DIR: pathForWindowsProcess(dataDir, cargo),
        STATIC_DIR: pathForWindowsProcess(staticDir, cargo),
        DEV_CORS: '0',
        RUST_LOG: process.env.RUST_LOG ?? 'danbooru_download_tool_pro=warn',
        ...(windowsBackend ? { WSLENV: wslEnv } : {}),
      },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
    })
    captureBackendOutput(backendProcess.stdout)
    captureBackendOutput(backendProcess.stderr)
    try {
      await waitForHealth(baseUrl, windowsBackend)
      return baseUrl
    } catch (error) {
      const addressInUse = /10048|AddrInUse|address.*in use/i.test(backendOutput)
      await stopBackend()
      if (!addressInUse || attempt === 7) throw error
    }
  }
  throw new Error('无法为真实后端测试分配空闲端口。')
}

function findWindowsNode() {
  for (const command of ['node.exe']) {
    const result = spawnSync(command, ['--version'], { encoding: 'utf8', windowsHide: true })
    if (!result.error && result.status === 0) return command
  }
  throw new Error('Windows 后端需要 Windows Node.js 执行真实 E2E 请求。')
}

async function stopBackend() {
  if (!backendProcess || backendProcess.exitCode !== null) return
  backendProcess.kill()
  const exited = await Promise.race([
    once(backendProcess, 'exit'),
    new Promise((resolve) => setTimeout(() => resolve(null), 3_000)),
  ])
  if (exited === null && backendProcess.exitCode === null) {
    backendProcess.kill('SIGKILL')
    await Promise.race([
      once(backendProcess, 'exit'),
      new Promise((resolve) => setTimeout(resolve, 3_000)),
    ])
  }
}

async function removeRunRoot() {
  if (!runRoot) return
  let lastError
  for (let attempt = 0; attempt < 8; attempt += 1) {
    try {
      await rm(runRoot, { recursive: true, force: true })
      return
    } catch (error) {
      lastError = error
      if (!['EACCES', 'EPERM', 'EBUSY', 'ENOTEMPTY'].includes(error?.code)) throw error
      await new Promise((resolve) => setTimeout(resolve, 100 * 2 ** attempt))
    }
  }
  throw lastError
}

async function cleanup() {
  if (cleaning) return
  cleaning = true
  await stopBackend()
  await removeRunRoot()
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
  const windowsBackend = isWindowsExecutable(cargo)
  const baseUrl = await startBackend(executable, cargo, dataDir)
  const testNode = windowsBackend ? findWindowsNode() : process.execPath
  const testCli = windowsBackend ? pathForWindowsProcess(playwrightCli, cargo) : playwrightCli
  const testConfig = windowsBackend
    ? pathForWindowsProcess(path.join(frontendDir, 'playwright.real.config.ts'), cargo)
    : path.join(frontendDir, 'playwright.real.config.ts')
  const realE2eVariables = ['REAL_E2E_BASE_URL', 'REAL_E2E_DATA_DIR', 'REAL_E2E_MEDIA_DIR', 'REAL_E2E_BACKEND_PLATFORM']
  const testWslEnv = [...new Set([...(process.env.WSLENV ?? '').split(':').filter(Boolean), ...realE2eVariables])].join(':')

  const playwright = spawn(
    testNode,
    [testCli, 'test', '--config', testConfig, ...process.argv.slice(2)],
    {
      cwd: frontendDir,
      env: {
        ...process.env,
        REAL_E2E_BASE_URL: baseUrl,
        REAL_E2E_DATA_DIR: windowsBackend ? pathForWindowsProcess(dataDir, cargo) : dataDir,
        REAL_E2E_MEDIA_DIR: windowsBackend ? pathForWindowsProcess(mediaDir, cargo) : mediaDir,
        REAL_E2E_BACKEND_PLATFORM: windowsBackend ? 'windows' : process.platform,
        ...(windowsBackend ? { WSLENV: testWslEnv } : {}),
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
