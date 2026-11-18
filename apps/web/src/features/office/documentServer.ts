/** The editor that the Document Server's `api.js` mounts. */
export interface DocEditor {
  destroyEditor(): void
}

/**
 * The events of the editor that Orbit answers. The Document Server sends one of the two when it refuses the
 * `document.key` as outdated: `onRequestRefreshFile`, or `onOutdatedVersion` on older versions.
 */
export interface DocEditorEvents {
  onRequestRefreshFile(): void
  onOutdatedVersion(): void
}

/** What the Document Server's `api.js` defines on `window`. */
export interface DocsApi {
  DocEditor: new (placeholderId: string, config: Record<string, unknown>) => DocEditor
}

declare global {
  interface Window {
    DocsAPI?: DocsApi
  }
}

const loading = new Map<string, Promise<DocsApi>>()

/** The address of the Document Server's script. */
export function docsApiUrl(documentServerUrl: string) {
  return `${documentServerUrl.replace(/\/+$/, '')}/web-apps/apps/api/documents/api.js`
}

/**
 * Loads the Document Server's `api.js` once per address and answers with `DocsAPI`. Fails when the script does
 * not load (the Document Server is down, or the browser may not reach it); the next call tries again.
 */
export function loadDocsApi(documentServerUrl: string): Promise<DocsApi> {
  const src = docsApiUrl(documentServerUrl)
  const pending = loading.get(src)
  if (pending) return pending
  const promise = new Promise<DocsApi>((resolve, reject) => {
    const script = document.createElement('script')
    script.src = src
    script.async = true
    script.dataset.officeApi = ''
    const fail = () => {
      script.remove()
      loading.delete(src)
      reject(new Error('The Document Server could not be reached.'))
    }
    script.addEventListener('load', () => {
      if (window.DocsAPI) resolve(window.DocsAPI)
      else fail()
    })
    script.addEventListener('error', fail)
    document.head.append(script)
  })
  loading.set(src, promise)
  return promise
}
