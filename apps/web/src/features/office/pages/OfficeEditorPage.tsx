import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link, useParams, useSearchParams } from 'react-router'
import { DocumentText } from 'reicon-react'
import { getOfficeEditorConfig } from '@orbit/api/generated/sdk.gen'
import type { OfficeEditorView } from '@orbit/api/generated/types.gen'
import { ApiProblem } from '@orbit/api/problem'
import { apiClient } from '@/api/client'
import { queryKeys } from '@/api/queryKeys'
import { EmptyState } from '@/components/common/EmptyState'
import { LoadingScreen } from '@/components/common/LoadingScreen'
import { Button, buttonVariants } from '@/components/ui/button'
import { useTheme } from '@/lib/themeContext'
import { loadDocsApi } from '@/features/office/documentServer'

/** A path of this Orbit to go back to; anything else leads home. */
function backPath(value: string | null) {
  return value && value.startsWith('/') && !value.startsWith('//') && !value.includes('\\') ? value : '/'
}

/** What to say about a failed config request. `final`: asking again would give the same answer. */
function problemMessage(error: unknown): { title: string; description: string; final: boolean } {
  const code = error instanceof ApiProblem ? error.code : null
  if (code === 'office_not_configured') {
    return { title: 'Office editing is off', description: 'This Orbit has no Document Server, or an admin turned office editing off. You can still download the file.', final: true }
  }
  if (code === 'office_file_unsupported') {
    return { title: 'Not an office file', description: 'The office editor does not open this kind of file. You can still download it.', final: true }
  }
  if (code === 'office_file_not_found') {
    return { title: 'File not found', description: 'The file does not exist or you cannot open it.', final: true }
  }
  return { title: 'The file could not be opened', description: 'Orbit could not prepare the editor. Try again.', final: false }
}

/**
 * `/office/:kind/:fileId`: one office file in the Document Server's editor, full height. `?back=` is the page of
 * Orbit that the file was opened from.
 */
export function OfficeEditorPage() {
  const { kind = '', fileId = '' } = useParams()
  const [search] = useSearchParams()
  const back = backPath(search.get('back'))
  const { theme } = useTheme()
  const config = useQuery({
    // The theme is part of the signed config: another theme is another config.
    queryKey: [...queryKeys.officeEditorConfig, kind, fileId, theme, back],
    queryFn: async () => (await getOfficeEditorConfig({
      client: apiClient,
      throwOnError: true,
      path: { kind, file_id: fileId },
      query: { theme, back: back === '/' ? undefined : back },
    })).data,
    // A config opens one version of the file for five minutes: never reuse an old one.
    staleTime: 0,
    gcTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  })

  if (config.isPending) return <LoadingScreen />
  if (config.isError) {
    const message = problemMessage(config.error)
    return (
      <Notice title={message.title} description={message.description} back={back}>
        {message.final ? null : <Button variant="outline" onClick={() => void config.refetch()}>Try again</Button>}
      </Notice>
    )
  }
  return <Editor key={config.dataUpdatedAt} view={config.data} back={back} onRetry={() => void config.refetch()} />
}

function Notice({ title, description, back, children }: { title: string; description: string; back: string; children?: React.ReactNode }) {
  return (
    <main className="grid min-h-full place-items-center bg-background p-4">
      <div role="alert">
        <EmptyState
          icon={DocumentText}
          title={title}
          description={description}
          action={<>{children}<Link to={back} className={buttonVariants()}>Back to Orbit</Link></>}
        />
      </div>
    </main>
  )
}

function Editor({ view, back, onRetry }: { view: OfficeEditorView; back: string; onRetry: () => void }) {
  const container = useRef<HTMLDivElement>(null)
  const [state, setState] = useState<'loading' | 'ready' | 'unreachable'>('loading')

  useEffect(() => {
    let cancelled = false
    let editor: { destroyEditor(): void } | null = null
    const host = container.current
    void loadDocsApi(view.document_server_url).then((api) => {
      if (cancelled || !host) return
      // The editor replaces its placeholder with a frame: React must not own that element.
      const placeholder = document.createElement('div')
      placeholder.id = 'office-editor'
      host.replaceChildren(placeholder)
      editor = new api.DocEditor(placeholder.id, { ...view.config, width: '100%', height: '100%', type: 'desktop' })
      setState('ready')
    }, () => {
      if (!cancelled) setState('unreachable')
    })
    return () => {
      cancelled = true
      editor?.destroyEditor()
      host?.replaceChildren()
    }
  }, [view])

  if (state === 'unreachable') {
    return (
      <Notice title="The Document Server could not be reached" description="The editor did not load. The file is unchanged, and you can still download it." back={back}>
        <Button variant="outline" onClick={onRetry}>Try again</Button>
      </Notice>
    )
  }
  return (
    <main className="relative h-full bg-background">
      {state === 'loading' ? <div className="absolute inset-0"><LoadingScreen /></div> : null}
      <div ref={container} data-slot="office-editor" className="h-full" />
    </main>
  )
}
