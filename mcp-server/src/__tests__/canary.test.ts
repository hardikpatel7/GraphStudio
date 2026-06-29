import { readFileSync } from 'fs'
import { resolve } from 'path'

/** Canary: mcp-server reads SMARTSTUDIO_URL to locate the backend.
 *  Stays GREEN — this env var is NOT renamed. */
test('http.ts reads SMARTSTUDIO_URL env var', () => {
  const src = readFileSync(resolve(__dirname, '../../src/http.ts'), 'utf8')
  expect(src).toContain('SMARTSTUDIO_URL')
})
