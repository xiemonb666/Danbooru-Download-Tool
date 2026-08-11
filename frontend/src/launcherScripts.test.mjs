import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { chooseVllmPort } from '../../scripts/select-vllm-port.mjs'

const linuxLauncher = readFileSync(resolve(process.cwd(), '..', 'run.sh'), 'utf8')
const windowsLauncher = readFileSync(resolve(process.cwd(), '..', 'run.bat'), 'utf8')
const linuxVllmLauncher = readFileSync(resolve(process.cwd(), '..', 'start_vllm.sh'), 'utf8')
const windowsVllmLauncher = readFileSync(resolve(process.cwd(), '..', 'start_vllm.bat'), 'utf8')

describe('production one-click launchers', () => {
  it('selects the next free port when the preferred port belongs to another service', async () => {
    const selection = await chooseVllmPort(8000, {
      isVllmReady: async () => false,
      isPortAvailable: async (port) => port === 8001,
    })

    expect(selection).toEqual({ action: 'start', port: 8001, preferredPortBusy: true })
  })

  it('reuses the endpoint of a model process that is still loading', async () => {
    const selection = await chooseVllmPort(8000, {
      isVllmReady: async () => false,
      getLoadingPort: async () => 8001,
      isPortAvailable: async () => {
        throw new Error('must not scan while a tracked model process is alive')
      },
    })

    expect(selection).toEqual({ action: 'loading', port: 8001, preferredPortBusy: true })
  })

  it('reports a tracked loading endpoint as ready after its model API comes online', async () => {
    const selection = await chooseVllmPort(8000, {
      isVllmReady: async (port) => port === 8001,
      getLoadingPort: async () => 8001,
      isPortAvailable: async () => false,
    })

    expect(selection).toEqual({ action: 'ready', port: 8001, preferredPortBusy: true })
  })

  it('executes the release binary directly after building it', () => {
    expect(linuxLauncher).toContain('exec "$BACKEND_BIN"')
    expect(linuxLauncher).not.toContain('cargo run')
    expect(windowsLauncher).toContain('"%BACKEND_BIN%"')
    expect(windowsLauncher).not.toContain('cargo run')
  })

  it('uses script-relative absolute runtime paths and loopback defaults', () => {
    expect(linuxLauncher).toContain('ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"')
    expect(linuxLauncher).toContain('export HOST="${HOST:-127.0.0.1}"')
    expect(linuxLauncher).toContain('export DATA_DIR="${DATA_DIR:-$ROOT_DIR}"')
    expect(linuxLauncher).toContain('export STATIC_DIR="${STATIC_DIR:-$FRONTEND_DIR/dist}"')

    expect(windowsLauncher).toContain('set "ROOT_DIR=%~dp0"')
    expect(windowsLauncher).toContain('if not defined HOST set "HOST=127.0.0.1"')
    expect(windowsLauncher).toContain('if not defined DATA_DIR set "DATA_DIR=%ROOT_DIR%"')
    expect(windowsLauncher).toContain('if not defined STATIC_DIR set "STATIC_DIR=%FRONTEND_DIR%\\dist"')
  })

  it('reinstalls frontend dependencies only when the lockfile changes', () => {
    expect(linuxLauncher).toContain('cmp -s "$FRONTEND_DIR/package-lock.json" "$LOCK_STAMP"')
    expect(linuxLauncher).toContain('cp "$FRONTEND_DIR/package-lock.json" "$LOCK_STAMP"')
    expect(windowsLauncher).toContain('fc /b "%FRONTEND_DIR%\\package-lock.json" "%LOCK_STAMP%"')
    expect(windowsLauncher).toContain('copy /y package-lock.json "%LOCK_STAMP%"')
  })

  it('fails early with the supported Node.js version requirement', () => {
    for (const launcher of [linuxLauncher, windowsLauncher]) {
      expect(launcher).toContain('process.versions.node')
      expect(launcher).toContain('Node.js 20.19+ or 22.12+ is required')
    }
  })

  it('keeps the bundled vLLM launcher disabled until the user explicitly loads a model', () => {
    expect(linuxLauncher).toContain('START_VLLM="${START_VLLM:-0}"')
    expect(linuxLauncher).toContain('"$ROOT_DIR/start_vllm.sh"')
    expect(linuxLauncher).toContain('VLLM_PID_FILE')

    expect(windowsLauncher).toContain('if not defined START_VLLM set "START_VLLM=0"')
    expect(windowsLauncher).toContain('start "Danbooru Tool vLLM"')
    expect(windowsLauncher).toContain('start_vllm.bat')
  })

  it('uses the verified vLLM port selection and passes its endpoint to the backend', () => {
    for (const launcher of [linuxLauncher, windowsLauncher]) {
      expect(launcher).toContain('scripts/select-vllm-port.mjs')
      expect(launcher).toContain('VLLM_BASE_URL')
    }
  })

  it('tracks the active loading process so repeated launches keep the same endpoint', () => {
    expect(linuxLauncher).toContain('vllm.state.json')
    expect(windowsLauncher).toContain('vllm.state.json')
    expect(linuxVllmLauncher).toContain('vllm.state.json')
  })

  it('keeps vLLM loopback-only and prevents duplicate model processes without killing ports', () => {
    expect(linuxVllmLauncher).toContain('VLLM_HOST="${VLLM_HOST:-127.0.0.1}"')
    expect(linuxVllmLauncher).toContain('flock -n 9')
    expect(linuxVllmLauncher).not.toContain('fuser -k')
  })

  it('allows conda activation scripts to read optional unset environment variables', () => {
    expect(linuxVllmLauncher).toMatch(/set \+u\s+conda activate "\$VLLM_CONDA_ENV"\s+set -u/)
  })

  it('keeps vLLM IPC temporary files on the Linux filesystem under WSL', () => {
    expect(linuxVllmLauncher).toContain('export TMPDIR="${VLLM_TMPDIR:-/tmp}"')
    expect(linuxVllmLauncher).toContain('export TMP="$TMPDIR"')
    expect(linuxVllmLauncher).toContain('export TEMP="$TMPDIR"')
  })

  it('starts the Windows vLLM batch file without a UTF-8 BOM command prefix', () => {
    expect(windowsVllmLauncher.charCodeAt(0)).not.toBe(0xfeff)
    expect(windowsVllmLauncher.startsWith('@echo off')).toBe(true)
  })
})
