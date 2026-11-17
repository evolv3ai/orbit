import { expect, test } from 'bun:test'
import { fireEvent, render } from '@testing-library/react'
import { waitForAbsence } from '@/test/waitForAbsence'
import type { Attachment } from '@/lib/attachmentLib'
import { Attachments } from './Attachments'

const image: Attachment = {
  id: 'image-1',
  fileName: 'diagram.png',
  mimeType: 'image/png',
  fileSize: 1024,
  url: 'data:image/png;base64,AA==',
}

test('opens an image in a modal and closes it from the backdrop', async () => {
  const view = render(<Attachments attachments={[image]} />)

  fireEvent.click(view.getByRole('button', { name: 'Open image diagram.png' }))

  const dialog = view.getByRole('dialog', { name: 'diagram.png' })
  expect(dialog.querySelector('img')?.getAttribute('src')).toBe(image.url)
  fireEvent.click(view.getByRole('button', { name: 'Close image viewer' }))
  // the viewer unmounts after its exit animation
  await waitForAbsence(() => view.queryByRole('dialog'))
})

test('the image viewer uses the usable screen height on a phone', () => {
  const view = render(<Attachments attachments={[image]} />)
  fireEvent.click(view.getByRole('button', { name: 'Open image diagram.png' }))

  const dialog = view.getByRole('dialog', { name: 'diagram.png' })
  expect(dialog.className).toContain('h-[var(--app-height,100svh)]')
  expect(dialog.className).toContain('pt-[env(safe-area-inset-top,0px)]')
  expect(dialog.className).toContain('pb-[env(safe-area-inset-bottom,0px)]')
  const img = dialog.querySelector('img')
  expect(img?.className).toContain('max-h-full')
  expect(img?.className).not.toContain('100vh')
})

test('closes the image modal with Escape', async () => {
  const view = render(<Attachments attachments={[image]} />)
  fireEvent.click(view.getByRole('button', { name: 'Open image diagram.png' }))

  fireEvent.keyDown(document, { key: 'Escape' })

  await waitForAbsence(() => view.queryByRole('dialog'))
})

test('a single image with a known size holds its place before it loads, at most 300px high', () => {
  const view = render(<Attachments attachments={[{ ...image, width: 1200, height: 600 }]} />)
  const img = view.getByRole('button', { name: 'Open image diagram.png' }).querySelector('img')!
  expect(img.getAttribute('width')).toBe('1200')
  expect(img.getAttribute('height')).toBe('600')
  expect(img.style.aspectRatio).toBe('1200 / 600')
  expect(img.style.width).toBe('600px')
  view.unmount()

  // a small image is not scaled up, and an image of unknown size sizes itself as before
  const small = render(<Attachments attachments={[{ ...image, width: 80, height: 40 }]} />)
  expect(small.getByRole('button', { name: 'Open image diagram.png' }).querySelector('img')!.style.width).toBe('80px')
  small.unmount()
  const unknown = render(<Attachments attachments={[image]} />)
  expect(unknown.getByRole('button', { name: 'Open image diagram.png' }).querySelector('img')!.style.width).toBe('')
})

test('a deleted file says so and offers no image or download', () => {
  const view = render(<Attachments attachments={[{ ...image, deleted: true }, { ...image, id: 'file-1', fileName: 'notes.pdf', mimeType: 'application/pdf', deleted: true }]} />)
  expect(view.getAllByText('Deleted file')).toHaveLength(2)
  expect(view.getByText('diagram.png · an admin deleted it')).toBeTruthy()
  expect(view.queryAllByRole('button', { name: 'Open image diagram.png' })).toHaveLength(0)
  expect(view.container.querySelectorAll('a')).toHaveLength(0)
})

test('"Open in editor" shows on the files the caller gives an address for', () => {
  const files: Attachment[] = [
    { id: 'a', fileName: 'report.docx', mimeType: 'application/octet-stream', fileSize: 10, url: '/files/a' },
    { id: 'b', fileName: 'notes.txt', mimeType: 'text/plain', fileSize: 10, url: '/files/b' },
    { id: 'c', fileName: 'gone.docx', mimeType: 'application/octet-stream', fileSize: 10, url: '/files/c', deleted: true },
  ]
  const editorHref = (name: string, url: string) => (name.endsWith('.docx') ? `/office${url}` : null)

  const view = render(<Attachments attachments={files} editorHref={editorHref} />)
  const links = view.getAllByRole('link', { name: 'Open in editor' })
  // Not the text file, and not the file an admin deleted.
  expect(links.map((link) => link.getAttribute('href'))).toEqual(['/office/files/a'])
  expect(links[0].getAttribute('target')).toBe('_blank')
  view.unmount()

  // Office editing is off: the caller passes nothing.
  expect(render(<Attachments attachments={files} />).queryByRole('link', { name: 'Open in editor' })).toBeNull()
})
