import { readFileSync } from 'fs'
import { resolve } from 'path'

/** Canary: agent/App.tsx heading contains "SmartStudio Agent".
 *  Uses source-file check to avoid mounting the complex agent component tree.
 *  Turns RED after agent/App.tsx is updated in Task 14. */
test('agent App heading contains SmartStudio Agent', () => {
  const src = readFileSync(resolve(__dirname, '../agent/App.tsx'), 'utf8')
  expect(src).toContain('SmartStudio Agent')
})
