import { afterEach, expect, test } from 'bun:test'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter, Route, Routes } from 'react-router'
import { OfficeEditorPage } from './OfficeEditorPage'

// happy-dom loads no script: appending one fails it at once, or, with this setting, loads it at once.
const scripts = (window as unknown as { happyDOM: { settings: { handleDisabledFileLoadingAsSuccess: boolean } } }).happyDOM.settings

const originalFetch = globalThis.fetch
afterEach(() => {
  cleanup()
  globalThis.fetch = originalFetch
  scripts.handleDisabledFileLoadingAsSuccess = false
  delete window.DocsAPI
  document.head.querySelectorAll('script[data-office-api]').forEach((script) => script.remove())
})

const FILE = '01900000-0000-7000-8000-000000000001'
const CONFIG = { documentType: 'word', document: { key: `${FILE}-abc` }, editorConfig: { mode: 'edit' }, token: 'signed' }

function renderPage(search = '') {
  const client = new QueryClient()
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/office/attachment/${FILE}${search}`]}>
        <Routes>
          <Route path="office/:kind/:fileId" element={<OfficeEditorPage />} />
          <Route path="tasks/:taskId" element={<p>The task</p>} />
          <Route path="/" element={<p>Home</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

/** Answers the config request with `body`, and records the addresses asked for. */
function serve(body: object, status = 200) {
  const asked: URL[] = []
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    asked.push(new URL((input as Request).url))
    return Response.json(body, { status, headers: status === 200 ? {} : { 'content-type': 'application/problem+json' } })
  }) as unknown as typeof fetch
  return asked
}

function problem(code: string, status: number) {
  return { type: 'about:blank', title: code, status, code, detail: code, instance: '/api/v1/office', request_id: 'test' }
}

/** The script tag of the Document Server, once the page has asked for it. */
async function officeScript() {
  return waitFor(() => {
    const script = document.head.querySelector<HTMLScriptElement>('script[data-office-api]')
    if (!script) throw new Error('no script yet')
    return script
  })
}

test('loads the Document Server script and mounts the editor with the signed config', async () => {
  const asked = serve({ document_server_url: 'https://office.one.example/', config: CONFIG })
  const mounted: { id: string; config: Record<string, unknown> }[] = []
  let destroyed = 0
  scripts.handleDisabledFileLoadingAsSuccess = true
  window.DocsAPI = {
    DocEditor: class {
      constructor(id: string, config: Record<string, unknown>) { mounted.push({ id, config }) }
      destroyEditor() { destroyed += 1 }
    },
  }
  const view = renderPage('?back=/tasks/42')

  const script = await officeScript()
  expect(script.src).toBe('https://office.one.example/web-apps/apps/api/documents/api.js')
  expect(asked[0].pathname).toBe(`/api/v1/office/attachment/${FILE}/config`)
  expect(asked[0].searchParams.get('back')).toBe('/tasks/42')
  expect(['light', 'dark']).toContain(asked[0].searchParams.get('theme') ?? '')
  await waitFor(() => expect(mounted).toHaveLength(1))
  expect(document.getElementById(mounted[0].id)).toBeTruthy()
  // The signed values go to the editor as the server gave them.
  expect(mounted[0].config).toEqual({ ...CONFIG, width: '100%', height: '100%', type: 'desktop' })

  view.unmount()
  expect(destroyed).toBe(1)
})

test('says so when the Document Server cannot be reached, and tries again', async () => {
  const asked = serve({ document_server_url: 'https://office.two.example', config: CONFIG })
  const view = renderPage('?back=/tasks/42')

  expect(await view.findByText('The Document Server could not be reached')).toBeTruthy()
  expect(document.head.querySelector('script[data-office-api]')).toBeNull()
  expect(view.getByRole('link', { name: 'Back to Orbit' }).getAttribute('href')).toBe('/tasks/42')

  await userEvent.click(view.getByRole('button', { name: 'Try again' }))
  // A new config, and a new try at the script.
  await waitFor(() => expect(asked).toHaveLength(2))
  expect(await view.findByText('The Document Server could not be reached')).toBeTruthy()
})

test('says that office editing is off', async () => {
  serve(problem('office_not_configured', 409), 409)
  const view = renderPage()
  expect(await view.findByText('Office editing is off')).toBeTruthy()
  expect(view.queryByRole('button', { name: 'Try again' })).toBeNull()
  expect(document.head.querySelector('script[data-office-api]')).toBeNull()
  // Without a page to go back to, and with one that is not of this Orbit, the way back is home.
  expect(view.getByRole('link', { name: 'Back to Orbit' }).getAttribute('href')).toBe('/')
})

test('says that the file is not there, and never leaves Orbit on the way back', async () => {
  serve(problem('office_file_not_found', 404), 404)
  const view = renderPage('?back=//evil.example')
  expect(await view.findByText('File not found')).toBeTruthy()
  expect(view.getByRole('link', { name: 'Back to Orbit' }).getAttribute('href')).toBe('/')
})

test('offers another try when the server fails for another reason', async () => {
  const asked = serve(problem('internal_error', 500), 500)
  const view = renderPage()
  expect(await view.findByText('The file could not be opened')).toBeTruthy()
  await userEvent.click(view.getByRole('button', { name: 'Try again' }))
  await waitFor(() => expect(asked).toHaveLength(2))
})
