const UUID = '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'
/** The download paths the server hands out for the files that the office editor opens. */
const PAGE_FILE = new RegExp(`^/api/v1/workspaces/${UUID}/pages/${UUID}/files/(${UUID})$`)
const TASK_ATTACHMENT = new RegExp(`^/api/v1/workspaces/${UUID}/tasks/${UUID}/(?:comments/${UUID}/)?attachments/(${UUID})/download$`)

/** The formats the office editor opens (the server's `office_format`): the Office and OpenDocument ones, and PDF. */
const EXTENSIONS = new Set(['docx', 'docm', 'dotx', 'doc', 'odt', 'rtf', 'xlsx', 'xlsm', 'xls', 'ods', 'pptx', 'ppsx', 'ppt', 'odp', 'pdf'])

/** True when the office editor opens a file with this name. */
export function isOfficeFileName(name: string) {
  const dot = name.lastIndexOf('.')
  return dot > 0 && EXTENSIONS.has(name.slice(dot + 1).toLowerCase())
}

/** The file that a download address of Orbit names, when it is a file of a task, a comment or a Docs page. */
export function officeFileOfUrl(url: string): { kind: 'attachment' | 'page_file'; id: string } | null {
  let path: string
  try {
    const parsed = new URL(url, window.location.origin)
    if (parsed.origin !== window.location.origin) return null
    path = parsed.pathname
  } catch {
    return null
  }
  const page = PAGE_FILE.exec(path)
  if (page) return { kind: 'page_file', id: page[1] }
  const attachment = TASK_ATTACHMENT.exec(path)
  return attachment ? { kind: 'attachment', id: attachment[1] } : null
}

/** The editor page of a file. `back`: the page of Orbit it is opened from. */
export function officeEditorPath(kind: string, id: string, back: string) {
  return `/office/${kind}/${id}?back=${encodeURIComponent(back)}`
}

/**
 * Where "Open in editor" leads for a file with this name and download address, opened from the page the browser
 * shows now. `null` for a file the editor does not open.
 */
export function officeEditorHref(name: string, url: string) {
  if (!isOfficeFileName(name)) return null
  const file = officeFileOfUrl(url)
  return file ? officeEditorPath(file.kind, file.id, `${window.location.pathname}${window.location.search}`) : null
}
