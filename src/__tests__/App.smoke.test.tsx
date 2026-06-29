import { render, screen, waitFor } from '@testing-library/react'
import { vi } from 'vitest'

vi.mock('@/api/client', () => ({
  api: {
    getIdentity: vi.fn().mockResolvedValue({
      id: 'test-tenant',
      client: 'test',
      app_type: 'test',
      environment: 'test',
      display_name: 'Test',
    }),
  },
}))

// Stub the activity SSE hook so it doesn't open a real EventSource.
vi.mock('@/hooks/useActivitySSE', () => ({
  useActivitySSE: () => ({
    notifications: [],
    setNotifications: vi.fn(),
    readIdsRef: { current: new Set() },
    saveReadIds: vi.fn(),
    reload: vi.fn(),
  }),
}))

// Stub the Sidebar so it doesn't make real API calls.
vi.mock('@/components/Sidebar', () => ({ default: () => <div data-testid="sidebar-stub" /> }))

// Stub heavy modals / panels that aren't under test.
vi.mock('@/components/SettingsModal', () => ({ SettingsModal: () => null }))
vi.mock('@/components/BundleModal', () => ({ BundleModal: () => null }))
vi.mock('@/components/InspectorPanel', () => ({ InspectorPanel: () => null }))
vi.mock('@/components/ActivityPanel', () => ({ ActivityPanel: () => null }))

import App from '@/App'

test('App renders without crashing and shows section tabs', async () => {
  render(<App />)
  // Tabs appear once identity resolves.
  await waitFor(() => {
    expect(screen.getByText('DataViews')).toBeInTheDocument()
  })
})
