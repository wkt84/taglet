import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const root = resolve(import.meta.dirname, '..')
const packageJsonPath = resolve(root, 'package.json')
const packageLockPath = resolve(root, 'package-lock.json')
const cargoTomlPath = resolve(root, 'src-tauri', 'Cargo.toml')

const packageJson = JSON.parse(readFileSync(packageJsonPath, 'utf8'))
const version = packageJson.version

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error(`package.json version is not a valid SemVer: ${version}`)
}

const packageLock = JSON.parse(readFileSync(packageLockPath, 'utf8'))
packageLock.version = version
if (packageLock.packages?.['']) {
  packageLock.packages[''].version = version
}
writeFileSync(packageLockPath, `${JSON.stringify(packageLock, null, 2)}\n`)

const cargoToml = readFileSync(cargoTomlPath, 'utf8')
const nextCargoToml = cargoToml.replace(
  /^version = ".*"$/m,
  `version = "${version}"`,
)

if (nextCargoToml === cargoToml && !cargoToml.includes(`version = "${version}"`)) {
  throw new Error('Could not find package version in src-tauri/Cargo.toml')
}

writeFileSync(cargoTomlPath, nextCargoToml)

console.log(`Synced Taglet version to ${version}`)
