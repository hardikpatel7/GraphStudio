import { render, screen } from '@testing-library/react'
import { vi } from 'vitest'

vi.mock('@/hooks/useActivitySSE', () => ({
  useActivitySSE: () => ({
    notifications: [],
    setNotifications: vi.fn(),
    readIdsRef: { current: new Set() },
    saveReadIds: vi.fn(),
    reload: vi.fn(),
  }),
}))

vi.mock('@/components/Sidebar', () => ({ default: () => <div /> }))
vi.mock('@/components/SettingsModal', () => ({ SettingsModal: () => null }))
vi.mock('@/components/BundleModal', () => ({ BundleModal: () => null }))
vi.mock('@/components/InspectorPanel', () => ({ InspectorPanel: () => null }))
vi.mock('@/components/ActivityPanel', () => ({ ActivityPanel: () => null }))

import { WorkspaceLayout } from '@/layouts/WorkspaceLayout'

const identity = {
  id: 'test-tenant',
  client: 'test',
  app_type: 'test',
  environment: 'test',
  display_name: 'Test',
}

/** Canary: brand label renders "SmartStudio".
 *  Turns RED after WorkspaceLayout.tsx is updated in Task 14. */
test('sidebar brand label renders SmartStudio', () => {
  render(<WorkspaceLayout tenantId="test-tenant" identity={identity} workspace={<div />} />)
  expect(screen.getByText('SmartStudio')).toBeInTheDocument()
})
