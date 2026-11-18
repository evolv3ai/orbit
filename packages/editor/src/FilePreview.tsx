import { FileBlockWrapper } from '@blocknote/react'
import type { ReactCustomBlockRenderProps } from '@blocknote/react'
import { useContext, useEffect, useState } from 'react'
import { OfficeLinkContext, OfficeSaveFailedContext, SAVE_FAILED_HELP } from './officeLink'
import { filePreviewKind, isPdfBytes, showsFilePreview, type FilePreviewKind } from './mediaFiles'

/** Text shown in the page. A longer file keeps the file row instead of a cut-off preview. */
const TEXT_LIMIT = 200_000
/** A PDF the browser can show from memory. A larger PDF keeps the file row. */
const PDF_LIMIT = 20_000_000

type FileBlockProps = ReactCustomBlockRenderProps<
  {
    type: 'file'
    propSchema: { backgroundColor: { default: 'default' }; name: { default: '' }; url: { default: '' }; caption: { default: '' } }
    content: 'none'
  },
  {
    type: 'file'
    propSchema: { backgroundColor: { default: 'default' }; name: { default: '' }; url: { default: '' }; caption: { default: '' } }
    content: 'none'
  }
>

interface FileFields {
  name: string
  url: string
}

/** The file block: a player or a text/PDF preview when the file is one of those, otherwise BlockNote's file row. */
export function FilePreview(props: FileBlockProps) {
  const { name, url } = props.block.props
  const kind = filePreviewKind(name, url)
  const show = showsFilePreview(kind, url)
  // Set by the app while office editing is on, for the files the office editor opens.
  const officeHref = useContext(OfficeLinkContext)?.(name, url) ?? null
  return (
    <FileBlockWrapper {...props} style={show || officeHref ? { width: '100%' } : undefined}>
      {show ? <Preview kind={kind} name={name} url={url} officeHref={officeHref} /> : officeHref ? <OfficeFile name={name} url={url} officeHref={officeHref} /> : undefined}
    </FileBlockWrapper>
  )
}

/** "Open in editor": the office editor, in a new tab, so the page stays as it is. */
function OfficeLink({ href, url }: { href: string; url: string }) {
  const failed = useContext(OfficeSaveFailedContext)?.(url) ?? false
  return (
    <>
      {failed ? (
        <span data-slot="file-office-save-failed" role="status" title={SAVE_FAILED_HELP} className="shrink-0 text-sm font-medium text-destructive">
          Last save failed
        </span>
      ) : null}
      <a data-slot="file-office-link" href={href} target="_blank" rel="noopener" className="shrink-0 text-sm font-medium underline underline-offset-4">
        Open in editor
      </a>
    </>
  )
}

/** An office file without a preview: its name, which downloads it, and "Open in editor". */
function OfficeFile({ name, url, officeHref }: FileFields & { officeHref: string }) {
  const keep = (event: { stopPropagation: () => void }) => event.stopPropagation()
  return (
    <div data-slot="file-office" className="flex w-full min-w-0 items-center gap-3" style={{ display: 'flex', alignItems: 'center', gap: 12, width: '100%' }} onMouseDown={keep}>
      <a href={url} download={name || undefined} className="min-w-0 flex-1 truncate text-sm">
        {name || url}
      </a>
      <OfficeLink href={officeHref} url={url} />
    </div>
  )
}

/** A link, for copy and for other apps. The player exists only in the editor. */
export function FileExternalHtml({ block }: { block: { props: FileFields & { caption: string } } }) {
  const { url, name, caption } = block.props
  if (!url) return <p>Add file</p>
  const link = <a href={url}>{name || url}</a>
  if (!caption) return link
  return (
    <div>
      {link}
      <p>{caption}</p>
    </div>
  )
}

function Preview({ kind, name, url, officeHref }: { kind: FilePreviewKind; officeHref: string | null } & FileFields) {
  // The block is one node. A click that reaches it selects the block and the control never sees it.
  const keep = (event: { stopPropagation: () => void }) => event.stopPropagation()
  return (
    <div data-slot="file-preview" data-kind={kind} className="flex w-full min-w-0 flex-col gap-2" style={{ width: '100%' }} onMouseDown={keep}>
      {name || officeHref ? (
        <div className="flex min-w-0 items-center gap-3" style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <div className="min-w-0 flex-1 truncate text-sm">{name}</div>
          {officeHref ? <OfficeLink href={officeHref} url={url} /> : null}
        </div>
      ) : null}
      {kind === 'video' ? <Video url={url} /> : null}
      {kind === 'audio' ? <Audio url={url} /> : null}
      {kind === 'pdf' ? <PdfPreview name={name} url={url} /> : null}
      {kind === 'text' ? <TextPreview name={name} url={url} /> : null}
    </div>
  )
}

function Video({ url }: { url: string }) {
  return <video controls preload="metadata" src={url} className="max-h-96 w-full rounded-md bg-black" style={{ display: 'block', width: '100%', maxHeight: 360, borderRadius: 8, background: '#000' }} />
}

function Audio({ url }: { url: string }) {
  return <audio controls preload="metadata" src={url} className="w-full" style={{ display: 'block', width: '100%' }} />
}

function FileLink({ name, url }: FileFields) {
  return (
    <a href={url} target="_blank" rel="noopener noreferrer" className="text-sm underline">
      {name || url}
    </a>
  )
}

function PdfPreview({ name, url }: FileFields) {
  const [src, setSrc] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const controller = new AbortController()
    let objectUrl = ''
    setSrc(null)
    setFailed(false)
    void (async () => {
      try {
        const bytes = await readBounded(url, PDF_LIMIT, controller.signal)
        if (!bytes || !isPdfBytes(bytes)) throw new Error('not a pdf')
        const copy = new ArrayBuffer(bytes.byteLength)
        new Uint8Array(copy).set(bytes)
        objectUrl = URL.createObjectURL(new Blob([copy], { type: 'application/pdf' }))
        if (controller.signal.aborted) {
          URL.revokeObjectURL(objectUrl)
          return
        }
        setSrc(objectUrl)
      } catch {
        if (!controller.signal.aborted) setFailed(true)
      }
    })()
    return () => {
      controller.abort()
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [url])

  if (failed) return <FileLink name={name} url={url} />
  if (!src) return <p className="text-sm text-muted-foreground">Loading…</p>
  return (
    <iframe
      title={name || 'PDF'}
      src={src}
      className="h-[480px] w-full rounded-md border bg-white"
      style={{ display: 'block', width: '100%', height: 480, border: 0, borderRadius: 8, background: '#fff' }}
    />
  )
}

function TextPreview({ name, url }: FileFields) {
  const [text, setText] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const controller = new AbortController()
    setText(null)
    setFailed(false)
    void (async () => {
      try {
        const bytes = await readBounded(url, TEXT_LIMIT, controller.signal)
        if (!bytes) throw new Error('too large')
        if (!controller.signal.aborted) setText(new TextDecoder().decode(bytes))
      } catch {
        if (!controller.signal.aborted) setFailed(true)
      }
    })()
    return () => controller.abort()
  }, [url])

  if (failed) return <FileLink name={name} url={url} />
  if (text === null) return <p className="text-sm text-muted-foreground">Loading…</p>
  return (
    <pre className="max-h-80 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-[0.8125rem] whitespace-pre-wrap" style={{ maxHeight: 320, overflow: 'auto', whiteSpace: 'pre-wrap', margin: 0 }}>
      {text}
    </pre>
  )
}

/** The body, when it is at most `limit` bytes. Null when it is larger. Throws when the request fails. */
async function readBounded(url: string, limit: number, signal: AbortSignal): Promise<Uint8Array | null> {
  const response = await fetch(url, { credentials: 'same-origin', signal })
  if (!response.ok) throw new Error(String(response.status))
  const announced = Number(response.headers.get('content-length') ?? '')
  if (Number.isFinite(announced) && announced > limit) return null
  const reader = response.body?.getReader()
  if (!reader) {
    const bytes = new Uint8Array(await response.arrayBuffer())
    return bytes.byteLength > limit ? null : bytes
  }
  const chunks: Uint8Array[] = []
  let size = 0
  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    size += value.byteLength
    if (size > limit) {
      await reader.cancel()
      return null
    }
    chunks.push(value)
  }
  const bytes = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return bytes
}
