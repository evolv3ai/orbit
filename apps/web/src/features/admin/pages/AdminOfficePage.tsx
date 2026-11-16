import { useState } from 'react'
import { toast } from 'sonner'
import type { OfficeSettingsView } from '@orbit/api/generated/types.gen'
import { ApiProblem } from '@orbit/api/problem'
import { confirmAction } from '@/components/common/confirmAction'
import { InfoTip } from '@/components/common/InfoTip'
import { SettingsCard } from '@/components/common/SettingsCard'
import { Button } from '@/components/ui/button'
import { Field, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { PasswordInput } from '@/components/ui/password-input'
import { Switch } from '@/components/ui/switch'
import { useOfficeSettings, useRemoveOfficeSettings, useSaveOfficeSettings, useTestOfficeConnection } from '@/features/admin/api'
import { SetByEnvironment } from '@/features/admin/components/SetByEnvironment'
import { FieldGrid, RequiredMark } from '@/features/settings/components/SettingsParts'
import { LoadingText } from '@/components/ui/spinner'

const TITLE = 'Office editing'

function problemText(error: unknown, fallback: string) {
  return error instanceof ApiProblem ? error.detail : fallback
}

/** The ONLYOFFICE Document Server that opens Word, Excel, PowerPoint and PDF files. */
export function AdminOfficePage() {
  const settings = useOfficeSettings()
  if (settings.isPending) return <SettingsCard title={TITLE}><p><LoadingText>Loading settings…</LoadingText></p></SettingsCard>
  if (settings.isError) return <SettingsCard title={TITLE}><p role="alert">Settings could not be loaded. <Button variant="ghost" onClick={() => void settings.refetch()}>Retry</Button></p></SettingsCard>
  // Remount on save so the form shows what the server stored.
  return <OfficeCard key={JSON.stringify(settings.data)} office={settings.data} />
}

function OfficeCard({ office }: { office: OfficeSettingsView }) {
  const save = useSaveOfficeSettings()
  const remove = useRemoveOfficeSettings()
  const test = useTestOfficeConnection()
  const locked = office.set_by_environment
  const saved = office.url != null
  const [draft, setDraft] = useState({
    // A new Document Server is on as soon as it is saved.
    enabled: saved ? office.enabled : true,
    url: office.url ?? '',
    internal_url: office.internal_url ?? '',
    orbit_url: office.orbit_url ?? '',
    jwt_secret: '',
  })
  const set = (field: 'url' | 'internal_url' | 'orbit_url' | 'jwt_secret') => (event: React.ChangeEvent<HTMLInputElement>) => setDraft((current) => ({ ...current, [field]: event.target.value }))

  function submit(event: React.FormEvent) {
    event.preventDefault()
    save.mutate({
      enabled: draft.enabled,
      url: draft.url.trim(),
      internal_url: draft.internal_url.trim() || null,
      orbit_url: draft.orbit_url.trim() || null,
      // An empty field keeps the saved secret.
      jwt_secret: draft.jwt_secret || undefined,
    }, {
      onSuccess: () => toast('Office settings saved'),
      onError: (error) => toast.error(problemText(error, 'Office settings could not be saved.')),
    })
  }

  async function removeServer() {
    const confirmed = await confirmAction({
      title: 'Remove the Document Server?',
      description: 'Office files no longer open in the editor. People can still download them. The files and their earlier versions stay.',
      confirmLabel: 'Remove',
      danger: true,
    })
    if (!confirmed) return
    remove.mutate(undefined, {
      onSuccess: () => toast('Document Server removed'),
      onError: (error) => toast.error(problemText(error, 'The Document Server could not be removed.')),
    })
  }

  return (
    <form onSubmit={submit}>
      <SettingsCard
        title={TITLE}
        description="An ONLYOFFICE Document Server opens Word, Excel, PowerPoint and PDF files of tasks and docs in the browser. The files stay in Orbit."
        actions={saved ? (
          <>
            <Button type="button" variant="outline" disabled={test.isPending} onClick={() => test.mutate(undefined, {
              onSuccess: () => toast('The Document Server answered'),
              onError: (error) => toast.error(problemText(error, 'The Document Server could not be reached.')),
            })}>
              {test.isPending ? 'Testing…' : 'Test connection'}
            </Button>
            <Button type="button" variant="ghost" className="text-destructive" disabled={locked || remove.isPending} onClick={() => void removeServer()}>Remove</Button>
          </>
        ) : undefined}
      >
        <FieldGrid>
          <Field>
            <FieldLabel htmlFor="office-url">Document Server URL <RequiredMark /> <InfoTip text="The address that browsers load the editor from, for example https://office.example.com." /></FieldLabel>
            <Input id="office-url" required type="url" placeholder="https://office.example.com" disabled={locked} value={draft.url} onChange={set('url')} />
          </Field>
          <Field>
            <FieldLabel htmlFor="office-secret">JWT secret {saved ? null : <RequiredMark />} <InfoTip text="The JWT_SECRET of the Document Server. Orbit and the Document Server sign their requests to each other with it." /></FieldLabel>
            <PasswordInput id="office-secret" required={!saved} autoComplete="new-password" placeholder={office.jwt_secret_set ? 'Saved. Leave empty to keep it.' : ''} disabled={locked} value={draft.jwt_secret} onChange={set('jwt_secret')} />
          </Field>
          <Field>
            <FieldLabel htmlFor="office-internal-url">Internal Document Server URL <InfoTip text="How Orbit reaches the Document Server, when that is another address than the one browsers use (for example on a private network). Empty: the Document Server URL." /></FieldLabel>
            <Input id="office-internal-url" type="url" placeholder="http://documentserver" disabled={locked} value={draft.internal_url} onChange={set('internal_url')} />
          </Field>
          <Field>
            <FieldLabel htmlFor="office-orbit-url">Orbit URL for the Document Server <InfoTip text="How the Document Server reaches Orbit to download and save files, when that is another address than Orbit's public one. Empty: Orbit's public address." /></FieldLabel>
            <Input id="office-orbit-url" type="url" placeholder="http://orbit:8080" disabled={locked} value={draft.orbit_url} onChange={set('orbit_url')} />
          </Field>
        </FieldGrid>
        <div className="mt-4 flex items-center gap-2 text-[0.8125rem]">
          <Switch
            aria-label="Open office files in the editor"
            checked={draft.enabled}
            disabled={locked}
            onCheckedChange={(checked: boolean) => setDraft((current) => ({ ...current, enabled: checked }))}
          />
          <span className="inline-flex items-center gap-1.5 font-medium text-foreground">Open office files in the editor <InfoTip text="Off: Orbit keeps these settings, and office files only download." /></span>
        </div>
        <div className="mt-4 flex items-center justify-end gap-3">
          {locked ? <SetByEnvironment variable="ORBIT_ONLYOFFICE_*" className="mr-auto" /> : null}
          <Button type="submit" disabled={locked || save.isPending}>{save.isPending ? 'Saving…' : 'Save office settings'}</Button>
        </div>
      </SettingsCard>
    </form>
  )
}
