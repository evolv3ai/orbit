import { afterEach, expect, test } from 'bun:test'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { OfficeSettingsView } from '@orbit/api/generated/types.gen'
import { AdminOfficePage } from './AdminOfficePage'

const originalFetch = globalThis.fetch
afterEach(() => { globalThis.fetch = originalFetch })

const EMPTY: OfficeSettingsView = { enabled: false, url: null, internal_url: null, orbit_url: null, jwt_secret_set: false, set_by_environment: false }
const SAVED: OfficeSettingsView = { enabled: true, url: 'https://office.example.com', internal_url: 'http://documentserver', orbit_url: null, jwt_secret_set: true, set_by_environment: false }

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><AdminOfficePage /></QueryClientProvider>)
}

test('the first save sends the secret and turns the editor on', async () => {
  const bodies: unknown[] = []
  let settings = EMPTY
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const request = input as Request
    if (request.method === 'PUT') {
      bodies.push(await request.json())
      settings = SAVED
    }
    return Response.json(settings)
  }) as unknown as typeof fetch
  const view = renderPage()

  expect(view.queryByRole('button', { name: 'Test connection' })).toBeNull()
  await userEvent.type(await view.findByLabelText(/^Document Server URL/), 'https://office.example.com')
  await userEvent.type(view.getByLabelText(/^JWT secret/), 'shared-secret')
  await userEvent.click(view.getByRole('button', { name: 'Save office settings' }))
  await waitFor(() => expect(bodies).toHaveLength(1))
  expect(bodies[0]).toEqual({ enabled: true, url: 'https://office.example.com', internal_url: null, orbit_url: null, jwt_secret: 'shared-secret' })
  expect(await view.findByRole('button', { name: 'Test connection' })).toBeTruthy()
})

test('saving with an empty secret field keeps the saved secret', async () => {
  const bodies: unknown[] = []
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const request = input as Request
    if (request.method === 'PUT') bodies.push(await request.json())
    return Response.json(SAVED)
  }) as unknown as typeof fetch
  const view = renderPage()

  await userEvent.click(await view.findByRole('switch', { name: 'Open office files in the editor' }))
  await userEvent.click(view.getByRole('button', { name: 'Save office settings' }))
  await waitFor(() => expect(bodies).toHaveLength(1))
  expect(bodies[0]).toEqual({ enabled: false, url: 'https://office.example.com', internal_url: 'http://documentserver', orbit_url: null })
})

test('the test button reports what the server answers', async () => {
  const calls: string[] = []
  let healthy = true
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const request = input as Request
    const path = new URL(request.url).pathname
    if (path.endsWith('/test')) {
      calls.push(request.method)
      return healthy
        ? new Response(null, { status: 204 })
        : Response.json({ code: 'office_unreachable', title: 'Document Server unreachable', status: 502, detail: 'Orbit could not use the Document Server at http://documentserver: the connection failed' }, { status: 502, headers: { 'content-type': 'application/problem+json' } })
    }
    return Response.json(SAVED)
  }) as unknown as typeof fetch
  const view = renderPage()

  await userEvent.click(await view.findByRole('button', { name: 'Test connection' }))
  await waitFor(() => expect(calls).toEqual(['POST']))
  healthy = false
  await waitFor(() => expect((view.getByRole('button', { name: 'Test connection' }) as HTMLButtonElement).disabled).toBe(false))
  await userEvent.click(view.getByRole('button', { name: 'Test connection' }))
  await waitFor(() => expect(calls).toEqual(['POST', 'POST']))
})

test('a Document Server set by the environment is read-only, and the test still works', async () => {
  globalThis.fetch = (async () => Response.json({ ...SAVED, set_by_environment: true })) as unknown as typeof fetch
  const view = renderPage()

  const url = await view.findByLabelText(/^Document Server URL/)
  expect((url as HTMLInputElement).disabled).toBe(true)
  expect((view.getByLabelText(/^JWT secret/) as HTMLInputElement).disabled).toBe(true)
  expect((view.getByRole('button', { name: 'Save office settings' }) as HTMLButtonElement).disabled).toBe(true)
  expect((view.getByRole('button', { name: 'Remove' }) as HTMLButtonElement).disabled).toBe(true)
  expect((view.getByRole('button', { name: 'Test connection' }) as HTMLButtonElement).disabled).toBe(false)
  expect(view.getByText('ORBIT_ONLYOFFICE_*')).toBeTruthy()
})
