import { spawnSync } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import openapiTS, { astToString } from 'openapi-typescript'

const frontendDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const backendManifest = path.resolve(frontendDir, '../backend/Cargo.toml')
const schemaPath = path.resolve(frontendDir, 'openapi.json')
const generatedPath = path.resolve(frontendDir, 'src/api/generated.ts')
const checkOnly = process.argv.includes('--check')

function pathForCargo(pathname, cargo) {
  // A WSL Node process can discover only cargo.exe while native Cargo is not
  // installed. Windows Cargo cannot resolve /mnt/c paths, so convert both
  // input and output paths explicitly instead of relying on shell expansion.
  if (process.platform !== 'win32' && /\.exe$/i.test(cargo)) {
    const converted = spawnSync('wslpath', ['-w', pathname], { encoding: 'utf8' })
    if (converted.status === 0 && converted.stdout.trim()) return converted.stdout.trim()
  }
  return pathname
}

function exportRustSchema() {
  const candidates = process.env.CARGO
    ? [process.env.CARGO]
    : process.platform === 'win32'
      ? ['cargo.exe', 'cargo']
      : ['cargo', 'cargo.exe']

  for (const cargo of candidates) {
    const manifestPath = pathForCargo(backendManifest, cargo)
    const outputPath = pathForCargo(schemaPath, cargo)
    const result = spawnSync(
      cargo,
      [
        'run',
        '--manifest-path',
        manifestPath,
        '--locked',
        '--',
        '--export-openapi',
        outputPath,
      ],
      { stdio: 'inherit' },
    )

    if (result.error?.code === 'ENOENT') continue
    if (result.error) throw result.error
    if (result.status !== 0) process.exit(result.status ?? 1)
    return
  }

  throw new Error('找不到 Cargo；请安装 Rust，或通过 CARGO 环境变量指定可执行文件。')
}

exportRustSchema()

const ast = await openapiTS(pathToFileURL(schemaPath))
const generated = astToString(ast)

if (checkOnly) {
  let current = ''
  try {
    current = await readFile(generatedPath, 'utf8')
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error
  }

  if (current !== generated) {
    console.error('OpenAPI 类型已过期；请运行 npm run api:generate。')
    process.exit(1)
  }
} else {
  await writeFile(generatedPath, generated, 'utf8')
  console.log(`已生成 ${path.relative(frontendDir, generatedPath)}`)
}
