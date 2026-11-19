import { expect, test } from 'bun:test'
import { isOfficeFileName, officeEditorHref, officeFileOfUrl, editorLanguage } from './officeFiles'

const WORKSPACE = '01900000-0000-7000-8000-000000000001'
const PARENT = '01900000-0000-7000-8000-000000000002'
const COMMENT = '01900000-0000-7000-8000-000000000003'
const FILE = '01900000-0000-7000-8000-000000000004'
const TASK_FILE = `/api/v1/workspaces/${WORKSPACE}/tasks/${PARENT}/attachments/${FILE}/download`
const COMMENT_FILE = `/api/v1/workspaces/${WORKSPACE}/tasks/${PARENT}/comments/${COMMENT}/attachments/${FILE}/download`
const PAGE_FILE = `/api/v1/workspaces/${WORKSPACE}/pages/${PARENT}/files/${FILE}`
const CHAT_FILE = `/api/v1/workspaces/${WORKSPACE}/chat/files/${FILE}`

test('the extension says whether the office editor opens a file', () => {
  for (const name of ['Report.DOCX', 'budget.xlsx', 'deck.pptx', 'scan.pdf', 'old.doc', 'sheet.ods']) expect(isOfficeFileName(name)).toBe(true)
  for (const name of ['photo.png', 'notes.txt', 'docx', '.pdf', 'archive.pdf.zip', '']) expect(isOfficeFileName(name)).toBe(false)
})

test('a download address names its file and kind', () => {
  expect(officeFileOfUrl(TASK_FILE)).toEqual({ kind: 'attachment', id: FILE })
  expect(officeFileOfUrl(COMMENT_FILE)).toEqual({ kind: 'attachment', id: FILE })
  expect(officeFileOfUrl(PAGE_FILE)).toEqual({ kind: 'page_file', id: FILE })
  expect(officeFileOfUrl(CHAT_FILE)).toEqual({ kind: 'chat_file', id: FILE })
  expect(officeFileOfUrl(`${window.location.origin}${PAGE_FILE}`)).toEqual({ kind: 'page_file', id: FILE })
})

test('other addresses name no file', () => {
  for (const url of [
    `/api/v1/workspaces/${WORKSPACE}/mail/attachments/${FILE}`,
    `/api/v1/workspaces/${WORKSPACE}/chat/conversations/${PARENT}/files`,
    `https://elsewhere.example${PAGE_FILE}`,
    `${PAGE_FILE}/extra`,
    'data:application/pdf;base64,AAAA',
    '',
  ]) expect(officeFileOfUrl(url)).toBeNull()
})

test('the editor address carries the page it is opened from', () => {
  window.history.replaceState(null, '', '/tasks/42?view=board')
  expect(officeEditorHref('report.docx', TASK_FILE)).toBe(`/office/attachment/${FILE}?back=%2Ftasks%2F42%3Fview%3Dboard`)
  expect(officeEditorHref('budget.xlsx', PAGE_FILE)).toBe(`/office/page_file/${FILE}?back=%2Ftasks%2F42%3Fview%3Dboard`)
  expect(officeEditorHref('photo.png', TASK_FILE)).toBeNull()
  expect(officeEditorHref('report.docx', CHAT_FILE)).toBe(`/office/chat_file/${FILE}?back=%2Ftasks%2F42%3Fview%3Dboard`)
  expect(officeEditorHref('photo.png', CHAT_FILE)).toBeNull()
})

test('the editor gets the language of the browser, when it is a plain language tag', () => {
  expect(editorLanguage('de')).toBe('de')
  expect(editorLanguage('pt-BR')).toBe('pt-BR')
  expect(editorLanguage('zh-Hant-TW')).toBe('zh-Hant-TW')
  // What the server would refuse is not sent: the editor then opens in its own language.
  expect(editorLanguage('')).toBeUndefined()
  expect(editorLanguage('x')).toBeUndefined()
  expect(editorLanguage('en_US')).toBeUndefined()
  expect(editorLanguage('en-US-x-a-very-long-tag')).toBeUndefined()
  expect(editorLanguage(undefined)).toBeUndefined()
})
