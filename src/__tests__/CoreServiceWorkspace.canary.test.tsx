import { readFileSync } from 'fs'
import { resolve } from 'path'

/** Canary: CoreServiceWorkspace default GCS placeholder contains "smartstudio-data".
 *  Stays GREEN — this GCS path is NOT renamed.  */
test('CoreServiceWorkspace GCS placeholder contains smartstudio-data', () => {
  const src = readFileSync(
    resolve(__dirname, '../components/workspace/CoreServiceWorkspace.tsx'),
    'utf8'
  )
  expect(src).toContain('smartstudio-data')
})
